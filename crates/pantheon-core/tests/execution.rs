//! Architecture-level evidence for Issue #28's pure routing boundary.

mod common;

use common::VALID_SOURCE;
use pantheon_core::config::Digest;
use pantheon_core::config::compile::compile;
use pantheon_core::execution::{
    AgentResolutionError, BackendDescriptor, CandidateRejection, ConfigurationBinding,
    ControllerSafetyFacts, ExecutionOffer, LaunchSemantics, ResolvedAgent, ResourceQuantity,
    build_execution_request, resolve_agents, select_execution_candidate,
    validate_execution_candidate,
};
use pantheon_core::planning::task::{
    AcceptanceContract, AcceptanceCriterion, Severity, TaskInput, TaskOutput, TaskScope, TaskSpec,
};

fn task(task_type: &str, competencies: &[&str]) -> TaskSpec {
    TaskSpec {
        task_type: task_type.to_string(),
        objective: "perform the bounded test task".to_string(),
        inputs: vec![TaskInput {
            name: "input".to_string(),
            reference: "artifact://input".to_string(),
        }],
        outputs: vec![TaskOutput {
            name: "report".to_string(),
            kind: "report".to_string(),
            required: true,
        }],
        competencies: competencies
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        scope: TaskScope {
            resources: vec!["workspace://src/**".to_string()],
            permitted_effects: vec!["filesystem.read".to_string()],
            forbidden_effects: Vec::new(),
        },
        acceptance: AcceptanceContract {
            criteria: vec![AcceptanceCriterion {
                id: "complete".to_string(),
                statement: "the task is complete".to_string(),
                evaluator_ref: "check://task".to_string(),
                evaluator_version: "check-v1".to_string(),
                severity: Severity::Required,
            }],
            evaluator_registry_digest: Digest::of(b"evaluator"),
            configuration_activation_sequence: 1,
        },
        goal_id: "goal-1".to_string(),
        goal_revision: 1,
    }
}

fn binding(compiled: &pantheon_core::config::CompiledConfiguration) -> ConfigurationBinding {
    ConfigurationBinding::new(1, compiled.revision_digest(), compiled.component_digests())
}

fn safety(observational_launch_safe: bool) -> ControllerSafetyFacts {
    ControllerSafetyFacts {
        isolation_guarantees: vec!["isolation.control-plane".to_string()],
        observational_launch_safe,
    }
}

fn resolved_agent() -> (pantheon_core::config::CompiledConfiguration, ResolvedAgent) {
    let compiled = compile(VALID_SOURCE).expect("fixture compiles");
    let result = resolve_agents(
        &task("code-change", &["rust"]),
        &compiled,
        binding(&compiled),
    )
    .expect("the builder is eligible");
    (
        compiled,
        result.eligible.into_iter().next().expect("one Agent"),
    )
}

fn descriptor(
    backend_id: &str,
    revision: u64,
    launch_semantics: LaunchSemantics,
) -> BackendDescriptor {
    BackendDescriptor {
        backend_id: backend_id.to_string(),
        revision,
        available_for_offers: true,
        placement: vec!["local".to_string()],
        supported_execution_features: vec!["exec.shell".to_string()],
        context_capacity_tokens: 16_000,
        isolation_facts: vec!["isolation.control-plane".to_string()],
        resources: Vec::new(),
        launch_semantics,
    }
}

fn offer(
    request: &pantheon_core::execution::ExecutionRequest,
    backend_id: &str,
    revision: u64,
    launch_semantics: LaunchSemantics,
) -> ExecutionOffer {
    ExecutionOffer {
        request_digest: request.digest(),
        backend_id: backend_id.to_string(),
        descriptor_revision: revision,
        descriptor_digest: descriptor(backend_id, revision, launch_semantics).digest(),
        supported_execution_features: vec!["exec.shell".to_string()],
        context_capacity_tokens: 16_000,
        placement: vec!["local".to_string()],
        isolation_facts: vec!["isolation.control-plane".to_string()],
        resources: Vec::new(),
        launch_semantics,
        offer_reference: format!("opaque-{backend_id}-{revision}"),
    }
}

