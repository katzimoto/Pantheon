//! End-to-end evidence for Issue #24: the full semantic path from an active
//! ConfigurationRevision to a Ready Task, with evaluator resolution coming
//! from the active configuration rather than a test double.

use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::planning::{GoalPhase, TaskPhase};
use pantheon_store::{Command, Store};

use super::{PlanningController, PlanningError};
use crate::configuration::{ConfigurationAuthority, SourceSet};

/// A configuration whose evaluator registry resolves the MVP ref, so
/// resolution exercises the real stored component.
fn configuration_source() -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"accepts":["code-change"],"competencies":["rust"],
    "routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"]}}],
  "routing": {{"policies":[{{"name":"default","ordering":["featureMatch"],"tieBreak":"backendId"}}]}},
  "execution": {{
    "profiles":[{{"name":"strict","isolationClass":"CONTAINER",
      "guarantees":["isolation.control-plane"],"networkMode":"NONE",
      "environmentIdentity":"sha256:image"}}],
    "backends":[{{"backendId":"fake-local","enabled":true,"selector":"fake"}}]}},
  "evaluators": {{
    "versions":[{{"id":"unit-tests-v1","kind":"check","argv":["/bin/check"],"timeoutMs":1000,
      "sandboxProfile":"strict","resultProtocol":"p-v1"}}],
    "refs":[{{"ref":"{}","currentVersion":"unit-tests-v1"}}]}},
  "context": {{"schemaVersion":1,"mandatorySections":["task"],"preloadPriority":["task"],
    "memoryLimitTokens":4000,"workspaceOrientationLimitTokens":2000,
    "safetyMarginTokens":512,"optionalDropOrder":["memory"]}},
  "authorization": {{"schemaVersion":1,"rules":[{{"action":"filesystem.read","effect":"permit"}}]}}
}}"#,
        direct::MVP_EVALUATOR_REF
    )
}

fn goal_spec() -> GoalSpec {
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

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pantheon-planning-test-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated test directory");
        Self(dir)
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.0.join("pantheon.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command<'a>(epoch: &'a str, id: &'a str, hash: &'a [u8; 32], event: &'a str) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: hash,
        event_type: event,
    }
}

/// A store with an active configuration, ready for planning.
fn prepared(label: &str) -> (TempDir, Store) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.db_path()).expect("open store");
    {
        let authority = ConfigurationAuthority::new(&store);
        let epoch = store.restore_generation().expect("generation");
        authority
            .activate(
                &command(
                    epoch.as_str(),
                    "cfg-1",
                    &[9u8; 32],
                    "configuration.activated",
                ),
                &SourceSet::single("pantheon.json", configuration_source()),
            )
            .expect("configuration activates");
    }
    (dir, store)
}

fn proposal(store: &Store, goal_id: &str, spec: &GoalSpec) -> pantheon_core::planning::Proposal {
    let goal = store.goal(goal_id).expect("goal").expect("exists");
    let graph = store.task_graph(goal_id).expect("graph").expect("exists");
    let sequence = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active")
        .activation_sequence;
    direct::plan(&PlanningInput {
        goal_id,
        goal_revision: goal.current_revision,
        goal: spec,
        expected_graph_revision: graph.revision.get(),
        configuration_activation_sequence: sequence,
        trigger: Trigger::Initial,
    })
}

