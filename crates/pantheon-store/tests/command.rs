//! Executable evidence for Issue #18 command identity: a new command
//! executes once, the same identity and request hash replays the durable
//! outcome without re-executing, a reused identity with a different request
//! hash fails closed, and a stale command epoch fails closed before the
//! command ledger is consulted.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{
    Invocations, TempDir, allocator, count, counting_mutation, fixture_row, request_hash,
    seed_fixture,
};
use pantheon_store::{Command, Committed, Store, StoreError};

#[test]
fn a_new_command_executes_once_and_commits_state_outcome_and_event_together() {
    let dir = TempDir::new("cmd-new");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap();
    let hash = request_hash(1);
    let seen: Invocations = Arc::new(AtomicUsize::new(0));

    let committed = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-1",
                request_hash: &hash,
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect("the command executes");

    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "mutation must run exactly once"
    );
    match &committed {
        Committed::Executed { value, cursor } => {
            assert_eq!(*value, 8);
            assert_eq!(cursor.sequence(), 1, "first Event in this journal epoch");
        }
        Committed::Replayed { .. } => panic!("a brand-new command must not report a replay"),
    }

    // State, durable outcome and Event are all present after the one commit.
    assert_eq!(fixture_row(&dir.db_path()), (8, "applied".to_string()));
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM commands"),
        1,
        "exactly one durable command row"
    );
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM event_journal"),
        1,
        "exactly one Event"
    );

    // The Event carries the command causality pair, and it is the command's
    // epoch rather than the journal's.
    let conn = rusqlite::Connection::open(dir.db_path()).unwrap();
    let (event_epoch, event_id, event_type): (String, String, String) = conn
        .query_row(
            "SELECT command_epoch, command_id, event_type FROM event_journal",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(event_epoch, epoch.as_str());
    assert_eq!(event_id, "cmd-1");
    assert_eq!(event_type, "fixture.mutated");
    let (journal_epoch, next) = allocator(&dir.db_path());
    assert_ne!(
        journal_epoch,
        epoch.as_str(),
        "JournalEpoch must not be derived from the RestoreGeneration"
    );
    assert_eq!(next, 2, "the durable allocator advanced exactly once");
}

#[test]
fn a_replayed_command_returns_the_prior_outcome_without_invoking_the_mutation() {
    let dir = TempDir::new("cmd-replay");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap();
    let hash = request_hash(1);
    let command = Command {
        epoch: epoch.as_str(),
        id: "cmd-1",
        request_hash: &hash,
        event_type: "fixture.mutated",
    };

    let first_seen: Invocations = Arc::new(AtomicUsize::new(0));
    let first = store
        .execute_command(&command, counting_mutation(&first_seen, "applied"))
        .expect("first execution");
    assert!(first.was_executed());
    let first_cursor = first.cursor().clone();

    // A second, freshly built closure — the retry is a real retry, not a
    // reuse of an already-consumed FnOnce.
    let replay_seen: Invocations = Arc::new(AtomicUsize::new(0));
    let replayed = store
        .execute_command(
            &command,
            counting_mutation(&replay_seen, "should-not-apply"),
        )
        .expect("the retry reconciles rather than failing");

    assert_eq!(
        replay_seen.load(Ordering::SeqCst),
        0,
        "the mutation body must not run on a replay"
    );
    match &replayed {
        Committed::Replayed {
            cursor,
            recorded_at,
        } => {
            assert_eq!(cursor, &first_cursor);
            // The reconciled timestamp is the durable one, not "now".
            let durable: i64 = rusqlite::Connection::open(dir.db_path())
                .unwrap()
                .query_row("SELECT recorded_at FROM commands", [], |row| row.get(0))
                .unwrap();
            assert_eq!(*recorded_at, durable);
        }
        Committed::Executed { .. } => panic!("a repeated command must not execute again"),
    }

    // The Event type is not part of the identity: the same identity and
    // request hash still reconciles even if the caller names a different
    // Event, and appends nothing further.
    let other_event_seen: Invocations = Arc::new(AtomicUsize::new(0));
    let other_event = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-1",
                request_hash: &hash,
                event_type: "fixture.other",
            },
            counting_mutation(&other_event_seen, "should-not-apply"),
        )
        .expect("a differing event type does not change the command identity");
    assert!(!other_event.was_executed());
    assert_eq!(other_event_seen.load(Ordering::SeqCst), 0);

    // Nothing was duplicated and nothing advanced.
    assert_eq!(fixture_row(&dir.db_path()), (8, "applied".to_string()));
    assert_eq!(count(&dir.db_path(), "SELECT COUNT(*) FROM commands"), 1);
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM event_journal"),
        1
    );
    assert_eq!(
        allocator(&dir.db_path()).1,
        2,
        "a replay must not consume a journal sequence"
    );
}

#[test]
fn the_same_command_id_with_a_different_request_hash_fails_closed() {
    let dir = TempDir::new("cmd-conflict");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let epoch = store.restore_generation().unwrap();
    let first_hash = request_hash(1);
    let other_hash = request_hash(2);

    let seen: Invocations = Arc::new(AtomicUsize::new(0));
    store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-1",
                request_hash: &first_hash,
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect("first execution");

    let conflicting_seen: Invocations = Arc::new(AtomicUsize::new(0));
    let err = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-1",
                request_hash: &other_hash,
                event_type: "fixture.mutated",
            },
            counting_mutation(&conflicting_seen, "should-not-apply"),
        )
        .expect_err("a reused command id with a different request must fail closed");

    assert!(
        matches!(err, StoreError::CommandConflict { ref command_id } if command_id == "cmd-1"),
        "unexpected error: {err}"
    );
    assert_eq!(conflicting_seen.load(Ordering::SeqCst), 0);
    assert_eq!(fixture_row(&dir.db_path()), (8, "applied".to_string()));
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM event_journal"),
        1
    );
    assert_eq!(allocator(&dir.db_path()).1, 2);

    // The durable identity keeps its original hash: a conflicting retry must
    // not inherit the first request's authority.
    let conn = rusqlite::Connection::open(dir.db_path()).unwrap();
    let stored: Vec<u8> = conn
        .query_row("SELECT request_hash FROM commands", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, first_hash.to_vec());
}