#[test]
fn an_eligible_agent_is_resolved_from_task_type_and_competencies() {
    let compiled = compile(VALID_SOURCE).expect("fixture compiles");
    let result = resolve_agents(
        &task("code-change", &["rust"]),
        &compiled,
        binding(&compiled),
    )
    .expect("the builder is eligible");

    assert_eq!(result.eligible[0].identity.name, "builder");
    assert_eq!(result.eligible[0].identity.version, 1);
    assert!(result.rejected.is_empty());
}

#[test]
fn unsupported_task_type_is_a_structured_rejection() {
    let compiled = compile(VALID_SOURCE).expect("fixture compiles");
    let error = resolve_agents(
        &task("research-code", &["rust"]),
        &compiled,
        binding(&compiled),
    )
    .expect_err("the Agent does not accept this type");

    assert!(matches!(
        error,
        AgentResolutionError::NoEligibleAgent { causes, .. }
            if matches!(causes[0].reason, pantheon_core::execution::AgentRejectionReason::UnsupportedTaskType { .. })
    ));
}

#[test]
fn missing_competency_is_a_structured_rejection_without_fallback() {
    let compiled = compile(VALID_SOURCE).expect("fixture compiles");
    let error = resolve_agents(
        &task("code-change", &["rust", "security.audit"]),
        &compiled,
        binding(&compiled),
    )
    .expect_err("no incompatible fallback is allowed");

    assert!(matches!(
        error,
        AgentResolutionError::NoEligibleAgent { causes, .. }
            if matches!(
                causes[0].reason,
                pantheon_core::execution::AgentRejectionReason::MissingCompetencies { ref missing }
                    if missing == &["security.audit".to_string()]
            )
    ));
}

#[test]
fn disabled_and_non_current_versions_are_not_inferred_from_order() {
    let source = VALID_SOURCE.replacen(
        r#""version": 1,"#,
        r#""version": 1, "enabled": false, "current": true,"#,
        1,
    );
    let compiled = compile(&source).expect("fixture compiles");
    let error = resolve_agents(
        &task("code-change", &["rust"]),
        &compiled,
        binding(&compiled),
    )
    .expect_err("a disabled current Agent is not eligible");
    assert!(matches!(
        error,
        AgentResolutionError::NoEligibleAgent { causes, .. }
            if matches!(causes[0].reason, pantheon_core::execution::AgentRejectionReason::Disabled)
    ));

    let old = VALID_SOURCE.replacen(
        r#""agents": ["#,
        r#""agents": [{"name":"builder","version":2,"enabled":true,"current":false,"accepts":["code-change"],"competencies":["rust"],"routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,"sandboxProfile":"strict-local-container","sandboxRequirements":["isolation.control-plane"],"actions":["filesystem.read"]},"#,
        1,
    );
    let compiled = compile(&old).expect("version history compiles");
    let result = resolve_agents(
        &task("code-change", &["rust"]),
        &compiled,
        binding(&compiled),
    )
    .expect("the current version remains eligible");
    assert_eq!(result.eligible.len(), 1);
    assert!(result.rejected.iter().any(|rejection| matches!(
        rejection.reason,
        pantheon_core::execution::AgentRejectionReason::NotCurrent
    )));
}

#[test]
fn pins_and_exclusions_are_configuration_authority() {
    let pinned = VALID_SOURCE.replacen(
        r#""policies": ["#,
        r#""agentPins":[{"name":"builder","version":1}],"policies": ["#,
        1,
    );
    let compiled = compile(&pinned).expect("pin compiles");
    assert!(
        resolve_agents(
            &task("code-change", &["rust"]),
            &compiled,
            binding(&compiled)
        )
        .is_ok()
    );

    let excluded = pinned.replacen(
        r#""agentPins":[{"name":"builder","version":1}]"#,
        r#""agentPins":[],"agentExclusions":[{"name":"builder","version":1}]"#,
        1,
    );
    let compiled = compile(&excluded).expect("exclusion compiles");
    assert!(matches!(
        resolve_agents(&task("code-change", &["rust"]), &compiled, binding(&compiled)),
        Err(AgentResolutionError::NoEligibleAgent { causes, .. })
            if matches!(causes[0].reason, pantheon_core::execution::AgentRejectionReason::Excluded)
    ));
}

