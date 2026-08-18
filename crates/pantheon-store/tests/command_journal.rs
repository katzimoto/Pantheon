//! Executable evidence for Issue #18 atomicity and the Event Journal: a
//! failed command leaves no state, no outcome and no Event; the state
//! mutation and its Event share one transaction; journal sequencing is
//! durable and monotonic across reopen; and concurrent callers of one new
//! command identity execute it once.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use common::{
    Invocations, TempDir, allocator, count, counting_mutation, fixture_row, request_hash,
    seed_fixture,
};
use pantheon_store::{Command, Committed, Revision, Store, StoreError, Value};

#[test]
fn a_failure_inside_the_command_rolls_back_state_outcome_and_event_together() {
    let dir = TempDir::new("cmd-rollback");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap();

    let err = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-doomed",
                request_hash: &request_hash(3),
                event_type: "fixture.mutated",
            },
            |w| {
                // A real intermediate write lands first, so this proves a
                // completed mutation was undone rather than never attempted.
                let current = w.revision_of("cas_fixture", "row-a")?.expect("row");
                let next = w.update_revisioned(
                    "cas_fixture",
                    "row-a",
                    current,
                    &[("value", Value::from("doomed"))],
                )?;
                assert_eq!(next, Revision::new(8));
                Err::<(), StoreError>(StoreError::InvariantViolated("injected".to_string()))
            },
        )
        .expect_err("the injected failure must abort the command");
    assert!(matches!(err, StoreError::InvariantViolated(ref d) if d == "injected"));

    assert_eq!(
        fixture_row(&dir.db_path()),
        (7, "original".to_string()),
        "the state mutation must not survive"
    );
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM commands"),
        0,
        "no falsely completed command"
    );
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM event_journal"),
        0,
        "no Event from the failed attempt"
    );
    assert_eq!(
        allocator(&dir.db_path()).1,
        1,
        "the durable journal allocator must not advance on a rolled-back command"
    );

    // The store is still usable, and the identity is still free.
    let seen: Invocations = Arc::new(AtomicUsize::new(0));
    store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-doomed",
                request_hash: &request_hash(3),
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect("the store remains usable after a rolled-back command");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[test]
fn the_state_mutation_and_its_event_share_one_transaction() {
    // Collide the Event insert: pre-occupy the exact (journal_epoch, sequence)
    // the allocator will hand out, so the envelope's append fails AFTER the
    // caller's mutation has already succeeded. If the state mutation were
    // committed separately from the Event, `row-a` would survive at 8.
    let dir = TempDir::new("cmd-atomic");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap();
    let (journal_epoch, next_sequence) = allocator(&dir.db_path());

    {
        let conn = rusqlite::Connection::open(dir.db_path()).expect("open blocker connection");
        conn.execute(
            "INSERT INTO event_journal
                 (event_id, journal_epoch, sequence, event_type, recorded_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, 'blocker', unixepoch())",
            rusqlite::params![journal_epoch, next_sequence],
        )
        .expect("occupy the next journal slot");
    }

    let seen: Invocations = Arc::new(AtomicUsize::new(0));
    let err = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-collide",
                request_hash: &request_hash(4),
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect_err("the Event append must fail on the occupied slot");
    // An occupied slot means the journal disagrees with its own allocator:
    // a violated invariant, reported as such rather than as a storage error.
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("already occupied")),
        "unexpected error: {err}"
    );

    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "the mutation did run before the Event append failed"
    );
    assert_eq!(
        fixture_row(&dir.db_path()),
        (7, "original".to_string()),
        "the state mutation must roll back with the failed Event append"
    );
    assert_eq!(count(&dir.db_path(), "SELECT COUNT(*) FROM commands"), 0);
    // Only the pre-planted blocker Event survives, and the allocator did not
    // burn a sequence on the rolled-back command.
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM event_journal"),
        1
    );
    assert_eq!(allocator(&dir.db_path()).1, next_sequence);
}

