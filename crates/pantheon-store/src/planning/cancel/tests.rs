//! Evidence that cancellation is a fence, not an outcome.

use pantheon_core::planning::{GoalPhase, TaskPhase};

use crate::error::StoreError;
use crate::planning::tests::{
    command, goal_spec, plan_and_record, store_with_configuration, tasks_of, validated_for,
};
use crate::store::Store;
use crate::transaction::Revision;

/// A Goal with one materialized Ready Task, the shape #24 produces.
pub(crate) fn goal_with_ready_task(store: &Store, sequence: i64, goal_id: &str) {
    let epoch = store.restore_generation().expect("generation");
    store
        .create_goal(
            &command(
                epoch.as_str(),
                &format!("create-{goal_id}"),
                &[9u8; 32],
                "goal.created",
            ),
            goal_id,
            &goal_spec(),
        )
        .expect("goal commits");
    let op = format!("op-{goal_id}");
    plan_and_record(store, goal_id, sequence, &op);
    let registry = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active")
        .components
        .evaluator_registry;
    let plan = validated_for(goal_id, sequence, registry, "unit-tests-v1");
    store
        .materialize_plan(
            &command(
                epoch.as_str(),
                &format!("materialize-{goal_id}"),
                &[10u8; 32],
                "task.materialized",
            ),
            &op,
            &format!("task-{goal_id}"),
            &plan,
        )
        .expect("materialization commits");
}

fn cancel(store: &Store, goal_id: &str, command_id: &str) -> Result<(), StoreError> {
    let epoch = store.restore_generation().expect("generation");
    store
        .cancel_goal(
            &command(
                epoch.as_str(),
                command_id,
                &[11u8; 32],
                "goal.cancel.requested",
            ),
            goal_id,
        )
        .map(|_| ())
}

#[test]
fn cancelling_a_planning_goal_targets_cancelled_without_terminalizing_it() {
    // The contract's transition is Planning -> Finalizing/terminalTarget, and
    // "Goal becomes Cancelled only when those obligations are safely
    // finalized". A store that jumped straight to Cancelled would be
    // asserting a finalization nothing performed.
    let (_dir, store, _sequence) = store_with_configuration("cancel-planning");
    let epoch = store.restore_generation().expect("generation");
    store
        .create_goal(
            &command(epoch.as_str(), "create", &[9u8; 32], "goal.created"),
            "goal-1",
            &goal_spec(),
        )
        .expect("goal commits");

    cancel(&store, "goal-1", "cancel-1").expect("cancellation commits");

    let goal = store.goal("goal-1").expect("read").expect("goal exists");
    assert_eq!(goal.phase, GoalPhase::Finalizing);
    assert!(
        goal.phase.is_nonterminal(),
        "cancellation must not terminalize the Goal itself"
    );
    assert_eq!(
        goal.revision,
        Revision::new(2),
        "the fence is a revisioned CAS"
    );
}

#[test]
fn cancellation_fences_every_nonterminal_task_in_the_same_transaction() {
    // A Goal fenced without its Tasks would leave a Ready Task that a
    // Scheduler could still pick up.
    let (_dir, store, sequence) = store_with_configuration("cancel-tasks");
    goal_with_ready_task(&store, sequence, "goal-1");

    let before = tasks_of(&store, "goal-1");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].phase, TaskPhase::Ready);

    cancel(&store, "goal-1", "cancel-1").expect("cancellation commits");

    let after = tasks_of(&store, "goal-1");
    assert_eq!(after[0].phase, TaskPhase::Finalizing);
    assert_ne!(
        after[0].revision, before[0].revision,
        "the Task fence is its own revisioned write"
    );
}

#[test]
fn cancelling_an_already_cancelling_goal_changes_nothing_further() {
    let (_dir, store, sequence) = store_with_configuration("cancel-twice");
    goal_with_ready_task(&store, sequence, "goal-1");

    cancel(&store, "goal-1", "cancel-1").expect("first cancellation commits");
    let once = store.goal("goal-1").expect("read").expect("goal");
    let tasks_once = tasks_of(&store, "goal-1");

    // A *different* command id, so this is a genuine second execution rather
    // than the command kernel replaying the first one.
    cancel(&store, "goal-1", "cancel-2").expect("second cancellation commits");
    let twice = store.goal("goal-1").expect("read").expect("goal");
    let tasks_twice = tasks_of(&store, "goal-1");

    assert_eq!(
        once.revision, twice.revision,
        "no second Goal revision burned"
    );
    assert_eq!(tasks_once[0].revision, tasks_twice[0].revision);
}