#[test]
fn request_identity_is_provider_neutral_and_agent_specific() {
    let (compiled, agent) = resolved_agent();
    let request = build_execution_request(
        "task-1",
        &task("code-change", &["rust"]),
        &agent,
        &compiled,
        binding(&compiled),
    )
    .expect("request builds");
    let encoded = request.to_value().to_string();

    assert_eq!(request.agent.name, "builder");
    assert!(!encoded.contains("provider"));
    assert!(!encoded.contains("model"));
    assert!(!encoded.contains("harness"));
    assert!(!encoded.contains("runtime"));
}

#[test]
fn compatible_offer_becomes_an_agent_offer_candidate() {
    let (compiled, agent) = resolved_agent();
    let request = build_execution_request(
        "task-1",
        &task("code-change", &["rust"]),
        &agent,
        &compiled,
        binding(&compiled),
    )
    .expect("request builds");
    let keyed_descriptor = descriptor("fake-local", 1, LaunchSemantics::KeyedIdempotent);
    let candidate = validate_execution_candidate(
        &request,
        &agent,
        &keyed_descriptor,
        &offer(&request, "fake-local", 1, LaunchSemantics::KeyedIdempotent),
        true,
        &safety(false),
    )
    .expect("facts are compatible");

    assert_eq!(candidate.agent.name, "builder");
    assert_eq!(candidate.offer.backend_id, "fake-local");
}

#[test]
fn feature_capacity_isolation_and_launch_mismatches_fail_closed() {
    let (compiled, agent) = resolved_agent();
    let request = build_execution_request(
        "task-1",
        &task("code-change", &["rust"]),
        &agent,
        &compiled,
        binding(&compiled),
    )
    .expect("request builds");

    let mut missing_feature = request.clone();
    missing_feature
        .required_execution_features
        .push("exec.structured".to_string());
    let mut feature_offer = offer(
        &missing_feature,
        "fake-local",
        1,
        LaunchSemantics::KeyedIdempotent,
    );
    let keyed_descriptor = descriptor("fake-local", 1, LaunchSemantics::KeyedIdempotent);
    let mut feature_descriptor = keyed_descriptor.clone();
    feature_descriptor
        .supported_execution_features
        .push("exec.structured".to_string());
    feature_offer.descriptor_digest = feature_descriptor.digest();
    assert!(matches!(
        validate_execution_candidate(
            &missing_feature,
            &agent,
            &feature_descriptor,
            &feature_offer,
            true,
            &safety(false)
        ),
        Err(CandidateRejection::MissingExecutionFeatures { .. })
    ));

    let mut small = offer(&request, "fake-local", 1, LaunchSemantics::KeyedIdempotent);
    small.context_capacity_tokens = 1;
    assert!(matches!(
        validate_execution_candidate(
            &request,
            &agent,
            &keyed_descriptor,
            &small,
            true,
            &safety(false)
        ),
        Err(CandidateRejection::ContextCapacityTooSmall { .. })
    ));

    let mut misplaced = offer(&request, "fake-local", 1, LaunchSemantics::KeyedIdempotent);
    misplaced.placement = vec!["remote".to_string()];
    let mut placement_request = request.clone();
    placement_request.placement_constraints = vec!["local".to_string()];
    misplaced.request_digest = placement_request.digest();
    assert!(matches!(
        validate_execution_candidate(
            &placement_request,
            &agent,
            &keyed_descriptor,
            &misplaced,
            true,
            &safety(false)
        ),
        Err(CandidateRejection::PlacementIncompatible { .. })
    ));

    let mut resource_request = request.clone();
    resource_request.resource_requirements = vec![ResourceQuantity {
        name: "cpu".to_string(),
        quantity: 2,
    }];
    let mut resource_descriptor = keyed_descriptor.clone();
    resource_descriptor.resources = vec![ResourceQuantity {
        name: "cpu".to_string(),
        quantity: 1,
    }];
    let mut resource_offer = offer(
        &resource_request,
        "fake-local",
        1,
        LaunchSemantics::KeyedIdempotent,
    );
    resource_offer.descriptor_digest = resource_descriptor.digest();
    resource_offer.resources = resource_descriptor.resources.clone();
    assert!(matches!(
        validate_execution_candidate(
            &resource_request,
            &agent,
            &resource_descriptor,
            &resource_offer,
            true,
            &safety(false)
        ),
        Err(CandidateRejection::ResourceIncompatible { .. })
    ));

    let unproven = offer(&request, "fake-local", 1, LaunchSemantics::KeyedIdempotent);
    let no_isolation_safety = ControllerSafetyFacts {
        isolation_guarantees: Vec::new(),
        observational_launch_safe: false,
    };
    assert!(matches!(
        validate_execution_candidate(
            &request,
            &agent,
            &keyed_descriptor,
            &unproven,
            true,
            &no_isolation_safety
        ),
        Err(CandidateRejection::IsolationNotProven { .. })
    ));

    let observational = offer(&request, "fake-local", 1, LaunchSemantics::Observational);
    let observational_descriptor = descriptor("fake-local", 1, LaunchSemantics::Observational);
    assert!(matches!(
        validate_execution_candidate(
            &request,
            &agent,
            &observational_descriptor,
            &observational,
            true,
            &safety(false)
        ),
        Err(CandidateRejection::UnsafeLaunchSemantics)
    ));

    let stale = offer(&request, "fake-local", 2, LaunchSemantics::KeyedIdempotent);
    assert!(matches!(
        validate_execution_candidate(
            &request,
            &agent,
            &keyed_descriptor,
            &stale,
            true,
            &safety(false)
        ),
        Err(CandidateRejection::DescriptorRevisionMismatch)
    ));

    let mut wrong_request = offer(&request, "fake-local", 1, LaunchSemantics::KeyedIdempotent);
    wrong_request.request_digest = Digest::of(b"another-request");
    assert!(matches!(
        validate_execution_candidate(
            &request,
            &agent,
            &keyed_descriptor,
            &wrong_request,
            true,
            &safety(false)
        ),
        Err(CandidateRejection::OfferRequestMismatch)
    ));
}