#[test]
fn event_sequences_are_monotonic_within_one_journal_epoch() {
    let dir = TempDir::new("cmd-sequencing");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap();

    for (index, id) in ["cmd-1", "cmd-2", "cmd-3"].into_iter().enumerate() {
        let seen: Invocations = Arc::new(AtomicUsize::new(0));
        let committed = store
            .execute_command(
                &Command {
                    epoch: epoch.as_str(),
                    id,
                    request_hash: &request_hash(u8::try_from(index).unwrap()),
                    event_type: "fixture.mutated",
                },
                counting_mutation(&seen, "applied"),
            )
            .expect("command executes");
        assert_eq!(
            committed.cursor().sequence(),
            i64::try_from(index).unwrap() + 1
        );
    }

    let conn = rusqlite::Connection::open(dir.db_path()).unwrap();
    let mut stmt = conn
        .prepare("SELECT journal_epoch, sequence, command_id FROM event_journal ORDER BY sequence")
        .unwrap();
    let rows: Vec<(String, i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    // Ordering evidence comes from `sequence` alone: `unixepoch()` has
    // whole-second resolution, so timestamps cannot order these.
    assert_eq!(rows.len(), 3);
    let single_epoch = &rows[0].0;
    for (index, (row_epoch, sequence, command_id)) in rows.iter().enumerate() {
        assert_eq!(
            row_epoch, single_epoch,
            "all Events share one journal epoch"
        );
        assert_eq!(*sequence, i64::try_from(index).unwrap() + 1);
        assert_eq!(command_id, &format!("cmd-{}", index + 1));
    }
    assert_eq!(allocator(&dir.db_path()).1, 4);
}

#[test]
fn close_and_reopen_preserves_the_journal_epoch_sequence_and_command_replay() {
    let dir = TempDir::new("cmd-reopen");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap();
    let hash = request_hash(1);

    for id in ["cmd-1", "cmd-2"] {
        let seen: Invocations = Arc::new(AtomicUsize::new(0));
        store
            .execute_command(
                &Command {
                    epoch: epoch.as_str(),
                    id,
                    request_hash: &hash,
                    event_type: "fixture.mutated",
                },
                counting_mutation(&seen, "applied"),
            )
            .expect("command executes");
    }
    let (journal_epoch_before, next_before) = allocator(&dir.db_path());
    assert_eq!(next_before, 3, "two commands consumed sequences 1 and 2");
    store.close().expect("close");

    // Read off disk with nothing open, and asserted against the literal value
    // rather than against itself: an allocator kept in process memory would
    // leave `next_sequence` at 1 here.
    assert_eq!(allocator(&dir.db_path()), (journal_epoch_before.clone(), 3));

    let store = Store::open(dir.db_path()).expect("reopen");
    assert_eq!(
        store.restore_generation().unwrap(),
        epoch,
        "ordinary reopen must not rotate the RestoreGeneration"
    );

    // A replay still reconciles after reopen — decided from durable state,
    // not process memory.
    let replay_seen: Invocations = Arc::new(AtomicUsize::new(0));
    let replayed = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-1",
                request_hash: &hash,
                event_type: "fixture.mutated",
            },
            counting_mutation(&replay_seen, "should-not-apply"),
        )
        .expect("replay after reopen");
    assert!(!replayed.was_executed());
    assert_eq!(replay_seen.load(Ordering::SeqCst), 0);

    // And sequencing continues rather than restarting.
    let seen: Invocations = Arc::new(AtomicUsize::new(0));
    let committed = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-3",
                request_hash: &hash,
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect("a new command after reopen");
    assert_eq!(
        committed.cursor().sequence(),
        3,
        "sequence must not restart"
    );
    assert_eq!(committed.cursor().epoch(), journal_epoch_before);
}

#[test]
fn concurrent_callers_of_the_same_new_command_execute_the_mutation_once() {
    let dir = TempDir::new("cmd-race");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap().as_str().to_string();

    let store = Arc::new(store);
    let barrier = Arc::new(Barrier::new(2));
    let seen: Invocations = Arc::new(AtomicUsize::new(0));

    let contenders: Vec<_> = ["A", "B"]
        .into_iter()
        .map(|label| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let seen = Arc::clone(&seen);
            let epoch = epoch.clone();
            std::thread::spawn(move || {
                let hash = request_hash(7);
                // Released only once both threads hold the identical, never
                // before issued command identity. The wait is outside the
                // closure: waiting inside would deadlock on the serialized
                // writer.
                barrier.wait();
                let outcome = store.execute_command(
                    &Command {
                        epoch: &epoch,
                        id: "cmd-contended",
                        request_hash: &hash,
                        event_type: "fixture.mutated",
                    },
                    counting_mutation(&seen, "applied"),
                );
                (label, outcome)
            })
        })
        .collect();

    let results: Vec<_> = contenders
        .into_iter()
        .map(|handle| handle.join().expect("contender thread"))
        .collect();

    let executed = results
        .iter()
        .filter(|(_, outcome)| matches!(outcome, Ok(Committed::Executed { .. })))
        .count();
    let replayed = results
        .iter()
        .filter(|(_, outcome)| matches!(outcome, Ok(Committed::Replayed { .. })))
        .count();
    for (label, outcome) in &results {
        assert!(outcome.is_ok(), "contender {label} failed: {outcome:?}");
    }

    // Asserted as a multiset: which thread wins is the scheduler's business.
    assert_eq!(executed, 1, "exactly one caller may execute the mutation");
    assert_eq!(replayed, 1, "the other must reconcile the durable outcome");
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "the mutation body must run exactly once across both callers"
    );
    assert_eq!(fixture_row(&dir.db_path()), (8, "applied".to_string()));
    assert_eq!(count(&dir.db_path(), "SELECT COUNT(*) FROM commands"), 1);
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM event_journal"),
        1
    );
}
