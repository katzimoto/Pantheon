//! The immutable Task specification and the canonical Task lifecycle.

use crate::config::Digest;
use crate::config::canonical::Value;

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
}

fn strings(values: &[String]) -> Value {
    Value::array(values.iter().map(Value::string))
}
