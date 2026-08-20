//! Pure Logical Agent resolution and pre-Run Execution Fabric rules.
//!
//! This module stops at a recomputable `LogicalAgent + ExecutionOffer` result.
//! It has no execution authority and no knowledge of concrete provider, model,
//! harness, runtime or backend implementation identities.

use std::cmp::Ordering;

use crate::config::canonical::Value;
use crate::config::model::{Agent, AuthorizationRule, RoutePolicy, SandboxProfile};
use crate::config::{CompiledConfiguration, ComponentDigests, Digest};
use crate::planning::TaskSpec;

pub use crate::config::model::LogicalAgentVersion;

/// Exact configuration provenance used for one routing operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigurationBinding {
    pub activation_sequence: i64,
    pub content_digest: Digest,
    pub component_digests: ComponentDigests,
}

impl ConfigurationBinding {
    #[must_use]
    pub const fn new(
        activation_sequence: i64,
        content_digest: Digest,
        component_digests: ComponentDigests,
    ) -> Self {
        Self {
            activation_sequence,
            content_digest,
            component_digests,
        }
    }

    #[must_use]
    pub fn matches(self, other: Self) -> bool {
        self.activation_sequence == other.activation_sequence
            && self.content_digest == other.content_digest
            && self.component_digests == other.component_digests
    }

    #[must_use]
    fn to_value(self) -> Value {
        Value::object([
            (
                "activationSequence",
                Value::Integer(self.activation_sequence),
            ),
            (
                "contentDigest",
                Value::string(self.content_digest.to_string()),
            ),
            (
                "componentDigests",
                Value::object([
                    (
                        "agents",
                        Value::string(self.component_digests.agents.to_string()),
                    ),
                    (
                        "routing",
                        Value::string(self.component_digests.routing.to_string()),
                    ),
                    (
                        "executionProfiles",
                        Value::string(self.component_digests.execution_profile.to_string()),
                    ),
                    (
                        "evaluators",
                        Value::string(self.component_digests.evaluator_registry.to_string()),
                    ),
                    (
                        "context",
                        Value::string(self.component_digests.context_policy.to_string()),
                    ),
                    (
                        "authorization",
                        Value::string(self.component_digests.authorization.to_string()),
                    ),
                ]),
            ),
        ])
    }
}

/// Whether a later execution layer must provide duplicate-safe launch
/// semantics for this route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSafety {
    KeyedRequired,
    ObservationalAllowed,
}

/// Factual launch behavior reported by a backend descriptor and offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSemantics {
    KeyedIdempotent,
    Observational,
}

impl LaunchSemantics {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KeyedIdempotent => "KEYED_IDEMPOTENT",
            Self::Observational => "OBSERVATIONAL",
        }
    }
}

/// Controller-owned safety evidence supplied beside a backend port.
///
/// These facts are deliberately not returned by `ExecutorBackend::describe` or
/// `offer`: backend self-attestation cannot prove physical isolation or provide
/// an outer duplicate-prevention supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSafetyFacts {
    pub isolation_guarantees: Vec<String>,
    pub observational_launch_safe: bool,
}

/// A generic resource quantity used only for factual compatibility checks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceQuantity {
    pub name: String,
    pub quantity: i64,
}

/// Revisioned factual information published by an ExecutorBackend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendDescriptor {
    pub backend_id: String,
    pub revision: u64,
    pub available_for_offers: bool,
    pub placement: Vec<String>,
    pub supported_execution_features: Vec<String>,
    pub context_capacity_tokens: i64,
    /// Backend-reported compatibility facts; not security proof.
    pub isolation_facts: Vec<String>,
    pub resources: Vec<ResourceQuantity>,
    pub launch_semantics: LaunchSemantics,
}

impl BackendDescriptor {
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            ("backendId", Value::string(&self.backend_id)),
            ("revision", Value::string(self.revision.to_string())),
            ("availableForOffers", Value::Bool(self.available_for_offers)),
            ("placement", strings(&self.placement)),
            (
                "supportedExecutionFeatures",
                strings(&self.supported_execution_features),
            ),
            (
                "contextCapacityTokens",
                Value::Integer(self.context_capacity_tokens),
            ),
            ("isolationFacts", isolation_facts(&self.isolation_facts)),
            ("resources", resources(&self.resources)),
            (
                "launchSemantics",
                Value::string(self.launch_semantics.as_str()),
            ),
        ])
    }
}

