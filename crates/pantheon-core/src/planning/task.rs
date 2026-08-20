//! The immutable Task specification and the canonical Task lifecycle.

use crate::config::Digest;
use crate::config::canonical::Value;
use crate::config::parse;

/// The canonical v1 Task phases.
///
/// `docs/architecture/tasks/task-lifecycle.md` defines all ten. This mission
/// only reaches `Ready`, but the representation is the real one: the Issue
/// forbids an MVP-only lifecycle that a later mission must replace, and the
/// persisted phase domain has to accept `Active`, `Evaluating`, `Finalizing`
/// and the terminal phases without a migration that rewrites history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhase {
    /// Logical prerequisites are not yet satisfied — dependencies or graph
    /// activation gates. Resource or backend scarcity never means `Pending`.
    Pending,
    /// Logically eligible for scheduling, owning no nonterminal Run.
    Ready,
    Active,
    Waiting,
    Evaluating,
    Finalizing,
    Succeeded,
    Failed,
    Cancelled,
    Superseded,
}

impl TaskPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Ready => "Ready",
            Self::Active => "Active",
            Self::Waiting => "Waiting",
            Self::Evaluating => "Evaluating",
            Self::Finalizing => "Finalizing",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Superseded => "Superseded",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Superseded
        )
    }

    /// Whether this phase requires zero nonterminal Runs.
    ///
    /// Lifecycle invariant: `Task Ready|Waiting => zero nonterminal Runs`.
    #[must_use]
    pub const fn requires_no_run(self) -> bool {
        matches!(self, Self::Ready | Self::Waiting)
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "Pending" => Self::Pending,
            "Ready" => Self::Ready,
            "Active" => Self::Active,
            "Waiting" => Self::Waiting,
            "Evaluating" => Self::Evaluating,
            "Finalizing" => Self::Finalizing,
            "Succeeded" => Self::Succeeded,
            "Failed" => Self::Failed,
            "Cancelled" => Self::Cancelled,
            "Superseded" => Self::Superseded,
            _ => return None,
        })
    }
}

/// A typed input slot the Task declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInput {
    pub name: String,
    /// An opaque resource reference. Binding a slot to a concrete upstream
    /// output is graph state, not spec content.
    pub reference: String,
}

/// A typed output slot the Task must produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutput {
    pub name: String,
    pub kind: String,
    pub required: bool,
}

/// The Task's least-privilege ceiling.
///
/// A Task may narrow enclosing Goal authority but never broaden it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskScope {
    pub resources: Vec<String>,
    pub permitted_effects: Vec<String>,
    pub forbidden_effects: Vec<String>,
}

/// Whether a criterion must pass for the Task to be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Required,
    Advisory,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Advisory => "advisory",
        }
    }
}

/// One acceptance criterion with its evaluator pinned to an exact version.
///
/// The pin is a *coordinate*, not a command: the logical ref for readability,
/// the resolved immutable version id, and the evaluator-registry digest that
/// produced the resolution. `configuration_components` is content-addressed,
/// so that coordinate recovers the exact evaluator definition permanently —
/// while the Task itself embeds no executable command, which the Task
/// contract forbids outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub statement: String,
    /// The logical evaluator reference, kept as provenance.
    pub evaluator_ref: String,
    /// The exact immutable evaluator version this criterion is pinned to.
    pub evaluator_version: String,
    pub severity: Severity,
}

/// The Task acceptance contract, with evaluator resolution provenance.
///
/// v1 strategy is fixed: every required criterion must pass. There is no
/// weighted, quorum or threshold strategy, so the strategy is not a field a
/// caller can vary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceContract {
    pub criteria: Vec<AcceptanceCriterion>,
    /// The evaluator-registry component digest the refs were resolved
    /// through. Later registry changes cannot reach back through this.
    pub evaluator_registry_digest: Digest,
    /// The ConfigurationRevision that was active at resolution.
    pub configuration_activation_sequence: i64,
}

