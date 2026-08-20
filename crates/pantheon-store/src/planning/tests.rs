//! Evidence for the durable half of Issue #24: the whole path from Goal to a
//! Ready Task, and the fencing that stops a stale plan reaching current state.

use pantheon_core::config::Digest;
use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::planning::validate::{self, Authority, EvaluatorResolver, Materializable};
use pantheon_core::planning::{GoalPhase, TaskPhase};

use crate::command::Command;
use crate::error::StoreError;
use crate::planning::{PlanningDecision, PlanningState, ProposalRecord};
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;

struct Registry(&'static str);

impl EvaluatorResolver for Registry {
    fn resolve(&self, reference: &str) -> Option<String> {
        (reference == direct::MVP_EVALUATOR_REF).then(|| self.0.to_string())
    }
}

pub(crate) fn goal_spec() -> GoalSpec {
    GoalSpec {
        objective: "Fix the checkout timeout with the smallest safe change.".to_string(),
        inputs: vec![GoalInput {
            name: "repository".to_string(),
            reference: "repo://whiskyshop".to_string(),
        }],
        deliverables: vec![Deliverable {
            name: "changeset".to_string(),
            kind: "code.changeset".to_string(),
            required: true,
        }],
        constraints: GoalConstraints {
            permitted_effects: vec![
                "filesystem.read".to_string(),
                "filesystem.write".to_string(),
            ],
            forbidden_effects: vec!["git.push".to_string()],
            permitted_resources: vec!["workspace://src/**".to_string()],
        },
    }
}

/// A store whose active configuration exists, so materialization has a policy
/// to re-read. Configuration content is #23's concern; this only needs an
/// active revision to fence against.
pub(crate) fn store_with_configuration(label: &str) -> (TempDir, Store, i64) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    activate_configuration(&store, "cfg-1", 4000);
    let sequence = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active")
        .activation_sequence;
    (dir, store, sequence)
}

pub(crate) fn activate_configuration(store: &Store, command_id: &str, memory_limit: i64) {
    let source = crate::configuration::tests::source(memory_limit);
    let compiled = pantheon_core::config::compile::compile(&source).expect("fixture compiles");
    let epoch = store.restore_generation().expect("generation");
    let expected = store.configuration_pointer().expect("pointer").revision;
    store
        .activate_configuration(
            &Command {
                epoch: epoch.as_str(),
                id: command_id,
                request_hash: &[3u8; 32],
                event_type: "configuration.activated",
            },
            &compiled,
            Digest::of(b"sources"),
            expected,
        )
        .expect("activation commits");
}

pub(crate) fn command<'a>(
    epoch: &'a str,
    id: &'a str,
    hash: &'a [u8; 32],
    event: &'a str,
) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: hash,
        event_type: event,
    }
}

/// Runs the whole path and returns the operation id.
pub(crate) fn plan_and_record(
    store: &Store,
    goal_id: &str,
    config_sequence: i64,
    op_id: &str,
) -> String {
    let spec = goal_spec();
    let input = PlanningInput {
        goal_id,
        goal_revision: 1,
        goal: &spec,
        expected_graph_revision: 0,
        configuration_activation_sequence: config_sequence,
        trigger: Trigger::Initial,
    };
    let proposal = direct::plan(&input);
    let canonical =
        String::from_utf8(proposal.to_value().to_canonical_bytes()).expect("canonical utf-8");
    let epoch = store.restore_generation().expect("generation");
    store
        .record_direct_planning(
            &command(epoch.as_str(), op_id, &[1u8; 32], "planning.recorded"),
            &PlanningDecision {
                operation_id: op_id,
                goal_id,
                goal_revision: 1,
                expected_graph_revision: 0,
                configuration_activation_sequence: config_sequence,
                planning_input_digest: input.digest(),
                trigger_kind: Trigger::Initial.as_str(),
                planner_implementation: direct::PLANNER_IMPLEMENTATION,
                planner_version: direct::PLANNER_VERSION,
            },
            &ProposalRecord {
                digest: proposal.digest(),
                canonical: &canonical,
                normalization_provenance: "direct/v1",
            },
        )
        .expect("planning commits");
    op_id.to_string()
}

pub(crate) fn validated(
    config_sequence: i64,
    registry_digest: Digest,
    version: &'static str,
) -> Materializable {
    let spec = goal_spec();
    let input = PlanningInput {
        goal_id: "goal-1",
        goal_revision: 1,
        goal: &spec,
        expected_graph_revision: 0,
        configuration_activation_sequence: config_sequence,
        trigger: Trigger::Initial,
    };
    let proposal = direct::plan(&input);
    let registry = Registry(version);
    validate::validate(
        &proposal,
        &Authority {
            goal: &spec,
            goal_id: "goal-1",
            goal_revision: 1,
            evaluators: &registry,
            evaluator_registry_digest: registry_digest,
            configuration_activation_sequence: config_sequence,
        },
    )
    .expect("the proposal validates")
}

