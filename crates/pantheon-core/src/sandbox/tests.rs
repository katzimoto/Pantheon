use super::*;

#[test]
fn sandbox_phase_round_trip() {
    for phase in [
        SandboxPhase::Requested,
        SandboxPhase::Preparing,
        SandboxPhase::Ready,
        SandboxPhase::Releasing,
        SandboxPhase::Released,
        SandboxPhase::Error,
    ] {
        assert_eq!(SandboxPhase::parse(phase.as_str()), Some(phase));
    }
    assert_eq!(SandboxPhase::parse("Unknown"), None);
}

#[test]
fn sandbox_presence_round_trip() {
    for presence in [
        SandboxPresence::Present,
        SandboxPresence::Absent,
        SandboxPresence::Unknown,
    ] {
        assert_eq!(SandboxPresence::parse(presence.as_str()), Some(presence));
    }
    assert_eq!(SandboxPresence::parse("Missing"), None);
}

#[test]
fn sandbox_key_validation() {
    assert!(SandboxKey::new("valid-key_123").is_ok());
    assert!(matches!(SandboxKey::new(""), Err(SandboxKeyError::Empty)));
    assert!(matches!(
        SandboxKey::new("a".repeat(129)),
        Err(SandboxKeyError::TooLong)
    ));
}

#[test]
fn sandbox_plan_digest_is_stable() {
    let plan = SandboxPlan {
        sandbox_profile_digest: Digest::of(b"profile"),
        environment_identity: "sha256:abc123".to_string(),
        mounts: vec![SandboxMount {
            source: "/host/workspace".to_string(),
            destination: "/workspace".to_string(),
            read_only: false,
        }],
        network_mode: SandboxNetworkMode::None,
        cpu_limit_millicores: Some(1000),
        memory_limit_mb: Some(512),
    };
    let digest_a = plan.digest();
    let digest_b = plan.digest();
    assert_eq!(digest_a, digest_b);
}

#[test]
fn sandbox_plan_digest_differs_on_change() {
    let plan_a = SandboxPlan {
        sandbox_profile_digest: Digest::of(b"profile"),
        environment_identity: "sha256:abc123".to_string(),
        mounts: vec![],
        network_mode: SandboxNetworkMode::None,
        cpu_limit_millicores: None,
        memory_limit_mb: None,
    };
    let plan_b = SandboxPlan {
        sandbox_profile_digest: Digest::of(b"profile"),
        environment_identity: "sha256:def456".to_string(),
        mounts: vec![],
        network_mode: SandboxNetworkMode::None,
        cpu_limit_millicores: None,
        memory_limit_mb: None,
    };
    assert_ne!(plan_a.digest(), plan_b.digest());
}

#[test]
fn sandbox_verification_all_passed() {
    let ok = SandboxVerification {
        sandbox_key: SandboxKey::new("k").unwrap(),
        holder_id: "run_1".to_string(),
        environment_identity: "img".to_string(),
        mounts_verified: true,
        network_mode_verified: true,
        privilege_verified: true,
        capability_verified: true,
        agent_control_route_verified: true,
        workspace_binding_verified: true,
        resource_limits_verified: true,
        seccomp_active_verified: true,
        host_pid_hidden_verified: true,
        host_user_namespace_verified: true,
        host_mount_namespace_verified: true,
        cloud_metadata_unreachable_verified: true,
        dns_resolution_denied_verified: true,
        forbidden_mounts_absent_verified: true,
        runtime_socket_absent_verified: true,
        cross_attempt_isolation_verified: true,
        control_plane_unreachable_verified: true,
        probe_results: Vec::new(),
        backend_descriptor: "test".to_string(),
        backend_version: "0.0.0".to_string(),
        platform: "linux".to_string(),
        architecture: "x86_64".to_string(),
        probe_implementation_version: "1".to_string(),
    };
    assert!(ok.all_passed());

    let bad = SandboxVerification {
        mounts_verified: false,
        ..ok.clone()
    };
    assert!(!bad.all_passed());

    let bad_seccomp = SandboxVerification {
        seccomp_active_verified: false,
        ..ok
    };
    assert!(!bad_seccomp.all_passed());
}
