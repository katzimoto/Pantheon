//! Pure scheduler vocabulary and the deterministic ordering decision.
//!
//! `docs/architecture/scheduling/scheduler-task-ordering-and-fairness.md` is
//! canonical for the two-level rule: Goals are served fairly first, then the
//! oldest scheduler-eligible Task inside the chosen Goal is selected. The
//! inputs to that decision are durable controller state — service sequences,
//! `eligible_since` intervals, stable identifiers — never hash-map iteration
//! order, process uptime or restart timing, which is why the whole decision is
//! a pure function over an explicit snapshot.
//!
//! This module also holds the immutable strategy and context-source identities
//! a Run freezes at T3. Those types name authority; they perform none.

use crate::config::canonical::Value;
use crate::config::{ComponentDigests, Digest, parse};
use crate::execution::LogicalAgentVersion;

/// The operator's durable desired dispatch state.
///
/// Distinct from recovery/configuration readiness: `Paused` fences new T3
/// Run-intent commits and survives restart without cancelling anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    Running,
    Paused,
}

impl DispatchMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Paused => "PAUSED",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "RUNNING" => Self::Running,
            "PAUSED" => Self::Paused,
            _ => return None,
        })
    }
}

/// One Goal's durable fairness position, as read from authoritative state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalFairness {
    /// The stable Goal identifier. Doubles as the deterministic tie-breaker,
    /// so no other ordering input is needed to make selection reproducible.
    pub goal_id: String,
    /// The last service sequence this Goal was charged, or `None` when it has
    /// never successfully received a Run intent. A `None` sorts first.
    pub last_served_sequence: Option<i64>,
}

/// One scheduler-eligible Task, as read from authoritative state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulableTask {
    pub task_id: String,
    pub goal_id: String,
    /// When the current `SchedulingEligible` interval began. Older intervals
    /// are served first; the stable Task id breaks ties.
    pub eligible_since: i64,
}

/// Chooses the next (Goal, Task) pair from durable scheduling state.
///
/// Within one priority class — v0.1.0 has exactly one — the Goal that was
/// least recently *successfully* served wins, with the stable Goal id as the
/// tie-breaker between equally unserved or equal-sequence Goals; then the Task
/// with the oldest current eligibility interval wins, with the stable Task id
/// as its tie-breaker. Bounded aging across classes is later-slice policy and
/// deliberately absent here.
///
/// Deterministic by construction: every input names its own position, so the
/// same snapshot always yields the same answer regardless of how the caller
/// collected it.
/// The full service order over durable scheduling state.
///
/// The same deterministic rule as the contract's selection algorithm, applied
/// to every candidate: least-recently-served Goal first (never served sorts
/// first; stable Goal id breaks ties), then oldest eligibility interval within
/// the Goal (stable Task id breaks ties). The engine walks this order so a
/// temporarily unavailable older Task never blocks a newer one.
#[must_use]
pub fn service_order(goals: &[GoalFairness], tasks: &[SchedulableTask]) -> Vec<(String, String)> {
    // Owned decision records rather than borrows, so a Goal with no fairness
    // row can participate under its synthesized never-served position without
    // lifetime gymnastics.
    let mut candidates: Vec<(i64, String, i64, String)> = tasks
        .iter()
        .map(|task| {
            let served = goals
                .iter()
                .find(|goal| goal.goal_id == task.goal_id)
                .and_then(|goal| goal.last_served_sequence);
            (
                served.unwrap_or(i64::MIN),
                task.goal_id.clone(),
                task.eligible_since,
                task.task_id.clone(),
            )
        })
        .collect();

    candidates.sort();

    candidates
        .into_iter()
        .map(|(_, goal_id, _, task_id)| (goal_id, task_id))
        .collect()
}

/// Chooses the next (Goal, Task) pair: the head of [`service_order`].
#[must_use]
pub fn select_service(
    goals: &[GoalFairness],
    tasks: &[SchedulableTask],
) -> Option<(String, String)> {
    service_order(goals, tasks).into_iter().next()
}

/// Why a Task was not selected in this cycle despite being eligible.
///
/// These suppressions do not end the Task's semantic eligibility interval and
/// must not reset its waiting age; they only defer consideration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suppression {
    /// The operator's durable desired state is PAUSED.
    OperatorPause,
    /// No usable ConfigurationRevision is published.
    ConfigurationUnavailable,
    /// The single global execution slot is durably held by a nonterminal Run.
    SlotHeld,
}

