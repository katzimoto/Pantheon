//! The Goal and its immutable revision content.

use crate::config::Digest;
use crate::config::canonical::Value;
use crate::config::parse;

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

    /// Reads a Goal revision back from its stored canonical form.
    ///
    /// Validation must run against the Goal Pantheon durably holds, not a
    /// copy a caller supplies alongside it. Without this the scope ceiling
    /// and deliverable coverage would be checked against whatever the caller
    /// said the Goal was, which is exactly the authority the mission exists
    /// to keep out of the caller's hands.
    ///
    /// # Errors
    ///
    /// [`GoalDecodeError`] when the stored text is not a Goal revision. This
    /// fails closed rather than degrading to an empty Goal, because an empty
    /// constraint set would be a *wider* ceiling, not a narrower one.
    pub fn from_canonical_json(text: &str) -> Result<Self, GoalDecodeError> {
        let value = parse::parse(text).map_err(|err| GoalDecodeError(err.to_string()))?;
        let field = |name: &str| {
            value
                .get(name)
                .ok_or_else(|| GoalDecodeError(format!("missing {name}")))
        };

        let Value::String(objective) = field("objective")? else {
            return Err(GoalDecodeError("objective is not a string".to_string()));
        };

        let mut inputs = Vec::new();
        if let Value::Array(entries) = field("inputs")? {
            for entry in entries {
                let (Some(Value::String(name)), Some(Value::String(reference))) =
                    (entry.get("name"), entry.get("ref"))
                else {
                    return Err(GoalDecodeError("malformed input".to_string()));
                };
                inputs.push(GoalInput {
                    name: name.clone(),
                    reference: reference.clone(),
                });
            }
        }

        let mut deliverables = Vec::new();
        if let Value::Array(entries) = field("deliverables")? {
            for entry in entries {
                let (
                    Some(Value::String(name)),
                    Some(Value::String(kind)),
                    Some(Value::Bool(required)),
                ) = (entry.get("name"), entry.get("kind"), entry.get("required"))
                else {
                    return Err(GoalDecodeError("malformed deliverable".to_string()));
                };
                deliverables.push(Deliverable {
                    name: name.clone(),
                    kind: kind.clone(),
                    required: *required,
                });
            }
        }

        let constraints_value = field("constraints")?;
        let list = |name: &str| -> Result<Vec<String>, GoalDecodeError> {
            let Some(Value::Array(entries)) = constraints_value.get(name) else {
                return Err(GoalDecodeError(format!("missing constraints.{name}")));
            };
            entries
                .iter()
                .map(|entry| match entry {
                    Value::String(text) => Ok(text.clone()),
                    _ => Err(GoalDecodeError(format!(
                        "constraints.{name} is not strings"
                    ))),
                })
                .collect()
        };

        Ok(Self {
            objective: objective.clone(),
            inputs,
            deliverables,
            constraints: GoalConstraints {
                permitted_effects: list("permittedEffects")?,
                forbidden_effects: list("forbiddenEffects")?,
                permitted_resources: list("permittedResources")?,
            },
        })
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

/// A stored Goal revision that cannot be read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalDecodeError(pub String);

impl std::fmt::Display for GoalDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stored goal revision is not readable: {}", self.0)
    }
}

impl std::error::Error for GoalDecodeError {}
