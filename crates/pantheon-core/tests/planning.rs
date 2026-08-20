//! Executable evidence for Issue #24 planning semantics: DIRECT planning is
//! deterministic, a proposal is not authority, and validation rejects what the
//! Task and Goal contracts forbid.

use pantheon_core::config::Digest;
use pantheon_core::planning::direct::{self, MVP_EVALUATOR_REF, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::planning::proposal::Proposal;
use pantheon_core::planning::task::Severity;
use pantheon_core::planning::validate::{self, Authority, EvaluatorResolver};

/// Resolves exactly the MVP evaluator, standing in for the active
/// ConfigurationRevision's evaluator registry.
struct Registry {
    version: Option<String>,
}

impl EvaluatorResolver for Registry {
    fn resolve(&self, reference: &str) -> Option<String> {
        if reference == MVP_EVALUATOR_REF {
            self.version.clone()
        } else {
            None
        }
    }
}

fn goal() -> GoalSpec {
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
                "process.spawn".to_string(),
            ],
            forbidden_effects: vec!["git.push".to_string()],
            permitted_resources: vec!["workspace://src/**".to_string()],
        },
    }
}

fn input<'a>(goal: &'a GoalSpec) -> PlanningInput<'a> {
    PlanningInput {
        goal_id: "goal-1",
        goal_revision: 1,
        goal,
        expected_graph_revision: 0,
        configuration_activation_sequence: 1,
        trigger: Trigger::Initial,
    }
}

fn registry() -> Registry {
    Registry {
        version: Some("unit-tests-v1".to_string()),
    }
}

fn authority<'a>(goal: &'a GoalSpec, evaluators: &'a Registry) -> Authority<'a> {
    Authority {
        goal,
        goal_id: "goal-1",
        goal_revision: 1,
        evaluators,
        evaluator_registry_digest: Digest::of(b"registry"),
        configuration_activation_sequence: 1,
    }
}

#[test]
fn direct_planning_is_deterministic() {
    // The planning input digest is only meaningful as reproduction provenance
    // if the same input really does produce the same proposal.
    let goal = goal();
    let first = direct::plan(&input(&goal));
    let second = direct::plan(&input(&goal));
    assert_eq!(first, second);
    assert_eq!(first.digest(), second.digest());
}

#[test]
fn a_direct_proposal_is_one_task_and_no_dependencies() {
    let goal = goal();
    let proposal = direct::plan(&input(&goal));
    assert_eq!(proposal.tasks.len(), 1);
    assert!(
        proposal.edges.is_empty(),
        "DIRECT must not invent dependencies to exercise graph machinery"
    );
}

#[test]
fn the_planning_input_digest_covers_every_thing_the_decision_depended_on() {
    // Changing any of Goal revision, Goal content, graph precondition or
    // configuration must change the recorded provenance, or the digest cannot
    // identify what the planner observed.
    let goal = goal();
    let base = input(&goal).digest();

    let mut other_goal = goal.clone();
    other_goal.objective = "Something else entirely.".to_string();
    assert_ne!(base, input(&other_goal).digest(), "goal content");

    let mut probe = input(&goal);
    probe.goal_revision = 2;
    assert_ne!(base, probe.digest(), "goal revision");

    let mut probe = input(&goal);
    probe.expected_graph_revision = 1;
    assert_ne!(base, probe.digest(), "graph precondition");

    let mut probe = input(&goal);
    probe.configuration_activation_sequence = 2;
    assert_ne!(base, probe.digest(), "configuration");
}

#[test]
fn a_valid_proposal_pins_the_exact_evaluator_version() {
    let goal = goal();
    let registry = registry();
    let proposal = direct::plan(&input(&goal));

    let materializable = validate::validate(&proposal, &authority(&goal, &registry))
        .expect("the proposal validates");
    let spec = materializable.spec();

    assert_eq!(spec.acceptance.criteria.len(), 1);
    let criterion = &spec.acceptance.criteria[0];
    assert_eq!(criterion.evaluator_ref, MVP_EVALUATOR_REF);
    assert_eq!(criterion.evaluator_version, "unit-tests-v1");
    assert_eq!(criterion.severity, Severity::Required);
    // The resolution provenance travels with the pin, so a later registry
    // change is distinguishable from the one this Task was built on.
    assert_eq!(
        spec.acceptance.evaluator_registry_digest,
        Digest::of(b"registry")
    );
    assert_eq!(spec.acceptance.configuration_activation_sequence, 1);
    assert_eq!(spec.goal_revision, 1);
}

#[test]
fn a_later_evaluator_resolution_produces_a_different_task_identity() {
    // The pin is what stops a registry change altering an existing Task: the
    // same proposal under a moved registry is a *different* spec, so an
    // already-materialized Task cannot silently become it.
    let goal = goal();
    let proposal = direct::plan(&input(&goal));

    let before = validate::validate(&proposal, &authority(&goal, &registry()))
        .expect("validates")
        .spec()
        .digest();

    let moved = Registry {
        version: Some("unit-tests-v2".to_string()),
    };
    let after = validate::validate(&proposal, &authority(&goal, &moved))
        .expect("validates")
        .spec()
        .digest();

    assert_ne!(
        before, after,
        "a different pinned version is a different Task"
    );
}

#[test]
fn a_proposal_naming_an_unresolvable_evaluator_is_rejected() {
    let goal = goal();
    let empty = Registry { version: None };
    let proposal = direct::plan(&input(&goal));

    let err = validate::validate(&proposal, &authority(&goal, &empty))
        .expect_err("an unresolvable evaluator must be rejected");
    assert_eq!(err.kind(), "unknown-evaluator", "unexpected: {err}");
}

