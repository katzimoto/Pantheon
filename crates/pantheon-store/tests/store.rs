//! Executable evidence for Issue #16: fresh initialization, the required
//! connection policy, ordered migrations, stable RestoreGeneration across
//! close/reopen, and fail-closed behavior on unsupported/inconsistent
//! migration state.
//!
//! And for Issue #17: the serialized authoritative writer, `BEGIN
//! IMMEDIATE` write intent, revision/CAS semantics under genuine
//! contention, and read/write separation — all exercised through the
//! public API a controller will use.

mod common;

use std::path::Path;
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use common::{TempDir, fixture_row, seed_fixture};
use pantheon_store::{Revision, Store, StoreError, Value};

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
                max_known: 10
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

// ---------------------------------------------------------------------------
// Issue #17 evidence
// ---------------------------------------------------------------------------

/// A probe connection that never waits for a lock, so a contended
/// `BEGIN IMMEDIATE` fails immediately instead of blocking for the store's
/// five-second `busy_timeout`.
fn impatient_probe(path: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).expect("open probe connection");
    conn.busy_timeout(Duration::from_millis(0))
        .expect("disable probe busy timeout");
    conn
}

fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DatabaseBusy
    )
}

#[test]
fn two_mutations_from_the_same_observed_revision_cannot_both_commit() {
    let dir = TempDir::new("cas-race");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());

    let store = Arc::new(store);
    // Exactly the two contenders. Each waits once, so neither can begin
    // writing until both have already observed the revision.
    let barrier = Arc::new(Barrier::new(2));

    let contenders: Vec<_> = ["A", "B"]
        .into_iter()
        .map(|label| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                // Observe first. This read is deliberately outside any
                // transaction: that is what optimistic concurrency means,
                // and it is what makes the race real.
                let observed = store
                    .revision_of("cas_fixture", "row-a")
                    .expect("read revision")
                    .expect("row exists");

                // Released only once BOTH contenders hold revision 7, so
                // neither could have derived it from the other's commit.
                barrier.wait();

                let outcome = store.write(|w| {
                    w.update_revisioned(
                        "cas_fixture",
                        "row-a",
                        observed,
                        &[("value", Value::from(label))],
                    )
                });
                (label, observed, outcome)
            })
        })
        .collect();

    let results: Vec<_> = contenders
        .into_iter()
        .map(|handle| handle.join().expect("contender thread"))
        .collect();

    // Both genuinely started from the same observed revision.
    for (label, observed, _) in &results {
        assert_eq!(
            *observed,
            Revision::new(7),
            "contender {label} did not observe revision 7"
        );
    }

    let winners: Vec<_> = results
        .iter()
        .filter_map(|(label, _, outcome)| outcome.as_ref().ok().map(|rev| (*label, *rev)))
        .collect();
    let losers: Vec<_> = results
        .iter()
        .filter_map(|(label, _, outcome)| outcome.as_ref().err().map(|err| (*label, err)))
        .collect();

    // The outcome is asserted as a multiset. Which thread wins is decided by
    // the OS scheduler and is none of this test's business; that exactly one
    // wins is the invariant.
    assert_eq!(winners.len(), 1, "exactly one mutation must commit");
    assert_eq!(losers.len(), 1, "exactly one mutation must be rejected");

    let (winner_label, winner_revision) = winners[0];
    assert_eq!(winner_revision, Revision::new(8));

    let (_, loser_error) = losers[0];
    assert!(
        matches!(
            loser_error,
            StoreError::RevisionConflict {
                table: "cas_fixture",
                expected: 7,
                actual: Some(8),
                ..
            }
        ),
        "the loser must receive a typed stale-revision conflict, got: {loser_error}"
    );
    assert!(
        !is_busy_store_error(loser_error),
        "a semantic CAS conflict must never surface as a transient lock failure"
    );

    // Exactly one increment reached the database, and the surviving payload
    // is the winner's rather than a blend of both.
    let (revision, value) = fixture_row(&dir.db_path());
    assert_eq!(revision, 8, "the revision must advance exactly once");
    assert_eq!(value, winner_label);
}

fn is_busy_store_error(err: &StoreError) -> bool {
    matches!(err, StoreError::Sqlite(inner) if is_busy(inner))
}

