//! The immutable compiled configuration components.
//!
//! Each component is an independently digestible unit of configuration
//! authority. `docs/architecture/operations/configuration-and-policy-revisions.md`
//! §2 is explicit that this must not collapse into one ambiguous hash:
//!
//! > Pantheon must not use one ambiguous `policyHash` for unrelated domains.
//!
//! so a later immutable decision can record exactly which semantic generation
//! of routing, or of the evaluator registry, it bound — without being
//! invalidated by an unrelated change elsewhere in the same revision.
//!
//! # Provider neutrality
//!
//! `docs/architecture/execution/execution-fabric.md` draws the line these
//! types sit behind: core "contains no business logic keyed to a concrete
//! provider, model, harness or runtime name", and "no provider/model allowlist
//! in core". So a backend registration here carries a stable id, an enabled
//! flag, and an opaque `selector` that `pantheond` resolves to an
//! implementation at composition time. Core never matches on the selector, and
//! a sandbox profile's environment identity is an opaque immutable string —
//! `sandbox-broker-and-isolation.md` requires "an image digest rather than a
//! mutable tag", which Pantheon stores and compares but never parses.

use crate::config::Digest;
use crate::config::canonical::Value;

/// A compiled component that can state its own canonical form.
pub trait Component {
    /// The canonical value whose encoding is this component's identity.
    fn to_value(&self) -> Value;

    /// This component's content digest.
    fn digest(&self) -> Digest {
        self.to_value().digest()
    }
}

/// The configured Logical Agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentComponent {
    pub agents: Vec<Agent>,
}

/// One Logical Agent, provider-neutral.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub version: u32,
    /// Task types this Agent accepts — a hard eligibility input.
    pub accepts: Vec<String>,
    pub competencies: Vec<String>,
    /// Names a route policy in the routing component.
    pub route_policy: String,
    /// Factual mechanisms an offer must support.
    pub execution_features: Vec<String>,
    pub min_context_tokens: i64,
    /// Names a profile in the execution component.
    pub sandbox_profile: String,
    /// Guarantees the profile must assert for this Agent to be placeable.
    pub sandbox_requirements: Vec<String>,
    /// The semantic action surface. Availability is not authorization.
    pub actions: Vec<String>,
}

impl Component for AgentComponent {
    fn to_value(&self) -> Value {
        Value::object([(
            "agents",
            Value::array(self.agents.iter().map(|agent| {
                Value::object([
                    ("name", Value::string(&agent.name)),
                    ("version", Value::Integer(i64::from(agent.version))),
                    ("accepts", strings(&agent.accepts)),
                    ("competencies", strings(&agent.competencies)),
                    ("routePolicy", Value::string(&agent.route_policy)),
                    ("executionFeatures", strings(&agent.execution_features)),
                    ("minContextTokens", Value::Integer(agent.min_context_tokens)),
                    ("sandboxProfile", Value::string(&agent.sandbox_profile)),
                    ("sandboxRequirements", strings(&agent.sandbox_requirements)),
                    ("actions", strings(&agent.actions)),
                ])
            })),
        )])
    }
}

/// Deterministic routing policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingComponent {
    pub policies: Vec<RoutePolicy>,
}

/// One named route policy.
///
/// v1 ranking is deliberately deterministic and simple: an explicit ordering
/// of candidate keys, then a total tie-break so two runs cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePolicy {
    pub name: String,
    pub ordering: Vec<String>,
    pub tie_break: String,
}

impl Component for RoutingComponent {
    fn to_value(&self) -> Value {
        Value::object([(
            "policies",
            Value::array(self.policies.iter().map(|policy| {
                Value::object([
                    ("name", Value::string(&policy.name)),
                    ("ordering", strings(&policy.ordering)),
                    ("tieBreak", Value::string(&policy.tie_break)),
                ])
            })),
        )])
    }
}

/// Sandbox/execution profiles and backend registrations.
///
/// The configuration contract's revision manifest names an `executionProfiles`
/// component but no separate `sandboxProfiles` or `backends` component, while
/// the Agent manifest refers to "a logical SandboxProfile from
/// ConfigurationRevision". This component is therefore where both live. That
/// is a deliberate MVP decision recorded here, not a documented fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionComponent {
    pub profiles: Vec<SandboxProfile>,
    pub backends: Vec<BackendRegistration>,
}

/// The isolation class a profile asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationClass {
    TrustedHost,
    Container,
}

impl IsolationClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrustedHost => "TRUSTED_HOST",
            Self::Container => "CONTAINER",
        }
    }

    /// Whether this class can isolate a worker from Pantheon's control plane.
    ///
    /// `sandbox-broker-and-isolation.md` requires model-driven arbitrary
    /// shell/process execution to run under `isolation.control-plane`, which a
    /// trusted-host profile cannot assert.
    #[must_use]
    pub const fn can_isolate_control_plane(self) -> bool {
        matches!(self, Self::Container)
    }
}

/// How a sandboxed workload may reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    None,
    Brokered,
}

impl NetworkMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Brokered => "BROKERED",
        }
    }
}

/// One sandbox profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProfile {
    pub name: String,
    pub isolation_class: IsolationClass,
    /// Guarantees this profile asserts, for example `isolation.control-plane`.
    pub guarantees: Vec<String>,
    pub network_mode: NetworkMode,
    /// Immutable content identity of the execution environment — an image
    /// digest rather than a mutable tag. Opaque to core.
    pub environment_identity: String,
}

