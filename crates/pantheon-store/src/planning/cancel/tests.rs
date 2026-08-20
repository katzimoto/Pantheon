//! Evidence that cancellation is a fence, not an outcome.

use pantheon_core::planning::{GoalPhase, TaskPhase};

use crate::error::StoreError;
use crate::planning::tests::{
    command, goal_spec, plan_and_record, store_with_configuration, validated_for,
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

    let before = store.tasks_for_goal("goal-1").expect("tasks");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].phase, TaskPhase::Ready);

    cancel(&store, "goal-1", "cancel-1").expect("cancellation commits");

    let after = store.tasks_for_goal("goal-1").expect("tasks");
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
    let tasks_once = store.tasks_for_goal("goal-1").expect("tasks");

    // A *different* command id, so this is a genuine second execution rather
    // than the command kernel replaying the first one.
    cancel(&store, "goal-1", "cancel-2").expect("second cancellation commits");
    let twice = store.goal("goal-1").expect("read").expect("goal");
    let tasks_twice = store.tasks_for_goal("goal-1").expect("tasks");

    assert_eq!(
        once.revision, twice.revision,
        "no second Goal revision burned"
    );
    assert_eq!(tasks_once[0].revision, tasks_twice[0].revision);
}

#[test]
fn a_goal_finalizing_toward_another_outcome_is_refused_and_nothing_is_written() {
    // Retargeting Finalizing/Succeeded to Cancelled is a transition the Goal
    // lifecycle table does not grant, so it fails closed rather than being
    // guessed at.
    let (_dir, store, _sequence) = store_with_configuration("cancel-retarget");
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
                    ("phase", crate::transaction::Value::from("Finalizing")),
                    (
                        "terminal_target",
                        crate::transaction::Value::from("Succeeded"),
                    ),
                ],
            )
        })
        .expect("fixture transition commits");
    assert_eq!(fixture, Revision::new(2));
    let before = store.goal("goal-1").expect("read").expect("goal");

    let err = cancel(&store, "goal-1", "cancel-1").expect_err("must be refused");
    match err {
        StoreError::GoalNotCancellable {
            phase,
            terminal_target,
            ..
        } => {
            assert_eq!(phase, "Finalizing");
            assert_eq!(terminal_target.as_deref(), Some("Succeeded"));
        }
        other => panic!("unexpected error: {other}"),
    }

    let after = store.goal("goal-1").expect("read").expect("goal");
    assert_eq!(before.revision, after.revision, "a refusal writes nothing");
    assert_eq!(after.phase, GoalPhase::Finalizing);
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
