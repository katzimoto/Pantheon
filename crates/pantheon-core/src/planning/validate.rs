//! Validating a proposal against authoritative state.
//!
//! This is the Planner-authority boundary in code. A [`Proposal`] is
//! evidence; a [`Materializable`] is the only thing the store will write, and
//! it can be produced solely by [`validate`]. The caller supplies the
//! authoritative facts, which the store reads *inside* the write transaction,
//! so a proposal cannot be materialized against state that has since moved.
//!
//! The contract this enforces:
//!
//! > Graph Controller separately rechecks GoalRevision/GraphRevision/current
//! > policy before GraphPatch commit.
//!
//! Note what is deliberately *not* here. There is no cycle-detection
//! algorithm and no decomposition budget: with exactly one task and zero
//! edges there is nothing to detect, and inventing a general DAG walk to have
//! something to run would be theatre. What replaces them is a shape
//! rejection — a DIRECT proposal that is not one task with no edges is
//! refused outright.

use crate::config::Digest;
use crate::planning::goal::GoalSpec;
use crate::planning::proposal::Proposal;
use crate::planning::task::{
    AcceptanceContract, AcceptanceCriterion, Severity, TaskInput, TaskOutput, TaskScope, TaskSpec,
};

/// Why a proposal cannot become authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The proposal is not the shape DIRECT planning produces.
    Shape { detail: String },
    /// A field is absent or unacceptable.
    InvalidValue { path: String, detail: String },
    /// The proposal names an evaluator the active configuration does not
    /// resolve.
    UnknownEvaluator { reference: String },
    /// The proposal would grant authority the Goal does not permit.
    ScopeEscalation { detail: String },
    /// The proposal cannot produce a required Goal deliverable.
    DeliverableNotCovered { name: String, kind: String },
    /// The proposal carries something a Task specification may never contain.
    ForbiddenTaskContent { detail: String },
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape { detail } => write!(f, "proposal shape is invalid: {detail}"),
            Self::InvalidValue { path, detail } => {
                write!(f, "invalid value at {path}: {detail}")
            }
            Self::UnknownEvaluator { reference } => write!(
                f,
                "proposal names evaluator {reference:?}, which the active configuration does not resolve"
            ),
            Self::ScopeEscalation { detail } => {
                write!(f, "proposal would exceed the Goal's authority: {detail}")
            }
            Self::DeliverableNotCovered { name, kind } => write!(
                f,
                "no proposed output can satisfy required deliverable {name:?} of kind {kind:?}"
            ),
            Self::ForbiddenTaskContent { detail } => {
                write!(f, "a Task specification may not contain {detail}")
            }
        }
    }
}

impl std::error::Error for PlanError {}

impl PlanError {
    /// A short stable label for diagnostics and tests.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Shape { .. } => "shape",
            Self::InvalidValue { .. } => "invalid-value",
            Self::UnknownEvaluator { .. } => "unknown-evaluator",
            Self::ScopeEscalation { .. } => "scope-escalation",
            Self::DeliverableNotCovered { .. } => "deliverable-not-covered",
            Self::ForbiddenTaskContent { .. } => "forbidden-task-content",
        }
    }
}

/// The authoritative facts a proposal is validated against.
///
/// Every field is read by the caller from current state; validation never
/// reads anything itself, which is what keeps it pure and keeps the
/// authoritative read inside the caller's transaction.
#[derive(Debug, Clone, Copy)]
pub struct Authority<'a> {
    /// The Goal as Pantheon durably holds it — never a caller's copy.
    pub goal: &'a GoalSpec,
    pub goal_id: &'a str,
    pub goal_revision: i64,
    /// Resolves a logical evaluator ref to its immutable version id under the
    /// active configuration.
    pub evaluators: &'a dyn EvaluatorResolver,
    pub evaluator_registry_digest: Digest,
    pub configuration_activation_sequence: i64,
}

/// Resolves logical evaluator references against the active configuration.
///
/// A trait rather than a concrete registry so `pantheon-core` stays free of
/// the store: the caller resolves from whatever the active
/// ConfigurationRevision says, and validation only asks whether a ref
/// resolves.
pub trait EvaluatorResolver {
    /// The immutable version id `reference` currently resolves to.
    fn resolve(&self, reference: &str) -> Option<String>;
}

