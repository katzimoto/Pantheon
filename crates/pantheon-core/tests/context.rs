//! Pure-domain evidence for Issue #30: deterministic ContextPlan construction
//! over a frozen source universe.
//!
//! These tests pin the properties the canonical contract makes load-bearing:
//! byte-for-byte reproducibility, order independence, exact frozen provenance,
//! deterministic policy-driven dropping, bounded references, and the complete
//! absence of authorization/credential/host-path material from plan identity.
//! Persistence and orchestration are proven where they belong, in
//! `pantheon-store` and `pantheon-engine`.

use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value;
use pantheon_core::config::model::{ContextComponent, LogicalAgentVersion};
use pantheon_core::context::{
    AgentGuidance, ContextPlanError, FrozenSources, GuidanceSourceError, InclusionClass,
    PrecedenceStratum, SECTION_AGENT_BEHAVIOR, SECTION_AGENT_SOUL, SECTION_GOAL_CONTRACT,
    SECTION_REFERENCE_INPUT, SECTION_TASK_CONTRACT, SECTION_WORKSPACE_ORIENTATION,
    apply_optional_drop, build_context_plan, frozen_agent_guidance, guidance_digest,
};
use pantheon_core::planning::TaskSpec;
use pantheon_core::planning::task::{
    AcceptanceContract, AcceptanceCriterion, Severity, TaskInput, TaskOutput, TaskScope,
};
use pantheon_core::scheduling::ContextSourceSnapshot;

const SOUL: &str = "A careful coding agent that protects operator trust.";
const BEHAVIOR: &str = "Plan before editing. Keep changes minimal.";

fn task_spec(goal_id: &str, inputs: Vec<TaskInput>) -> TaskSpec {
    TaskSpec {
        task_type: "code.change".to_string(),
        objective: "Fix the checkout timeout with the smallest safe change.".to_string(),
        inputs,
        outputs: vec![TaskOutput {
            name: "changeset".to_string(),
            kind: "code.changeset".to_string(),
            required: true,
        }],
        competencies: vec!["code.editing".to_string()],
        scope: TaskScope {
            resources: vec!["workspace://src/**".to_string()],
            permitted_effects: vec!["filesystem.write".to_string()],
            forbidden_effects: vec![],
        },
        acceptance: AcceptanceContract {
            criteria: vec![AcceptanceCriterion {
                id: "unit-tests".to_string(),
                statement: "the suite passes".to_string(),
                evaluator_ref: "check://project/unit-tests".to_string(),
                evaluator_version: "unit-tests-v1".to_string(),
                severity: Severity::Required,
            }],
            evaluator_registry_digest: Digest::of(b"registry"),
            configuration_activation_sequence: 43,
        },
        goal_id: goal_id.to_string(),
        goal_revision: 1,
    }
}

fn snapshot(spec: &TaskSpec) -> ContextSourceSnapshot {
    ContextSourceSnapshot {
        task_spec_digest: spec.digest(),
        goal_id: spec.goal_id.clone(),
        goal_revision: spec.goal_revision,
        graph_revision: 47,
        agent: LogicalAgentVersion {
            name: "builder".to_string(),
            version: 1,
        },
        configuration_activation_sequence: 43,
        context_policy_digest: Digest::of(b"context-policy"),
        agent_soul_digest: guidance_digest(SOUL),
        agent_behavior_digest: guidance_digest(BEHAVIOR),
        workspace_id: "ws-1".to_string(),
        workspace_resolved_base: "a".repeat(40),
    }
}

fn policy() -> ContextComponent {
    ContextComponent {
        schema_version: 1,
        mandatory_sections: vec![
            SECTION_TASK_CONTRACT.to_string(),
            SECTION_GOAL_CONTRACT.to_string(),
            SECTION_AGENT_SOUL.to_string(),
            SECTION_AGENT_BEHAVIOR.to_string(),
        ],
        preload_priority: vec![SECTION_WORKSPACE_ORIENTATION.to_string()],
        memory_limit_tokens: 4000,
        workspace_orientation_limit_tokens: 2000,
        safety_margin_tokens: 512,
        optional_drop_order: vec![SECTION_WORKSPACE_ORIENTATION.to_string()],
    }
}

fn sources<'a>(spec: &'a TaskSpec, goal_digest: Digest) -> FrozenSources<'a> {
    FrozenSources {
        task_spec: spec,
        goal_content_digest: goal_digest,
        soul: SOUL,
        behavior: BEHAVIOR,
        workspace_repository: "repo://project",
    }
}

