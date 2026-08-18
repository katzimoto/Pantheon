//! Evidence that needs crate-internal reach: the command envelope's write
//! intent, its behaviour under a genuine SQL failure, and its behaviour when
//! the caller's mutation panics.

use crate::command::{self, Command, Committed};
use crate::error::StoreError;
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Value;

const FIXTURE_DDL: &str = "CREATE TABLE cas_fixture (
        id       TEXT    PRIMARY KEY,
        revision INTEGER NOT NULL CHECK (revision > 0),
        value    TEXT    NOT NULL
    ) STRICT;
    INSERT INTO cas_fixture (id, revision, value) VALUES ('row-a', 7, 'original');";

fn store_with_fixture(label: &str) -> (TempDir, Store) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    store
        .write(|w| {
            w.execute_batch_for_test(FIXTURE_DDL)?;
            Ok(())
        })
        .expect("create fixture");
    (dir, store)
}

fn hash(seed: u8) -> [u8; 32] {
    [seed; 32]
}

fn advance(w: &crate::transaction::Writer<'_>, value: &'static str) -> Result<i64, StoreError> {
    let current = w.revision_of("cas_fixture", "row-a")?.expect("row exists");
    let next = w.update_revisioned(
        "cas_fixture",
        "row-a",
        current,
        &[("value", Value::from(value))],
    )?;
    Ok(next.get())
}

#[test]
fn a_sql_failure_inside_the_command_mutation_rolls_back_the_whole_command() {
    let (_dir, store) = store_with_fixture("cmd-sql-failure");
    let epoch = store.restore_generation().unwrap();

    let err = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-1",
                request_hash: &hash(1),
                event_type: "fixture.mutated",
            },
            |w| {
                let applied = advance(w, "intermediate")?;
                assert_eq!(applied, 8);
                // Fails while stepping, with real work already done in this
                // transaction.
                w.execute(
                    "INSERT INTO cas_fixture (id, revision, value) VALUES ('row-a', 1, 'clash')",
                    &[],
                )?;
                Ok(())
            },
        )
        .expect_err("the constraint violation must abort the command");
    assert!(matches!(err, StoreError::Sqlite(_)), "unexpected: {err}");

    let row = store
        .read_row_for_test(
            "SELECT revision, value FROM cas_fixture WHERE id = ?1",
            "row-a",
        )
        .expect("read fixture");
    assert_eq!(row, Some((7, "original".to_string())));
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM commands")
            .expect("count commands"),
        vec![0]
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM event_journal")
            .expect("count events"),
        vec![0]
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT next_sequence FROM journal_epochs WHERE is_current = 1")
            .expect("read allocator"),
        vec![1],
        "a rolled-back command must not consume a journal sequence"
    );
}

#[test]
fn a_panic_inside_the_command_mutation_leaves_no_command_no_event_and_no_state() {
    let dir = TempDir::new("cmd-panic");
    let path = dir.path().join("pantheon.db");
    {
        let store = Store::open(&path).expect("open store");
        store
            .write(|w| {
                w.execute_batch_for_test(FIXTURE_DDL)?;
                Ok(())
            })
            .expect("fixture");
        let epoch = store.restore_generation().unwrap();

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.execute_command(
                &Command {
                    epoch: epoch.as_str(),
                    id: "cmd-panic",
                    request_hash: &hash(1),
                    event_type: "fixture.mutated",
                },
                |w| {
                    let applied = advance(w, "doomed")?;
                    assert_eq!(applied, 8);
                    panic!("injected panic after a real write");
                    #[allow(unreachable_code)]
                    Ok::<(), StoreError>(())
                },
            )
        }));
        assert!(panicked.is_err(), "the closure must have panicked");
    }

    // A fresh store proves nothing from the panicking command committed.
    let reopened = Store::open(&path).expect("reopen after panic");
    let row = reopened
        .read_row_for_test(
            "SELECT revision, value FROM cas_fixture WHERE id = ?1",
            "row-a",
        )
        .expect("read fixture");
    assert_eq!(row, Some((7, "original".to_string())));
    assert_eq!(
        reopened
            .read_all_for_test("SELECT COUNT(*) FROM commands")
            .expect("count commands"),
        vec![0]
    );
    assert_eq!(
        reopened
            .read_all_for_test("SELECT COUNT(*) FROM event_journal")
            .expect("count events"),
        vec![0]
    );
}