impl std::fmt::Debug for dyn EvaluatorResolver + '_ {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EvaluatorResolver")
    }
}

/// A validated proposal, and the only input the store will materialize.
///
/// Constructible only by [`validate`]. Holding one is the evidence that the
/// proposal was checked against the authoritative state named inside it — and
/// the store re-checks that `goal_revision` and the graph revision are still
/// current before it commits, so even this is not a licence to write against
/// moved state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Materializable {
    spec: TaskSpec,
    proposal_digest: Digest,
    goal_revision: i64,
    /// The content identity of the Goal revision this was validated against.
    ///
    /// The store compares this with the stored `goal_revisions.content_digest`
    /// before writing, so a plan validated against a *different* Goal than the
    /// one durably recorded cannot materialize — fencing the revision number
    /// alone would not catch that.
    goal_digest: Digest,
}

impl Materializable {
    /// The immutable Task specification to materialize.
    #[must_use]
    pub const fn spec(&self) -> &TaskSpec {
        &self.spec
    }

    /// The proposal this was validated from.
    #[must_use]
    pub const fn proposal_digest(&self) -> Digest {
        self.proposal_digest
    }

    /// The Goal revision the proposal was validated against. The store
    /// re-checks this is still current before committing.
    #[must_use]
    pub const fn goal_revision(&self) -> i64 {
        self.goal_revision
    }

    /// The content identity of the Goal this was validated against.
    #[must_use]
    pub const fn goal_digest(&self) -> Digest {
        self.goal_digest
    }
}