fn build(spec: &TaskSpec) -> pantheon_core::context::ContextPlan {
    build_context_plan(
        &snapshot(spec),
        &sources(spec, Digest::of(b"goal-content")),
        &policy(),
    )
    .expect("the frozen sources satisfy the policy")
}

/// String field lookup over a canonical object value.
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value.get(key) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

#[test]
fn the_same_frozen_sources_produce_byte_identical_content_and_the_same_digest() {
    // Core invariant: the plan is a deterministic function of its inputs. Two
    // preparations of the same Run — possibly in different processes after a
    // restart — must agree bit for bit, or attachment reconciliation could
    // never converge.
    let spec = task_spec(
        "goal-1",
        vec![TaskInput {
            name: "repository".to_string(),
            reference: "repo://project".to_string(),
        }],
    );
    let first = build(&spec);
    let second = build(&spec);

    let first_bytes = first.to_value().to_canonical_bytes();
    let second_bytes = second.to_value().to_canonical_bytes();
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(first.digest(), second.digest());
}

#[test]
fn selection_ordering_and_dropping_ignore_collection_order() {
    // Core invariant: neither the eligible set's order nor the caller's
    // iteration order may influence selection. The drop machinery is the one
    // piece that consumes an unordered candidate list, so it is pinned here
    // with a shuffled input set and a synthetic capacity measurement.
    let make = |name: &str| pantheon_core::context::ContextSection {
        kind: SECTION_WORKSPACE_ORIENTATION,
        key: name.to_string(),
        inclusion: InclusionClass::Preload,
        precedence: PrecedenceStratum::ReferenceData,
        provenance: Value::object([("id", Value::string(name))]),
        instruction: None,
    };
    let mut ordered = vec![make("a"), make("b"), make("c")];
    ordered.sort_by(|left, right| left.order_key().cmp(&right.order_key()));

    // Capacity fits nothing: every preload section drops, highest-ordered
    // first, whatever order the candidates arrived in.
    let mut reversed = ordered.clone();
    reversed.reverse();
    for candidates in [ordered.clone(), reversed] {
        let (kept, dropped) = apply_optional_drop(
            &candidates,
            &[SECTION_WORKSPACE_ORIENTATION.to_string()],
            |_| true,
        );
        assert!(kept.is_empty());
        let dropped_keys: Vec<_> = dropped.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(
            dropped_keys,
            ["c", "b", "a"],
            "drop order runs from the tail of the total order inside a tier"
        );
        assert!(dropped.iter().all(|d| d.reason == "policy-drop-order"));
    }

    // A budget that fits exactly two drops the lowest-ordered survivor first
    // when pressure appears, and never touches mandatory content.
    let mut sections = vec![make("a"), make("b")];
    sections.push(pantheon_core::context::ContextSection {
        kind: SECTION_TASK_CONTRACT,
        key: String::new(),
        inclusion: InclusionClass::Mandatory,
        precedence: PrecedenceStratum::GoalTaskContract,
        provenance: Value::Null,
        instruction: None,
    });
    let (kept, dropped) = apply_optional_drop(
        &sections,
        &[SECTION_WORKSPACE_ORIENTATION.to_string()],
        |kept| {
            kept.iter()
                .filter(|s| s.inclusion == InclusionClass::Preload)
                .count()
                > 1
        },
    );
    assert_eq!(dropped.len(), 1);
    assert_eq!(
        dropped[0].key, "b",
        "the least important survivor drops first: canonical order follows the \
         frozen policy's priority, so its tail goes"
    );
    assert!(
        kept.iter()
            .any(|s| s.inclusion == InclusionClass::Mandatory),
        "mandatory content is never dropped"
    );
}

#[test]
fn the_plan_binds_the_exact_frozen_snapshot_and_policy() {
    // Provenance invariant: the plan records which source universe and which
    // frozen policy produced it, so attachment can prove plan↔Run↔snapshot
    // equality relationally instead of by trust.
    let spec = task_spec("goal-1", vec![]);
    let snap = snapshot(&spec);
    let plan =
        build_context_plan(&snap, &sources(&spec, Digest::of(b"goal")), &policy()).expect("builds");
    assert_eq!(plan.source_snapshot_digest, snap.digest());
    assert_eq!(plan.context_policy_digest, snap.context_policy_digest);

    // And the binding is part of content identity: a plan claiming another
    // snapshot or another policy is a different plan.
    let mut other_snapshot = snap.clone();
    other_snapshot.graph_revision += 1;
    let other = build_context_plan(
        &other_snapshot,
        &sources(&spec, Digest::of(b"goal")),
        &policy(),
    )
    .expect("builds");
    assert_ne!(other.digest(), plan.digest());
}

