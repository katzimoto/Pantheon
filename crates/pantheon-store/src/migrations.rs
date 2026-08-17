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
//! This mission's initial schema is deliberately minimal: migration
//! bookkeeping plus the `system_state` singleton needed for installation
//! identity / RestoreGeneration. It does not implement any future
//! production table family.

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
];

/// Runs the production migration set against `conn`.
pub(crate) fn run(conn: &mut Connection) -> Result<(), StoreError> {
    run_with(conn, MIGRATIONS)
}

/// Runs `migrations` against `conn`. Exposed at this granularity so tests
/// can exercise ordering and fail-closed behavior against a fixture
/// migration set without touching the production schema.
pub(crate) fn run_with(conn: &mut Connection, migrations: &[Migration]) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            checksum TEXT NOT NULL,
            applied_at TEXT NOT NULL
        ) STRICT;",
    )?;

    let max_known = migrations.last().map_or(0, |m| m.version);
    let current_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if current_version > max_known {
        return Err(StoreError::UnsupportedSchemaVersion {
            found: current_version,
            max_known,
        });
    }

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
        "INSERT INTO schema_migrations (version, name, checksum, applied_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
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
        assert_eq!(user_version, 2);

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
                max_known: 2
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
