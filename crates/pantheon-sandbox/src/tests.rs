use crate::LocalContainerBackend;

#[test]
fn detect_returns_some_on_typical_system() {
    // Most CI and dev machines have either podman or docker.
    // This test documents that detect() does not panic.
    let _ = LocalContainerBackend::detect();
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod integration {
    use super::LocalContainerBackend;
    use pantheon_core::config::Digest;
    use pantheon_core::sandbox::{
        SandboxKey, SandboxMount, SandboxNetworkMode, SandboxPlan, SandboxPresence,
    };
    use pantheon_engine::sandbox::SandboxBackend;

    fn test_plan() -> SandboxPlan {
        SandboxPlan {
            sandbox_profile_digest: Digest::of(b"profile"),
            environment_identity: "alpine:latest".to_string(),
            mounts: vec![
                SandboxMount {
                    source: "/tmp/pantheon-test-ws".to_string(),
                    destination: "/workspace".to_string(),
                    read_only: false,
                },
                SandboxMount {
                    source: "/tmp/pantheon-test-scratch".to_string(),
                    destination: "/scratch".to_string(),
                    read_only: false,
                },
            ],
            network_mode: SandboxNetworkMode::None,
            cpu_limit_millicores: Some(500),
            memory_limit_mb: Some(256),
        }
    }

    #[test]
    fn ensure_and_release_roundtrip() {
        let backend = LocalContainerBackend::detect().expect("no container runtime found");
        let key = SandboxKey::new("pantheon-test-roundtrip").unwrap();
        let plan = test_plan();

        // Bind-mount sources must exist before container creation.
        std::fs::create_dir_all("/tmp/pantheon-test-ws").unwrap();
        std::fs::create_dir_all("/tmp/pantheon-test-scratch").unwrap();

        // Clean up any leftover from a previous aborted run
        let _ = backend.release_sandbox(&key);

        let presence = backend.ensure_sandbox(&key, &plan).unwrap();
        assert_eq!(presence, SandboxPresence::Present);

        let verified = backend.verify_sandbox(&key, &plan).unwrap();
        if !verified.all_passed() {
            // Diagnostic: print the container inspect JSON so CI logs reveal
            // exactly what the runtime reports for mounts, capabilities, etc.
            let inspect = backend
                .inspect_container_json(&key)
                .unwrap_or_else(|_| "<inspect failed>".to_string());
            eprintln!("=== container inspect ===\n{inspect}\n=========================");
        }
        assert!(
            verified.all_passed(),
            "verification failed: mounts={} network={} privilege={} capability={} agent_route={} workspace={} resources={}",
            verified.mounts_verified,
            verified.network_mode_verified,
            verified.privilege_verified,
            verified.capability_verified,
            verified.agent_control_route_verified,
            verified.workspace_binding_verified,
            verified.resource_limits_verified,
        );

        backend.release_sandbox(&key).unwrap();

        let after = backend.inspect_sandbox(&key).unwrap();
        assert_eq!(after, SandboxPresence::Absent);
    }
}
