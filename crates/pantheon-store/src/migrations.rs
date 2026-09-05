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
//! The schema is deliberately minimal, and grows only with the behaviour
//! that reads it: migration bookkeeping and the `system_state` singleton for
//! installation identity / RestoreGeneration; the durable command ledger,
//! Event Journal and journal epoch/sequence state the command mutation kernel
//! requires; the configuration component/revision/active-pointer families;
//! the Goal, planning, TaskGraph and Task families; and the Task-owned
//! `workspaces` family. No production table family is created ahead of the
//! behaviour that needs it.
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
    Migration {
        version: 8,
        name: "create_task_workspaces",
        // `workspaces` is the canonical table name from the persistence
        // contract's "Table families" section (SANDBOX / WORKSPACE). The
        // sibling families named there — `repositories`,
        // `workspace_revisions`, `integration_intents` — arrive with the
        // behaviour that needs them, as every other family here has.
        //
        // The phase domain is the full canonical list from
        // docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md
        // ("Workspace phases"), not only the phases this mission drives, for
        // the same reason `tasks` accepts all ten Task phases.
        sql: "CREATE TABLE workspaces (
            id              TEXT    NOT NULL PRIMARY KEY,
            -- Workspace ownership is Task-scoped and survives Run turnover,
            -- so the Task is the holder and there is no Run column here.
            task_id         TEXT    NOT NULL REFERENCES tasks(id),
            -- The opaque repository reference the Task declared. Recorded
            -- verbatim: no canonical contract defines a URI shape to parse.
            repository      TEXT    NOT NULL CHECK (length(repository) BETWEEN 1 AND 512),
            -- The controller-trusted local repository root. Global recovery
            -- requires durable Pantheon records to define the roots recovery
            -- may inspect, rather than following repository indirection to
            -- discover one.
            source_path     TEXT    NOT NULL CHECK (length(source_path) > 0),
            -- What was asked for, which may move afterwards.
            requested_base  TEXT    NOT NULL CHECK (length(requested_base) BETWEEN 1 AND 255),
            -- What it resolved to, once. Immutable for the life of the
            -- Workspace: 40 hexadecimal characters for a SHA-1 repository,
            -- 64 for a SHA-256 one.
            resolved_base   TEXT    NOT NULL CHECK (length(resolved_base) IN (40, 64)),
            phase           TEXT    NOT NULL CHECK (phase IN (
                'Requested', 'Materializing', 'Ready', 'Frozen',
                'Releasing', 'Released', 'Error')),
            -- The strongest factual observation about external state, kept
            -- independent of `phase` for the reason the Sandbox contract
            -- gives: a controller error never proves external absence.
            materialization TEXT    NOT NULL CHECK (materialization IN (
                'Present', 'Absent', 'Unknown')),
            revision        INTEGER NOT NULL CHECK (revision > 0),
            created_at      INTEGER NOT NULL,
            -- A partially materialized Workspace can never be reported
            -- Ready. This is the mission's central fence, so it is a
            -- database constraint rather than a controller promise.
            CHECK (phase != 'Ready' OR materialization = 'Present'),
            -- Durable intention exists before any side effect: while a
            -- Workspace is still Requested, recovery may conclude that no
            -- external state exists. Crossing to Materializing is what
            -- gives up that conclusion.
            CHECK (phase != 'Requested' OR materialization = 'Absent'),
            -- Releasing the Task's Workspace slot requires established
            -- absence, never a timeout or an error.
            CHECK (phase != 'Released' OR materialization = 'Absent')
        ) STRICT;

        -- A coding Task owns exactly one current Workspace. Enforced by the
        -- database because it is a cardinality invariant, not a scheduling
        -- convention: two concurrent materializations for one Task must fail
        -- rather than race to create a second authority.
        CREATE UNIQUE INDEX workspaces_one_current_per_task
            ON workspaces (task_id) WHERE phase != 'Released';",
    },
    Migration {
        version: 9,
        name: "create_scheduling_and_run_intent",
        // Table names follow the canonical persistence contract's "Durable
        // scheduler state", "ExecutionBinding", "Context source snapshot and
        // ContextPlan attachment" and "Run status" sections, which sit in the
        // SCHEDULING / EXECUTION table family. Only the columns this mission's
        // behaviour writes exist: lease/attempt/candidate columns arrive with
        // the behaviour that needs them, as every other family here has.
        //
        // One deliberate deviation from the contract's sketch is recorded
        // here rather than silently made: the composite foreign key from
        // `tasks.active_run_id` to `runs(id, task_id)` is not added, because
        // SQLite cannot add a constraint to the existing `tasks` table
        // without rebuilding it. Holder safety is carried instead by
        // `run_status`'s own composite FK plus the T3 transaction that writes
        // both rows atomically; a later migration that legitimately rebuilds
        // `tasks` should add it then.
        sql: "CREATE TABLE scheduler_state (
            id                    TEXT    NOT NULL PRIMARY KEY CHECK (id = 'singleton'),
            -- Operator desired state only. It is never rewritten to stand for
            -- recovery or configuration readiness.
            dispatch_mode         TEXT    NOT NULL CHECK (dispatch_mode IN ('RUNNING', 'PAUSED')),
            -- Logical fairness counter. Advances only inside a successful T3,
            -- never from MAX()+1 or a wall clock.
            next_service_sequence INTEGER NOT NULL CHECK (next_service_sequence >= 1),
            revision              INTEGER NOT NULL CHECK (revision > 0),
            updated_at            INTEGER NOT NULL
        ) STRICT;

        CREATE TABLE goal_scheduling_state (
            goal_id              TEXT    NOT NULL PRIMARY KEY REFERENCES goals(id),
            -- NULL means this Goal has never successfully received service.
            last_served_sequence INTEGER CHECK (
                last_served_sequence IS NULL OR last_served_sequence >= 1),
            revision             INTEGER NOT NULL CHECK (revision > 0),
            created_at           INTEGER NOT NULL,
            updated_at           INTEGER NOT NULL
        ) STRICT;

        CREATE TABLE task_scheduling_state (
            task_id                  TEXT    NOT NULL PRIMARY KEY REFERENCES tasks(id),
            -- Start of the current SchedulingEligible interval. NULL exactly
            -- when the Task is not currently eligible; written with the
            -- transition that changes eligibility, not by queue rebuilds.
            eligible_since           INTEGER,
            -- Temporary backoff suppression point. Never mutates Task
            -- lifecycle and never resets eligible_since.
            next_attempt_at          INTEGER,
            last_failure_code        TEXT,
            last_failure_detail_json TEXT,
            revision                 INTEGER NOT NULL CHECK (revision > 0),
            updated_at               INTEGER NOT NULL
        ) STRICT;

        CREATE TABLE execution_bindings (
            -- Content-addressed and immutable: the same frozen strategy is
            -- the same row, reached twice or not.
            digest                            BLOB NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            task_id                           TEXT NOT NULL REFERENCES tasks(id),
            configuration_activation_sequence INTEGER NOT NULL,
            configuration_content_digest      BLOB NOT NULL CHECK (length(configuration_content_digest) = 32),
            canonical_json                    TEXT NOT NULL,
            created_at                        INTEGER NOT NULL
        ) STRICT;

        CREATE TABLE context_source_snapshots (
            digest                            BLOB NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            configuration_activation_sequence INTEGER NOT NULL,
            context_policy_digest             BLOB NOT NULL CHECK (length(context_policy_digest) = 32),
            task_spec_digest                  BLOB NOT NULL CHECK (length(task_spec_digest) = 32),
            goal_id                           TEXT NOT NULL REFERENCES goals(id),
            goal_revision                     INTEGER NOT NULL,
            graph_revision                    INTEGER NOT NULL,
            agent_name                        TEXT NOT NULL,
            agent_version                     INTEGER NOT NULL,
            workspace_id                      TEXT NOT NULL REFERENCES workspaces(id),
            workspace_resolved_base           TEXT NOT NULL,
            canonical_json                    TEXT NOT NULL,
            created_at                        INTEGER NOT NULL
        ) STRICT;

        CREATE TABLE runs (
            id                             TEXT    NOT NULL PRIMARY KEY,
            task_id                        TEXT    NOT NULL REFERENCES tasks(id),
            binding_digest                 BLOB    NOT NULL REFERENCES execution_bindings(digest),
            context_source_snapshot_digest BLOB    NOT NULL REFERENCES context_source_snapshots(digest),
            created_at                     INTEGER NOT NULL,
            -- The composite parent key the holder-safe FKs below require.
            UNIQUE (id, task_id)
        ) STRICT;

        CREATE TABLE run_status (
            run_id          TEXT    NOT NULL PRIMARY KEY,
            -- Immutable copy of runs.task_id, constrained to agree through
            -- the composite FK: run holder identity cannot drift.
            task_id         TEXT    NOT NULL,
            phase           TEXT    NOT NULL CHECK (phase IN (
                'Active', 'Finalizing', 'Completed', 'Failed',
                'Cancelled', 'Yielded')),
            terminal_target TEXT,
            revision        INTEGER NOT NULL CHECK (revision > 0),
            -- The v0.1.0 single global execution slot. A nonterminal Run holds
            -- it installation-wide; reaching a terminal phase releases it.
            active_slot     TEXT,
            updated_at      INTEGER NOT NULL,
            FOREIGN KEY (run_id, task_id) REFERENCES runs(id, task_id),
            CHECK (
                phase != 'Finalizing' OR terminal_target IS NOT NULL
            ),
            CHECK (
                (phase IN ('Active', 'Finalizing') AND active_slot = 'global')
                OR (phase NOT IN ('Active', 'Finalizing') AND active_slot IS NULL)
            )
        ) STRICT;

        -- At most one nonterminal Run per Task: the canonical one-live-Run
        -- rule as a real database backstop, not controller discipline.
        CREATE UNIQUE INDEX one_nonterminal_run_per_task
            ON run_status (task_id) WHERE phase IN ('Active', 'Finalizing');

        -- The MVP's one deliberate global active-execution slot: at most one
        -- nonterminal Run installation-wide. Two concurrent T3 commits under
        -- different command identities therefore cannot both succeed even if
        -- every other fence were removed.
        CREATE UNIQUE INDEX one_active_execution_slot
            ON run_status (active_slot) WHERE active_slot IS NOT NULL;",
    },
    Migration {
        version: 10,
        name: "bootstrap_scheduler_state",
        // Same idempotent shape as the other bootstrap migrations. Dispatch
        // defaults to RUNNING because nothing canonical names a paused
        // default; an operator pause is a deliberate durable act that then
        // survives restart.
        //
        // The second statement backfills eligibility state for Tasks that are
        // already Ready on an upgraded database: materialization begins
        // recording `eligible_since` only from this schema version onward, and
        // an existing Ready Task's current eligibility interval is taken to
        // have started at migration time — the earliest instant this build can
        // honestly name.
        sql: "INSERT INTO scheduler_state (id, dispatch_mode, next_service_sequence, revision, updated_at)
            SELECT 'singleton', 'RUNNING', 1, 1, unixepoch()
            WHERE NOT EXISTS (SELECT 1 FROM scheduler_state WHERE id = 'singleton');

        INSERT INTO task_scheduling_state (task_id, eligible_since, next_attempt_at,
                                           last_failure_code, last_failure_detail_json,
                                           revision, updated_at)
            SELECT t.id, unixepoch(), NULL, NULL, NULL, 1, unixepoch()
            FROM tasks t
            WHERE t.phase = 'Ready'
              AND NOT EXISTS (
                  SELECT 1 FROM task_scheduling_state s WHERE s.task_id = t.id);",
    },
    Migration {
        version: 11,
        name: "create_artifacts_and_workspace_revisions",
        // The ARTIFACTS and WORKSPACE families from the canonical
        // persistence contract's "Table families" section that Issue #32's
        // sealing path needs: `workspace_revisions` (SANDBOX / WORKSPACE)
        // and `blobs` / `artifacts` / `artifact_members` (ARTIFACTS). The
        // remaining families named there — production records, retention,
        // candidates — arrive with the behaviour that reads them.
        //
        // Content identity is separate from production metadata throughout:
        // every row below is keyed by content or by an immutable binding,
        // and the mutable provenance (`created_at`) is a column, never part
        // of identity.
        sql: "CREATE TABLE blobs (
            -- Blob identity is digest + size; the digest is the key and the
            -- size is checked against it on every publication.
            digest     BLOB    NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            size       INTEGER NOT NULL CHECK (size >= 0),
            created_at INTEGER NOT NULL
        ) STRICT;

        CREATE TABLE artifacts (
            -- Artifact identity is the digest of the canonical manifest.
            -- Reaching the same manifest twice is one row, not two.
            digest         BLOB    NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            artifact_kind  TEXT    NOT NULL CHECK (length(artifact_kind) BETWEEN 1 AND 128),
            canonical_json TEXT    NOT NULL,
            created_at     INTEGER NOT NULL
        ) STRICT;

        -- Relational reachability between an Artifact and the Blobs its
        -- semantics require, so CAS completeness/GC has a graph to follow
        -- that does not depend on re-parsing manifests.
        CREATE TABLE artifact_members (
            artifact_digest BLOB NOT NULL REFERENCES artifacts(digest),
            blob_digest     BLOB NOT NULL REFERENCES blobs(digest),
            PRIMARY KEY (artifact_digest, blob_digest)
        ) STRICT;

        CREATE TABLE workspace_revisions (
            id             TEXT    NOT NULL PRIMARY KEY CHECK (length(id) = 32),
            workspace_id   TEXT    NOT NULL REFERENCES workspaces(id),
            -- The repository identity and immutable base this checkpoint was
            -- captured against, copied from the Workspace's durable binding.
            repository     TEXT    NOT NULL CHECK (length(repository) BETWEEN 1 AND 512),
            resolved_base  TEXT    NOT NULL CHECK (length(resolved_base) IN (40, 64)),
            -- The immutable semantic-state digest: deterministic over the
            -- captured logical state alone. One Workspace cannot hold two
            -- rows claiming different content for the same state digest, so
            -- re-capturing identical state converges on the existing row.
            state_digest   BLOB    NOT NULL CHECK (length(state_digest) = 32),
            canonical_json TEXT    NOT NULL,
            created_at     INTEGER NOT NULL,
            UNIQUE (workspace_id, state_digest)
        ) STRICT;",
    },
    Migration {
        version: 12,
        name: "create_context_plan_attachment",
        // The context-plan families from the canonical persistence contract's
        // "Context source snapshot and ContextPlan attachment" section, which
        // Issue #30's preparation path needs:
        //
        // - `context_plans` — immutable content-addressed plans;
        // - `run_context_plans` — the one-time Run attachment.
        //
        // The composite foreign keys are quoted from that contract rather
        // than invented here: they prove that an attached plan was built from
        // exactly the source snapshot its Run froze at T3, and `run_id` as
        // primary key proves at most one attachment per Run.
        //
        // The parent key `runs(id, context_source_snapshot_digest)` did not
        // exist when migration 9 created `runs`, and SQLite cannot add a
        // UNIQUE constraint to a table without rebuilding it — so it arrives
        // here as a unique index, which is what SQLite requires of a
        // composite foreign-key parent anyway. No existing table is rebuilt.
        //
        // Two nullable columns extend `context_source_snapshots` with the
        // frozen Agent guidance digests this mission added to the snapshot
        // vocabulary. ALTER TABLE cannot add a CHECK or NOT NULL column
        // without a default, so nullability is the schema-level shape; every
        // snapshot written from this build onward populates both columns in
        // T3, and preparation fails closed on a legacy NULL rather than
        // substituting current guidance.
        sql: "ALTER TABLE context_source_snapshots
                  ADD COLUMN agent_soul_digest BLOB CHECK (length(agent_soul_digest) = 32);

        ALTER TABLE context_source_snapshots
                  ADD COLUMN agent_behavior_digest BLOB CHECK (length(agent_behavior_digest) = 32);

        -- Composite parent key for run_context_plans' holder-safe FK below.
        CREATE UNIQUE INDEX runs_id_and_source_snapshot
            ON runs (id, context_source_snapshot_digest);

        CREATE TABLE context_plans (
            digest                 BLOB    NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            -- A plan exists only relative to one exact frozen source universe.
            source_snapshot_digest BLOB    NOT NULL
                                           REFERENCES context_source_snapshots(digest),
            builder_version        TEXT    NOT NULL
                                           CHECK (length(builder_version) BETWEEN 1 AND 128),
            canonical_json         TEXT    NOT NULL,
            created_at             INTEGER NOT NULL,
            UNIQUE (digest, source_snapshot_digest)
        ) STRICT;

        CREATE TABLE run_context_plans (
            -- Primary key over the Run alone: at most one attachment per Run,
            -- enforced by the database rather than controller discipline.
            -- Existence of the Run is proved by the composite FK below, which
            -- also proves the snapshot pairing; a bare run_id FK would say
            -- less.
            run_id                         TEXT    NOT NULL PRIMARY KEY,
            context_source_snapshot_digest BLOB    NOT NULL,
            context_plan_digest            BLOB    NOT NULL
                                                   CHECK (length(context_plan_digest) = 32),
            attached_at                    INTEGER NOT NULL,
            -- Holder-safe FKs proving both halves at once: the attachment
            -- names THIS Run's exact frozen snapshot, and the plan was built
            -- from that same snapshot. A plan for another snapshot is not
            -- merely rejected by the controller; the database refuses it.
            FOREIGN KEY (run_id, context_source_snapshot_digest)
                REFERENCES runs(id, context_source_snapshot_digest),
            FOREIGN KEY (context_plan_digest, context_source_snapshot_digest)
                REFERENCES context_plans(digest, source_snapshot_digest)
        ) STRICT;",
    },
    Migration {
        version: 13,
        name: "create_attempt_lineage",
        // The execution-lineage families Issue #31's Run Controller needs,
        // from the canonical persistence contract's "Attempt and
        // launch-contact state" and "Agent Control" sections:
        //
        // - `attempts` — immutable identity: one ordinal within its Run and
        //   one globally unique LaunchKey;
        // - `attempt_status` — mutable lifecycle carrying an immutable copy of
        //   `run_id`, so the one-nonterminal-Attempt rule can be a partial
        //   unique index where terminality lives;
        // - `agent_control_sessions` — the Attempt-scoped worker identity.
        //   Only the SHA-256 *verifier* of the raw bearer is ever stored;
        //   bearer material never reaches this database at all.
        //
        // `run_status.current_attempt_id` also arrives here. SQLite cannot
        // attach a foreign key to an existing table without rebuilding it, so
        // unlike migration 12's additive columns this migration rebuilds
        // `run_status`: identical columns and constraints plus the new
        // nullable pointer, constrained through the same holder-safe composite
        // FK shape the contract specifies — `(current_attempt_id, run_id)`
        // must name exactly this Run's Attempt. Existing rows are copied in
        // the same transaction, so the rebuild commits or rolls back whole.
        sql: "CREATE TABLE attempts (
            id         TEXT    NOT NULL PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 128),
            run_id     TEXT    NOT NULL REFERENCES runs(id),
            -- Position of this Attempt within its Run's history, 1-based,
            -- assigned inside T4/T8. Not unique across Runs.
            ordinal    INTEGER NOT NULL CHECK (ordinal >= 1),
            -- The immutable external-launch idempotency key. 64 hex
            -- characters = 256 bits drawn from the installation's entropy
            -- source before any external contact.
            launch_key TEXT    NOT NULL UNIQUE CHECK (length(launch_key) = 64),
            created_at INTEGER NOT NULL,
            -- Composite parent key for the holder-safe FKs below.
            UNIQUE (id, run_id)
        ) STRICT;

        CREATE TABLE attempt_status (
            attempt_id               TEXT    NOT NULL PRIMARY KEY,
            -- Immutable copy of attempts.run_id, constrained to agree by the
            -- composite FK: Attempt holder identity cannot drift.
            run_id                   TEXT    NOT NULL,
            observed_execution       TEXT    NOT NULL CHECK (observed_execution IN (
                                         'ABSENT', 'STARTING', 'RUNNING',
                                         'EXITED', 'UNKNOWN')),
            terminal                 INTEGER NOT NULL CHECK (terminal IN (0, 1)),
            revision                 INTEGER NOT NULL CHECK (revision > 0),
            launch_contact_state     TEXT    NOT NULL CHECK (launch_contact_state IN (
                                         'NOT_CONTACTED', 'CONTACT_MAY_HAVE_OCCURRED')),
            launch_contact_initiated_at INTEGER,
            -- Provenance of the controller incarnation that crossed the
            -- boundary, recorded with the state it belongs to.
            launch_contact_epoch     TEXT,
            started_at               INTEGER,
            finished_at              INTEGER,
            updated_at               INTEGER NOT NULL,
            CHECK ((launch_contact_state = 'NOT_CONTACTED')
                   = (launch_contact_initiated_at IS NULL)
                   AND (launch_contact_state = 'NOT_CONTACTED')
                   = (launch_contact_epoch IS NULL)),
            CHECK (terminal != 0 OR finished_at IS NULL),
            FOREIGN KEY (attempt_id, run_id) REFERENCES attempts(id, run_id)
        ) STRICT;

        -- V1 permits at most one nonterminal Attempt per Run. Enforced here,
        -- on the status table where terminality lives, not only by the T4/T8
        -- transaction that re-reads it.
        CREATE UNIQUE INDEX one_nonterminal_attempt_per_run
            ON attempt_status (run_id) WHERE terminal = 0;

        CREATE TABLE agent_control_sessions (
            id                  TEXT    NOT NULL PRIMARY KEY
                                        CHECK (length(id) BETWEEN 1 AND 128),
            attempt_id          TEXT    NOT NULL UNIQUE REFERENCES attempts(id),
            -- Immutable authority fencing: the RestoreGeneration this session
            -- was created under, copied from system_state inside T4/T8.
            restore_generation  TEXT    NOT NULL CHECK (length(restore_generation) = 32),
            credential_revision INTEGER NOT NULL CHECK (credential_revision >= 1),
            -- One-way verifier of the raw bearer; the bearer itself is never
            -- persisted anywhere.
            credential_hash     BLOB    NOT NULL CHECK (length(credential_hash) = 32),
            credential_rekeyed_at INTEGER,
            state               TEXT    NOT NULL CHECK (state IN ('ACTIVE', 'REVOKED')),
            created_at          INTEGER NOT NULL,
            revoked_at          INTEGER,
            revocation_reason   TEXT,
            CHECK ((state = 'ACTIVE') = (revoked_at IS NULL))
        ) STRICT;

        CREATE TABLE run_status_new (
            run_id             TEXT    NOT NULL PRIMARY KEY,
            task_id            TEXT    NOT NULL,
            phase              TEXT    NOT NULL CHECK (phase IN (
                'Active', 'Finalizing', 'Completed', 'Failed',
                'Cancelled', 'Yielded')),
            terminal_target    TEXT,
            revision           INTEGER NOT NULL CHECK (revision > 0),
            active_slot        TEXT,
            current_attempt_id TEXT,
            updated_at         INTEGER NOT NULL,
            FOREIGN KEY (run_id, task_id) REFERENCES runs(id, task_id),
            -- Holder safety for the current-Attempt pointer: it may only name
            -- an Attempt that exists and belongs to exactly this Run.
            FOREIGN KEY (current_attempt_id, run_id) REFERENCES attempts(id, run_id),
            CHECK (
                phase != 'Finalizing' OR terminal_target IS NOT NULL
            ),
            CHECK (
                (phase IN ('Active', 'Finalizing') AND active_slot = 'global')
                OR (phase NOT IN ('Active', 'Finalizing') AND active_slot IS NULL)
            )
        ) STRICT;

        INSERT INTO run_status_new
            SELECT run_id, task_id, phase, terminal_target, revision,
                   active_slot, NULL, updated_at
            FROM run_status;
        DROP TABLE run_status;
        ALTER TABLE run_status_new RENAME TO run_status;

        CREATE UNIQUE INDEX one_nonterminal_run_per_task
            ON run_status (task_id) WHERE phase IN ('Active', 'Finalizing');

        CREATE UNIQUE INDEX one_active_execution_slot
            ON run_status (active_slot) WHERE active_slot IS NOT NULL;",
    },
    Migration {
        version: 14,
        name: "create_candidate_submission",
        // The families Issue #33's Agent Control submission path needs, from
        // the canonical persistence contract's ARTIFACTS table family
        // ("production_records", "candidates", "candidate_outputs") and its
        // SCHEDULING / EXECUTION family ("agent_requests"), plus the
        // `run_status.candidate_digest` fact the "Run status" section makes
        // load-bearing (`Run Completed => candidate_digest not null`).
        //
        // Content identity stays separate from production provenance
        // throughout, per the Artifact model: producing the same Artifact
        // twice yields ONE content row and MULTIPLE ProductionRecords — one
        // per (producing Run, output slot). The Candidate is keyed by the
        // digest of its canonical semantic document and constrained to at
        // most one per Run; its outputs reach Artifacts only *through* the
        // producing Run's ProductionRecord for that exact slot, so content
        // reuse across Runs can never masquerade as this Run's own work.
        //
        // `agent_requests` is the worker-facing idempotency/authority ledger.
        // Its identity is `(attempt_id, request_id)`; RestoreGeneration is
        // deliberately absent because a stale-generation session fails before
        // request lookup/creation ever runs, and credential revision is
        // deliberately absent because a successful worker request can occur
        // only after launch contact froze the session credential.
        //
        // As in migration 13, `run_status` gains a column through a rebuild:
        // SQLite cannot attach either a column-with-CHECK semantics change or
        // a new table-level CHECK to an existing table. Existing rows are
        // copied unchanged (their `candidate_digest` is NULL: nothing could
        // legally complete a Run before this schema existed); the rebuild
        // commits or rolls back whole.
        sql: "CREATE TABLE agent_requests (
            attempt_id   TEXT    NOT NULL CHECK (length(attempt_id) BETWEEN 1 AND 128),
            request_id   TEXT    NOT NULL CHECK (length(request_id) BETWEEN 1 AND 128),
            -- One-way digest of the normalized operation and its semantic
            -- payload. Raw bearer material is never persisted anywhere and
            -- secret-derived material never enters this hash.
            request_hash BLOB    NOT NULL CHECK (length(request_hash) = 32),
            operation    TEXT    NOT NULL CHECK (operation IN
                                     ('artifact.seal', 'task.submit_result')),
            state        TEXT    NOT NULL CHECK (state IN
                                     ('STARTED', 'SUCCEEDED', 'FAILED')),
            result_ref   TEXT    CHECK (result_ref IS NULL
                                     OR length(result_ref) BETWEEN 1 AND 128),
            problem_code TEXT    CHECK (problem_code IS NULL
                                     OR length(problem_code) BETWEEN 1 AND 64),
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL,
            PRIMARY KEY (attempt_id, request_id),
            FOREIGN KEY (attempt_id) REFERENCES attempts(id),
            CHECK ((state = 'SUCCEEDED') = (result_ref IS NOT NULL)),
            CHECK ((state = 'FAILED') = (problem_code IS NOT NULL))
        ) STRICT;

        CREATE TABLE production_records (
            -- Identity is the producing Run and the exact TaskSpec output
            -- slot: one Run produces one settled Artifact per slot (the
            -- frozen-Workspace fence makes re-sealing converge), while the
            -- same Artifact content may be produced independently by many
            -- Runs. Content reuse is recorded, never conflated with current
            -- ownership.
            run_id                TEXT    NOT NULL,
            output_slot           TEXT    NOT NULL
                                          CHECK (length(output_slot) BETWEEN 1 AND 128),
            task_id               TEXT    NOT NULL,
            attempt_id            TEXT    NOT NULL,
            workspace_revision_id TEXT    NOT NULL
                                          REFERENCES workspace_revisions(id),
            artifact_digest       BLOB    NOT NULL
                                          REFERENCES artifacts(digest),
            created_at            INTEGER NOT NULL,
            PRIMARY KEY (run_id, output_slot),
            -- Holder safety: the recorded Task owns the Run, and the recorded
            -- Attempt belongs to that Run. Neither pointer can drift.
            FOREIGN KEY (run_id, task_id) REFERENCES runs(id, task_id),
            FOREIGN KEY (attempt_id, run_id) REFERENCES attempts(id, run_id),
            -- Parent key for candidate_outputs' provenance foreign key: the
            -- exact Artifact bound to this Run and this slot.
            UNIQUE (run_id, output_slot, artifact_digest)
        ) STRICT;

        CREATE TABLE candidates (
            -- Immutable content-addressed identity over the canonical
            -- semantic document Pantheon computes; submitters cannot mint it.
            digest         BLOB    NOT NULL PRIMARY KEY CHECK (length(digest) = 32),
            task_id        TEXT    NOT NULL,
            run_id         TEXT    NOT NULL,
            canonical_json TEXT    NOT NULL,
            created_at     INTEGER NOT NULL,
            -- At most one Candidate per Run, enforced by the database rather
            -- than controller discipline.
            UNIQUE (run_id),
            FOREIGN KEY (run_id, task_id) REFERENCES runs(id, task_id)
        ) STRICT;

        CREATE TABLE candidate_outputs (
            candidate_digest BLOB    NOT NULL REFERENCES candidates(digest),
            output_slot      TEXT    NOT NULL CHECK (length(output_slot) BETWEEN 1 AND 128),
            artifact_digest  BLOB    NOT NULL REFERENCES artifacts(digest),
            -- The Run whose sealed production the output claims. Provenance
            -- is relational: the composite foreign key below admits only a
            -- ProductionRecord that binds THIS exact Run, THIS exact slot,
            -- and THIS exact Artifact. An Artifact that merely exists under
            -- some other lineage's digest cannot be referenced here.
            production_run_id TEXT   NOT NULL,
            PRIMARY KEY (candidate_digest, output_slot),
            FOREIGN KEY (candidate_digest) REFERENCES candidates(digest),
            FOREIGN KEY (artifact_digest) REFERENCES artifacts(digest),
            FOREIGN KEY (production_run_id, output_slot, artifact_digest)
                REFERENCES production_records(run_id, output_slot, artifact_digest)
        ) STRICT;

        CREATE TABLE run_status_new (
            run_id             TEXT    NOT NULL PRIMARY KEY,
            task_id            TEXT    NOT NULL,
            phase              TEXT    NOT NULL CHECK (phase IN (
                'Active', 'Finalizing', 'Completed', 'Failed',
                'Cancelled', 'Yielded')),
            terminal_target    TEXT,
            revision           INTEGER NOT NULL CHECK (revision > 0),
            active_slot        TEXT,
            current_attempt_id TEXT,
            candidate_digest   BLOB    CHECK (length(candidate_digest) = 32),
            updated_at         INTEGER NOT NULL,
            FOREIGN KEY (run_id, task_id) REFERENCES runs(id, task_id),
            FOREIGN KEY (current_attempt_id, run_id) REFERENCES attempts(id, run_id),
            CHECK (
                phase != 'Finalizing' OR terminal_target IS NOT NULL
            ),
            CHECK (
                phase != 'Completed' OR candidate_digest IS NOT NULL
            ),
            CHECK (
                (phase IN ('Active', 'Finalizing') AND active_slot = 'global')
                OR (phase NOT IN ('Active', 'Finalizing') AND active_slot IS NULL)
            )
        ) STRICT;

        INSERT INTO run_status_new
            SELECT run_id, task_id, phase, terminal_target, revision,
                   active_slot, current_attempt_id, NULL, updated_at
            FROM run_status;
        DROP TABLE run_status;
        ALTER TABLE run_status_new RENAME TO run_status;

        CREATE UNIQUE INDEX one_nonterminal_run_per_task
            ON run_status (task_id) WHERE phase IN ('Active', 'Finalizing');

        CREATE UNIQUE INDEX one_active_execution_slot
            ON run_status (active_slot) WHERE active_slot IS NOT NULL;",
    },
    Migration {
        version: 15,
        name: "create_sandbox_instances",
        // The SANDBOX table family from the canonical persistence contract's
        // "Table families" section, which Issue #34's strict local container
        // SandboxBackend needs.
        //
        // A SandboxInstance belongs to exactly one Run holder. Its lifecycle
        // phase and external observed presence are separate durable facts,
        // per the canonical contract: a controller error is never proof that
        // an external runtime does not exist.
        //
        // The `sandbox_plan_digest` and `environment_identity` are immutable:
        // they are set at creation and never change for the same SandboxKey.
        sql: "CREATE TABLE sandbox_instances (
            id                    TEXT    NOT NULL PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 128),
            -- The Run that owns this Sandbox. A Run has at most one
            -- current/non-RELEASED Sandbox.
            -- NOTE: MVP omits the REFERENCES runs(id) foreign key so that
            -- sandbox store tests can exercise the table without the full
            -- scheduling pipeline.  The RunController is the only production
            -- caller and it already validates run existence before creating
            -- a sandbox.
            run_id                TEXT    NOT NULL,
            -- Immutable identity of the SandboxPlan that authorized this
            -- instance, so verification can check that the external runtime
            -- matches the plan Pantheon committed to.
            sandbox_plan_digest   BLOB    NOT NULL CHECK (length(sandbox_plan_digest) = 32),
            -- Immutable content identity of the execution environment — an
            -- image digest rather than a mutable tag.
            environment_identity  TEXT    NOT NULL CHECK (length(environment_identity) > 0),
            phase                 TEXT    NOT NULL CHECK (phase IN (
                'Requested', 'Preparing', 'Ready', 'Releasing',
                'Released', 'Error')),
            -- The strongest factual observation about external runtime state,
            -- kept independent of `phase`.
            observed_presence     TEXT    NOT NULL CHECK (observed_presence IN (
                'Present', 'Absent', 'Unknown')),
            revision              INTEGER NOT NULL CHECK (revision > 0),
            created_at            INTEGER NOT NULL,
            updated_at            INTEGER NOT NULL,
            -- A partially prepared Sandbox can never be reported Ready.
            CHECK (phase != 'Ready' OR observed_presence = 'Present'),
            -- Durable intention exists before any side effect.
            CHECK (phase != 'Requested' OR observed_presence = 'Absent'),
            -- Ordinary release requires established external absence.
            CHECK (phase != 'Released' OR observed_presence = 'Absent')
        ) STRICT;

        -- At most one current Sandbox per Run: the canonical one-live-Sandbox
        -- rule as a real database backstop.
        CREATE UNIQUE INDEX sandbox_one_current_per_run
            ON sandbox_instances (run_id) WHERE phase != 'Released';",
    },
    Migration {
        version: 16,
        name: "create_sandbox_probe_results",
        // Controller-owned probe evidence for Issue #117.
        // Every failed required probe produces a durable record containing
        // expected-versus-observed facts and the applicable identities.
        // This table is not a parallel log; it is the canonical durable
        // representation of probe evidence, referenced by SandboxKey.
        sql: "CREATE TABLE sandbox_probe_results (
            id                        INTEGER PRIMARY KEY,
            sandbox_id                TEXT    NOT NULL,
            probe_name                TEXT    NOT NULL,
            expected                  TEXT    NOT NULL,
            observed                  TEXT    NOT NULL,
            passed                    INTEGER NOT NULL CHECK (passed IN (0, 1)),
            backend_descriptor        TEXT    NOT NULL,
            backend_version           TEXT    NOT NULL,
            platform                  TEXT    NOT NULL,
            architecture              TEXT    NOT NULL,
            probe_implementation_version TEXT NOT NULL,
            recorded_at               INTEGER NOT NULL
        ) STRICT;

        CREATE INDEX idx_sandbox_probe_results_sandbox_id
            ON sandbox_probe_results (sandbox_id);",
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