pub(crate) fn validated_for(
    goal_id: &str,
    config_sequence: i64,
    registry_digest: Digest,
    version: &'static str,
) -> Materializable {
    let spec = goal_spec();
    let input = PlanningInput {
        goal_id,
        goal_revision: 1,
        goal: &spec,
        expected_graph_revision: 0,
        configuration_activation_sequence: config_sequence,
        trigger: Trigger::Initial,
    };
    let proposal = direct::plan(&input);
    let registry = Registry(version);
    validate::validate(
        &proposal,
        &Authority {
            goal: &spec,
            goal_id,
            goal_revision: 1,
            evaluators: &registry,
            evaluator_registry_digest: registry_digest,
            configuration_activation_sequence: config_sequence,
        },
    )
    .expect("the proposal validates")
}

pub(crate) fn create_goal(store: &Store, goal_id: &str, command_id: &str) {
    let epoch = store.restore_generation().expect("generation");
    store
        .create_goal(
            &command(epoch.as_str(), command_id, &[2u8; 32], "goal.created"),
            goal_id,
            &goal_spec(),
        )
        .expect("goal creation commits");
}

pub(crate) fn materialize(
    store: &Store,
    op_id: &str,
    task_id: &str,
    plan: &Materializable,
    command_id: &str,
) -> Result<crate::planning::MaterializedPlan, StoreError> {
    let epoch = store.restore_generation().expect("generation");
    store
        .materialize_plan(
            &command(epoch.as_str(), command_id, &[4u8; 32], "graph.patched"),
            op_id,
            task_id,
            plan,
        )
        .map(|committed| match committed {
            crate::command::Committed::Executed { value, .. } => value,
            crate::command::Committed::Replayed { .. } => {
                panic!("expected execution, got a replay")
            }
        })
}

#[test]
fn a_new_goal_begins_in_planning_with_an_empty_graph() {
    let (_dir, store, _seq) = store_with_configuration("goal-fresh");
    create_goal(&store, "goal-1", "cmd-goal");

    let goal = store.goal("goal-1").expect("read goal").expect("exists");
    assert_eq!(
        goal.phase,
        GoalPhase::Planning,
        "a Goal with no graph is Planning"
    );
    assert_eq!(goal.current_revision, 1);

    let graph = store
        .task_graph("goal-1")
        .expect("read graph")
        .expect("exists");
    assert_eq!(
        graph.revision,
        Revision::new(0),
        "the graph exists before any Task"
    );
    assert!(
        store
            .tasks_for_goal("goal-1")
            .expect("read tasks")
            .is_empty()
    );
}

#[test]
fn the_whole_path_produces_one_ready_task_in_one_graph_revision() {
    let (_dir, store, seq) = store_with_configuration("goal-path");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");

    // The planning decision is durable evidence and has changed nothing.
    let operation = store
        .planning_operation(&op)
        .expect("read operation")
        .expect("exists");
    assert_eq!(operation.state, PlanningState::Planned);
    assert_eq!(operation.goal_revision, 1);
    assert_eq!(operation.expected_graph_revision, 0);
    assert!(
        store.tasks_for_goal("goal-1").expect("tasks").is_empty(),
        "a recorded proposal must not have created a Task"
    );
    assert_eq!(
        store
            .task_graph("goal-1")
            .expect("graph")
            .expect("exists")
            .revision,
        Revision::new(0),
        "a recorded proposal must not have patched the graph"
    );

    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    let outcome = materialize(&store, &op, "task-1", &plan, "cmd-materialize")
        .expect("materialization commits");

    assert_eq!(outcome.graph_revision, 1);
    assert_eq!(outcome.goal_phase, GoalPhase::Active);

    let tasks = store.tasks_for_goal("goal-1").expect("tasks");
    assert_eq!(tasks.len(), 1, "DIRECT materializes exactly one Task");
    let task = &tasks[0];
    assert_eq!(
        task.phase,
        TaskPhase::Ready,
        "no unmet prerequisites, no Runs"
    );
    assert_eq!(
        task.active_run_id, None,
        "Ready means zero nonterminal Runs"
    );
    assert_eq!(task.created_graph_revision, 1);

    // The Goal now has coherent graph authority, but Task creation is not
    // Goal success.
    let goal = store.goal("goal-1").expect("goal").expect("exists");
    assert_eq!(goal.phase, GoalPhase::Active);
    assert!(goal.phase.is_nonterminal());

    // The decision is spent, so it cannot patch the graph again.
    let operation = store.planning_operation(&op).expect("op").expect("exists");
    assert_eq!(operation.state, PlanningState::Materialized);
}

#[test]
fn the_materialized_task_pins_the_evaluator_version_durably() {
    let (_dir, store, seq) = store_with_configuration("goal-pin");
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", seq, "op-1");
    let plan = validated(seq, Digest::of(b"registry"), "unit-tests-v1");
    materialize(&store, &op, "task-1", &plan, "cmd-materialize").expect("commits");

    let task = store.tasks_for_goal("goal-1").expect("tasks").remove(0);
    let spec_json = store
        .task_spec_json(task.spec_digest)
        .expect("read spec")
        .expect("exists");

    assert!(
        spec_json.contains("unit-tests-v1"),
        "the exact version is pinned"
    );
    assert!(
        spec_json.contains("evaluatorRegistryDigest"),
        "resolution provenance travels with the pin"
    );
    // The Task embeds no executable command: only the coordinate that
    // recovers one from content-addressed configuration.
    assert!(
        !spec_json.contains("argv"),
        "a Task may not embed an executable command"
    );
}
