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
mod tests {
    use super::*;

    fn open_conn() -> (crate::test_support::TempDir, Connection) {
        let dir = crate::test_support::TempDir::new("migrations");
        let conn = Connection::open(dir.path().join("pantheon.db")).expect("open connection");
        (dir, conn)
    }

    #[test]
    fn applies_migrations_in_order_and_records_bookkeeping() {
        let (_dir, mut conn) = open_conn();

        run_with(&mut conn, MIGRATIONS).expect("migrations apply");

        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(user_version, 4);

        let mut stmt = conn
            .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
            .unwrap();
        let applied: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            applied,
            vec![
                (1, "create_system_state".to_string()),
                (2, "bootstrap_installation_identity".to_string()),
                (3, "create_command_and_journal_state".to_string()),
                (4, "bootstrap_journal_epoch".to_string()),
            ]
        );

        // Migration 2 could only have succeeded after migration 1's table
        // existed, which is the ordering guarantee under test.
        let generation: String = conn
            .query_row(
                "SELECT restore_generation FROM system_state WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(generation.len(), 32);
    }

    #[test]
    fn reopening_an_already_migrated_database_is_a_no_op() {
        let (_dir, mut conn) = open_conn();
        run_with(&mut conn, MIGRATIONS).expect("first run applies migrations");
        let generation_before: String = conn
            .query_row(
                "SELECT restore_generation FROM system_state WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();

        run_with(&mut conn, MIGRATIONS).expect("second run is a no-op");
        let generation_after: String = conn
            .query_row(
                "SELECT restore_generation FROM system_state WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(generation_before, generation_after);
    }

    #[test]
    fn failing_migration_leaves_no_partial_schema_and_does_not_advance_version() {
        let (_dir, mut conn) = open_conn();

        let migrations = [
            Migration {
                version: 1,
                name: "create_marker",
                sql: "CREATE TABLE marker (id INTEGER PRIMARY KEY) STRICT;",
            },
            Migration {
                version: 2,
                name: "deliberately_broken",
                // References a table that does not exist: this must fail
                // and roll back rather than partially apply.
                sql: "CREATE TABLE also_marker (id INTEGER PRIMARY KEY) STRICT;
                      INSERT INTO does_not_exist (id) VALUES (1);",
            },
        ];

        let err = run_with(&mut conn, &migrations).expect_err("migration 2 must fail");
        assert!(matches!(
            err,
            StoreError::MigrationFailed {
                version: 2,
                name: "deliberately_broken",
                ..
            }
        ));

        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            user_version, 1,
            "version must not advance past the last successful migration"
        );

        let recorded: Vec<i64> = conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            recorded,
            vec![1],
            "only the successful migration is recorded"
        );

        let also_marker_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'also_marker')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !also_marker_exists,
            "the failed migration's own DDL must not partially commit"
        );
    }

    #[test]
    fn unsupported_newer_schema_version_fails_closed() {
        let (_dir, mut conn) = open_conn();
        run_with(&mut conn, MIGRATIONS).expect("migrations apply");
        conn.pragma_update(None, "user_version", 999i64).unwrap();

        let err = run_with(&mut conn, MIGRATIONS).expect_err("must reject unknown newer schema");
        assert!(matches!(
            err,
            StoreError::UnsupportedSchemaVersion {
                found: 999,
                max_known: 4
            }
        ));
    }

    #[test]
    fn tampered_checksum_fails_closed() {
        let (_dir, mut conn) = open_conn();
        run_with(&mut conn, MIGRATIONS).expect("migrations apply");
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'tampered' WHERE version = 1",
            [],
        )
        .unwrap();

        let err = run_with(&mut conn, MIGRATIONS).expect_err("must reject checksum mismatch");
        assert!(matches!(err, StoreError::InconsistentMigrationState(_)));
    }

    #[test]
    fn gap_in_bookkeeping_fails_closed() {
        let (_dir, mut conn) = open_conn();
        run_with(&mut conn, MIGRATIONS).expect("migrations apply");
        conn.execute("DELETE FROM schema_migrations WHERE version = 1", [])
            .unwrap();

        let err = run_with(&mut conn, MIGRATIONS).expect_err("must reject a bookkeeping gap");
        assert!(matches!(err, StoreError::InconsistentMigrationState(_)));
    }
}
