//! Evidence that a stale or spent planning decision cannot reach current
//! authority, and that a failed patch leaves the graph untouched.

use pantheon_core::config::Digest;
use pantheon_core::planning::{GoalPhase, TaskPhase};

use crate::error::StoreError;
use crate::planning::PlanningState;
use crate::planning::tests::{
    activate_configuration, command, create_goal, goal_spec, materialize, plan_and_record,
    store_with_configuration, validated, validated_for,
};
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;

#[test]
fn a_stale_goal_revision_cannot_materialize() {
    let (_dir, store, seq) = store_with_configuration("goal-stale-goal");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");

    // The Goal moves on after planning observed revision 1.
    store
        .write(|writer| {
            writer.execute(
                "INSERT INTO goal_revisions (goal_id, revision, content_digest, canonical_json, created_at)
                 VALUES ('goal-1', 2, ?1, '{}', 0)",
                &[crate::transaction::Value::Blob(vec![9u8; 32])],
            )?;
            let _ = writer.update_revisioned(
                "goals",
                "goal-1",
                Revision::new(1),
                &[("current_revision", crate::transaction::Value::Integer(2))],
            )?;
            Ok(())
        })
        .expect("goal advances");

    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    let err = materialize(&store, &op, "task-1", &plan, "cmd-materialize")
        .expect_err("a plan for a superseded Goal revision must be refused");
    assert!(
        matches!(
            err,
            StoreError::RevisionConflict {
                table: "goal_revisions",
                ..
            }
        ),
        "unexpected: {err}"
    );

    assert!(
        store.tasks_for_goal("goal-1").expect("tasks").is_empty(),
        "a stale plan must not create a Task"
    );
    assert_eq!(
        store
            .task_graph("goal-1")
            .expect("graph")
            .expect("exists")
            .revision,
        Revision::new(0),
        "and must not patch the graph"
    );
}

#[test]
fn a_stale_graph_revision_cannot_materialize() {
    let (_dir, store, seq) = store_with_configuration("goal-stale-graph");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");

    // Something else patches the graph after planning froze revision 0.
    store
        .write(|writer| {
            let _ = writer.update_revisioned("task_graphs", "goal-1", Revision::new(0), &[])?;
            Ok(())
        })
        .expect("graph advances");

    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    let err = materialize(&store, &op, "task-1", &plan, "cmd-materialize")
        .expect_err("a plan against a superseded graph revision must be refused");
    assert!(
        matches!(
            err,
            StoreError::RevisionConflict {
                table: "task_graphs",
                ..
            }
        ),
        "unexpected: {err}"
    );
    assert!(store.tasks_for_goal("goal-1").expect("tasks").is_empty());
}

#[test]
fn a_configuration_change_since_planning_cannot_materialize() {
    // The contract requires rechecking current *policy*, not only Goal and
    // graph: the plan pinned evaluator versions from a configuration that is
    // no longer active.
    let (_dir, store, seq) = store_with_configuration("goal-stale-config");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");

    activate_configuration(&store, "cfg-2", 4096);

    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    let err = materialize(&store, &op, "task-1", &plan, "cmd-materialize")
        .expect_err("a plan under a superseded configuration must be refused");
    assert!(
        matches!(
            err,
            StoreError::RevisionConflict {
                table: "active_configuration",
                ..
            }
        ),
        "unexpected: {err}"
    );
    assert!(store.tasks_for_goal("goal-1").expect("tasks").is_empty());
}

#[test]
fn one_planning_decision_cannot_materialize_twice() {
    // Replay protection covers the same command identity; this covers the
    // other route — a second, different command reusing a spent decision.
    let (_dir, store, seq) = store_with_configuration("goal-spent");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");
    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    materialize(&store, &op, "task-1", &plan, "cmd-materialize").expect("first commits");

    let err = materialize(&store, &op, "task-2", &plan, "cmd-materialize-again")
        .expect_err("a spent decision must not patch the graph again");
    assert!(
        matches!(
            err,
            StoreError::RevisionConflict {
                table: "planning_operations",
                ..
            }
        ),
        "unexpected: {err}"
    );
    assert_eq!(
        store.tasks_for_goal("goal-1").expect("tasks").len(),
        1,
        "still exactly one Task"
    );
}