/// Reads the Goal row's `terminal_target` directly.
///
/// `GoalRecord` does not carry the target, and inventing a public read path
/// for one test is exactly what `scripts/check-store-read-paths.sh` exists to
/// refuse; the read-only connection is the honest way in for an in-crate test.
fn terminal_target_of(store: &Store, goal_id: &str) -> Option<String> {
    store
        .read(|conn| {
            // A NULL target arrives as `Ok(None)`; only a missing row errors.
            conn.query_row(
                "SELECT terminal_target FROM goals WHERE id = ?1",
                [goal_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(StoreError::Sqlite)
        })
        .expect("read")
}

/// Leaves `goal_id` `Finalizing` toward `target`, the state a completion
/// acceptance (`Succeeded`) or recovery policy (`Failed`) would produce.
/// Neither path exists yet — this fixture is how the lifecycle's
/// defined-but-unreachable half gets exercised without faking production
/// behaviour. Returns the row revision it left.
fn finalize_toward(store: &Store, goal_id: &str, target: &str) -> Revision {
    let from = store
        .revision_of("goals", goal_id)
        .expect("revision")
        .expect("goal exists");
    let at = store
        .write(|writer| {
            writer.update_revisioned(
                "goals",
                goal_id,
                from,
                &[
                    ("phase", crate::transaction::Value::from("Finalizing")),
                    ("terminal_target", crate::transaction::Value::from(target)),
                ],
            )
        })
        .expect("fixture transition commits");
    assert_eq!(at, Revision::new(from.get() + 1));
    at
}

#[test]
fn cancelling_a_finalizing_goal_retargets_it_to_cancelled_in_place() {
    // The contract's table grants cancellation while Finalizing for every
    // pending outcome: Finalizing means the terminal result is not yet
    // immutable history, so cancel retargets the same finalization instead of
    // refusing. The Goal stays in Finalizing — no shortcut around it — and
    // the write is an ordinary revisioned CAS.
    for target in ["Succeeded", "Failed"] {
        let label = format!("cancel-retarget-{target}");
        let (_dir, store, sequence) = store_with_configuration(&label);
        goal_with_ready_task(&store, sequence, "goal-1");
        let finalized_at = finalize_toward(&store, "goal-1", target);

        cancel(&store, "goal-1", "cancel-1").expect("retargeting cancellation commits");

        let after = store.goal("goal-1").expect("read").expect("goal");
        assert_eq!(after.phase, GoalPhase::Finalizing, "{target}");
        assert!(
            after.phase.is_nonterminal(),
            "{target}: retargeting must not jump around finalization"
        );
        assert_eq!(
            terminal_target_of(&store, "goal-1"),
            Some("Cancelled".to_string()),
            "{target}"
        );
        assert_eq!(
            after.revision,
            Revision::new(finalized_at.get() + 1),
            "{target}: exactly one retargeting write on top of the fixture"
        );

        let tasks = tasks_of(&store, "goal-1");
        assert_eq!(tasks[0].phase, TaskPhase::Finalizing, "{target}");
    }
}

#[test]
fn a_terminal_goal_refuses_cancellation_and_history_stays_exactly_as_committed() {
    // Terminal Goals never reopen: once the phase committed, cancellation can
    // neither reopen the Goal nor rewrite what it ended as. The refusal writes
    // nothing.
    let (_dir, store, sequence) = store_with_configuration("cancel-terminal");
    goal_with_ready_task(&store, sequence, "goal-1");
    let finalized_at = finalize_toward(&store, "goal-1", "Cancelled");
    let terminal = store
        .write(|writer| {
            writer.update_revisioned(
                "goals",
                "goal-1",
                finalized_at,
                &[("phase", crate::transaction::Value::from("Cancelled"))],
            )
        })
        .expect("fixture terminalization commits");
    assert_eq!(terminal, Revision::new(finalized_at.get() + 1));
    let before = store.goal("goal-1").expect("read").expect("goal");

    let err = cancel(&store, "goal-1", "cancel-1").expect_err("must be refused");
    match err {
        StoreError::GoalNotCancellable {
            phase,
            terminal_target,
            ..
        } => {
            assert_eq!(phase, "Cancelled");
            assert_eq!(terminal_target.as_deref(), Some("Cancelled"));
        }
        other => panic!("unexpected error: {other}"),
    }

    let after = store.goal("goal-1").expect("read").expect("goal");
    assert_eq!(before, after, "a refusal rewrites nothing");
}

#[test]
fn a_refused_cancellation_consumes_no_command_identity() {
    // The refusal rolls the transaction back, so the command ledger has no
    // row and the same commandId is still usable — which is what lets a
    // client retry after fixing the request rather than being told its
    // command already ran.
    let (_dir, store, _sequence) = store_with_configuration("cancel-refusal-ledger");
    let epoch = store.restore_generation().expect("generation");
    store
        .create_goal(
            &command(epoch.as_str(), "create", &[9u8; 32], "goal.created"),
            "goal-1",
            &goal_spec(),
        )
        .expect("goal commits");
    let fixture = store
        .write(|writer| {
            writer.update_revisioned(
                "goals",
                "goal-1",
                Revision::new(1),
                &[
                    ("phase", crate::transaction::Value::from("Cancelled")),
                    (
                        "terminal_target",
                        crate::transaction::Value::from("Cancelled"),
                    ),
                ],
            )
        })
        .expect("fixture transition commits");
    assert_eq!(fixture, Revision::new(2));

    cancel(&store, "goal-1", "reused").expect_err("terminal goals never reopen");

    // The same identity is free, so a later legitimate command may take it.
    let epoch = store.restore_generation().expect("generation");
    store
        .create_goal(
            &command(epoch.as_str(), "reused", &[12u8; 32], "goal.created"),
            "goal-2",
            &goal_spec(),
        )
        .expect("the command id was never durably consumed");
}
