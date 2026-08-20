//! Executable evidence for Issue #23 compilation: the pipeline a candidate
//! must complete, the stability of canonical identity, and the rejection of
//! configuration that parses but cannot mean anything.

mod common;

use common::{VALID_SOURCE, variant};
use pantheon_core::config::ConfigError;
use pantheon_core::config::compile::compile;
use pantheon_core::config::revision::COMPILER_VERSION;

#[test]
fn the_mvp_source_compiles_to_the_components_the_slice_needs() {
    let compiled = compile(VALID_SOURCE).expect("the MVP configuration compiles");

    assert_eq!(compiled.agents().agents.len(), 1);
    assert_eq!(compiled.agents().agents[0].name, "builder");
    assert_eq!(compiled.routing().policies.len(), 1);
    // The strict local container profile plus a separate verification profile,
    // because an evaluator never runs in the producer Run's sandbox.
    assert_eq!(compiled.execution().profiles.len(), 2);
    // Fake and production-local registrations, as the mission requires.
    assert_eq!(compiled.execution().backends.len(), 2);
    assert_eq!(compiled.evaluators().refs.len(), 1);
    assert_eq!(compiled.context().schema_version, 1);
    assert_eq!(compiled.authorization().rules.len(), 2);
}

#[test]
fn every_component_has_a_distinct_digest() {
    // The contract forbids one ambiguous policy hash: a later immutable
    // decision must be able to bind exactly the semantic generation it used.
    let digests = compile(VALID_SOURCE).expect("compiles").component_digests();
    let all = [
        digests.agents,
        digests.routing,
        digests.execution_profile,
        digests.evaluator_registry,
        digests.context_policy,
        digests.authorization,
    ];
    for (index, left) in all.iter().enumerate() {
        for right in all.iter().skip(index + 1) {
            assert_ne!(left, right, "components must not share a digest");
        }
    }
}

#[test]
fn source_formatting_does_not_change_semantic_identity() {
    // Issue #23: formatting differences that do not alter compiled semantics
    // must not create a different semantic identity.
    let reference = compile(VALID_SOURCE).expect("compiles");

    // Whitespace and key order differ; semantics do not.
    let reformatted = VALID_SOURCE.replace("\n", " ").replace("  ", " ").replace(
        r#"{ "name": "default", "ordering": ["contextCapacity"], "tieBreak": "backendId" }"#,
        r#"{"tieBreak":"backendId","ordering":["contextCapacity"],"name":"default"}"#,
    );
    let other = compile(&reformatted).expect("the reformatted source compiles");

    assert_eq!(
        reference.revision_digest(),
        other.revision_digest(),
        "reformatting must not change the revision identity"
    );
    assert_eq!(reference.component_digests(), other.component_digests());
}

#[test]
fn a_semantic_change_changes_the_owning_component_and_the_revision() {
    let reference = compile(VALID_SOURCE).expect("compiles");
    let changed = compile(&variant(
        r#""memoryLimitTokens": 4000"#,
        r#""memoryLimitTokens": 4096"#,
    ))
    .expect("the changed source compiles");

    let before = reference.component_digests();
    let after = changed.component_digests();

    assert_ne!(
        before.context_policy, after.context_policy,
        "the context component changed"
    );
    assert_ne!(
        reference.revision_digest(),
        changed.revision_digest(),
        "the revision identity follows its components"
    );
    // And only that component moved: an unrelated decision bound to routing
    // must not be invalidated by a context change.
    assert_eq!(before.agents, after.agents);
    assert_eq!(before.routing, after.routing);
    assert_eq!(before.evaluator_registry, after.evaluator_registry);
    assert_eq!(before.authorization, after.authorization);
    assert_eq!(before.execution_profile, after.execution_profile);
}

#[test]
fn malformed_source_is_a_typed_configuration_failure() {
    let err = compile("{ this is not json }").expect_err("malformed source is rejected");
    assert_eq!(err.kind(), "malformed", "unexpected: {err}");
}