/// One Agent-specific, provider-neutral request to the Execution Fabric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRequest {
    pub task_id: String,
    pub task_spec_digest: Digest,
    pub task_type: String,
    pub task_competencies: Vec<String>,
    pub agent: LogicalAgentVersion,
    pub required_execution_features: Vec<String>,
    pub min_context_tokens: i64,
    pub placement_constraints: Vec<String>,
    pub isolation_requirements: Vec<String>,
    pub resource_requirements: Vec<ResourceQuantity>,
    pub required_actions: Vec<String>,
    pub configuration: ConfigurationBinding,
    pub route_policy_digest: Digest,
    pub execution_profile_digest: Digest,
    pub sandbox_profile_digest: Digest,
    pub launch_safety: LaunchSafety,
}

impl ExecutionRequest {
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            ("taskId", Value::string(&self.task_id)),
            (
                "taskSpecDigest",
                Value::string(self.task_spec_digest.to_string()),
            ),
            ("taskType", Value::string(&self.task_type)),
            ("taskCompetencies", strings(&self.task_competencies)),
            ("agent", agent_value(&self.agent)),
            (
                "requiredExecutionFeatures",
                strings(&self.required_execution_features),
            ),
            ("minContextTokens", Value::Integer(self.min_context_tokens)),
            ("placementConstraints", strings(&self.placement_constraints)),
            (
                "isolationRequirements",
                strings(&self.isolation_requirements),
            ),
            (
                "resourceRequirements",
                resources(&self.resource_requirements),
            ),
            ("requiredActions", strings(&self.required_actions)),
            ("configuration", self.configuration.to_value()),
            (
                "routePolicyDigest",
                Value::string(self.route_policy_digest.to_string()),
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
                "launchSafety",
                Value::string(match self.launch_safety {
                    LaunchSafety::KeyedRequired => "KEYED_REQUIRED",
                    LaunchSafety::ObservationalAllowed => "OBSERVATIONAL_ALLOWED",
                }),
            ),
        ])
    }
}

/// A semantically eligible Agent together with the immutable configured facts
/// needed to construct its request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgent {
    pub identity: LogicalAgentVersion,
    pub agent: Agent,
    pub route_policy: RoutePolicy,
    pub sandbox_profile: SandboxProfile,
}

/// Why one configured Agent was not semantically eligible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRejectionReason {
    Disabled,
    NotCurrent,
    UnsupportedTaskType { task_type: String },
    MissingCompetencies { missing: Vec<String> },
    PinnedOut,
    Excluded,
    PolicyIncompatible { detail: String },
    ConfigurationIncompatible { detail: String },
}

/// A deterministic diagnostic for one rejected Agent version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRejection {
    pub agent: LogicalAgentVersion,
    pub reason: AgentRejectionReason,
}

/// The successful Agent-resolution result, including diagnostics for rejected
/// versions that were considered under the same snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResolution {
    pub eligible: Vec<ResolvedAgent>,
    pub rejected: Vec<AgentRejection>,
}

/// A deterministic failure when no configured Agent is semantically eligible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentResolutionError {
    ConfigurationMismatch {
        expected: Digest,
        actual: Digest,
    },
    NoEligibleAgent {
        task_type: String,
        causes: Vec<AgentRejection>,
    },
}