/// The immutable execution strategy a Run commits to at T3.
///
/// Freezes the exact selected Logical Agent version, request/offer identity,
/// backend descriptor revision and digest, applicable profile identities, and
/// the captured ConfigurationRevision with its domain-specific component
/// digests. There is deliberately no generic policy hash: each component keeps
/// its own name, per the Execution Fabric contract.
///
/// Notably absent by construction rather than by omission: no credential
/// material, no SecretVersionId, no backend-private session identity, and no
/// mutable latest reference belongs here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionBinding {
    pub task_id: String,
    pub agent: LogicalAgentVersion,
    pub request_digest: Digest,
    pub offer_digest: Digest,
    pub backend_id: String,
    pub descriptor_revision: u64,
    pub descriptor_digest: Digest,
    /// The configured execution-profile identity feasibility validated.
    pub execution_profile_digest: Digest,
    /// The configured sandbox-profile identity the route required. A concrete
    /// SandboxPlan arrives with the Sandbox Planner; until then this is the
    /// profile identity feasibility was proven against.
    pub sandbox_profile_digest: Digest,
    pub route_policy_digest: Digest,
    /// The captured ConfigurationRevision the whole routing cycle ran under.
    pub configuration_activation_sequence: i64,
    pub configuration_content_digest: Digest,
    pub component_digests: ComponentDigests,
}

impl ExecutionBinding {
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }

    /// The canonical encoding the digest is taken over.
    ///
    /// Stored verbatim beside the digest, so a persisted Binding can be
    /// re-hashed and compared against its own recorded identity.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            ("taskId", Value::string(&self.task_id)),
            ("agent", agent_value(&self.agent)),
            (
                "requestDigest",
                Value::string(self.request_digest.to_string()),
            ),
            ("offerDigest", Value::string(self.offer_digest.to_string())),
            ("backendId", Value::string(&self.backend_id)),
            (
                "descriptorRevision",
                Value::Integer(i64::try_from(self.descriptor_revision).unwrap_or(i64::MAX)),
            ),
            (
                "descriptorDigest",
                Value::string(self.descriptor_digest.to_string()),
            ),
            (
                "executionProfileDigest",
                Value::string(self.execution_profile_digest.to_string()),
            ),
            (
                "sandboxProfileDigest",
                Value::string(self.sandbox_profile_digest.to_string()),
            ),
            (
                "routePolicyDigest",
                Value::string(self.route_policy_digest.to_string()),
            ),
            (
                "configurationActivationSequence",
                Value::Integer(self.configuration_activation_sequence),
            ),
            (
                "configurationContentDigest",
                Value::string(self.configuration_content_digest.to_string()),
            ),
            (
                "componentDigests",
                component_digests(&self.component_digests),
            ),
        ])
    }
}

/// The frozen source-eligibility manifest committed at T3, before any context
/// construction.
///
/// It names the exact immutable source generations a later Context Builder may
/// select from for this Run: the Task/Goal/Graph revisions, the Agent version
/// together with the content digests of that version's static approved SOUL
/// and BEHAVIOR guidance, the captured ConfigurationRevision with its
/// context-policy component digest, and the Task-owned Workspace identity with
/// the immutable base it was verified against. It freezes eligibility, not
/// selection — no retrieval, rendering or repository reading happens to build
/// or store it.
///
/// Sources that do not yet exist in this build (Memory generations, Skill
/// versions, continuations) have no field here; naming them before they can be
/// resolved would freeze placeholders rather than authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSourceSnapshot {
    pub task_spec_digest: Digest,
    pub goal_id: String,
    pub goal_revision: i64,
    pub graph_revision: i64,
    pub agent: LogicalAgentVersion,
    /// The captured ConfigurationRevision, shared with the Binding.
    pub configuration_activation_sequence: i64,
    /// The context-policy component digest of that same revision.
    pub context_policy_digest: Digest,
    /// The SOUL guidance digest of exactly this Agent version under that
    /// revision's agents component. Frozen so a later active configuration can
    /// never substitute different guidance into this Run, and so preparation
    /// can prove the guidance it loads is the guidance T3 froze.
    pub agent_soul_digest: Digest,
    /// The BEHAVIOR guidance digest of exactly this Agent version.
    pub agent_behavior_digest: Digest,
    pub workspace_id: String,
    /// The immutable base the Workspace's materialization was verified
    /// against — the starting point a later plan may select from.
    pub workspace_resolved_base: String,
}

