//! Evidence that the Operator read paths cannot open a list/watch gap and
//! cannot silently resume from the wrong journal history.

use crate::error::StoreError;
use crate::operator::{Cursor, CursorError};
use pantheon_core::planning::{GoalPhase, TaskPhase};

use crate::planning::tests::{command, goal_spec, store_with_configuration};
use crate::store::Store;

fn create(store: &Store, goal_id: &str, command_id: &str) {
    let epoch = store.restore_generation().expect("generation");
    store
        .create_goal(
            &command(epoch.as_str(), command_id, &[7u8; 32], "goal.created"),
            goal_id,
            &goal_spec(),
        )
        .expect("goal commits");
}

#[test]
fn a_fresh_installation_reports_a_cursor_before_anything() {
    let (_dir, store, _sequence) = store_with_configuration("head-fresh");
    // The configuration activation in the fixture is itself a command, so the
    // head is already past zero; what matters is that it is the last
    // *committed* sequence, not the next one to be allocated.
    let head = store.journal_head().expect("head");
    let events = store
        .events_after(
            &Cursor {
                journal_epoch: head.journal_epoch.clone(),
                sequence: head.sequence,
            },
            16,
        )
        .expect("read")
        .expect("cursor accepted");
    assert!(
        events.is_empty(),
        "nothing has been committed after the head"
    );
}

#[test]
fn the_snapshot_cursor_is_the_position_the_listed_goals_correspond_to() {
    // This is the whole point of the same-transaction read: everything the
    // list already reflects is at or before the cursor, so watching after it
    // can neither skip an Event nor replay one the list already showed.
    let (_dir, store, _sequence) = store_with_configuration("snapshot-cursor");
    create(&store, "goal-1", "c1");
    create(&store, "goal-2", "c2");

    let snapshot = store.goal_snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .goals
            .iter()
            .map(|g| g.id.as_str())
            .collect::<Vec<_>>(),
        ["goal-1", "goal-2"]
    );

    let after = store
        .events_after(&snapshot.cursor, 16)
        .expect("read")
        .expect("cursor accepted");
    assert!(
        after.is_empty(),
        "the snapshot cursor must not leave already-listed state unwatched"
    );

    create(&store, "goal-3", "c3");
    let gap = store
        .events_after(&snapshot.cursor, 16)
        .expect("read")
        .expect("cursor accepted");
    assert_eq!(
        gap.len(),
        1,
        "the Event for the third Goal must be reachable"
    );
    assert_eq!(gap[0].event_type, "goal.created");
    assert_eq!(gap[0].command_id.as_deref(), Some("c3"));
}

#[test]
fn events_after_returns_history_in_committed_order_and_respects_the_limit() {
    let (_dir, store, _sequence) = store_with_configuration("events-order");
    let start = store.journal_head().expect("head");
    for (index, goal) in ["goal-1", "goal-2", "goal-3"].iter().enumerate() {
        create(&store, goal, &format!("c{index}"));
    }

    let all = store
        .events_after(&start, 16)
        .expect("read")
        .expect("cursor accepted");
    assert_eq!(all.len(), 3);
    assert!(
        all.windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence),
        "history must be oldest first"
    );

    let page = store
        .events_after(&start, 2)
        .expect("read")
        .expect("cursor accepted");
    assert_eq!(page.len(), 2);
    assert_eq!(page[1].cursor().sequence, all[1].sequence);
}

#[test]
fn a_cursor_from_another_journal_history_is_refused_rather_than_restarted() {
    // Silently restarting at the head would drop exactly the Events the
    // caller asked not to miss, which is the failure the cursor exists to
    // prevent. The contract's answer is a structured `cursor-gone`.
    let (_dir, store, _sequence) = store_with_configuration("cursor-epoch");
    create(&store, "goal-1", "c1");

    let refusal = store
        .events_after(
            &Cursor {
                journal_epoch: "0".repeat(32),
                sequence: 0,
            },
            16,
        )
        .expect("read")
        .expect_err("a foreign epoch must be refused");
    match refusal {
        CursorError::UnknownEpoch { supplied, current } => {
            assert_eq!(supplied, "0".repeat(32));
            assert_ne!(current, supplied);
        }
        other => panic!("unexpected refusal: {other}"),
    }
}

#[test]
fn a_cursor_ahead_of_the_journal_head_is_refused() {
    let (_dir, store, _sequence) = store_with_configuration("cursor-ahead");
    let head = store.journal_head().expect("head");

    let refusal = store
        .events_after(
            &Cursor {
                journal_epoch: head.journal_epoch.clone(),
                sequence: head.sequence + 1,
            },
            16,
        )
        .expect("read")
        .expect_err("a cursor past the head must be refused");
    match refusal {
        CursorError::AheadOfJournal { supplied, head: at } => {
            assert_eq!(supplied, head.sequence + 1);
            assert_eq!(at, head.sequence);
        }
        other => panic!("unexpected refusal: {other}"),
    }
}

#[test]
fn a_cursor_survives_its_wire_form_exactly() {
    let cursor = Cursor {
        journal_epoch: "a".repeat(32),
        sequence: 41,
    };
    assert_eq!(Cursor::parse(&cursor.to_wire()), Some(cursor.clone()));
    // The two halves must not be confusable: a bare number and an empty epoch
    // are both rejected rather than being read as sequence 0 of some epoch.
    assert_eq!(Cursor::parse("41"), None);
    assert_eq!(Cursor::parse(":41"), None);
    assert_eq!(Cursor::parse(&format!("{}:x", "a".repeat(32))), None);
}