#[test]
fn a_proposal_with_more_than_one_task_is_rejected() {
    let goal = goal();
    let registry = registry();
    let mut proposal = direct::plan(&input(&goal));
    let duplicate = proposal.tasks[0].clone();
    proposal.tasks.push(duplicate);

    let err = validate::validate(&proposal, &authority(&goal, &registry))
        .expect_err("DIRECT proposes exactly one task");
    assert_eq!(err.kind(), "shape", "unexpected: {err}");
}

#[test]
fn a_proposal_carrying_a_dependency_edge_is_rejected() {
    let goal = goal();
    let registry = registry();
    let mut proposal = direct::plan(&input(&goal));
    proposal.edges.push((0, 0));

    let err = validate::validate(&proposal, &authority(&goal, &registry))
        .expect_err("DIRECT proposes no dependencies");
    assert_eq!(err.kind(), "shape", "unexpected: {err}");
}

#[test]
fn a_proposal_that_widens_the_goal_authority_is_rejected() {
    // A Task may tighten the Goal ceiling, never broaden it. Both routes are
    // exercised separately: an effect that is *both* unlisted and explicitly
    // forbidden lets either rule alone pass the test while the other rots.
    let goal = goal();
    let registry = registry();

    // Not in the Goal's permitted set and not explicitly forbidden either, so
    // only the "must be permitted" rule can catch it.
    let mut unlisted = direct::plan(&input(&goal));
    unlisted.tasks[0]
        .permitted_effects
        .push("network.connect".to_string());
    let err = validate::validate(&unlisted, &authority(&goal, &registry))
        .expect_err("an effect the Goal never permitted must be rejected");
    assert_eq!(err.kind(), "scope-escalation", "unexpected: {err}");

    // Explicitly forbidden while also listed as permitted, so only the
    // "must not be forbidden" rule can catch it.
    let mut contradictory = goal.clone();
    contradictory
        .constraints
        .permitted_effects
        .push("git.push".to_string());
    let mut forbidden = direct::plan(&input(&contradictory));
    forbidden.tasks[0]
        .permitted_effects
        .push("git.push".to_string());
    let err = validate::validate(&forbidden, &authority(&contradictory, &registry))
        .expect_err("an effect the Goal forbids must be rejected even when also listed");
    assert_eq!(err.kind(), "scope-escalation", "unexpected: {err}");
}

#[test]
fn a_proposal_reaching_outside_the_goal_resource_scope_is_rejected() {
    let goal = goal();
    let registry = registry();
    let mut proposal = direct::plan(&input(&goal));
    proposal.tasks[0]
        .resources
        .push("workspace://secrets/**".to_string());

    let err = validate::validate(&proposal, &authority(&goal, &registry))
        .expect_err("reaching outside the Goal scope must be rejected");
    assert_eq!(err.kind(), "scope-escalation", "unexpected: {err}");
}

#[test]
fn a_proposal_that_cannot_produce_a_required_deliverable_is_rejected() {
    // The one compatibility check with real force at single-task scale.
    let goal = goal();
    let registry = registry();
    let mut proposal = direct::plan(&input(&goal));
    proposal.tasks[0].outputs.clear();

    let err = validate::validate(&proposal, &authority(&goal, &registry))
        .expect_err("an uncoverable deliverable must be rejected");
    assert_eq!(err.kind(), "deliverable-not-covered", "unexpected: {err}");
}

#[test]
fn a_proposal_smuggling_execution_identity_into_the_task_is_rejected() {
    // The Task contract forbids Agent, backend, provider, credential, Run,
    // Attempt and Sandbox identity in the immutable specification. A proposal
    // can only express those as references, so that is where they are caught.
    let goal = goal();
    let registry = registry();
    for smuggled in [
        "agent://builder",
        "backend://local",
        "model://some-model",
        "runtime://docker",
        "secret://token",
        "run://r1",
        "attempt://a1",
        "sandbox://s1",
    ] {
        let mut proposal = direct::plan(&input(&goal));
        proposal.tasks[0]
            .inputs
            .push(("smuggled".to_string(), smuggled.to_string()));
        let err = validate::validate(&proposal, &authority(&goal, &registry)).unwrap_err();
        assert_eq!(
            err.kind(),
            "forbidden-task-content",
            "{smuggled} must be refused: {err}"
        );
    }
}

#[test]
fn an_empty_proposal_is_rejected_rather_than_treated_as_nothing_to_do() {
    let goal = goal();
    let registry = registry();
    let proposal = Proposal {
        tasks: Vec::new(),
        edges: Vec::new(),
    };
    let err = validate::validate(&proposal, &authority(&goal, &registry))
        .expect_err("an empty proposal is not a valid DIRECT plan");
    assert_eq!(err.kind(), "shape", "unexpected: {err}");
}

#[test]
fn the_task_spec_digest_covers_the_pinned_acceptance_contract() {
    // The digest is taken post-pinning, so it identifies the contract that
    // will actually be evaluated rather than the unresolved proposal.
    let goal = goal();
    let registry = registry();
    let proposal = direct::plan(&input(&goal));
    let spec = validate::validate(&proposal, &authority(&goal, &registry))
        .expect("validates")
        .spec()
        .clone();

    let mut moved = spec.clone();
    moved.acceptance.criteria[0].evaluator_version = "unit-tests-v2".to_string();
    assert_ne!(spec.digest(), moved.digest());
    assert_ne!(spec.acceptance_digest(), moved.acceptance_digest());
}