/// Resolves only semantic Agent eligibility. Backend facts are intentionally
/// absent from this function.
pub fn resolve_agents(
    task: &TaskSpec,
    configuration: &CompiledConfiguration,
    binding: ConfigurationBinding,
) -> Result<AgentResolution, AgentResolutionError> {
    let actual = configuration.revision_digest();
    if actual != binding.content_digest
        || configuration.component_digests() != binding.component_digests
    {
        return Err(AgentResolutionError::ConfigurationMismatch {
            expected: binding.content_digest,
            actual,
        });
    }

    let pins = &configuration.routing().agent_pins;
    let exclusions = &configuration.routing().agent_exclusions;
    let mut agents = configuration.agents().agents.clone();
    agents.sort_by_key(Agent::identity);

    let mut eligible = Vec::new();
    let mut rejected = Vec::new();
    for agent in agents {
        let identity = agent.identity();
        let reason = if !agent.enabled {
            Some(AgentRejectionReason::Disabled)
        } else if !agent.current {
            Some(AgentRejectionReason::NotCurrent)
        } else if !agent
            .accepts
            .iter()
            .any(|accepted| accepted == &task.task_type)
        {
            Some(AgentRejectionReason::UnsupportedTaskType {
                task_type: task.task_type.clone(),
            })
        } else {
            let missing = sorted_difference(&task.competencies, &agent.competencies);
            if !missing.is_empty() {
                Some(AgentRejectionReason::MissingCompetencies { missing })
            } else if !pins.is_empty() && !pins.contains(&identity) {
                Some(AgentRejectionReason::PinnedOut)
            } else if exclusions.contains(&identity) {
                Some(AgentRejectionReason::Excluded)
            } else if let Some(detail) = policy_incompatibility(task, &agent, configuration) {
                Some(AgentRejectionReason::PolicyIncompatible { detail })
            } else if task.acceptance.configuration_activation_sequence
                > binding.activation_sequence
            {
                Some(AgentRejectionReason::ConfigurationIncompatible {
                    detail: "Task provenance names a future configuration activation".to_string(),
                })
            } else {
                None
            }
        };

        if let Some(reason) = reason {
            rejected.push(AgentRejection {
                agent: identity,
                reason,
            });
            continue;
        }

        let Some(route_policy) = configuration
            .routing()
            .policies
            .iter()
            .find(|policy| policy.name == agent.route_policy)
            .cloned()
        else {
            rejected.push(AgentRejection {
                agent: identity,
                reason: AgentRejectionReason::ConfigurationIncompatible {
                    detail: format!("unknown route policy {:?}", agent.route_policy),
                },
            });
            continue;
        };
        let Some(sandbox_profile) = configuration
            .execution()
            .profiles
            .iter()
            .find(|profile| profile.name == agent.sandbox_profile)
            .cloned()
        else {
            rejected.push(AgentRejection {
                agent: identity,
                reason: AgentRejectionReason::ConfigurationIncompatible {
                    detail: format!("unknown sandbox profile {:?}", agent.sandbox_profile),
                },
            });
            continue;
        };

        if agent
            .sandbox_requirements
            .iter()
            .any(|requirement| !sandbox_profile.guarantees.contains(requirement))
        {
            rejected.push(AgentRejection {
                agent: identity,
                reason: AgentRejectionReason::ConfigurationIncompatible {
                    detail: format!(
                        "sandbox profile {:?} does not satisfy the Agent requirements",
                        sandbox_profile.name
                    ),
                },
            });
            continue;
        }

        eligible.push(ResolvedAgent {
            identity,
            agent,
            route_policy,
            sandbox_profile,
        });
    }

    if eligible.is_empty() {
        return Err(AgentResolutionError::NoEligibleAgent {
            task_type: task.task_type.clone(),
            causes: rejected,
        });
    }

    Ok(AgentResolution { eligible, rejected })
}

/// Builds the one provider-neutral request for an eligible Agent.
pub fn build_execution_request(
    task_id: &str,
    task: &TaskSpec,
    agent: &ResolvedAgent,
    configuration: &CompiledConfiguration,
    binding: ConfigurationBinding,
) -> Result<ExecutionRequest, RequestBuildError> {
    if configuration.revision_digest() != binding.content_digest
        || configuration.component_digests() != binding.component_digests
    {
        return Err(RequestBuildError::ConfigurationMismatch);
    }
    let mut task_competencies = task.competencies.clone();
    let mut required_execution_features = agent.agent.execution_features.clone();
    let mut isolation_requirements = agent.agent.sandbox_requirements.clone();
    let mut required_actions = agent.agent.actions.clone();
    task_competencies.sort();
    required_execution_features.sort();
    isolation_requirements.sort();
    required_actions.sort();

    Ok(ExecutionRequest {
        task_id: task_id.to_string(),
        task_spec_digest: task.digest(),
        task_type: task.task_type.clone(),
        task_competencies,
        agent: agent.identity.clone(),
        required_execution_features,
        min_context_tokens: agent.agent.min_context_tokens,
        placement_constraints: Vec::new(),
        isolation_requirements,
        resource_requirements: Vec::new(),
        required_actions,
        configuration: binding,
        route_policy_digest: agent.route_policy.digest(),
        execution_profile_digest: binding.component_digests.execution_profile,
        sandbox_profile_digest: agent.sandbox_profile.digest(),
        launch_safety: if agent.route_policy.requires_keyed_launch {
            LaunchSafety::KeyedRequired
        } else {
            LaunchSafety::ObservationalAllowed
        },
    })
}

