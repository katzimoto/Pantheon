//! The ordered migration mechanism `pantheon-store` owns.
//!
//! Follows the canonical contract's "Migrations / backup" section:
//! `PRAGMA user_version` plus immutable checksummed `schema_migrations`
//! bookkeeping, with unknown newer schema failing startup. Each migration
//! applies in its own `BEGIN IMMEDIATE` transaction covering the schema
//! change, the `user_version` bump, and the bookkeeping row together, so a
//! failure rolls back all three: no partial schema and no falsely advanced
//! version.
//!
//! The schema is deliberately minimal: migration bookkeeping and the
//! `system_state` singleton for installation identity / RestoreGeneration,
//! plus the durable command ledger, Event Journal and journal epoch/sequence
//! state the command mutation kernel requires. It does not implement any
//! future production table family.
//!
//! Migrations are append-only. An applied migration's SQL is checksummed and
//! verified on every open, so editing an existing entry would make every
//! already-migrated database fail closed.

use rusqlite::{Connection, TransactionBehavior};

use crate::error::StoreError;

pub(crate) struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// The production migration set, applied in ascending `version` order.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "create_system_state",
        sql: "CREATE TABLE system_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            restore_generation TEXT NOT NULL CHECK (length(restore_generation) = 32)
        ) STRICT;",
    },
    Migration {
        version: 2,
        name: "bootstrap_installation_identity",
        // Idempotent: a fresh installation has no system_state row and gets
        // exactly one, with a fresh unpredictable RestoreGeneration drawn
        // from SQLite's OS-seeded randomblob(). Re-running this migration
        // against an already-bootstrapped database (which cannot normally
        // happen, since applied migrations are never re-run) would be a
        // no-op rather than a second row, because the singleton CHECK/PK on
        // system_state.id rejects a second row outright.
        sql: "INSERT INTO system_state (id, restore_generation)
            SELECT 1, lower(hex(randomblob(16)))
            WHERE NOT EXISTS (SELECT 1 FROM system_state WHERE id = 1);",
    },
    Migration {
        version: 3,
        name: "create_command_and_journal_state",
        // Table names follow the canonical "Table families" section of
        // docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md
        // (`commands` and `journal_epochs` in SYSTEM / CONFIGURATION,
        // `event_journal` in EVENTS); they are not invented here.
        sql: "CREATE TABLE journal_epochs (
            epoch         TEXT    PRIMARY KEY CHECK (length(epoch) = 32),
            -- The singleton next-sequence allocator the Event Journal
            -- contract requires. Advanced inside the same write transaction
            -- that appends the Event, never from MAX()+1, AUTOINCREMENT or
            -- an in-process counter.
            next_sequence INTEGER NOT NULL CHECK (next_sequence >= 1),
            is_current    INTEGER NOT NULL CHECK (is_current IN (0, 1))
        ) STRICT;

        -- At most one current journal history, enforced by the database
        -- rather than by controller discipline.
        CREATE UNIQUE INDEX journal_epochs_one_current
            ON journal_epochs (is_current) WHERE is_current = 1;

        CREATE TABLE event_journal (
            event_id      TEXT    NOT NULL PRIMARY KEY CHECK (length(event_id) = 32),
            journal_epoch TEXT    NOT NULL REFERENCES journal_epochs(epoch),
            -- Ordering metadata within a journal history, not Event identity.
            sequence      INTEGER NOT NULL CHECK (sequence >= 1),
            event_type    TEXT    NOT NULL CHECK (length(event_type) BETWEEN 1 AND 128),
            recorded_at   INTEGER NOT NULL,
            -- Command causality provenance. `command_epoch` is the command's
            -- RestoreGeneration, never this row's journal epoch, and the pair
            -- is all-or-none. Deliberately not a foreign key: this immutable
            -- historical identity must not acquire a retention dependency on
            -- the mutable idempotency ledger.
            command_epoch TEXT,
            command_id    TEXT,
            CHECK ((command_epoch IS NULL) = (command_id IS NULL)),
            UNIQUE (journal_epoch, sequence)
        ) STRICT;

        CREATE TABLE commands (
            -- The commandEpoch is the installation RestoreGeneration in
            -- force when the command was accepted.
            command_epoch TEXT    NOT NULL CHECK (length(command_epoch) = 32),
            command_id    TEXT    NOT NULL CHECK (length(command_id) BETWEEN 1 AND 128),
            -- Non-sensitive request digest, supplied by the caller. This
            -- crate never hashes anything and never sees a request body.
            request_hash  BLOB    NOT NULL CHECK (length(request_hash) = 32),
            -- The durable result reference: where this command's Event
            -- landed in the journal. Deliberately not a foreign key. The
            -- Event Journal is retained and pruned on its own schedule, and
            -- a referential dependency in either direction would make
            -- pruning a journal row fail while a command still names it.
            -- The Event contract states the same rule for the opposite
            -- direction, and the reasoning is symmetric.
            journal_epoch TEXT    NOT NULL,
            sequence      INTEGER NOT NULL,
            recorded_at   INTEGER NOT NULL,
            PRIMARY KEY (command_epoch, command_id)
        ) STRICT;",
    },
    Migration {
        version: 4,
        name: "bootstrap_journal_epoch",
        // Same idempotent shape as `bootstrap_installation_identity`. The
        // epoch is drawn independently of `system_state.restore_generation`:
        // JournalEpoch fences Event-stream continuity while RestoreGeneration
        // fences authority/idempotency continuity, and one must never be
        // derived from the other. Rotating it belongs to the disaster-restore
        // authority fence (T0), which this mission does not implement.
        sql: "INSERT INTO journal_epochs (epoch, next_sequence, is_current)
            SELECT lower(hex(randomblob(16))), 1, 1
            WHERE NOT EXISTS (SELECT 1 FROM journal_epochs WHERE is_current = 1);",
    },
    Migration {
        version: 5,
        name: "create_configuration_authority",
        // Table names follow the canonical "Persistence model" section of
        // docs/architecture/operations/configuration-and-policy-revisions.md.
        // Only the three families this mission's behaviour needs are created:
        // `configuration_sources`, `config_load_attempts` and
        // `config_reconciliations` arrive with the behaviour that reads them.
        sql: "CREATE TABLE configuration_components (
            -- Components are content-addressed and immutable: the same
            -- compiled component reached twice is the same row.
            digest         BLOB NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            domain         TEXT NOT NULL CHECK (domain IN (
                'agents', 'routing', 'executionProfiles',
                'evaluators', 'context', 'authorization')),
            canonical_json TEXT NOT NULL
        ) STRICT;

        CREATE TABLE configuration_revisions (
            -- Revision identity is historical activation identity, which is
            -- why the sequence is the key and the content digest is not:
            -- re-activating identical content is a new revision with a later
            -- sequence.
            activation_sequence       INTEGER NOT NULL PRIMARY KEY CHECK (activation_sequence >= 1),
            content_digest            BLOB NOT NULL CHECK (length(content_digest) = 32),
            compiler_version          TEXT NOT NULL,
            -- Provenance, never semantic identity.
            source_set_digest         BLOB NOT NULL CHECK (length(source_set_digest) = 32),
            agents_digest             BLOB NOT NULL REFERENCES configuration_components(digest),
            routing_digest            BLOB NOT NULL REFERENCES configuration_components(digest),
            execution_profile_digest  BLOB NOT NULL REFERENCES configuration_components(digest),
            evaluator_registry_digest BLOB NOT NULL REFERENCES configuration_components(digest),
            context_policy_digest     BLOB NOT NULL REFERENCES configuration_components(digest),
            authorization_digest      BLOB NOT NULL REFERENCES configuration_components(digest),
            recorded_at               INTEGER NOT NULL
        ) STRICT;

        CREATE TABLE active_configuration (
            -- One small pointer, as the contract requires. Singleton by
            -- constraint rather than by convention.
            id                  TEXT    NOT NULL PRIMARY KEY CHECK (id = 'singleton'),
            -- Carries a revision so activation is an ordinary revisioned CAS
            -- through the #17 primitive rather than a bespoke compare.
            revision            INTEGER NOT NULL CHECK (revision > 0),
            -- NULL until a fresh installation activates its first revision:
            -- that state is exactly \"not yet ready for authority-bearing
            -- work\", and is why it is nullable.
            activation_sequence INTEGER REFERENCES configuration_revisions(activation_sequence)
        ) STRICT;",
    },
    Migration {
        version: 6,
        name: "bootstrap_active_configuration_pointer",
        // The pointer row exists from installation with no revision, so
        // activation is always a revisioned update of an existing row and a
        // fresh install has an inspectable \"no active configuration\" state.
        sql: "INSERT INTO active_configuration (id, revision, activation_sequence)
            SELECT 'singleton', 1, NULL
            WHERE NOT EXISTS (SELECT 1 FROM active_configuration WHERE id = 'singleton');",
    },
    Migration {
        version: 7,
        name: "create_goal_planning_and_task_graph",
        // Table and column shapes follow the canonical persistence contract's
        // "Goal and Task", "Temporal TaskGraph" and "Planning" sections. The
        // row-local CHECK constraints below are quoted from that contract
        // rather than invented here.
        sql: "CREATE TABLE goals (
            id               TEXT    NOT NULL PRIMARY KEY,
            phase            TEXT    NOT NULL CHECK (phase IN (
                'Planning', 'Active', 'Evaluating', 'Finalizing',
                'Succeeded', 'Failed', 'Cancelled')),
            current_revision INTEGER NOT NULL CHECK (current_revision >= 1),
            -- Carries a revision so lifecycle transitions are ordinary
            -- revisioned CAS through the #17 primitive.
            revision         INTEGER NOT NULL CHECK (revision > 0),
            terminal_target  TEXT,
            created_at       INTEGER NOT NULL,
            -- Terminal-intent coherence: a crash cannot leave a Finalizing
            -- Goal whose intended outcome must be guessed, and a terminal
            -- Goal cannot retain a contradictory stale target.
            CHECK (
                (phase IN ('Planning', 'Active', 'Evaluating')
                 AND terminal_target IS NULL)
                OR (phase = 'Finalizing'
                    AND terminal_target IN ('Succeeded', 'Failed', 'Cancelled'))
                OR (phase IN ('Succeeded', 'Failed', 'Cancelled')
                    AND terminal_target = phase)
            )
        ) STRICT;

        CREATE TABLE goal_revisions (
            goal_id        TEXT    NOT NULL REFERENCES goals(id),
            revision       INTEGER NOT NULL CHECK (revision >= 1),
            content_digest BLOB    NOT NULL CHECK (length(content_digest) = 32),
            canonical_json TEXT    NOT NULL,
            created_at     INTEGER NOT NULL,
            PRIMARY KEY (goal_id, revision)
        ) STRICT;

        CREATE TABLE task_graphs (
            -- One graph per Goal. `id` is the Goal id and `revision` is the
            -- graph revision, so a graph patch is an ordinary revisioned CAS
            -- rather than a bespoke compare. Starts at 0: a Goal has a graph
            -- before it has any Task.
            id       TEXT    NOT NULL PRIMARY KEY REFERENCES goals(id),
            revision INTEGER NOT NULL CHECK (revision >= 0)
        ) STRICT;

        CREATE TABLE task_specs (
            -- Immutable and content-addressed: the same compiled spec reached
            -- twice is the same row.
            digest                            BLOB    NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            goal_id                           TEXT    NOT NULL REFERENCES goals(id),
            -- The Goal revision this Task was created from.
            goal_revision                     INTEGER NOT NULL,
            canonical_json                    TEXT    NOT NULL,
            -- The acceptance contract identity later evaluation binds.
            acceptance_digest                 BLOB    NOT NULL CHECK (length(acceptance_digest) = 32),
            -- Evaluator resolution provenance, pinned at materialization. A
            -- later registry change cannot reach back through this.
            evaluator_registry_digest         BLOB    NOT NULL CHECK (length(evaluator_registry_digest) = 32),
            configuration_activation_sequence INTEGER NOT NULL,
            FOREIGN KEY (goal_id, goal_revision) REFERENCES goal_revisions(goal_id, revision)
        ) STRICT;

        CREATE TABLE tasks (
            id                     TEXT    NOT NULL PRIMARY KEY,
            goal_id                TEXT    NOT NULL REFERENCES goals(id),
            -- The graph revision that created this Task.
            created_graph_revision INTEGER NOT NULL,
            phase                  TEXT    NOT NULL CHECK (phase IN (
                'Pending', 'Ready', 'Active', 'Waiting', 'Evaluating',
                'Finalizing', 'Succeeded', 'Failed', 'Cancelled', 'Superseded')),
            revision               INTEGER NOT NULL CHECK (revision > 0),
            terminal_target        TEXT,
            terminal_reason_json   TEXT,
            -- The responsible Run pointer. Always NULL in this mission; the
            -- column exists because the canonical phase invariants below are
            -- expressed in terms of it, and an MVP-only lifecycle a later
            -- mission must replace is exactly what the mission forbids.
            active_run_id          TEXT,
            spec_digest            BLOB    NOT NULL REFERENCES task_specs(digest),
            CHECK (
                (phase IN ('Pending', 'Ready', 'Active', 'Waiting', 'Evaluating')
                 AND terminal_target IS NULL)
                OR (phase = 'Finalizing'
                    AND terminal_target IN ('Succeeded', 'Failed', 'Cancelled', 'Superseded'))
                OR (phase IN ('Succeeded', 'Failed', 'Cancelled', 'Superseded')
                    AND terminal_target = phase)
            ),
            -- `Task Ready|Waiting => zero nonterminal Runs`, in the part
            -- SQLite can express row-locally.
            CHECK (phase NOT IN ('Ready', 'Waiting') OR active_run_id IS NULL),
            CHECK (phase != 'Active' OR active_run_id IS NOT NULL)
        ) STRICT;

        CREATE TABLE task_graph_edges (
            -- Temporal, never deleted: active at revision R when
            -- created <= R and removed is null or > R. No edge exists in this
            -- mission, but deletion-by-row-removal is the kind of MVP-only
            -- model a later mission would have to replace.
            goal_id                TEXT    NOT NULL REFERENCES task_graphs(id),
            upstream_task_id       TEXT    NOT NULL REFERENCES tasks(id),
            downstream_task_id     TEXT    NOT NULL REFERENCES tasks(id),
            kind                   TEXT    NOT NULL CHECK (kind = 'requires_success'),
            created_graph_revision INTEGER NOT NULL,
            removed_graph_revision INTEGER,
            CHECK (upstream_task_id != downstream_task_id),
            CHECK (removed_graph_revision IS NULL
                   OR removed_graph_revision > created_graph_revision)
        ) STRICT;

        CREATE TABLE planning_operations (
            id                                TEXT    NOT NULL PRIMARY KEY,
            goal_id                           TEXT    NOT NULL REFERENCES goals(id),
            -- The exact Goal revision and graph precondition this decision was
            -- frozen against. Materialization re-reads current state and
            -- compares against these.
            goal_revision                     INTEGER NOT NULL,
            expected_graph_revision           INTEGER NOT NULL CHECK (expected_graph_revision >= 0),
            trigger_kind                      TEXT    NOT NULL CHECK (trigger_kind IN ('initial')),
            planning_input_digest             BLOB    NOT NULL CHECK (length(planning_input_digest) = 32),
            -- Local deterministic planner provenance. The architecture names
            -- only an external 'Planner Agent snapshot'; a local planner still
            -- has to be identifiable to reproduce a decision.
            planner_implementation            TEXT    NOT NULL,
            planner_version                   TEXT    NOT NULL,
            configuration_activation_sequence INTEGER NOT NULL,
            -- Only the two states this mission's behaviour writes. A
            -- rejection state arrives with the behaviour that records one.
            state                             TEXT    NOT NULL CHECK (state IN (
                'Planned', 'Materialized')),
            revision                          INTEGER NOT NULL CHECK (revision > 0),
            created_at                        INTEGER NOT NULL,
            FOREIGN KEY (goal_id, goal_revision) REFERENCES goal_revisions(goal_id, revision)
        ) STRICT;

        CREATE TABLE planning_records (
            -- One immutable normalized proposal per operation. It is evidence,
            -- never authority: nothing reads this table to decide whether a
            -- graph may be mutated.
            planning_operation_id    TEXT    NOT NULL PRIMARY KEY
                                             REFERENCES planning_operations(id),
            proposal_digest          BLOB    NOT NULL CHECK (length(proposal_digest) = 32),
            canonical_proposal       TEXT    NOT NULL,
            normalization_provenance TEXT    NOT NULL,
            created_at               INTEGER NOT NULL
        ) STRICT;",
    },
];

