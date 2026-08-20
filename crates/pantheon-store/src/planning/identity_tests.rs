//! Evidence that materialization fences *content*, not only revision numbers:
//! the Goal a plan was validated against, the proposal the decision recorded,
//! and the Goal a spec belongs to.

use pantheon_core::config::Digest;

use crate::error::StoreError;
use crate::planning::tests::{
    create_goal, goal_spec, materialize, plan_and_record, store_with_configuration, validated_for,
};
use crate::transaction::Revision;

#[test]
fn a_plan_validated_against_a_different_goal_cannot_materialize() {
    // Fencing the revision *number* is not enough. This plan claims the right
    // revision but was validated against a Goal whose ceiling is wider than
    // the stored one — the exact way a caller could smuggle authority past a
    // number-only fence.
    use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
    use pantheon_core::planning::validate::{self, Authority, EvaluatorResolver};

    struct Registry;
    impl EvaluatorResolver for Registry {
        fn resolve(&self, reference: &str) -> Option<String> {
            (reference == direct::MVP_EVALUATOR_REF).then(|| "unit-tests-v1".to_string())
        }
    }

    let (_dir, store, seq) = store_with_configuration("goal-divergent-spec");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");

    // A Goal that permits more than the one actually stored.
    let mut wider = goal_spec();
    wider
        .constraints
        .permitted_effects
        .push("network.connect".to_string());
    wider
        .constraints
        .permitted_resources
        .push("workspace://secrets/**".to_string());

    let input = PlanningInput {
        goal_id: "goal-1",
        goal_revision: 1,
        goal: &wider,
        expected_graph_revision: 0,
        configuration_activation_sequence: seq,
        trigger: Trigger::Initial,
    };
    let proposal = direct::plan(&input);
    let plan = validate::validate(
        &proposal,
        &Authority {
            goal: &wider,
            goal_id: "goal-1",
            goal_revision: 1,
            evaluators: &Registry,
            evaluator_registry_digest: Digest::of(b"registry"),
            configuration_activation_sequence: seq,
        },
    )
    .expect("it validates against the wider goal, which is the point");

    let err = materialize(&store, &op, "task-1", &plan, "cmd-materialize")
        .expect_err("a plan validated against a different goal must be refused");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("not the stored revision")),
        "unexpected: {err}"
    );
    assert!(
        store.tasks_for_goal("goal-1").expect("tasks").is_empty(),
        "no Task may exceed the stored Goal's ceiling"
    );
    assert_eq!(
        store
            .task_graph("goal-1")
            .expect("graph")
            .expect("exists")
            .revision,
        Revision::new(0)
    );
}

#[test]
fn a_plan_that_is_not_the_recorded_proposal_cannot_materialize() {
    // The PlanningRecord is provenance; this is what makes a materialized Task
    // auditable back to it rather than to some other proposal.
    use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
    use pantheon_core::planning::validate::{self, Authority, EvaluatorResolver};

    struct Registry;
    impl EvaluatorResolver for Registry {
        fn resolve(&self, reference: &str) -> Option<String> {
            (reference == direct::MVP_EVALUATOR_REF).then(|| "unit-tests-v1".to_string())
        }
    }

    let (_dir, store, seq) = store_with_configuration("goal-other-proposal");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");

    // A different proposal for the same Goal: narrower, so it still validates.
    let spec = goal_spec();
    let input = PlanningInput {
        goal_id: "goal-1",
        goal_revision: 1,
        goal: &spec,
        expected_graph_revision: 0,
        configuration_activation_sequence: seq,
        trigger: Trigger::Initial,
    };
    let mut other = direct::plan(&input);
    other.tasks[0].objective = "A different objective entirely.".to_string();
    let plan = validate::validate(
        &other,
        &Authority {
            goal: &spec,
            goal_id: "goal-1",
            goal_revision: 1,
            evaluators: &Registry,
            evaluator_registry_digest: Digest::of(b"registry"),
            configuration_activation_sequence: seq,
        },
    )
    .expect("the substitute proposal is itself valid");

    let err = materialize(&store, &op, "task-1", &plan, "cmd-materialize")
        .expect_err("a proposal the operation did not record must be refused");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("recorded for operation")),
        "unexpected: {err}"
    );
    assert!(store.tasks_for_goal("goal-1").expect("tasks").is_empty());
}

#[test]
fn two_goals_with_identical_content_do_not_share_one_task_spec() {
    // The spec is content-addressed, so without the Goal in its identity two
    // identical Goals would collapse to one row and the second Goal's Task
    // would reference a spec attributed to the first.
    let (_dir, store, seq) = store_with_configuration("goal-identical");
    create_goal(&store, "goal-1", "cmd-goal-1");
    create_goal(&store, "goal-2", "cmd-goal-2");

    let op1 = plan_and_record(&store, "goal-1", seq, "op-1");
    let op2 = plan_and_record(&store, "goal-2", seq, "op-2");

    let plan1 = validated_for("goal-1", seq, Digest::of(b"registry"), "unit-tests-v1");
    let plan2 = validated_for("goal-2", seq, Digest::of(b"registry"), "unit-tests-v1");
    assert_ne!(
        plan1.spec().digest(),
        plan2.spec().digest(),
        "identical content under different Goals must not be one spec"
    );

    materialize(&store, &op1, "task-1", &plan1, "cmd-mat-1").expect("first commits");
    materialize(&store, &op2, "task-2", &plan2, "cmd-mat-2").expect("second commits");

    let first = store.tasks_for_goal("goal-1").expect("tasks").remove(0);
    let second = store.tasks_for_goal("goal-2").expect("tasks").remove(0);
    assert_ne!(first.spec_digest, second.spec_digest);

    // And each spec row is attributed to its own Goal.
    let attribution = store
        .read_all_for_test(
            "SELECT COUNT(*) FROM tasks task
             JOIN task_specs spec ON spec.digest = task.spec_digest
             WHERE spec.goal_id != task.goal_id",
        )
        .expect("count misattributions");
    assert_eq!(
        attribution,
        vec![0],
        "no spec may be attributed to another Goal"
    );
}