/// Why a request/descriptor/offer combination cannot become a candidate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateRejection {
    BackendDisabled,
    BackendUnavailable,
    BackendMismatch,
    DescriptorRevisionMismatch,
    OfferRequestMismatch,
    MissingExecutionFeatures {
        missing: Vec<String>,
    },
    ContextCapacityTooSmall {
        required: i64,
        offered: i64,
    },
    PlacementIncompatible {
        missing: Vec<String>,
    },
    IsolationNotProven {
        missing: Vec<String>,
    },
    ResourceIncompatible {
        name: String,
        required: i64,
        offered: i64,
    },
    LaunchSemanticsMismatch,
    UnsafeLaunchSemantics,
    RoutePolicyMismatch,
}

/// A validated final pre-Run routing unit: one Agent version and one offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionCandidate {
    pub agent: LogicalAgentVersion,
    pub request_digest: Digest,
    pub offer: ExecutionOffer,
    pub route_policy: RoutePolicy,
    pub execution_profile_digest: Digest,
}

impl ExecutionCandidate {
    #[must_use]
    pub fn digest(&self) -> Digest {
        Value::object([
            ("agent", agent_value(&self.agent)),
            (
                "requestDigest",
                Value::string(self.request_digest.to_string()),
            ),
            (
                "offerDigest",
                Value::string(self.offer.digest().to_string()),
            ),
            (
                "routePolicyDigest",
                Value::string(self.route_policy.digest().to_string()),
            ),
            (
                "executionProfileDigest",
                Value::string(self.execution_profile_digest.to_string()),
            ),
        ])
        .digest()
    }
}

/// Side-effect-free factual compatibility evidence for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOffer {
    pub request_digest: Digest,
    pub backend_id: String,
    pub descriptor_revision: u64,
    pub descriptor_digest: Digest,
    pub supported_execution_features: Vec<String>,
    pub context_capacity_tokens: i64,
    pub placement: Vec<String>,
    /// Backend-reported compatibility facts; not security proof.
    pub isolation_facts: Vec<String>,
    pub resources: Vec<ResourceQuantity>,
    pub launch_semantics: LaunchSemantics,
    /// Opaque to core; the adapter may use it later to bind backend-private
    /// facts without making them semantic Agent identity.
    pub offer_reference: String,
}

impl ExecutionOffer {
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            (
                "requestDigest",
                Value::string(self.request_digest.to_string()),
            ),
            ("backendId", Value::string(&self.backend_id)),
            (
                "descriptorRevision",
                Value::string(self.descriptor_revision.to_string()),
            ),
            (
                "descriptorDigest",
                Value::string(self.descriptor_digest.to_string()),
            ),
            (
                "supportedExecutionFeatures",
                strings(&self.supported_execution_features),
            ),
            (
                "contextCapacityTokens",
                Value::Integer(self.context_capacity_tokens),
            ),
            ("placement", strings(&self.placement)),
            ("isolationFacts", isolation_facts(&self.isolation_facts)),
            ("resources", resources(&self.resources)),
            (
                "launchSemantics",
                Value::string(self.launch_semantics.as_str()),
            ),
            ("offerReference", Value::string(&self.offer_reference)),
        ])
    }

    #[must_use]
    pub fn is_stale_against(&self, descriptor: &BackendDescriptor) -> bool {
        self.backend_id != descriptor.backend_id || self.descriptor_revision != descriptor.revision
    }
}