/// Runs the production migration set against `conn`.
pub(crate) fn run(conn: &mut Connection) -> Result<(), StoreError> {
    run_with(conn, MIGRATIONS)
}

/// Runs `migrations` against `conn`. Exposed at this granularity so tests
/// can exercise ordering and fail-closed behavior against a fixture
/// migration set without touching the production schema.
pub(crate) fn run_with(conn: &mut Connection, migrations: &[Migration]) -> Result<(), StoreError> {
    // Reject an unsupported newer schema before writing anything at all —
    // including the bookkeeping table below — so the fail-closed path stays
    // purely read-only rather than performing a (harmless but needless)
    // write against a database this build should not touch further.
    let max_known = migrations.last().map_or(0, |m| m.version);
    let current_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if current_version > max_known {
        return Err(StoreError::UnsupportedSchemaVersion {
            found: current_version,
            max_known,
        });
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            checksum TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        ) STRICT;",
    )?;

    verify_recorded_state(conn, migrations, current_version)?;

    for migration in migrations.iter().filter(|m| m.version > current_version) {
        apply_one(conn, migration)?;
    }

    Ok(())
}

/// Confirms `schema_migrations` recorded exactly the contiguous,
/// checksum-matching prefix of `migrations` that `user_version` claims is
/// applied, before trusting the database enough to apply anything further.
fn verify_recorded_state(
    conn: &Connection,
    migrations: &[Migration],
    current_version: i64,
) -> Result<(), StoreError> {
    let mut stmt =
        conn.prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut expected_next = 1i64;
    for row in rows {
        let (version, recorded_checksum) = row?;
        if version != expected_next {
            return Err(StoreError::InconsistentMigrationState(format!(
                "schema_migrations has a gap: expected version {expected_next}, found {version}"
            )));
        }
        let migration = migrations
            .iter()
            .find(|m| m.version == version)
            .ok_or_else(|| {
                StoreError::InconsistentMigrationState(format!(
                    "schema_migrations records applied version {version}, \
                     which is not a migration this build knows"
                ))
            })?;
        let expected_checksum = checksum(migration.sql);
        if recorded_checksum != expected_checksum {
            return Err(StoreError::InconsistentMigrationState(format!(
                "schema_migrations checksum for version {version} does not \
                 match the compiled migration"
            )));
        }
        expected_next += 1;
    }

    let highest_recorded = expected_next - 1;
    if highest_recorded != current_version {
        return Err(StoreError::InconsistentMigrationState(format!(
            "PRAGMA user_version={current_version} does not match highest \
             recorded migration {highest_recorded}"
        )));
    }

    Ok(())
}

fn apply_one(conn: &mut Connection, migration: &Migration) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    tx.execute_batch(migration.sql)
        .map_err(|source| StoreError::MigrationFailed {
            version: migration.version,
            name: migration.name,
            source,
        })?;

    tx.pragma_update(None, "user_version", migration.version)?;
    tx.execute(
        // unixepoch() keeps applied_at an integral base unit (whole seconds
        // since the epoch), per the contract's "integral base units for
        // ... timestamps" operating rule, rather than an ISO-8601 string.
        "INSERT INTO schema_migrations (version, name, checksum, applied_at)
         VALUES (?1, ?2, ?3, unixepoch())",
        rusqlite::params![migration.version, migration.name, checksum(migration.sql)],
    )?;

    tx.commit()?;
    Ok(())
}

/// A small dependency-free checksum over compiled-in migration SQL.
///
/// Migration text is compiled into this binary, not adversarial input, so
/// the goal is catching accidental drift between what a database recorded
/// and what this build would apply — not defeating a deliberate attacker.
/// FNV-1a is sufficient for that and avoids taking on a cryptographic-hash
/// dependency this mission does not otherwise need.
fn checksum(sql: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in sql.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests;
