//! Helpers shared by the `pantheon-store` integration test binaries.
//!
//! Integration tests are separate compilation units and cannot reach the
//! crate's internal `test_support`, so this module is where the isolated
//! temporary directory and the revisioned fixture live once rather than once
//! per test binary. Each binary compiles this module separately and uses a
//! subset of it, so unused items here are expected rather than dead code.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering};

use pantheon_store::{StoreError, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A minimal, dependency-free isolated directory per test. Mirrors
/// `pantheon-store`'s internal `test_support::TempDir`, which unit tests use
/// but which integration tests (a separate compilation unit) cannot reach.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pantheon-store-it-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated test directory");
        Self(dir)
    }

    pub(crate) fn db_path(&self) -> PathBuf {
        self.0.join("pantheon.db")
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The revisioned fixture table the CAS evidence uses.
///
/// It is created here, by the test, through an ordinary SQLite connection —
/// never by `migrations::MIGRATIONS`. No production database can grow it,
/// and `production_schema_contains_only_the_tables_this_behaviour_needs`
/// below is the standing guard that it never leaks in.
pub(crate) const FIXTURE_DDL: &str = "CREATE TABLE cas_fixture (
        id       TEXT    PRIMARY KEY,
        revision INTEGER NOT NULL CHECK (revision > 0),
        value    TEXT    NOT NULL
    ) STRICT;
    INSERT INTO cas_fixture (id, revision, value) VALUES ('row-a', 7, 'original');";

pub(crate) fn seed_fixture(path: &Path) {
    let conn = rusqlite::Connection::open(path).expect("open fixture connection");
    conn.execute_batch(FIXTURE_DDL).expect("create fixture");
}

pub(crate) fn fixture_row(path: &Path) -> (i64, String) {
    let conn = rusqlite::Connection::open(path).expect("open verification connection");
    conn.query_row(
        "SELECT revision, value FROM cas_fixture WHERE id = 'row-a'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .expect("read fixture row")
}

/// A distinct non-sensitive request digest per test scenario.
pub(crate) fn request_hash(seed: u8) -> [u8; 32] {
    [seed; 32]
}

/// Counts how many times a mutation body actually ran.
///
/// This is process memory, not database state, so a SQLite rollback cannot
/// undo it. That is exactly what separates the three behaviours this mission
/// distinguishes: body not invoked (correct replay), body invoked then rolled
/// back (wrong, but invisible to state assertions alone), and body invoked and
/// committed (wrong, double-applied).
pub(crate) type Invocations = Arc<AtomicUsize>;

/// Builds a mutation that advances `row-a` and records that it ran.
///
/// The revision is read *inside* the closure rather than hardcoded. Hardcoding
/// it would make a second invocation fail its CAS, and the test would then pass
/// for the wrong reason — reporting a double-execution defect as a revision
/// conflict instead of catching it.
pub(crate) fn counting_mutation(
    seen: &Invocations,
    value: &'static str,
) -> impl FnOnce(&pantheon_store::Writer<'_>) -> Result<i64, StoreError> {
    let seen = Arc::clone(seen);
    move |w| {
        seen.fetch_add(1, Ordering::SeqCst);
        let current = w
            .revision_of("cas_fixture", "row-a")?
            .expect("fixture row exists");
        let next = w.update_revisioned(
            "cas_fixture",
            "row-a",
            current,
            &[("value", Value::from(value))],
        )?;
        Ok(next.get())
    }
}

pub(crate) fn count(path: &Path, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("open counting connection");
    conn.query_row(sql, [], |row| row.get(0)).expect("count")
}

/// The durable allocator state, read straight from the database.
pub(crate) fn allocator(path: &Path) -> (String, i64) {
    let conn = rusqlite::Connection::open(path).expect("open allocator connection");
    conn.query_row(
        "SELECT epoch, next_sequence FROM journal_epochs WHERE is_current = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .expect("read allocator")
}

/// The column names of `table`, sorted.
///
/// Used by the standing production-schema guard: a table-level assertion
/// cannot see a speculative column being added, nor a request-body column
/// appearing on the command ledger.
pub(crate) fn columns(path: &Path, table: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(path).expect("open schema connection");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT name FROM pragma_table_info('{table}') ORDER BY name"
        ))
        .expect("prepare table_info");
    stmt.query_map([], |row| row.get(0))
        .expect("query table_info")
        .map(Result::unwrap)
        .collect()
}