/// The immutable Task specification.
///
/// # What is deliberately absent
///
/// The Task contract forbids Logical Agent assignment, backend, provider,
/// model or runtime identity, credentials, session IDs or LaunchKeys, runtime
/// status, retry counters, usage, worktree paths, process state, **dependency
/// edges**, mutable child-task IDs, raw result bodies, arbitrary executable
/// hooks, Agent memory and reasoning traces. None of those has a field here,
/// so a proposal cannot smuggle one in through this type — and
/// [`crate::planning::validate`] rejects a proposal that tries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
    /// The semantic task type, for example `code.change`.
    pub task_type: String,
    pub objective: String,
    pub inputs: Vec<TaskInput>,
    pub outputs: Vec<TaskOutput>,
    /// Semantic abilities required. Not execution features, and not
    /// authorization.
    pub competencies: Vec<String>,
    pub scope: TaskScope,
    pub acceptance: AcceptanceContract,
    /// The Goal this Task was created for.
    ///
    /// Part of the spec identity, not just provenance: two Goals with
    /// byte-identical content would otherwise produce one content-addressed
    /// spec row, and the second Goal's Task would reference a row attributed
    /// to the first.
    pub goal_id: String,
    /// The Goal revision this Task was created from.
    pub goal_revision: i64,
}