#[test]
fn a_replay_is_decided_from_durable_state_not_process_memory() {
    // The ledger row is written by an ordinary connection outside this
    // process's `Store`, so an implementation that tracked executed commands
    // in memory would execute the mutation instead of reconciling.
    let (dir, store) = store_with_fixture("cmd-durable-decision");
    let epoch = store.restore_generation().unwrap();
    let path = dir.path().join("pantheon.db");
    store.close().expect("close");

    {
        let conn = rusqlite::Connection::open(&path).expect("open external connection");
        let journal_epoch: String = conn
            .query_row(
                "SELECT epoch FROM journal_epochs WHERE is_current = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO event_journal
                 (event_id, journal_epoch, sequence, event_type, recorded_at, command_epoch, command_id)
             VALUES (lower(hex(randomblob(16))), ?1, 1, 'external', unixepoch(), ?2, 'cmd-external')",
            rusqlite::params![journal_epoch, epoch.as_str()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO commands
                 (command_epoch, command_id, request_hash, journal_epoch, sequence, recorded_at)
             VALUES (?1, 'cmd-external', ?2, ?3, 1, unixepoch())",
            rusqlite::params![epoch.as_str(), hash(1).to_vec(), journal_epoch],
        )
        .unwrap();
        conn.execute(
            "UPDATE journal_epochs SET next_sequence = 2 WHERE is_current = 1",
            [],
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("reopen");
    let outcome = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-external",
                request_hash: &hash(1),
                event_type: "fixture.mutated",
            },
            |_| -> Result<(), StoreError> {
                panic!("the mutation must not run for a durably recorded command")
            },
        )
        .expect("the externally recorded command reconciles");
    assert!(matches!(outcome, Committed::Replayed { .. }));
    assert_eq!(outcome.cursor().sequence(), 1);

    let row = store
        .read_row_for_test(
            "SELECT revision, value FROM cas_fixture WHERE id = ?1",
            "row-a",
        )
        .expect("read fixture");
    assert_eq!(
        row,
        Some((7, "original".to_string())),
        "no mutation occurred"
    );
}

#[test]
fn a_command_cannot_be_nested_inside_another_authoritative_write() {
    let (_dir, store) = store_with_fixture("cmd-nested");
    let epoch = store.restore_generation().unwrap();

    let err = store
        .write(|_| {
            store.execute_command(
                &Command {
                    epoch: epoch.as_str(),
                    id: "cmd-nested",
                    request_hash: &hash(1),
                    event_type: "fixture.mutated",
                },
                |w| advance(w, "applied"),
            )?;
            Ok(())
        })
        .expect_err("a nested command must be rejected rather than deadlock");
    assert!(
        matches!(err, StoreError::ConnectionUnavailable(_)),
        "unexpected error: {err}"
    );
}

#[test]
fn a_failure_after_the_whole_envelope_rolls_back_the_event_and_the_command_row() {
    // The only construction that observes a rollback *after* the Event and the
    // durable command row already exist. It is what forbids restructuring the
    // envelope into two transactions: a separately committed command row or
    // Event would survive this failure.
    let (_dir, store) = store_with_fixture("cmd-post-envelope");
    let epoch = store.restore_generation().unwrap();

    let err = store
        .write(|w| {
            command::execute(
                w,
                &Command {
                    epoch: epoch.as_str(),
                    id: "cmd-1",
                    request_hash: &hash(1),
                    event_type: "fixture.mutated",
                },
                |w| advance(w, "applied"),
            )?;
            // At this point the mutation, the Event and the ledger row have
            // all been written inside this transaction.
            Err::<(), StoreError>(StoreError::InvariantViolated(
                "after the envelope".to_string(),
            ))
        })
        .expect_err("the injected failure must abort everything");
    assert!(matches!(err, StoreError::InvariantViolated(ref d) if d == "after the envelope"));

    assert_eq!(
        store
            .read_row_for_test(
                "SELECT revision, value FROM cas_fixture WHERE id = ?1",
                "row-a"
            )
            .expect("read fixture"),
        Some((7, "original".to_string()))
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM commands")
            .expect("count commands"),
        vec![0],
        "a separately committed command row would survive this"
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM event_journal")
            .expect("count events"),
        vec![0],
        "a separately committed Event would survive this"
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT next_sequence FROM journal_epochs WHERE is_current = 1")
            .expect("read allocator"),
        vec![1],
        "the allocator must not have burned a sequence"
    );
}

#[test]
fn the_epoch_fence_reads_the_transactions_own_snapshot() {
    // Rotate the generation *uncommitted*, inside the transaction. Only the
    // transaction's own snapshot can see it. A fence that consulted the
    // store's read-only connection would still see the old value and accept
    // the command.
    let (_dir, store) = store_with_fixture("cmd-fence-snapshot");
    let stale_epoch = store.restore_generation().unwrap().as_str().to_string();

    store
        .write(|w| {
            w.execute(
                "UPDATE system_state SET restore_generation = 'ffffffffffffffffffffffffffffffff'
                 WHERE id = 1",
                &[],
            )?;

            let err = command::execute(
                w,
                &Command {
                    epoch: &stale_epoch,
                    id: "cmd-1",
                    request_hash: &hash(1),
                    event_type: "fixture.mutated",
                },
                |w| advance(w, "should-not-apply"),
            )
            .expect_err("the epoch is stale on this transaction's snapshot");
            assert!(
                matches!(err, StoreError::StaleCommandEpoch { ref current, .. }
                    if current == "ffffffffffffffffffffffffffffffff"),
                "unexpected error: {err}"
            );
            Ok(())
        })
        .expect_err("the failed command marks the transaction uncommittable");

    // The rotation rolled back with everything else.
    assert_eq!(
        store.restore_generation().unwrap().as_str(),
        stale_epoch,
        "the uncommitted rotation must not survive"
    );
}

#[test]
fn a_mutation_that_discards_a_failed_write_still_cannot_commit_the_command() {
    // The envelope proceeds to allocate, append and record after the closure
    // returns `Ok`. The commit gate in `transaction::run` is what makes that
    // safe; this pins that dependency from the command path.
    let (_dir, store) = store_with_fixture("cmd-swallowed");
    let epoch = store.restore_generation().unwrap();

    let err = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-1",
                request_hash: &hash(1),
                event_type: "fixture.mutated",
            },
            |w| {
                // Deliberately stale expected revision, and the error is
                // discarded rather than propagated.
                let _ = w.update_revisioned(
                    "cas_fixture",
                    "row-a",
                    crate::transaction::Revision::new(999),
                    &[("value", Value::from("nope"))],
                );
                Ok(())
            },
        )
        .expect_err("a discarded mutation error must not commit the command");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("discarded")),
        "unexpected error: {err}"
    );

    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM commands")
            .expect("count commands"),
        vec![0]
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM event_journal")
            .expect("count events"),
        vec![0]
    );
}