impl ContextSourceSnapshot {
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            (
                "taskSpecDigest",
                Value::string(self.task_spec_digest.to_string()),
            ),
            ("goalId", Value::string(&self.goal_id)),
            ("goalRevision", Value::Integer(self.goal_revision)),
            ("graphRevision", Value::Integer(self.graph_revision)),
            ("agent", agent_value(&self.agent)),
            (
                "configurationActivationSequence",
                Value::Integer(self.configuration_activation_sequence),
            ),
            (
                "contextPolicyDigest",
                Value::string(self.context_policy_digest.to_string()),
            ),
            (
                "agentSoulDigest",
                Value::string(self.agent_soul_digest.to_string()),
            ),
            (
                "agentBehaviorDigest",
                Value::string(self.agent_behavior_digest.to_string()),
            ),
            ("workspaceId", Value::string(&self.workspace_id)),
            (
                "workspaceResolvedBase",
                Value::string(&self.workspace_resolved_base),
            ),
        ])
    }

    /// Reads a frozen source snapshot back from its stored canonical form.
    ///
    /// Preparation reconstructs the Run's source universe through this decoder;
    /// it never accepts a caller-assembled substitute. Callers still re-digest
    /// the decoded value and compare against the Run's stored snapshot digest —
    /// decoding proves shape, the digest comparison proves identity.
    pub fn from_canonical_json(text: &str) -> Result<Self, SnapshotDecodeError> {
        let error = |detail: String| SnapshotDecodeError(detail);
        let value = parse::parse(text).map_err(|err| error(err.to_string()))?;
        let string = |name: &str| -> Result<String, SnapshotDecodeError> {
            match value.get(name) {
                Some(Value::String(text)) => Ok(text.clone()),
                Some(other) => Err(error(format!(
                    "{name} is not a string (found {})",
                    other.kind()
                ))),
                None => Err(error(format!("missing {name}"))),
            }
        };
        let integer = |name: &str| -> Result<i64, SnapshotDecodeError> {
            match value.get(name) {
                Some(Value::Integer(number)) => Ok(*number),
                Some(other) => Err(error(format!(
                    "{name} is not an integer (found {})",
                    other.kind()
                ))),
                None => Err(error(format!("missing {name}"))),
            }
        };
        let digest = |name: &str| -> Result<Digest, SnapshotDecodeError> {
            Digest::from_display(&string(name)?)
                .ok_or_else(|| error(format!("{name} is not a sha256 digest")))
        };

        let agent = value
            .get("agent")
            .ok_or_else(|| error("missing agent".to_string()))?;
        let (Some(Value::String(agent_name)), Some(Value::Integer(agent_version))) =
            (agent.get("name"), agent.get("version"))
        else {
            return Err(error("malformed agent identity".to_string()));
        };
        let agent_version: u32 = u32::try_from(*agent_version)
            .map_err(|_| error("agent version does not fit a 32-bit number".to_string()))?;
        Ok(Self {
            task_spec_digest: digest("taskSpecDigest")?,
            goal_id: string("goalId")?,
            goal_revision: integer("goalRevision")?,
            graph_revision: integer("graphRevision")?,
            agent: LogicalAgentVersion::new(agent_name, agent_version),
            configuration_activation_sequence: integer("configurationActivationSequence")?,
            context_policy_digest: digest("contextPolicyDigest")?,
            agent_soul_digest: digest("agentSoulDigest")?,
            agent_behavior_digest: digest("agentBehaviorDigest")?,
            workspace_id: string("workspaceId")?,
            workspace_resolved_base: string("workspaceResolvedBase")?,
        })
    }
}

/// A frozen source snapshot that cannot be read back from durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDecodeError(pub String);

impl std::fmt::Display for SnapshotDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "stored context source snapshot is not readable: {}",
            self.0
        )
    }
}

impl std::error::Error for SnapshotDecodeError {}

fn agent_value(agent: &LogicalAgentVersion) -> Value {
    Value::object([
        ("name", Value::string(&agent.name)),
        ("version", Value::Integer(i64::from(agent.version))),
    ])
}

fn component_digests(digests: &ComponentDigests) -> Value {
    Value::object([
        ("agents", Value::string(digests.agents.to_string())),
        ("routing", Value::string(digests.routing.to_string())),
        (
            "executionProfiles",
            Value::string(digests.execution_profile.to_string()),
        ),
        (
            "evaluators",
            Value::string(digests.evaluator_registry.to_string()),
        ),
        ("context", Value::string(digests.context_policy.to_string())),
        (
            "authorization",
            Value::string(digests.authorization.to_string()),
        ),
    ])
}