#[test]
fn replaying_the_command_identities_creates_no_duplicate_authority() {
    let (_dir, store, seq) = store_with_configuration("goal-replay");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");
    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    materialize(&store, &op, "task-1", &plan, "cmd-materialize").expect("commits");

    // Every command replayed with the same identity and hash.
    let epoch = store.restore_generation().expect("generation");
    let replayed_goal = store
        .create_goal(
            &command(epoch.as_str(), "cmd-goal", &[2u8; 32], "goal.created"),
            "goal-1",
            &goal_spec(),
        )
        .expect("goal creation reconciles");
    assert!(!replayed_goal.was_executed());

    let replayed_materialize = store
        .materialize_plan(
            &command(
                epoch.as_str(),
                "cmd-materialize",
                &[4u8; 32],
                "graph.patched",
            ),
            &op,
            "task-1",
            &plan,
        )
        .expect("materialization reconciles");
    assert!(!replayed_materialize.was_executed());

    // Exactly one of everything.
    assert_eq!(store.tasks_for_goal("goal-1").expect("tasks").len(), 1);
    assert_eq!(
        store
            .task_graph("goal-1")
            .expect("graph")
            .expect("exists")
            .revision,
        Revision::new(1)
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM goal_revisions")
            .expect("count"),
        vec![1]
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM planning_operations")
            .expect("count"),
        vec![1]
    );
}

#[test]
fn a_failure_during_materialization_leaves_the_graph_untouched() {
    let (_dir, store, seq) = store_with_configuration("goal-rollback");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");
    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");

    let epoch = store.restore_generation().expect("generation");
    let err = store
        .execute_command(
            &command(epoch.as_str(), "cmd-doomed", &[5u8; 32], "graph.patched"),
            |writer| {
                // The entire patch lands inside this transaction...
                crate::planning::materialize::apply(writer, &op, "task-1", &plan)?;
                // ...and then fails.
                Err::<(), StoreError>(StoreError::InvariantViolated("injected".to_string()))
            },
        )
        .expect_err("the injected failure aborts the patch");
    assert!(matches!(err, StoreError::InvariantViolated(ref d) if d == "injected"));

    assert!(
        store.tasks_for_goal("goal-1").expect("tasks").is_empty(),
        "no Task may survive"
    );
    assert_eq!(
        store
            .task_graph("goal-1")
            .expect("graph")
            .expect("exists")
            .revision,
        Revision::new(0),
        "the graph revision must not have advanced"
    );
    assert_eq!(
        store.goal("goal-1").expect("goal").expect("exists").phase,
        GoalPhase::Planning,
        "the Goal must not have left Planning"
    );
    assert_eq!(
        store
            .planning_operation(&op)
            .expect("op")
            .expect("exists")
            .state,
        PlanningState::Planned,
        "the decision must still be spendable"
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM task_specs")
            .expect("count"),
        vec![0],
        "no immutable spec may survive"
    );
}

#[test]
fn reopening_preserves_the_goal_graph_and_task_authority() {
    let dir = TempDir::new("goal-reopen");
    let path = dir.path().join("pantheon.db");
    let (before_task, before_graph, before_spec) = {
        let store = Store::open(&path).expect("open store");
        activate_configuration(&store, "cfg-1", 4000);
        let seq = store
            .configuration_pointer()
            .expect("pointer")
            .active
            .expect("active")
            .activation_sequence;
        create_goal(&store, "goal-1", "cmd-goal");
        let op = plan_and_record(&store, "goal-1", seq, "op-1");
        let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
        materialize(&store, &op, "task-1", &plan, "cmd-materialize").expect("commits");
        let task = store.tasks_for_goal("goal-1").expect("tasks").remove(0);
        let graph = store.task_graph("goal-1").expect("graph").expect("exists");
        let spec = store
            .task_spec_json(task.spec_digest)
            .expect("spec")
            .expect("exists");
        store.close().expect("close");
        (task, graph, spec)
    };

    let store = Store::open(&path).expect("reopen");
    let task = store.tasks_for_goal("goal-1").expect("tasks").remove(0);
    assert_eq!(task, before_task, "the Task survives reopen exactly");
    assert_eq!(task.phase, TaskPhase::Ready);
    assert_eq!(
        store.task_graph("goal-1").expect("graph").expect("exists"),
        before_graph
    );
    assert_eq!(
        store
            .task_spec_json(task.spec_digest)
            .expect("spec")
            .expect("exists"),
        before_spec,
        "the immutable spec is byte-identical"
    );
    assert_eq!(
        store.goal("goal-1").expect("goal").expect("exists").phase,
        GoalPhase::Active
    );
}

