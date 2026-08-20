//! The Goal and its immutable revision content.

use crate::config::Digest;
use crate::config::canonical::Value;

/// The canonical Goal lifecycle phases.
///
/// `docs/architecture/goals-and-planning/goal-resource.md` and the Goal
/// lifecycle contract define these; this mission exercises only
/// `Planning -> Active`, but the representation is the real one so later
/// missions extend rather than replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalPhase {
    /// The Goal exists but has no coherent TaskGraph yet. Initial planning
    /// happens here.
    Planning,
    /// Pantheon is pursuing the current Goal revision. Reached once a valid
    /// TaskGraph exists — and never left for `Planning` again, because
    /// replanning keeps the Goal Active with reconciliation conditions.
    Active,
    Evaluating,
    Finalizing,
    Succeeded,
    Failed,
    Cancelled,
}

impl GoalPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "Planning",
            Self::Active => "Active",
            Self::Evaluating => "Evaluating",
            Self::Finalizing => "Finalizing",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Whether this phase is nonterminal.
    #[must_use]
    pub const fn is_nonterminal(self) -> bool {
        matches!(
            self,
            Self::Planning | Self::Active | Self::Evaluating | Self::Finalizing
        )
    }

    /// Parses a stored phase.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "Planning" => Self::Planning,
            "Active" => Self::Active,
            "Evaluating" => Self::Evaluating,
            "Finalizing" => Self::Finalizing,
            "Succeeded" => Self::Succeeded,
            "Failed" => Self::Failed,
            "Cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

/// A durable reference the Goal supplies as input, for example a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalInput {
    pub name: String,
    /// An opaque URI-shaped resource reference. Core does not interpret it.
    pub reference: String,
}

/// A named top-level output slot representing a user-visible final result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deliverable {
    pub name: String,
    /// The output kind that may satisfy this slot.
    pub kind: String,
    pub required: bool,
}

/// The Goal's mandatory ceilings.
///
/// Modelled as a structured effect ceiling rather than prose on purpose: the
/// contract says descendants "may tighten but cannot turn Goal constraints
/// into new authority", and that check is only meaningful if the ceiling is
/// comparable. Free-text constraints would make the validation theatre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalConstraints {
    /// The maximum effect authority any descendant Task may request.
    pub permitted_effects: Vec<String>,
    /// Effects no descendant may request, whatever else it permits.
    pub forbidden_effects: Vec<String>,
    /// The resource scope descendants may not exceed.
    pub permitted_resources: Vec<String>,
}

/// The immutable content of one Goal revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSpec {
    /// The human-level desired outcome, without prescribing implementation.
    pub objective: String,
    pub inputs: Vec<GoalInput>,
    pub deliverables: Vec<Deliverable>,
    pub constraints: GoalConstraints,
}

impl GoalSpec {
    /// The canonical value this revision's identity is taken over.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            ("objective", Value::string(&self.objective)),
            (
                "inputs",
                Value::array(self.inputs.iter().map(|input| {
                    Value::object([
                        ("name", Value::string(&input.name)),
                        ("ref", Value::string(&input.reference)),
                    ])
                })),
            ),
            (
                "deliverables",
                Value::array(self.deliverables.iter().map(|deliverable| {
                    Value::object([
                        ("name", Value::string(&deliverable.name)),
                        ("kind", Value::string(&deliverable.kind)),
                        ("required", Value::Bool(deliverable.required)),
                    ])
                })),
            ),
            (
                "constraints",
                Value::object([
                    (
                        "permittedEffects",
                        strings(&self.constraints.permitted_effects),
                    ),
                    (
                        "forbiddenEffects",
                        strings(&self.constraints.forbidden_effects),
                    ),
                    (
                        "permittedResources",
                        strings(&self.constraints.permitted_resources),
                    ),
                ]),
            ),
        ])
    }

    /// The content identity of this revision.
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }

    /// The required deliverable kinds a materialized graph must be able to
    /// produce.
    #[must_use]
    pub fn required_deliverable_kinds(&self) -> Vec<&str> {
        self.deliverables
            .iter()
            .filter(|deliverable| deliverable.required)
            .map(|deliverable| deliverable.kind.as_str())
            .collect()
    }
}

fn strings(values: &[String]) -> Value {
    Value::array(values.iter().map(Value::string))
}