#[test]
fn observational_launch_is_allowed_only_when_the_route_policy_says_so() {
    let source = VALID_SOURCE.replacen(
        r#""tieBreak": "backendId""#,
        r#""tieBreak": "backendId", "requiresKeyedLaunch": false"#,
        1,
    );
    let compiled = compile(&source).expect("route policy compiles");
    let agent = resolve_agents(
        &task("code-change", &["rust"]),
        &compiled,
        binding(&compiled),
    )
    .expect("Agent resolves")
    .eligible
    .into_iter()
    .next()
    .expect("one Agent");
    let request = build_execution_request(
        "task-1",
        &task("code-change", &["rust"]),
        &agent,
        &compiled,
        binding(&compiled),
    )
    .expect("request builds");
    let observational = offer(&request, "fake-local", 1, LaunchSemantics::Observational);
    let candidate = validate_execution_candidate(
        &request,
        &agent,
        &descriptor("fake-local", 1, LaunchSemantics::Observational),
        &observational,
        true,
        &safety(true),
    )
    .expect("the route policy permits observational semantics");
    assert_eq!(
        candidate.offer.launch_semantics,
        LaunchSemantics::Observational
    );
}

#[test]
fn deterministic_selection_ignores_candidate_insertion_order() {
    let (compiled, agent) = resolved_agent();
    let request = build_execution_request(
        "task-1",
        &task("code-change", &["rust"]),
        &agent,
        &compiled,
        binding(&compiled),
    )
    .expect("request builds");
    let a_descriptor = descriptor("a-local", 1, LaunchSemantics::KeyedIdempotent);
    let z_descriptor = descriptor("z-local", 1, LaunchSemantics::KeyedIdempotent);
    let a = validate_execution_candidate(
        &request,
        &agent,
        &a_descriptor,
        &offer(&request, "a-local", 1, LaunchSemantics::KeyedIdempotent),
        true,
        &safety(false),
    )
    .expect("a candidate");
    let z = validate_execution_candidate(
        &request,
        &agent,
        &z_descriptor,
        &offer(&request, "z-local", 1, LaunchSemantics::KeyedIdempotent),
        true,
        &safety(false),
    )
    .expect("z candidate");

    let first = select_execution_candidate(&[z.clone(), a.clone()]).expect("selection");
    let second = select_execution_candidate(&[a, z]).expect("selection");
    assert_eq!(first.offer.backend_id, "a-local");
    assert_eq!(first, second);
}

#[test]
fn configuration_binding_detects_a_later_activation() {
    let compiled = compile(VALID_SOURCE).expect("fixture compiles");
    let first = binding(&compiled);
    let later = ConfigurationBinding::new(
        first.activation_sequence + 1,
        first.content_digest,
        first.component_digests,
    );
    assert!(!first.matches(later));
}