/// Validates the known, provider-neutral facts before a candidate can be
/// considered by route policy.
pub fn validate_execution_candidate(
    request: &ExecutionRequest,
    resolved_agent: &ResolvedAgent,
    descriptor: &BackendDescriptor,
    offer: &ExecutionOffer,
    backend_enabled: bool,
    safety: &ControllerSafetyFacts,
) -> Result<ExecutionCandidate, CandidateRejection> {
    if !backend_enabled {
        return Err(CandidateRejection::BackendDisabled);
    }
    if !descriptor.available_for_offers {
        return Err(CandidateRejection::BackendUnavailable);
    }
    if descriptor.backend_id != offer.backend_id {
        return Err(CandidateRejection::BackendMismatch);
    }
    if descriptor.revision != offer.descriptor_revision {
        return Err(CandidateRejection::DescriptorRevisionMismatch);
    }
    if descriptor.digest() != offer.descriptor_digest {
        return Err(CandidateRejection::DescriptorRevisionMismatch);
    }
    if request.digest() != offer.request_digest {
        return Err(CandidateRejection::OfferRequestMismatch);
    }
    if resolved_agent.identity != request.agent
        || resolved_agent.route_policy.digest() != request.route_policy_digest
    {
        return Err(CandidateRejection::RoutePolicyMismatch);
    }
    let missing = missing_strings(
        &request.required_execution_features,
        &descriptor.supported_execution_features,
    );
    if !missing.is_empty() {
        return Err(CandidateRejection::MissingExecutionFeatures { missing });
    }
    let missing = missing_strings(
        &request.required_execution_features,
        &offer.supported_execution_features,
    );
    if !missing.is_empty() {
        return Err(CandidateRejection::MissingExecutionFeatures { missing });
    }
    if descriptor.context_capacity_tokens < request.min_context_tokens {
        return Err(CandidateRejection::ContextCapacityTooSmall {
            required: request.min_context_tokens,
            offered: descriptor.context_capacity_tokens,
        });
    }
    if offer.context_capacity_tokens < request.min_context_tokens {
        return Err(CandidateRejection::ContextCapacityTooSmall {
            required: request.min_context_tokens,
            offered: offer.context_capacity_tokens,
        });
    }
    let missing = missing_strings(&request.placement_constraints, &descriptor.placement);
    if !missing.is_empty() {
        return Err(CandidateRejection::PlacementIncompatible { missing });
    }
    let missing = missing_strings(&request.placement_constraints, &offer.placement);
    if !missing.is_empty() {
        return Err(CandidateRejection::PlacementIncompatible { missing });
    }
    let missing = request
        .isolation_requirements
        .iter()
        .filter(|requirement| {
            !safety.isolation_guarantees.contains(requirement)
                || !descriptor.isolation_facts.contains(requirement)
                || !offer.isolation_facts.contains(requirement)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(CandidateRejection::IsolationNotProven { missing });
    }
    if descriptor.launch_semantics != offer.launch_semantics {
        return Err(CandidateRejection::LaunchSemanticsMismatch);
    }
    if request.launch_safety == LaunchSafety::KeyedRequired
        && offer.launch_semantics != LaunchSemantics::KeyedIdempotent
    {
        return Err(CandidateRejection::UnsafeLaunchSemantics);
    }
    if request.launch_safety == LaunchSafety::ObservationalAllowed
        && offer.launch_semantics == LaunchSemantics::Observational
        && !safety.observational_launch_safe
    {
        return Err(CandidateRejection::UnsafeLaunchSemantics);
    }
    for required in &request.resource_requirements {
        let Some(descriptor_quantity) = resource_quantity(&descriptor.resources, &required.name)
        else {
            return Err(CandidateRejection::ResourceIncompatible {
                name: required.name.clone(),
                required: required.quantity,
                offered: 0,
            });
        };
        if descriptor_quantity < required.quantity {
            return Err(CandidateRejection::ResourceIncompatible {
                name: required.name.clone(),
                required: required.quantity,
                offered: descriptor_quantity,
            });
        }
        let Some(offer_quantity) = resource_quantity(&offer.resources, &required.name) else {
            return Err(CandidateRejection::ResourceIncompatible {
                name: required.name.clone(),
                required: required.quantity,
                offered: 0,
            });
        };
        if offer_quantity < required.quantity {
            return Err(CandidateRejection::ResourceIncompatible {
                name: required.name.clone(),
                required: required.quantity,
                offered: offer_quantity,
            });
        }
    }

    Ok(ExecutionCandidate {
        agent: resolved_agent.identity.clone(),
        request_digest: request.digest(),
        offer: offer.clone(),
        route_policy: resolved_agent.route_policy.clone(),
        execution_profile_digest: request.execution_profile_digest,
    })
}

/// Selects one compatible candidate using only configured deterministic
/// preference keys and a canonical semantic identity tie-break.
pub fn select_execution_candidate(
    candidates: &[ExecutionCandidate],
) -> Result<ExecutionCandidate, SelectionError> {
    candidates
        .iter()
        .min_by(|left, right| compare_candidates(left, right))
        .cloned()
        .ok_or(SelectionError::NoCandidates)
}

/// The selected pre-Run routing decision and its exact provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingResult {
    pub task_id: String,
    pub task_revision: i64,
    pub task_spec_digest: Digest,
    pub configuration: ConfigurationBinding,
    pub request: ExecutionRequest,
    pub candidate: ExecutionCandidate,
}

