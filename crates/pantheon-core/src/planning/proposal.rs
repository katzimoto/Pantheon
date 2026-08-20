//! The normalized planning proposal.
//!
//! A [`Proposal`] is what a planner produced. It is evidence and provenance,
//! never authority: the persistence contract says a PlanningRecord "is not
//! lifecycle authority and is never proof that its proposal was
//! materialized". Nothing here can reach the database — turning a proposal
//! into something materializable requires
//! [`crate::planning::validate::validate`], which re-checks it against
//! authoritative state the caller reads inside the write transaction.

use crate::config::Digest;
use crate::config::canonical::Value;

/// A task a planner proposes, before validation and before evaluator pinning.
///
/// Deliberately *not* a [`crate::planning::TaskSpec`]: a proposal names
/// evaluator refs logically and has no resolved versions, so the two cannot
/// be confused and an unpinned proposal cannot be persisted as a spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedTask {
    pub task_type: String,
    pub objective: String,
    pub inputs: Vec<(String, String)>,
    pub outputs: Vec<(String, String, bool)>,
    pub competencies: Vec<String>,
    pub resources: Vec<String>,
    pub permitted_effects: Vec<String>,
    pub forbidden_effects: Vec<String>,
    /// Criteria naming evaluators by *logical ref only*.
    pub criteria: Vec<ProposedCriterion>,
}

/// An acceptance criterion as proposed, with an unresolved evaluator ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedCriterion {
    pub id: String,
    pub statement: String,
    pub evaluator_ref: String,
    pub required: bool,
}

/// A normalized planning proposal.
///
/// For DIRECT planning this carries exactly one task and no edges. The edge
/// list exists so that a proposal which *does* carry one can be rejected
/// rather than silently ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub tasks: Vec<ProposedTask>,
    /// Proposed dependency edges as `(upstream index, downstream index)`.
    /// Always empty for a valid DIRECT proposal.
    pub edges: Vec<(usize, usize)>,
}

impl Proposal {
    /// The canonical value this proposal's identity is taken over.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            (
                "tasks",
                Value::array(self.tasks.iter().map(|task| {
                    Value::object([
                        ("type", Value::string(&task.task_type)),
                        ("objective", Value::string(&task.objective)),
                        (
                            "inputs",
                            Value::array(task.inputs.iter().map(|(name, reference)| {
                                Value::object([
                                    ("name", Value::string(name)),
                                    ("ref", Value::string(reference)),
                                ])
                            })),
                        ),
                        (
                            "outputs",
                            Value::array(task.outputs.iter().map(|(name, kind, required)| {
                                Value::object([
                                    ("name", Value::string(name)),
                                    ("kind", Value::string(kind)),
                                    ("required", Value::Bool(*required)),
                                ])
                            })),
                        ),
                        ("competencies", strings(&task.competencies)),
                        (
                            "scope",
                            Value::object([
                                ("resources", strings(&task.resources)),
                                ("permittedEffects", strings(&task.permitted_effects)),
                                ("forbiddenEffects", strings(&task.forbidden_effects)),
                            ]),
                        ),
                        (
                            "criteria",
                            Value::array(task.criteria.iter().map(|criterion| {
                                Value::object([
                                    ("id", Value::string(&criterion.id)),
                                    ("statement", Value::string(&criterion.statement)),
                                    ("evaluatorRef", Value::string(&criterion.evaluator_ref)),
                                    ("required", Value::Bool(criterion.required)),
                                ])
                            })),
                        ),
                    ])
                })),
            ),
            (
                "edges",
                Value::array(self.edges.iter().map(|(upstream, downstream)| {
                    Value::object([
                        (
                            "upstream",
                            Value::Integer(i64::try_from(*upstream).unwrap_or(i64::MAX)),
                        ),
                        (
                            "downstream",
                            Value::Integer(i64::try_from(*downstream).unwrap_or(i64::MAX)),
                        ),
                    ])
                })),
            ),
        ])
    }

    /// The immutable proposal identity recorded on the PlanningRecord.
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }
}

fn strings(values: &[String]) -> Value {
    Value::array(values.iter().map(Value::string))
}