/// Replaces the installation's RestoreGeneration, standing in for the
/// disaster-restore authority fence that this mission does not implement —
/// the same way the #16 tests stand in for a future build by raw-updating
/// `user_version`. Done while the store is closed, so the result holds
/// whether or not the generation is cached at open.
fn rotate_generation(path: &Path) -> String {
    let conn = rusqlite::Connection::open(path).expect("open rotation connection");
    let fresh: String = conn
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .unwrap();
    conn.execute(
        "UPDATE system_state SET restore_generation = ?1 WHERE id = 1",
        rusqlite::params![fresh],
    )
    .expect("rotate generation");
    fresh
}

#[test]
fn a_stale_command_epoch_fails_closed_even_for_a_command_id_that_never_existed() {
    let dir = TempDir::new("cmd-stale");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let stale_epoch = store.restore_generation().unwrap().as_str().to_string();

    let seen: Invocations = Arc::new(AtomicUsize::new(0));
    store
        .execute_command(
            &Command {
                epoch: &stale_epoch,
                id: "cmd-1",
                request_hash: &request_hash(1),
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect("a command under the current epoch succeeds");
    store.close().expect("close");

    rotate_generation(&dir.db_path());
    let store = Store::open(dir.db_path()).expect("reopen after rotation");

    // A command ID with no row in `commands` under any epoch. If the
    // implementation looked the row up first and treated absence as "new",
    // this would execute.
    let unseen: Invocations = Arc::new(AtomicUsize::new(0));
    let err = store
        .execute_command(
            &Command {
                epoch: &stale_epoch,
                id: "never-seen-before",
                request_hash: &request_hash(9),
                event_type: "fixture.mutated",
            },
            counting_mutation(&unseen, "should-not-apply"),
        )
        .expect_err("a stale epoch must fail closed");

    assert!(
        matches!(err, StoreError::StaleCommandEpoch { .. }),
        "unexpected error: {err}"
    );
    assert_eq!(
        unseen.load(Ordering::SeqCst),
        0,
        "the mutation must not run"
    );
    assert_eq!(fixture_row(&dir.db_path()), (8, "applied".to_string()));
    assert_eq!(
        count(
            &dir.db_path(),
            "SELECT COUNT(*) FROM commands WHERE command_id = 'never-seen-before'"
        ),
        0,
        "no command row may be created for a stale request"
    );
    assert_eq!(
        count(&dir.db_path(), "SELECT COUNT(*) FROM event_journal"),
        1
    );
    assert_eq!(allocator(&dir.db_path()).1, 2);
}

#[test]
fn the_same_unseen_command_id_succeeds_under_the_current_epoch() {
    // The differential control for the test above: it proves the rejection
    // was caused by the epoch alone, not by the ID being unknown or by the
    // store being unusable after the generation changed.
    let dir = TempDir::new("cmd-stale-control");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    store.close().expect("close");

    let fresh_epoch = rotate_generation(&dir.db_path());
    let store = Store::open(dir.db_path()).expect("reopen");

    let seen: Invocations = Arc::new(AtomicUsize::new(0));
    store
        .execute_command(
            &Command {
                epoch: &fresh_epoch,
                id: "never-seen-before",
                request_hash: &request_hash(9),
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect("the same unseen id succeeds under the current epoch");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(fixture_row(&dir.db_path()), (8, "applied".to_string()));
}

#[test]
fn a_known_command_id_under_a_stale_epoch_is_stale_rather_than_a_replay() {
    // Kills an implementation that keys the ledger on `command_id` alone.
    let dir = TempDir::new("cmd-stale-known");
    let store = Store::open(dir.db_path()).expect("open store");
    seed_fixture(&dir.db_path());
    let stale_epoch = store.restore_generation().unwrap().as_str().to_string();
    let hash = request_hash(1);
    let seen: Invocations = Arc::new(AtomicUsize::new(0));
    store
        .execute_command(
            &Command {
                epoch: &stale_epoch,
                id: "cmd-1",
                request_hash: &hash,
                event_type: "fixture.mutated",
            },
            counting_mutation(&seen, "applied"),
        )
        .expect("first execution");
    store.close().expect("close");

    rotate_generation(&dir.db_path());
    let store = Store::open(dir.db_path()).expect("reopen");

    let retry_seen: Invocations = Arc::new(AtomicUsize::new(0));
    let err = store
        .execute_command(
            &Command {
                epoch: &stale_epoch,
                id: "cmd-1",
                request_hash: &hash,
                event_type: "fixture.mutated",
            },
            counting_mutation(&retry_seen, "should-not-apply"),
        )
        .expect_err("an old-epoch command must be stale, not replayed");
    assert!(
        matches!(err, StoreError::StaleCommandEpoch { .. }),
        "unexpected error: {err}"
    );
    assert_eq!(retry_seen.load(Ordering::SeqCst), 0);
}
