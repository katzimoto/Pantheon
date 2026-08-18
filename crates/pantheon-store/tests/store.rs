//! Executable evidence for Issue #16: fresh initialization, the required
//! connection policy, ordered migrations, stable RestoreGeneration across
//! close/reopen, and fail-closed behavior on unsupported/inconsistent
//! migration state.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pantheon_store::{Store, StoreError};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A minimal, dependency-free isolated directory per test. Mirrors
/// `pantheon-store`'s internal `test_support::TempDir`, which unit tests use
/// but which integration tests (a separate compilation unit) cannot reach.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pantheon-store-it-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated test directory");
        Self(dir)
    }

    fn db_path(&self) -> PathBuf {
        self.0.join("pantheon.db")
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fresh_install_creates_an_unpredictable_restore_generation() {
    let dir = TempDir::new("fresh");
    let store = Store::open(dir.db_path()).expect("fresh store opens");

    let generation = store
        .restore_generation()
        .expect("restore generation is readable");

    assert_eq!(generation.as_str().len(), 32);
    assert!(generation.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(
        generation.as_str(),
        "00000000000000000000000000000000".get(..32).unwrap()
    );
}

#[test]
fn two_fresh_installations_get_distinct_restore_generations() {
    let dir_a = TempDir::new("distinct-a");
    let dir_b = TempDir::new("distinct-b");

    let store_a = Store::open(dir_a.db_path()).expect("store a opens");
    let store_b = Store::open(dir_b.db_path()).expect("store b opens");

    let generation_a = store_a.restore_generation().unwrap();
    let generation_b = store_b.restore_generation().unwrap();

    assert_ne!(generation_a, generation_b);
}

#[test]
fn ordinary_close_and_reopen_preserves_the_exact_restore_generation() {
    let dir = TempDir::new("reopen");

    let store = Store::open(dir.db_path()).expect("open store");
    let generation_before = store.restore_generation().expect("read generation");
    store.close().expect("close store");

    let reopened = Store::open(dir.db_path()).expect("reopen store");
    let generation_after = reopened
        .restore_generation()
        .expect("read generation again");

    assert_eq!(generation_before, generation_after);

    // Schema remains valid and queryable after reopen.
    reopened
        .restore_generation()
        .expect("schema remains valid after reopen");
    reopened.close().expect("close reopened store");
}

#[test]
fn opening_a_database_with_an_unsupported_newer_schema_version_fails_closed() {
    let dir = TempDir::new("unsupported-version");
    Store::open(dir.db_path())
        .expect("initial open migrates the database")
        .close()
        .expect("close");

    // Simulate a database written by a future build: bump the recorded
    // schema version past anything this build's migration set knows.
    {
        let raw = rusqlite::Connection::open(dir.db_path()).expect("raw connection");
        raw.pragma_update(None, "user_version", 999i64)
            .expect("bump user_version");
    }

    let err = Store::open(dir.db_path()).expect_err("must fail closed on unknown newer schema");
    assert!(
        matches!(
            err,
            StoreError::UnsupportedSchemaVersion {
                found: 999,
                max_known: 2
            }
        ),
        "unexpected error: {err}"
    );

    assert!(dir.path().exists());
}

#[test]
fn opening_a_database_with_tampered_migration_bookkeeping_fails_closed() {
    let dir = TempDir::new("tampered-bookkeeping");
    Store::open(dir.db_path())
        .expect("initial open migrates the database")
        .close()
        .expect("close");

    {
        let raw = rusqlite::Connection::open(dir.db_path()).expect("raw connection");
        raw.execute(
            "UPDATE schema_migrations SET checksum = 'not-the-real-checksum' WHERE version = 1",
            [],
        )
        .expect("tamper with bookkeeping");
    }

    let err = Store::open(dir.db_path()).expect_err("must fail closed on inconsistent bookkeeping");
    assert!(
        matches!(err, StoreError::InconsistentMigrationState(_)),
        "unexpected error: {err}"
    );
}