impl TaskSpec {
    /// The canonical value this spec's identity is taken over.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            ("type", Value::string(&self.task_type)),
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
                "outputs",
                Value::array(self.outputs.iter().map(|output| {
                    Value::object([
                        ("name", Value::string(&output.name)),
                        ("kind", Value::string(&output.kind)),
                        ("required", Value::Bool(output.required)),
                    ])
                })),
            ),
            ("competencies", strings(&self.competencies)),
            (
                "scope",
                Value::object([
                    ("resources", strings(&self.scope.resources)),
                    ("permittedEffects", strings(&self.scope.permitted_effects)),
                    ("forbiddenEffects", strings(&self.scope.forbidden_effects)),
                ]),
            ),
            ("acceptance", self.acceptance_value()),
            ("goalId", Value::string(&self.goal_id)),
            ("goalRevision", Value::Integer(self.goal_revision)),
        ])
    }

    fn acceptance_value(&self) -> Value {
        Value::object([
            // v1 strategy is fixed, and recorded so the digest changes if a
            // later version admits another strategy.
            ("strategy", Value::string("all")),
            (
                "criteria",
                Value::array(self.acceptance.criteria.iter().map(|criterion| {
                    Value::object([
                        ("id", Value::string(&criterion.id)),
                        ("statement", Value::string(&criterion.statement)),
                        ("evaluatorRef", Value::string(&criterion.evaluator_ref)),
                        (
                            "evaluatorVersion",
                            Value::string(&criterion.evaluator_version),
                        ),
                        ("severity", Value::string(criterion.severity.as_str())),
                    ])
                })),
            ),
            (
                "evaluatorRegistryDigest",
                Value::string(self.acceptance.evaluator_registry_digest.to_string()),
            ),
            (
                "configurationActivationSequence",
                Value::Integer(self.acceptance.configuration_activation_sequence),
            ),
        ])
    }

    /// The immutable spec identity.
    ///
    /// Taken over the fully pinned contract, so the digest covers the exact
    /// evaluator versions rather than the unresolved refs.
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }

    /// The digest of the acceptance contract alone, which later evaluation
    /// binds independently of the rest of the spec.
    #[must_use]
    pub fn acceptance_digest(&self) -> Digest {
        self.acceptance_value().digest()
    }

    /// Reads an immutable Task specification from its stored canonical form.
    ///
    /// Routing uses this decoder to consume the durable Task created by the
    /// planning path; it never accepts an execution-specific replacement from
    /// a caller.
    pub fn from_canonical_json(text: &str) -> Result<Self, TaskDecodeError> {
        let value = parse::parse(text).map_err(|error| TaskDecodeError(error.to_string()))?;
        let required_string = |name: &str| -> Result<String, TaskDecodeError> {
            match value.get(name) {
                Some(Value::String(text)) => Ok(text.clone()),
                Some(other) => Err(TaskDecodeError(format!(
                    "{name} is not a string (found {})",
                    other.kind()
                ))),
                None => Err(TaskDecodeError(format!("missing {name}"))),
            }
        };
        let string_list = |parent: &Value, name: &str| -> Result<Vec<String>, TaskDecodeError> {
            let Some(Value::Array(entries)) = parent.get(name) else {
                return Err(TaskDecodeError(format!("missing or malformed {name}")));
            };
            entries
                .iter()
                .map(|entry| match entry {
                    Value::String(text) => Ok(text.clone()),
                    other => Err(TaskDecodeError(format!(
                        "{name} contains a non-string value ({})",
                        other.kind()
                    ))),
                })
                .collect()
        };

        let inputs = match value.get("inputs") {
            Some(Value::Array(entries)) => entries
                .iter()
                .map(|entry| {
                    let (Some(Value::String(name)), Some(Value::String(reference))) =
                        (entry.get("name"), entry.get("ref"))
                    else {
                        return Err(TaskDecodeError("malformed input".to_string()));
                    };
                    Ok(TaskInput {
                        name: name.clone(),
                        reference: reference.clone(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(TaskDecodeError("missing or malformed inputs".to_string())),
        };

        let outputs = match value.get("outputs") {
            Some(Value::Array(entries)) => entries
                .iter()
                .map(|entry| {
                    let (
                        Some(Value::String(name)),
                        Some(Value::String(kind)),
                        Some(Value::Bool(required)),
                    ) = (entry.get("name"), entry.get("kind"), entry.get("required"))
                    else {
                        return Err(TaskDecodeError("malformed output".to_string()));
                    };
                    Ok(TaskOutput {
                        name: name.clone(),
                        kind: kind.clone(),
                        required: *required,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(TaskDecodeError("missing or malformed outputs".to_string())),
        };

        let scope = value
            .get("scope")
            .ok_or_else(|| TaskDecodeError("missing scope".to_string()))?;
        let acceptance = value
            .get("acceptance")
            .ok_or_else(|| TaskDecodeError("missing acceptance".to_string()))?;
        let criteria_value = acceptance
            .get("criteria")
            .ok_or_else(|| TaskDecodeError("missing acceptance.criteria".to_string()))?;
        let criteria = match criteria_value {
            Value::Array(entries) => entries
                .iter()
                .map(|entry| {
                    let id = entry_string(entry, "id")?;
                    let statement = entry_string(entry, "statement")?;
                    let evaluator_ref = entry_string(entry, "evaluatorRef")?;
                    let evaluator_version = entry_string(entry, "evaluatorVersion")?;
                    let severity = match entry_string(entry, "severity")?.as_str() {
                        "required" => Severity::Required,
                        "advisory" => Severity::Advisory,
                        other => {
                            return Err(TaskDecodeError(format!(
                                "unknown criterion severity {other:?}"
                            )));
                        }
                    };
                    Ok(AcceptanceCriterion {
                        id,
                        statement,
                        evaluator_ref,
                        evaluator_version,
                        severity,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => {
                return Err(TaskDecodeError(
                    "acceptance.criteria is not an array".to_string(),
                ));
            }
        };
        let strategy = entry_string(acceptance, "strategy")?;
        if strategy != "all" {
            return Err(TaskDecodeError(format!(
                "unsupported acceptance strategy {strategy:?}"
            )));
        }
        let evaluator_registry_digest =
            Digest::from_display(&entry_string(acceptance, "evaluatorRegistryDigest")?)
                .ok_or_else(|| TaskDecodeError("invalid evaluator registry digest".to_string()))?;
        let configuration_activation_sequence =
            entry_integer(acceptance, "configurationActivationSequence")?;

        Ok(Self {
            task_type: required_string("type")?,
            objective: required_string("objective")?,
            inputs,
            outputs,
            competencies: string_list(&value, "competencies")?,
            scope: TaskScope {
                resources: string_list(scope, "resources")?,
                permitted_effects: string_list(scope, "permittedEffects")?,
                forbidden_effects: string_list(scope, "forbiddenEffects")?,
            },
            acceptance: AcceptanceContract {
                criteria,
                evaluator_registry_digest,
                configuration_activation_sequence,
            },
            goal_id: required_string("goalId")?,
            goal_revision: entry_integer(&value, "goalRevision")?,
        })
    }
}

fn entry_string(value: &Value, name: &str) -> Result<String, TaskDecodeError> {
    match value.get(name) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Err(TaskDecodeError(format!(
            "{name} is not a string (found {})",
            other.kind()
        ))),
        None => Err(TaskDecodeError(format!("missing {name}"))),
    }
}

fn entry_integer(value: &Value, name: &str) -> Result<i64, TaskDecodeError> {
    match value.get(name) {
        Some(Value::Integer(number)) => Ok(*number),
        Some(other) => Err(TaskDecodeError(format!(
            "{name} is not an integer (found {})",
            other.kind()
        ))),
        None => Err(TaskDecodeError(format!("missing {name}"))),
    }
}

fn strings(values: &[String]) -> Value {
    Value::array(values.iter().map(Value::string))
}

/// A stored Task specification that cannot be read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDecodeError(pub String);

impl std::fmt::Display for TaskDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stored task specification is not readable: {}", self.0)
    }
}

impl std::error::Error for TaskDecodeError {}