/// A registered execution backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRegistration {
    pub backend_id: String,
    pub enabled: bool,
    /// Resolved to a concrete adapter by the composition root. Core never
    /// branches on this value.
    pub selector: String,
}

impl Component for ExecutionComponent {
    fn to_value(&self) -> Value {
        Value::object([
            (
                "profiles",
                Value::array(self.profiles.iter().map(|profile| {
                    Value::object([
                        ("name", Value::string(&profile.name)),
                        (
                            "isolationClass",
                            Value::string(profile.isolation_class.as_str()),
                        ),
                        ("guarantees", strings(&profile.guarantees)),
                        ("networkMode", Value::string(profile.network_mode.as_str())),
                        (
                            "environmentIdentity",
                            Value::string(&profile.environment_identity),
                        ),
                    ])
                })),
            ),
            (
                "backends",
                Value::array(self.backends.iter().map(|backend| {
                    Value::object([
                        ("backendId", Value::string(&backend.backend_id)),
                        ("enabled", Value::Bool(backend.enabled)),
                        ("selector", Value::string(&backend.selector)),
                    ])
                })),
            ),
        ])
    }
}

/// The evaluator registry: logical refs resolved to immutable versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatorComponent {
    pub refs: Vec<EvaluatorRef>,
    pub versions: Vec<EvaluatorVersion>,
}

/// A logical evaluator reference and the version it currently resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatorRef {
    pub reference: String,
    pub current_version: String,
}

/// What an evaluator does. v1 admits deterministic kinds only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluatorKind {
    Check,
    Schema,
}

impl EvaluatorKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Schema => "schema",
        }
    }
}

/// One immutable evaluator version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatorVersion {
    pub id: String,
    pub kind: EvaluatorKind,
    /// An argv vector, never a shell string.
    pub argv: Vec<String>,
    pub timeout_ms: i64,
    /// Names a profile in the execution component.
    pub sandbox_profile: String,
    pub result_protocol: String,
}

impl Component for EvaluatorComponent {
    fn to_value(&self) -> Value {
        Value::object([
            (
                "refs",
                Value::array(self.refs.iter().map(|reference| {
                    Value::object([
                        ("ref", Value::string(&reference.reference)),
                        ("currentVersion", Value::string(&reference.current_version)),
                    ])
                })),
            ),
            (
                "versions",
                Value::array(self.versions.iter().map(|version| {
                    Value::object([
                        ("id", Value::string(&version.id)),
                        ("kind", Value::string(version.kind.as_str())),
                        ("argv", strings(&version.argv)),
                        ("timeoutMs", Value::Integer(version.timeout_ms)),
                        ("sandboxProfile", Value::string(&version.sandbox_profile)),
                        ("resultProtocol", Value::string(&version.result_protocol)),
                    ])
                })),
            ),
        ])
    }
}

/// Deterministic context construction policy, addressed by
/// `contextPolicyDigest`. It governs semantic selection and trimming only and
/// is never an authorization digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextComponent {
    pub schema_version: u32,
    pub mandatory_sections: Vec<String>,
    pub preload_priority: Vec<String>,
    pub memory_limit_tokens: i64,
    pub workspace_orientation_limit_tokens: i64,
    pub safety_margin_tokens: i64,
    /// The order optional sections are dropped in when trimming.
    pub optional_drop_order: Vec<String>,
}

impl Component for ContextComponent {
    fn to_value(&self) -> Value {
        Value::object([
            (
                "schemaVersion",
                Value::Integer(i64::from(self.schema_version)),
            ),
            ("mandatorySections", strings(&self.mandatory_sections)),
            ("preloadPriority", strings(&self.preload_priority)),
            (
                "memoryLimitTokens",
                Value::Integer(self.memory_limit_tokens),
            ),
            (
                "workspaceOrientationLimitTokens",
                Value::Integer(self.workspace_orientation_limit_tokens),
            ),
            (
                "safetyMarginTokens",
                Value::Integer(self.safety_margin_tokens),
            ),
            ("optionalDropOrder", strings(&self.optional_drop_order)),
        ])
    }
}

/// The compiled-in hard policy identity.
///
/// `configuration-and-policy-revisions.md` §4 requires the built-in
/// hard-policy version to participate in the authorization component digest,
/// so that operator configuration cannot weaken it without the identity
/// changing.
pub const HARD_POLICY_VERSION: &str = "pantheon-hard-policy-v1";

/// Authorization configuration and the hard-policy identity it sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationComponent {
    pub schema_version: u32,
    /// Configured rules, each naming a canonical action.
    pub rules: Vec<AuthorizationRule>,
}

/// One configured authorization rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationRule {
    pub action: String,
    pub effect: RuleEffect,
}

/// Whether a rule permits or forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleEffect {
    Permit,
    Forbid,
}

impl RuleEffect {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permit => "permit",
            Self::Forbid => "forbid",
        }
    }
}

impl Component for AuthorizationComponent {
    fn to_value(&self) -> Value {
        Value::object([
            (
                "schemaVersion",
                Value::Integer(i64::from(self.schema_version)),
            ),
            // The hard-policy identity is part of the digest, not a sibling
            // field a caller could omit.
            ("hardPolicyVersion", Value::string(HARD_POLICY_VERSION)),
            (
                "rules",
                Value::array(self.rules.iter().map(|rule| {
                    Value::object([
                        ("action", Value::string(&rule.action)),
                        ("effect", Value::string(rule.effect.as_str())),
                    ])
                })),
            ),
        ])
    }
}

fn strings(values: &[String]) -> Value {
    Value::array(values.iter().map(Value::string))
}