/// Validates a DIRECT proposal against current authoritative state.
///
/// # Errors
///
/// [`PlanError`] when the proposal is not the DIRECT shape, carries content a
/// Task may never contain, escalates beyond the Goal's authority, names an
/// unresolvable evaluator, or cannot produce a required deliverable.
pub fn validate(
    proposal: &Proposal,
    authority: &Authority<'_>,
) -> Result<Materializable, PlanError> {
    // Shape first. This is what replaces cycle detection and decomposition
    // budgets at one-task scale: anything that is not one task with no edges
    // is refused rather than partially understood.
    if proposal.tasks.len() != 1 {
        return Err(PlanError::Shape {
            detail: format!(
                "DIRECT planning proposes exactly one task, found {}",
                proposal.tasks.len()
            ),
        });
    }
    if !proposal.edges.is_empty() {
        return Err(PlanError::Shape {
            detail: format!(
                "DIRECT planning proposes no dependencies, found {} edge(s)",
                proposal.edges.len()
            ),
        });
    }

    let task = &proposal.tasks[0];

    if task.task_type.trim().is_empty() {
        return Err(PlanError::InvalidValue {
            path: "task.type".to_string(),
            detail: "must not be empty".to_string(),
        });
    }
    if task.objective.trim().is_empty() {
        return Err(PlanError::InvalidValue {
            path: "task.objective".to_string(),
            detail: "must not be empty".to_string(),
        });
    }
    if task.competencies.is_empty() {
        return Err(PlanError::InvalidValue {
            path: "task.competencies".to_string(),
            detail: "a Task must state the semantic abilities it needs".to_string(),
        });
    }
    if task.criteria.is_empty() {
        return Err(PlanError::InvalidValue {
            path: "task.criteria".to_string(),
            detail: "a Task must carry an acceptance contract".to_string(),
        });
    }

    // The negative list from the Task contract. These are the shapes a
    // proposal could use to smuggle execution identity into an immutable
    // spec, and each is refused by name so the rejection is legible.
    reject_forbidden_content(task)?;

    // Scope ceiling: a Task may tighten Goal authority, never broaden it.
    for effect in &task.permitted_effects {
        if !authority
            .goal
            .constraints
            .permitted_effects
            .contains(effect)
        {
            return Err(PlanError::ScopeEscalation {
                detail: format!("effect {effect:?} is not permitted by the Goal"),
            });
        }
        if authority
            .goal
            .constraints
            .forbidden_effects
            .contains(effect)
        {
            return Err(PlanError::ScopeEscalation {
                detail: format!("effect {effect:?} is forbidden by the Goal"),
            });
        }
    }
    for resource in &task.resources {
        if !authority
            .goal
            .constraints
            .permitted_resources
            .contains(resource)
        {
            return Err(PlanError::ScopeEscalation {
                detail: format!("resource {resource:?} is outside the Goal's scope"),
            });
        }
    }

    // Deliverable coverage: every required Goal deliverable must have a
    // proposed output that can satisfy it. Vacuous checks were avoided
    // elsewhere; this one has real force even with a single task.
    for deliverable in &authority.goal.deliverables {
        if !deliverable.required {
            continue;
        }
        let covered = task
            .outputs
            .iter()
            .any(|(_, kind, required)| *required && kind == &deliverable.kind);
        if !covered {
            return Err(PlanError::DeliverableNotCovered {
                name: deliverable.name.clone(),
                kind: deliverable.kind.clone(),
            });
        }
    }

    // Evaluator resolution and pinning. Resolution happens here, once,
    // against the configuration the caller read; the resulting version is
    // frozen into the spec so a later registry change cannot reach it.
    let mut criteria = Vec::with_capacity(task.criteria.len());
    for criterion in &task.criteria {
        let Some(version) = authority.evaluators.resolve(&criterion.evaluator_ref) else {
            return Err(PlanError::UnknownEvaluator {
                reference: criterion.evaluator_ref.clone(),
            });
        };
        criteria.push(AcceptanceCriterion {
            id: criterion.id.clone(),
            statement: criterion.statement.clone(),
            evaluator_ref: criterion.evaluator_ref.clone(),
            evaluator_version: version,
            severity: if criterion.required {
                Severity::Required
            } else {
                Severity::Advisory
            },
        });
    }

    let spec = TaskSpec {
        task_type: task.task_type.clone(),
        objective: task.objective.clone(),
        inputs: task
            .inputs
            .iter()
            .map(|(name, reference)| TaskInput {
                name: name.clone(),
                reference: reference.clone(),
            })
            .collect(),
        outputs: task
            .outputs
            .iter()
            .map(|(name, kind, required)| TaskOutput {
                name: name.clone(),
                kind: kind.clone(),
                required: *required,
            })
            .collect(),
        competencies: task.competencies.clone(),
        scope: TaskScope {
            resources: task.resources.clone(),
            permitted_effects: task.permitted_effects.clone(),
            forbidden_effects: task.forbidden_effects.clone(),
        },
        goal_id: authority.goal_id.to_string(),
        acceptance: AcceptanceContract {
            criteria,
            evaluator_registry_digest: authority.evaluator_registry_digest,
            configuration_activation_sequence: authority.configuration_activation_sequence,
        },
        goal_revision: authority.goal_revision,
    };

    Ok(Materializable {
        spec,
        proposal_digest: proposal.digest(),
        goal_revision: authority.goal_revision,
        goal_digest: authority.goal.digest(),
    })
}

/// Refuses the content a Task specification may never contain.
///
/// The Task contract lists these by name. A proposal can only express them as
/// resource or effect strings, so that is where they are caught.
fn reject_forbidden_content(
    task: &crate::planning::proposal::ProposedTask,
) -> Result<(), PlanError> {
    const FORBIDDEN_PREFIXES: &[(&str, &str)] = &[
        ("agent://", "a Logical Agent assignment"),
        ("backend://", "a backend assignment"),
        ("model://", "a provider or model assignment"),
        ("runtime://", "a runtime assignment"),
        ("secret://", "credential material"),
        ("run://", "a Run reference"),
        ("attempt://", "an Attempt reference"),
        ("sandbox://", "a Sandbox reference"),
    ];

    for reference in task.inputs.iter().map(|(_, reference)| reference) {
        for (prefix, description) in FORBIDDEN_PREFIXES {
            if reference.starts_with(prefix) {
                return Err(PlanError::ForbiddenTaskContent {
                    detail: format!("{description} ({reference})"),
                });
            }
        }
    }
    for resource in &task.resources {
        for (prefix, description) in FORBIDDEN_PREFIXES {
            if resource.starts_with(prefix) {
                return Err(PlanError::ForbiddenTaskContent {
                    detail: format!("{description} ({resource})"),
                });
            }
        }
    }
    Ok(())
}
