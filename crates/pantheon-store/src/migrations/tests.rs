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
    assert_eq!(user_version, 6);

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
            (5, "create_configuration_authority".to_string()),
            (6, "bootstrap_active_configuration_pointer".to_string()),
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
            max_known: 6
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
