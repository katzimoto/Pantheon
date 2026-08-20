//! The deterministic DIRECT planner.
//!
//! `docs/architecture/goals-and-planning/planner-and-task-decomposition.md`
//! defines DIRECT as "Goal is already one bounded Task; Planner proposes one
//! Task/minimal graph", and states that "A purely local/deterministic DIRECT
//! implementation still creates a durable `PlanningOperation`/`PlanningRecord`
//! boundary for revision fencing and audit, but it does not invent a
//! `PlanningAttempt` when no external execution/contact exists."
//!
//! DIRECT and "local/deterministic" are orthogonal axes: nothing forbids a
//! future model-backed DIRECT planner, which *would* need attempt lineage. So
//! the absence of attempt state here follows from this planner having no
//! external backend, not from the mode being DIRECT. When an external planner
//! arrives, its backend fields and its attempt lineage arrive together.
//!
//! This planner is a pure function of its input. Two runs over the same Goal
//! revision produce byte-identical proposals, which is what makes the
//! planning input digest meaningful as reproduction provenance.

use crate::config::Digest;
use crate::config::canonical::Value;
use crate::planning::goal::GoalSpec;
use crate::planning::proposal::{Proposal, ProposedCriterion, ProposedTask};

/// Identifies this planner in durable provenance.
///
/// The architecture names only a "Planner Agent snapshot/version", which is
/// the external case. A local deterministic planner still needs to be
/// identifiable to reproduce a decision, so it carries its own implementation
/// identity.
pub const PLANNER_IMPLEMENTATION: &str = "pantheon-direct-planner";

/// The version of this planner's decision procedure.
///
/// Participates in the planning input digest, so changing how DIRECT planning
/// derives a task produces different provenance rather than silently
/// reinterpreting an old decision.
pub const PLANNER_VERSION: &str = "v1";

/// Why planning was invoked.
///
/// The architecture enumerates only *re*planning triggers; a Goal that has
/// never been planned needs one too, so `Initial` is named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The Goal has no coherent TaskGraph yet.
    Initial,
}

impl Trigger {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
        }
    }
}

/// The exact input a DIRECT planning decision was derived from.
///
/// The contract requires the operation's input identity to be "sufficient to
/// determine which Goal/Graph/config/input snapshot the resulting
/// PlanningRecord was derived from", so all four are here and all four are
/// digested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanningInput<'a> {
    pub goal_id: &'a str,
    pub goal_revision: i64,
    pub goal: &'a GoalSpec,
    pub expected_graph_revision: i64,
    pub configuration_activation_sequence: i64,
    pub trigger: Trigger,
}

impl PlanningInput<'_> {
    /// The immutable planning input identity.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Value::object([
            ("goalId", Value::string(self.goal_id)),
            ("goalRevision", Value::Integer(self.goal_revision)),
            ("goalSpec", self.goal.to_value()),
            (
                "expectedGraphRevision",
                Value::Integer(self.expected_graph_revision),
            ),
            (
                "configurationActivationSequence",
                Value::Integer(self.configuration_activation_sequence),
            ),
            ("trigger", Value::string(self.trigger.as_str())),
            (
                "plannerImplementation",
                Value::string(PLANNER_IMPLEMENTATION),
            ),
            ("plannerVersion", Value::string(PLANNER_VERSION)),
        ])
        .digest()
    }
}

/// The evaluator reference the MVP deterministic acceptance contract uses.
///
/// Named here rather than derived from the Goal because the MVP Goal carries
/// no explicit evaluator criteria: the Goal contract permits zero, and the
/// deterministic Task-level check is what this mission pins.
pub const MVP_EVALUATOR_REF: &str = "check://project/unit-tests";

/// Plans one bounded Goal revision into exactly one Task.
///
/// Deterministic: the proposal is a pure function of `input`, with no clock,
/// no randomness and no map iteration order involved.
#[must_use]
pub fn plan(input: &PlanningInput<'_>) -> Proposal {
    let goal = input.goal;

    // The Task inherits the Goal's inputs, since a DIRECT Goal *is* the
    // bounded unit of work.
    let inputs = goal
        .inputs
        .iter()
        .map(|goal_input| (goal_input.name.clone(), goal_input.reference.clone()))
        .collect();

    // Every required Goal deliverable becomes a required Task output, which
    // is what makes the deliverable-coverage validation meaningful rather
    // than tautological — the planner could get this wrong, and validation
    // would catch it.
    let outputs = goal
        .deliverables
        .iter()
        .map(|deliverable| {
            (
                deliverable.name.clone(),
                deliverable.kind.clone(),
                deliverable.required,
            )
        })
        .collect();

    // The Task narrows to the Goal ceiling rather than restating it as new
    // authority: a Task may tighten, never broaden.
    let proposed = ProposedTask {
        task_type: "code.change".to_string(),
        objective: goal.objective.clone(),
        inputs,
        outputs,
        competencies: vec![
            "code.analysis".to_string(),
            "code.editing".to_string(),
            "test.execution".to_string(),
        ],
        resources: goal.constraints.permitted_resources.clone(),
        permitted_effects: goal.constraints.permitted_effects.clone(),
        forbidden_effects: goal.constraints.forbidden_effects.clone(),
        criteria: vec![ProposedCriterion {
            id: "acceptance-checks-pass".to_string(),
            statement: "The configured deterministic acceptance checks pass.".to_string(),
            evaluator_ref: MVP_EVALUATOR_REF.to_string(),
            required: true,
        }],
    };

    Proposal {
        tasks: vec![proposed],
        // DIRECT proposes no dependencies. An artificial edge to exercise
        // graph machinery is exactly what the mission forbids.
        edges: Vec::new(),
    }
}