#[test]
fn the_goal_snapshot_reports_the_stored_phase_rather_than_assuming_planning() {
    let (_dir, store, _sequence) = store_with_configuration("snapshot-phase");
    create(&store, "goal-1", "c1");
    let epoch = store.restore_generation().expect("generation");
    store
        .cancel_goal(
            &command(
                epoch.as_str(),
                "cancel",
                &[8u8; 32],
                "goal.cancel.requested",
            ),
            "goal-1",
        )
        .expect("cancellation commits");

    let snapshot = store.goal_snapshot().expect("snapshot");
    assert_eq!(
        snapshot.goals[0].phase,
        pantheon_core::planning::GoalPhase::Finalizing
    );
    assert_eq!(
        snapshot.goals[0].revision,
        store.goal("goal-1").expect("read").expect("goal").revision
    );
}

#[test]
fn the_schema_version_is_the_migrated_version_not_a_compiled_constant() {
    let (_dir, store, _sequence) = store_with_configuration("schema-version");
    let reported = store.schema_version().expect("version");
    let expected = store
        .read(|conn| {
            conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(StoreError::Sqlite)
        })
        .expect("bookkeeping");
    assert_eq!(reported, expected);
}

#[test]
fn a_concurrent_write_cannot_put_the_snapshot_cursor_ahead_of_the_listed_goals() {
    // The gap-free guarantee is exactly this: every `goal.created` Event at
    // or before the snapshot cursor must correspond to a Goal the snapshot
    // listed. If the cursor is read outside the transaction that read the
    // Goals, a commit landing between the two makes an Event reachable at or
    // before the cursor whose Goal is not in the list — and a client watching
    // strictly after the cursor never sees it.
    let (_dir, store, _sequence) = store_with_configuration("snapshot-race");
    let epoch = store.restore_generation().expect("generation");
    let epoch = epoch.as_str().to_string();
    const GOALS: usize = 60;

    std::thread::scope(|scope| {
        scope.spawn(|| {
            for index in 0..GOALS {
                store
                    .create_goal(
                        &command(&epoch, &format!("race-{index}"), &[7u8; 32], "goal.created"),
                        &format!("goal-{index:03}"),
                        &goal_spec(),
                    )
                    .expect("commits");
            }
        });

        let mut checked = 0;
        while checked < 400 {
            let snapshot = store.goal_snapshot().expect("snapshot");
            let history = store
                .events_after(
                    &Cursor {
                        journal_epoch: snapshot.cursor.journal_epoch.clone(),
                        sequence: 0,
                    },
                    4096,
                )
                .expect("read")
                .expect("cursor accepted");
            let created_at_or_before = history
                .iter()
                .filter(|event| {
                    event.event_type == "goal.created" && event.sequence <= snapshot.cursor.sequence
                })
                .count();
            assert_eq!(
                created_at_or_before,
                snapshot.goals.len(),
                "the cursor is ahead of the listed Goals: {} Goals created at or before {},                  but only {} listed",
                created_at_or_before,
                snapshot.cursor.sequence,
                snapshot.goals.len()
            );
            checked += 1;
            if snapshot.goals.len() == GOALS {
                break;
            }
        }
    });
}

#[test]
fn a_goal_detail_never_pairs_one_moments_phase_with_another_moments_tasks() {
    // The representation a client caches under a strong validator has to be a
    // state that existed. Composed from separate reads, a cancellation
    // landing between them yields an Active Goal whose Task is already
    // Finalizing — a pairing no transaction ever committed.
    //
    // Each Goal offers exactly one violable instant, because cancelling twice
    // is a no-op. So one reader spins on one Goal while that Goal — and only
    // that Goal — is cancelled, and the whole arrangement repeats. Spreading
    // the reader across many Goals instead would leave it looking at the
    // wrong one at the moment that matters.
    const GOALS: usize = 24;
    let (_dir, store, sequence) = store_with_configuration("detail-coherence");
    let epoch = store.restore_generation().expect("generation");
    let epoch = epoch.as_str().to_string();

    for index in 0..GOALS {
        let id = format!("goal-{index:02}");
        crate::planning::cancel::tests::goal_with_ready_task(&store, sequence, &id);

        let observed = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                // Read until the cancellation is visible, then a little
                // longer so the loop is already running when it lands.
                let mut after = 0;
                while after < 64 {
                    let detail = store.goal_detail(&id).expect("read").expect("goal exists");
                    let fenced = detail.goal.phase == GoalPhase::Finalizing;
                    for task in &detail.tasks {
                        assert!(
                            task.phase != TaskPhase::Finalizing || fenced,
                            "{id}: a fenced Task was paired with a Goal in {:?}",
                            detail.goal.phase
                        );
                    }
                    observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if fenced {
                        after += 1;
                    }
                }
            });

            store
                .cancel_goal(
                    &command(
                        &epoch,
                        &format!("cancel-{id}"),
                        &[8u8; 32],
                        "goal.cancel.requested",
                    ),
                    &id,
                )
                .expect("cancellation commits");
            reader.join().expect("the reader thread ran");
        });

        assert!(
            observed.load(std::sync::atomic::Ordering::Relaxed) > 64,
            "{id}: the reader must have observed the Goal before the cancellation, not only after"
        );
    }
}