#[test]
fn the_full_path_reaches_a_ready_task_with_a_pinned_evaluator() {
    let (_dir, store) = prepared("full-path");
    let controller = PlanningController::new(&store);
    let spec = goal_spec();
    let epoch = store.restore_generation().expect("generation");

    controller
        .create_goal(
            &command(epoch.as_str(), "cmd-goal", &[1u8; 32], "goal.created"),
            "goal-1",
            &spec,
        )
        .expect("goal created");
    assert_eq!(
        store.goal("goal-1").expect("goal").expect("exists").phase,
        GoalPhase::Planning
    );

    controller
        .plan(
            &command(epoch.as_str(), "cmd-plan", &[2u8; 32], "planning.recorded"),
            "op-1",
            "goal-1",
        )
        .expect("planning recorded");

    // Evidence only: the graph has not moved.
    assert_eq!(
        store
            .task_graph("goal-1")
            .expect("graph")
            .expect("exists")
            .revision
            .get(),
        0
    );

    let proposal = proposal(&store, "goal-1", &spec);
    let outcome = controller
        .materialize(
            &command(
                epoch.as_str(),
                "cmd-materialize",
                &[3u8; 32],
                "graph.patched",
            ),
            "op-1",
            "task-1",
            "goal-1",
            &proposal,
        )
        .expect("materialized");
    let plan = match outcome {
        pantheon_store::Committed::Executed { value, .. } => value,
        pantheon_store::Committed::Replayed { .. } => panic!("expected execution"),
    };

    assert_eq!(plan.graph_revision, 1);
    assert_eq!(plan.task.phase, TaskPhase::Ready);
    assert_eq!(plan.goal_phase, GoalPhase::Active);

    // The evaluator version came from the active ConfigurationRevision, not a
    // test double.
    let spec_json = store
        .task_spec_json(plan.task.spec_digest)
        .expect("spec")
        .expect("exists");
    assert!(spec_json.contains("unit-tests-v1"));
    assert!(spec_json.contains(direct::MVP_EVALUATOR_REF));
}

#[test]
fn a_proposal_that_escalates_beyond_the_goal_never_reaches_the_store() {
    // Validation is pure, so a refused proposal provably cannot have written
    // anything — there is no path from here to a transaction.
    let (_dir, store) = prepared("escalation");
    let controller = PlanningController::new(&store);
    let spec = goal_spec();
    let epoch = store.restore_generation().expect("generation");
    controller
        .create_goal(
            &command(epoch.as_str(), "cmd-goal", &[1u8; 32], "goal.created"),
            "goal-1",
            &spec,
        )
        .expect("goal created");
    controller
        .plan(
            &command(epoch.as_str(), "cmd-plan", &[2u8; 32], "planning.recorded"),
            "op-1",
            "goal-1",
        )
        .expect("planning recorded");

    let mut broken = proposal(&store, "goal-1", &spec);
    broken.tasks[0]
        .permitted_effects
        .push("git.push".to_string());

    let err = controller
        .materialize(
            &command(
                epoch.as_str(),
                "cmd-materialize",
                &[3u8; 32],
                "graph.patched",
            ),
            "op-1",
            "task-1",
            "goal-1",
            &broken,
        )
        .expect_err("an escalating proposal is refused");
    assert!(
        matches!(err, PlanningError::Invalid(_)),
        "unexpected: {err}"
    );

    assert!(store.tasks_for_goal("goal-1").expect("tasks").is_empty());
    assert_eq!(
        store
            .task_graph("goal-1")
            .expect("graph")
            .expect("exists")
            .revision
            .get(),
        0
    );
    assert_eq!(
        store.goal("goal-1").expect("goal").expect("exists").phase,
        GoalPhase::Planning,
        "a rejected plan leaves the Goal in Planning rather than failing it"
    );
}

#[test]
fn planning_resolves_evaluators_from_the_active_configuration() {
    // If the active configuration cannot resolve the MVP evaluator ref,
    // validation must refuse rather than pin nothing.
    let dir = TempDir::new("no-evaluator");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    // A configuration whose registry resolves a different logical ref.
    let source = configuration_source().replace(direct::MVP_EVALUATOR_REF, "check://other/suite");
    authority
        .activate(
            &command(
                epoch.as_str(),
                "cfg-1",
                &[9u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single("pantheon.json", source),
        )
        .expect("configuration activates");

    let controller = PlanningController::new(&store);
    let spec = goal_spec();
    controller
        .create_goal(
            &command(epoch.as_str(), "cmd-goal", &[1u8; 32], "goal.created"),
            "goal-1",
            &spec,
        )
        .expect("goal created");

    let proposal = proposal(&store, "goal-1", &spec);
    let err = controller
        .validate("goal-1", &proposal)
        .expect_err("an unresolvable evaluator must be refused");
    assert!(
        matches!(err, PlanningError::Invalid(ref inner) if inner.kind() == "unknown-evaluator"),
        "unexpected: {err}"
    );
}