impl RoutingResult {
    #[must_use]
    pub fn is_stale_against(&self, current: ConfigurationBinding) -> bool {
        !self.configuration.matches(current)
    }
}

/// Failure to select from already validated candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    NoCandidates,
}

/// Failure while constructing an Agent-specific request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBuildError {
    ConfigurationMismatch,
}

fn policy_incompatibility(
    task: &TaskSpec,
    agent: &Agent,
    configuration: &CompiledConfiguration,
) -> Option<String> {
    agent
        .actions
        .iter()
        .find_map(|action| {
            configuration
                .authorization()
                .rules
                .iter()
                .find(|rule: &&AuthorizationRule| {
                    rule.action == *action
                        && matches!(rule.effect, crate::config::model::RuleEffect::Forbid)
                })
                .map(|_| format!("current policy forbids Agent action {action:?}"))
        })
        .or_else(|| {
            task.scope.permitted_effects.iter().find_map(|effect| {
                configuration
                    .authorization()
                    .rules
                    .iter()
                    .find(|rule: &&AuthorizationRule| {
                        rule.action == *effect
                            && matches!(rule.effect, crate::config::model::RuleEffect::Forbid)
                    })
                    .map(|_| format!("current policy forbids Task effect {effect:?}"))
            })
        })
}

fn compare_candidates(left: &ExecutionCandidate, right: &ExecutionCandidate) -> Ordering {
    let priority = right.route_policy.priority.cmp(&left.route_policy.priority);
    if priority != Ordering::Equal {
        return priority;
    }
    if left.route_policy.name != right.route_policy.name {
        return left.route_policy.name.cmp(&right.route_policy.name);
    }
    for key in &left.route_policy.ordering {
        let order = match key.as_str() {
            "contextCapacity" => right
                .offer
                .context_capacity_tokens
                .cmp(&left.offer.context_capacity_tokens),
            _ => Ordering::Equal,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    let tie_break = match left.route_policy.tie_break.as_str() {
        "backendId" => left.offer.backend_id.cmp(&right.offer.backend_id),
        "agentId" => agent_order(&left.agent).cmp(&agent_order(&right.agent)),
        _ => Ordering::Equal,
    };
    if tie_break != Ordering::Equal {
        return tie_break;
    }
    (
        &left.agent.name,
        left.agent.version,
        &left.offer.backend_id,
        left.offer.descriptor_revision,
        left.offer.digest(),
    )
        .cmp(&(
            &right.agent.name,
            right.agent.version,
            &right.offer.backend_id,
            right.offer.descriptor_revision,
            right.offer.digest(),
        ))
}

fn agent_order(agent: &LogicalAgentVersion) -> (&str, u32) {
    (&agent.name, agent.version)
}

fn sorted_difference(required: &[String], available: &[String]) -> Vec<String> {
    let mut missing = required
        .iter()
        .filter(|value| !available.contains(value))
        .cloned()
        .collect::<Vec<_>>();
    missing.sort();
    missing.dedup();
    missing
}

fn missing_strings(required: &[String], available: &[String]) -> Vec<String> {
    sorted_difference(required, available)
}

fn agent_value(agent: &LogicalAgentVersion) -> Value {
    Value::object([
        ("name", Value::string(&agent.name)),
        ("version", Value::Integer(i64::from(agent.version))),
    ])
}

fn strings(values: &[String]) -> Value {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    Value::array(values.into_iter().map(Value::string))
}

fn isolation_facts(values: &[String]) -> Value {
    strings(values)
}

fn resources(values: &[ResourceQuantity]) -> Value {
    let mut values = values.to_vec();
    values.sort();
    Value::array(values.into_iter().map(|resource| {
        Value::object([
            ("name", Value::string(resource.name)),
            ("quantity", Value::Integer(resource.quantity)),
        ])
    }))
}

fn resource_quantity(values: &[ResourceQuantity], name: &str) -> Option<i64> {
    values
        .iter()
        .find(|resource| resource.name == name)
        .map(|resource| resource.quantity)
}