#[test]
fn provenance_comes_from_the_frozen_identities_not_any_current_pointer() {
    // Every provenance field must be traceable to something T3 froze. Nothing
    // here reads or names current state: no activation pointer beyond the
    // frozen sequence, no latest generation, no wall clock.
    let spec = task_spec("goal-1", vec![]);
    let plan = build(&spec);
    assert_eq!(plan.task_spec_digest, spec.digest());
    assert_eq!(plan.goal_id, "goal-1");
    assert_eq!(plan.goal_revision, 1);
    assert_eq!(plan.graph_revision, 47);
    assert_eq!(
        plan.agent,
        LogicalAgentVersion {
            name: "builder".to_string(),
            version: 1
        }
    );
    assert_eq!(plan.agent_soul_digest, guidance_digest(SOUL));
    assert_eq!(plan.agent_behavior_digest, guidance_digest(BEHAVIOR));
    assert_eq!(plan.workspace_id, "ws-1");
    assert_eq!(plan.workspace_resolved_base, "a".repeat(40));

    let value = plan.to_value();
    let task = value.get("task").expect("task provenance");
    assert_eq!(
        text(task, "specDigest"),
        Some(spec.digest().to_string()).as_deref(),
        "the plan names the frozen spec digest, not a re-resolution"
    );
    assert_eq!(text(task, "goalId"), Some("goal-1"));
    let builder = value.get("builder").expect("builder provenance");
    assert_eq!(
        text(builder, "contextPolicyDigest"),
        // The plan records the digest frozen at T3 — copied from the snapshot
        // identity, never recomputed from a caller-supplied policy object.
        Some(Digest::of(b"context-policy").to_string()).as_deref()
    );
}

#[test]
fn a_mandatory_section_the_builder_cannot_produce_fails_closed() {
    // Policy is authority, not decoration: if the frozen policy demands a
    // mandatory section this build cannot select, preparation must fail
    // rather than ship a plan that violates its own frozen policy.
    let spec = task_spec("goal-1", vec![]);
    let mut demanding = policy();
    demanding
        .mandatory_sections
        .push("continuation".to_string());
    let err = build_context_plan(
        &snapshot(&spec),
        &sources(&spec, Digest::of(b"goal")),
        &demanding,
    )
    .expect_err("an unproducible mandatory section fails");
    assert_eq!(
        err,
        ContextPlanError::MandatorySectionUnsatisfiable {
            section: "continuation".to_string()
        }
    );

    // The inverse direction stays legal: the policy may require fewer kinds
    // than the builder produces.
    let mut narrow = policy();
    narrow.mandatory_sections = vec![SECTION_TASK_CONTRACT.to_string()];
    build_context_plan(
        &snapshot(&spec),
        &sources(&spec, Digest::of(b"goal")),
        &narrow,
    )
    .expect("a subset of producible sections satisfies the policy");
}

#[test]
fn large_inputs_stay_bounded_references_not_embedded_bodies() {
    // Bounded-context invariant: required inputs are discoverable references;
    // the plan never embeds arbitrary repository/input bodies, and workspace
    // orientation carries identity metadata rather than a captured tree.
    let long_reference = format!("artifact://sha256/{}", "e".repeat(64));
    let spec = task_spec(
        "goal-1",
        vec![
            TaskInput {
                name: "findings".to_string(),
                reference: long_reference.clone(),
            },
            TaskInput {
                name: "baseline".to_string(),
                reference: "artifact://sha256/".to_string() + &"f".repeat(64),
            },
        ],
    );
    let plan = build(&spec);
    let input_sections: Vec<_> = plan
        .sections
        .iter()
        .filter(|s| s.kind == SECTION_REFERENCE_INPUT)
        .collect();
    assert_eq!(input_sections.len(), 2);
    for section in &input_sections {
        assert_eq!(section.inclusion, InclusionClass::OnDemand);
        assert!(
            section.instruction.is_none(),
            "reference sections carry no body"
        );
    }
    // Sorted by key regardless of declaration order.
    assert_eq!(input_sections[0].key, "baseline");
    assert_eq!(input_sections[1].key, "findings");
    let findings = input_sections[1].to_value();
    assert_eq!(
        text(findings.get("provenance").expect("provenance"), "ref"),
        Some(long_reference.as_str()),
        "the reference itself is the payload"
    );

    // Orientation stays metadata: id, repository reference, immutable base.
    let orientation = plan
        .sections
        .iter()
        .find(|s| s.kind == SECTION_WORKSPACE_ORIENTATION)
        .expect("orientation present");
    assert_eq!(orientation.inclusion, InclusionClass::Preload);
    assert_eq!(orientation.precedence, PrecedenceStratum::ReferenceData);
    let provenance = orientation.to_value();
    let provenance = provenance.get("provenance").unwrap();
    assert_eq!(text(provenance, "workspaceId"), Some("ws-1"));
    assert_eq!(
        text(provenance, "resolvedBase"),
        Some("a".repeat(40).as_str())
    );
}