#[test]
fn readiness_follows_the_dependency_predicate_rather_than_assuming_a_first_task_is_ready() {
    // #24 materializes one Task with no dependencies, so both branches of the
    // readiness decision produce `Ready` and the predicate is never exercised
    // by the product path. Seeding an upstream Task and an active
    // `requires_success` edge directly is what gives the predicate teeth —
    // without putting an artificial dependency into a materialized graph,
    // which the mission forbids.
    let (_dir, store, seq) = store_with_configuration("ready-predicate");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");
    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    materialize(&store, &op, "task-1", &plan, "cmd-materialize").expect("commits");

    // An upstream Task that has not succeeded, and an edge into the existing
    // Task, active at the current graph revision.
    store
        .write(|writer| {
            writer.execute(
                "INSERT INTO tasks (id, goal_id, created_graph_revision, phase, revision,
                                    terminal_target, terminal_reason_json, active_run_id, spec_digest)
                 SELECT 'task-upstream', 'goal-1', 1, 'Ready', 1, NULL, NULL, NULL, spec_digest
                 FROM tasks WHERE id = 'task-1'",
                &[],
            )?;
            writer.execute(
                "INSERT INTO task_graph_edges (goal_id, upstream_task_id, downstream_task_id,
                                               kind, created_graph_revision, removed_graph_revision)
                 VALUES ('goal-1', 'task-upstream', 'task-1', 'requires_success', 1, NULL)",
                &[],
            )?;
            Ok(())
        })
        .expect("seed an unsatisfied prerequisite");

    // The predicate now reports the prerequisite unmet, so a Task created
    // under it would be Pending rather than Ready.
    let satisfied = store
        .write(|writer| crate::planning::materialize::prerequisites_satisfied(writer, "task-1", 1))
        .expect("evaluate the predicate");
    assert!(
        !satisfied,
        "a Task whose upstream has not succeeded has unmet prerequisites"
    );

    // And a terminally *failed* upstream is never silently satisfied.
    store
        .write(|writer| {
            writer.execute(
                "UPDATE tasks SET phase = 'Finalizing', terminal_target = 'Failed'
                 WHERE id = 'task-upstream'",
                &[],
            )?;
            writer.execute(
                "UPDATE tasks SET phase = 'Failed', terminal_target = 'Failed'
                 WHERE id = 'task-upstream'",
                &[],
            )?;
            Ok(())
        })
        .expect("upstream fails");
    let satisfied = store
        .write(|writer| crate::planning::materialize::prerequisites_satisfied(writer, "task-1", 1))
        .expect("evaluate the predicate");
    assert!(
        !satisfied,
        "a failed upstream must never count as a satisfied prerequisite"
    );

    // Only terminal success satisfies it.
    store
        .write(|writer| {
            writer.execute(
                "UPDATE tasks SET phase = 'Succeeded', terminal_target = 'Succeeded'
                 WHERE id = 'task-upstream'",
                &[],
            )?;
            Ok(())
        })
        .expect("upstream succeeds");
    let satisfied = store
        .write(|writer| crate::planning::materialize::prerequisites_satisfied(writer, "task-1", 1))
        .expect("evaluate the predicate");
    assert!(satisfied, "a succeeded upstream satisfies the prerequisite");
}