#[test]
fn a_missing_component_is_reported_by_path() {
    let err = compile(r#"{"agents":[]}"#).expect_err("an incomplete candidate is rejected");
    assert!(
        matches!(err, ConfigError::MissingField { ref path } if path == "routing"),
        "unexpected: {err}"
    );
}

#[test]
fn an_agent_referencing_an_undeclared_route_policy_is_rejected() {
    // Rule 1: syntactically valid, internally inconsistent.
    let err = compile(&variant(
        r#""routePolicy": "default""#,
        r#""routePolicy": "nonexistent""#,
    ))
    .expect_err("an unknown route policy is rejected");
    assert!(
        matches!(
            err,
            ConfigError::UnknownReference { kind: "route policy", ref id, .. } if id == "nonexistent"
        ),
        "unexpected: {err}"
    );
}

#[test]
fn an_agent_referencing_an_undeclared_sandbox_profile_is_rejected() {
    let err = compile(&variant(
        r#""sandboxProfile": "strict-local-container""#,
        r#""sandboxProfile": "nonexistent""#,
    ))
    .expect_err("an unknown sandbox profile is rejected");
    assert!(
        matches!(
            err,
            ConfigError::UnknownReference {
                kind: "sandbox profile",
                ..
            }
        ),
        "unexpected: {err}"
    );
}

#[test]
fn a_profile_that_cannot_meet_its_agents_requirement_is_rejected() {
    // Rule 2: a profile name is desired policy, not proof. Dropping the
    // guarantee leaves an Agent that can never be placed.
    let err = compile(&variant(
        r#""guarantees": ["isolation.control-plane", "isolation.peer-workspaces"]"#,
        r#""guarantees": ["isolation.peer-workspaces"]"#,
    ))
    .expect_err("a profile missing a required guarantee is rejected");
    assert!(
        matches!(err, ConfigError::IncompatibleCombination { .. }),
        "unexpected: {err}"
    );
}

#[test]
fn a_shell_capable_agent_cannot_run_on_a_trusted_host_profile() {
    // Rule 3: arbitrary shell execution requires control-plane isolation.
    let source = variant(
        r#""isolationClass": "CONTAINER""#,
        r#""isolationClass": "TRUSTED_HOST""#,
    );
    let err = compile(&source).expect_err("shell on a trusted host is rejected");
    assert!(
        matches!(err, ConfigError::IncompatibleCombination { ref detail } if detail.contains("control plane")),
        "unexpected: {err}"
    );
}

#[test]
fn an_evaluator_ref_that_resolves_to_nothing_is_rejected() {
    // Rule 5.
    let err = compile(&variant(
        r#""currentVersion": "unit-tests-v1""#,
        r#""currentVersion": "unit-tests-v9""#,
    ))
    .expect_err("an unresolvable evaluator ref is rejected");
    assert!(
        matches!(
            err,
            ConfigError::UnknownReference {
                kind: "evaluator version",
                ..
            }
        ),
        "unexpected: {err}"
    );
}

#[test]
fn an_evaluator_referencing_an_undeclared_profile_is_rejected() {
    // Rule 4.
    let err = compile(&variant(
        r#""sandboxProfile": "verification-default""#,
        r#""sandboxProfile": "nonexistent""#,
    ))
    .expect_err("an unknown evaluator profile is rejected");
    assert!(
        matches!(
            err,
            ConfigError::UnknownReference {
                kind: "sandbox profile",
                ..
            }
        ),
        "unexpected: {err}"
    );
}

#[test]
fn a_mandatory_context_section_cannot_be_dropped() {
    // Rule 6: mandatory context is never silently truncated.
    let err = compile(&variant(
        r#""optionalDropOrder": ["workspace", "memory"]"#,
        r#""optionalDropOrder": ["workspace", "task"]"#,
    ))
    .expect_err("dropping a mandatory section is rejected");
    assert!(
        matches!(err, ConfigError::IncompatibleCombination { ref detail } if detail.contains("mandatory")),
        "unexpected: {err}"
    );
}

#[test]
fn duplicate_identities_are_conflicting_declarations() {
    let duplicated = VALID_SOURCE.replacen(
        r#"{ "backendId": "fake-local", "enabled": true, "selector": "fake" },"#,
        r#"{ "backendId": "fake-local", "enabled": true, "selector": "fake" },
      { "backendId": "fake-local", "enabled": false, "selector": "other" },"#,
        1,
    );
    let err = compile(&duplicated).expect_err("a duplicate backend id is rejected");
    assert!(
        matches!(
            err,
            ConfigError::DuplicateIdentity {
                kind: "backend",
                ..
            }
        ),
        "unexpected: {err}"
    );
}

#[test]
fn an_evaluator_must_be_an_argv_vector_not_a_shell_string() {
    let err = compile(&variant(
        r#""argv": ["/usr/bin/pantheon-check", "--suite", "unit"]"#,
        r#""argv": []"#,
    ))
    .expect_err("an empty argv is rejected");
    assert!(
        matches!(err, ConfigError::InvalidValue { ref detail, .. } if detail.contains("argv")),
        "unexpected: {err}"
    );
}

#[test]
fn a_rule_naming_an_unknown_action_is_rejected() {
    let err = compile(&variant(
        r#"{ "action": "shell.execute", "effect": "permit" }"#,
        r#"{ "action": "not.a.pantheon.action", "effect": "permit" }"#,
    ))
    .expect_err("an unknown action is rejected");
    assert!(
        matches!(err, ConfigError::InvalidValue { ref detail, .. } if detail.contains("canonical")),
        "unexpected: {err}"
    );
}

#[test]
fn the_hard_policy_identity_participates_in_the_authorization_digest() {
    // §4: operator configuration cannot weaken built-in hard policy without
    // the authorization identity changing.
    let compiled = compile(VALID_SOURCE).expect("compiles");
    let encoded = String::from_utf8(
        pantheon_core::config::model::Component::to_value(compiled.authorization())
            .to_canonical_bytes(),
    )
    .expect("utf-8");
    assert!(
        encoded.contains(pantheon_core::config::model::HARD_POLICY_VERSION),
        "the hard-policy identity must be inside the digested value"
    );
}

#[test]
fn configuration_cannot_permit_the_hard_denied_action() {
    // `agent-manifest.md`: Agent `secret.read` is hard-denied by v1 built-in
    // policy "even if a malformed manifest attempted to permit it". A
    // candidate that tries must not be activatable.
    let err = compile(&variant(
        r#"{ "action": "secret.read", "effect": "forbid" }"#,
        r#"{ "action": "secret.read", "effect": "permit" }"#,
    ))
    .expect_err("permitting the hard-denied action must be rejected");
    assert_eq!(err.kind(), "hard-policy-violation", "unexpected: {err}");

    // An explicit forbid is redundant but not a weakening, so it still
    // compiles — otherwise the rule would be untestable in the fixture.
    compile(VALID_SOURCE).expect("an explicit forbid remains valid");
}

#[test]
fn an_agent_cannot_declare_the_hard_denied_action() {
    // The same rule from the other direction: hard policy is not something an
    // Agent manifest opts out of by declaring the action on itself.
    let err = compile(&variant(
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"]"#,
        r#""actions": ["shell.execute", "secret.read"]"#,
    ))
    .expect_err("an Agent declaring the hard-denied action must be rejected");
    assert_eq!(err.kind(), "hard-policy-violation", "unexpected: {err}");
}

#[test]
fn agent_actions_are_validated_against_the_canonical_vocabulary() {
    // Agent actions and authorization rules must describe one world: an
    // action Pantheon does not define cannot be declared on either side.
    let err = compile(&variant(
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"]"#,
        r#""actions": ["shell.execute", "workspace.read"]"#,
    ))
    .expect_err("a non-canonical Agent action must be rejected");
    assert!(
        matches!(err, ConfigError::InvalidValue { ref detail, .. } if detail.contains("canonical")),
        "unexpected: {err}"
    );
}

#[test]
fn the_action_vocabulary_is_the_canonical_one() {
    // Pins the names to `permissions-and-capabilities.md` rather than a local
    // invention, so a rename there is caught here instead of silently
    // diverging.
    use pantheon_core::config::compile::ACTIONS;
    for canonical in [
        "filesystem.read",
        "filesystem.write",
        "filesystem.delete",
        "shell.execute",
        "process.spawn",
        "artifact.read",
        "artifact.seal",
        "secret.read",
        "secret.use",
    ] {
        assert!(
            ACTIONS.contains(&canonical),
            "{canonical} must be canonical"
        );
    }
    for invented in ["workspace.read", "workspace.write", "artifact.write"] {
        assert!(
            !ACTIONS.contains(&invented),
            "{invented} is not a canonical Pantheon action"
        );
    }
}

#[test]
fn a_process_spawning_agent_also_requires_control_plane_isolation() {
    // The sandbox contract says "arbitrary shell/process execution" requires
    // control-plane isolation. Reading that as shell-only would leave the same
    // escape open under a different action name.
    let trusted_host = variant(
        r#""isolationClass": "CONTAINER""#,
        r#""isolationClass": "TRUSTED_HOST""#,
    )
    .replacen(
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"]"#,
        r#""actions": ["process.spawn", "filesystem.read"]"#,
        1,
    );
    let err =
        compile(&trusted_host).expect_err("process spawning on a trusted host must be rejected");
    assert!(
        matches!(err, ConfigError::IncompatibleCombination { ref detail } if detail.contains("process.spawn")),
        "unexpected: {err}"
    );
}

#[test]
fn a_process_spawning_agent_needs_the_control_plane_guarantee_asserted() {
    // Class alone is not proof: the profile must actually assert the guarantee.
    let source = variant(
        r#""guarantees": ["isolation.control-plane", "isolation.peer-workspaces"]"#,
        r#""guarantees": ["isolation.peer-workspaces"]"#,
    )
    .replacen(
        r#""sandboxRequirements": ["isolation.control-plane"]"#,
        r#""sandboxRequirements": []"#,
        1,
    )
    .replacen(
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"]"#,
        r#""actions": ["process.spawn"]"#,
        1,
    );
    let err = compile(&source).expect_err("a profile without the guarantee must be rejected");
    assert!(
        matches!(err, ConfigError::IncompatibleCombination { ref detail } if detail.contains("isolation.control-plane")),
        "unexpected: {err}"
    );
}

#[test]
fn agent_status_and_launch_safety_are_part_of_configuration_identity() {
    let reference = compile(VALID_SOURCE).expect("reference compiles");
    let changed = compile(&variant(
        r#""version": 1,"#,
        r#""version": 1, "enabled": false, "current": true,"#,
    ))
    .expect("status change compiles");
    assert_ne!(
        reference.component_digests().agents,
        changed.component_digests().agents,
        "Agent status is authority-bearing configuration"
    );

    let changed = compile(&variant(
        r#""tieBreak": "backendId" }"#,
        r#""tieBreak": "backendId", "requiresKeyedLaunch": false }"#,
    ))
    .expect("launch policy change compiles");
    assert_ne!(
        reference.component_digests().routing,
        changed.component_digests().routing,
        "launch safety is part of route policy identity"
    );
}

#[test]
fn invalid_agent_pin_and_conflicting_current_versions_are_rejected_before_activation() {
    let unknown_pin = VALID_SOURCE.replacen(
        r#""policies": ["#,
        r#""agentPins":[{"name":"missing","version":1}],"policies": ["#,
        1,
    );
    let err = compile(&unknown_pin).expect_err("an unknown pin is invalid");
    assert!(
        matches!(
            err,
            ConfigError::UnknownReference {
                kind: "Agent version",
                ..
            }
        ),
        "unexpected: {err}"
    );

    let conflicting = VALID_SOURCE.replacen(
        r#""agents": ["#,
        r#""agents": [{"name":"builder","version":2,"enabled":true,"current":true,"accepts":["code-change"],"competencies":["rust"],"routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,"sandboxProfile":"strict-local-container","sandboxRequirements":["isolation.control-plane"],"actions":["filesystem.read"]},"#,
        1,
    );
    let err = compile(&conflicting).expect_err("two current versions are invalid");
    assert!(
        matches!(err, ConfigError::IncompatibleCombination { ref detail } if detail.contains("more than one current")),
        "unexpected: {err}"
    );
}

#[test]
fn compiled_agent_declarations_reject_unknown_or_empty_manifest_fields() {
    let unknown = variant(
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"]"#,
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"], "unexpected": true"#,
    );
    let err = compile(&unknown).expect_err("unknown Agent fields are not authority");
    assert!(
        matches!(err, ConfigError::InvalidValue { ref detail, .. } if detail.contains("unknown field")),
        "unexpected: {err}"
    );

    let empty = variant(r#""accepts": ["code-change"]"#, r#""accepts": []"#);
    let err = compile(&empty).expect_err("an Agent must declare applicability");
    assert!(
        matches!(err, ConfigError::InvalidValue { ref path, .. } if path == "agents[0].accepts"),
        "unexpected: {err}"
    );
}

#[test]
fn agent_membership_order_does_not_change_compiled_identity() {
    let reference = compile(VALID_SOURCE).expect("reference compiles");
    let reordered = variant(
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"]"#,
        r#""actions": ["filesystem.write", "shell.execute", "filesystem.read"]"#,
    );
    let other = compile(&reordered).expect("reordered Agent compiles");
    assert_eq!(
        reference.component_digests().agents,
        other.component_digests().agents
    );
    assert_eq!(reference.revision_digest(), other.revision_digest());
}

#[test]
fn a_repeated_entry_in_a_set_valued_list_is_rejected() {
    // These lists canonicalize as sorted sets, so a repeated entry would
    // change the component digest without changing what the configuration
    // means. Rejecting it keeps source and compiled identity in step, and
    // matches the `uniqueItems` the Agent manifest schema already declares.
    let source = variant(
        r#""actions": ["shell.execute", "filesystem.read", "filesystem.write"]"#,
        r#""actions": ["shell.execute", "filesystem.read", "shell.execute"]"#,
    );
    match compile(&source).expect_err("a repeated action is not a set") {
        ConfigError::DuplicateIdentity { kind, id } => {
            assert_eq!(kind, "agent action");
            assert_eq!(id, "shell.execute");
        }
        other => panic!("unexpected rejection: {other:?}"),
    }
}

#[test]
fn a_route_preference_key_outside_the_vocabulary_is_rejected_at_activation() {
    // `featureMatch` is the specific key this rejects, and it is not an
    // arbitrary example: candidate validation already fails closed on a
    // missing required execution feature, so counting the matched ones can
    // only restate how many the Agent asked for. A preference key that cannot
    // express a preference has to fail at activation rather than quietly
    // ordering nothing.
    let source = variant(
        r#""ordering": ["contextCapacity"]"#,
        r#""ordering": ["featureMatch"]"#,
    );
    match compile(&source).expect_err("an unusable preference key is refused") {
        ConfigError::InvalidValue { path, detail } => {
            assert!(path.ends_with("ordering"), "unexpected path {path:?}");
            assert!(
                detail.contains("featureMatch"),
                "unexpected detail {detail:?}"
            );
        }
        other => panic!("unexpected rejection: {other:?}"),
    }
}

#[test]
fn the_compiler_version_is_part_of_revision_identity() {
    // The constant exists so that a change to how Pantheon compiles
    // configuration produces a different identity instead of two incompatible
    // compilations both claiming one version. That only holds if the version
    // actually reaches the digest.
    let components = compile(VALID_SOURCE).expect("compiles").component_digests();

    assert_ne!(
        components.revision_digest("pantheon-config-v1"),
        components.revision_digest(COMPILER_VERSION),
        "the compiler version must reach the revision identity: a digest that \
         ignores it lets two incompatible compilations claim one version"
    );
}