#[test]
fn no_authorization_or_secret_or_host_path_material_enter_plan_identity() {
    // Security invariants: actions and effects are availability, never
    // permission; credentials have no place here at all; and the
    // controller-side host root is provenance Pantheon never exposes through
    // a plan. The canonical encoding is searched as bytes so any future field
    // that leaks one of these fails this test.
    let spec = task_spec("goal-1", vec![]);
    let plan = build(&spec);
    let canonical = String::from_utf8(plan.to_value().to_canonical_bytes()).expect("utf-8");

    for forbidden in [
        "\"filesystem\"",
        "shell.execute",
        "\"effect\"",
        "forbid",
        "permit",
        "Bearer ",
        "/tmp/",
        "source_path",
        "restore_generation",
    ] {
        assert!(
            !canonical.contains(forbidden),
            "plan identity must not contain {forbidden:?}:\n{canonical}"
        );
    }
}

#[test]
fn frozen_agent_guidance_is_extracted_per_exact_version() {
    let component = pantheon_core::config::parse::parse(&format!(
        r#"{{"agents":[
            {{"name":"builder","version":1,"soul":{soul},"behavior":{behavior}}},
            {{"name":"builder","version":2,"soul":"newer soul","behavior":"newer behavior"}}]}}"#,
        soul = Value::string(SOUL),
        behavior = Value::string(BEHAVIOR),
    ))
    .expect("fixture parses");

    // The frozen version extracts its own guidance, never the newest.
    let v1 = LogicalAgentVersion {
        name: "builder".to_string(),
        version: 1,
    };
    assert_eq!(
        frozen_agent_guidance(&component, &v1).expect("v1 exists"),
        AgentGuidance {
            soul: SOUL.to_string(),
            behavior: BEHAVIOR.to_string(),
        }
    );

    // A version absent from the frozen component fails closed with the typed
    // absence error — it is never satisfied from a different version.
    let ghost = LogicalAgentVersion {
        name: "ghost".to_string(),
        version: 9,
    };
    match frozen_agent_guidance(&component, &ghost) {
        Err(GuidanceSourceError::VersionAbsent { agent }) => {
            assert_eq!(agent.name, "ghost");
            assert_eq!(agent.version, 9);
        }
        other => panic!("expected VersionAbsent, got {other:?}"),
    }

    // And a malformed component is a malformed source, not an empty result.
    let broken = pantheon_core::config::parse::parse(r#"{"agents":[]}"#).expect("parses");
    assert!(matches!(
        frozen_agent_guidance(&broken, &v1),
        Err(GuidanceSourceError::VersionAbsent { .. })
    ));
}

#[test]
fn a_frozen_snapshot_round_trips_through_its_canonical_json() {
    // Reconstruction depends on decoding stored bytes back into the typed
    // snapshot. The round trip must preserve identity exactly.
    let spec = task_spec("goal-1", vec![]);
    let snap = snapshot(&spec);
    let canonical = String::from_utf8(snap.to_value().to_canonical_bytes()).expect("utf-8");
    let decoded = ContextSourceSnapshot::from_canonical_json(&canonical).expect("decodes");
    assert_eq!(decoded, snap);
    assert_eq!(decoded.digest(), snap.digest());
}

#[test]
fn guidance_digests_are_canonical_content_addresses() {
    // Guidance identity uses the canonical value mechanism like everything
    // else, so the same body always yields the same digest and different
    // bodies never collide here.
    assert_eq!(guidance_digest(SOUL), Value::string(SOUL).digest());
    assert_ne!(guidance_digest(SOUL), guidance_digest(BEHAVIOR));
    assert_ne!(guidance_digest(SOUL), guidance_digest(&format!("{SOUL}\n")));
}