#[test]
fn revision_cas_is_enforced_by_the_database_not_by_one_process_lock() {
    let dir = TempDir::new("cas-cross-connection");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());

    // Observed through the store, before anything writes.
    let observed = store.revision_of("cas_fixture", "row-a").unwrap().unwrap();
    assert_eq!(observed, Revision::new(7));

    // A competing writer that is NOT this store's serialized writer — a
    // plain SQLite connection, standing in for anything outside Pantheon's
    // boundary — advances the row first.
    {
        let other = rusqlite::Connection::open(dir.db_path()).expect("open competing connection");
        let changed = other
            .execute(
                "UPDATE cas_fixture SET value = 'outside', revision = revision + 1
                 WHERE id = 'row-a' AND revision = 7",
                [],
            )
            .expect("competing update");
        assert_eq!(changed, 1);
    }

    // The store's CAS now fails on the revision predicate itself. Nothing
    // in this process's mutex could have detected that, which is what makes
    // this evidence that the check lives in the database.
    let err = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture",
                "row-a",
                observed,
                &[("value", Value::from("mine"))],
            )
        })
        .expect_err("the store must lose to the earlier committed write");
    assert!(
        matches!(
            err,
            StoreError::RevisionConflict {
                expected: 7,
                actual: Some(8),
                ..
            }
        ),
        "unexpected error: {err}"
    );
    assert_eq!(fixture_row(&dir.db_path()), (8, "outside".to_string()));
}

#[test]
fn a_second_store_cannot_open_the_same_database() {
    let dir = TempDir::new("no-second-writer");
    let first = Store::open(dir.db_path()).expect("first store opens");

    // The bypass AC-1 forbids: a caller cannot obtain a second authoritative
    // writer connection by simply opening the store again.
    let err = Store::open(dir.db_path()).expect_err("a competing writer must be refused");
    assert!(
        matches!(err, StoreError::AlreadyOpen { .. }),
        "unexpected error: {err}"
    );

    first.close().expect("close");
    Store::open(dir.db_path()).expect("an ordinary restart still works");
}

#[test]
fn the_authoritative_transaction_holds_the_write_lock_before_it_writes_anything() {
    let dir = TempDir::new("immediate-lock");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let path = dir.db_path();

    // Positive control: with no authoritative transaction open, the probe
    // can take the write lock. Without this, an unopenable database would
    // make the real assertion below pass for the wrong reason.
    {
        let probe = impatient_probe(&path);
        probe
            .execute_batch("BEGIN IMMEDIATE")
            .expect("probe can take the write lock when the store is idle");
        probe.execute_batch("ROLLBACK").expect("release probe");
    }

    let (inside_tx, inside_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let writer = {
        let store = Arc::new(store);
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            store.write(|_| {
                // Signal without having executed a single statement. A
                // DEFERRED transaction would hold no lock at this point.
                inside_tx.send(()).expect("signal inside transaction");
                release_rx
                    .recv_timeout(Duration::from_secs(30))
                    .expect("wait for the probe");
                Ok(())
            })
        })
    };

    inside_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("writer entered the transaction");

    // The assertion: the store's transaction already owns write authority.
    let probe = impatient_probe(&path);
    let err = probe
        .execute_batch("BEGIN IMMEDIATE")
        .expect_err("the authoritative transaction must already hold the write lock");
    assert!(
        is_busy(&err),
        "expected SQLITE_BUSY from the contended write lock, got: {err}"
    );

    release_tx.send(()).expect("release the writer");
    writer
        .join()
        .expect("writer thread")
        .expect("write commits");
}

#[test]
fn a_deferred_transaction_would_not_hold_that_lock() {
    // The differential control for the test above. It proves the probe can
    // tell the two transaction modes apart, so the SQLITE_BUSY observed
    // there is evidence of IMMEDIATE rather than of an always-busy probe.
    let dir = TempDir::new("deferred-control");
    Store::open(dir.db_path())
        .expect("open store")
        .close()
        .expect("close");
    let path = dir.db_path();

    let mut owner = rusqlite::Connection::open(&path).expect("open owner connection");
    let tx = owner
        .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
        .expect("begin deferred");

    let probe = impatient_probe(&path);
    probe
        .execute_batch("BEGIN IMMEDIATE")
        .expect("a deferred transaction that has not written holds no write lock");
    probe.execute_batch("ROLLBACK").expect("release probe");

    tx.rollback().expect("release owner");
}
