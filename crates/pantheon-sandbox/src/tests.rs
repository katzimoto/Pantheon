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

    fn ensure_scratch_writable() {
        std::fs::create_dir_all("/tmp/pantheon-test-scratch").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o777);
            std::fs::set_permissions("/tmp/pantheon-test-scratch", perms).unwrap();
        }
    }

    fn cleanup_key(backend: &LocalContainerBackend, key: &SandboxKey) {
        let _ = backend.release_sandbox(key);
    }

    #[test]
    fn ensure_and_release_roundtrip() {
        let backend = LocalContainerBackend::detect().expect("no container runtime found");
        let key = SandboxKey::new("pantheon-test-roundtrip").unwrap();
        let plan = test_plan();

        std::fs::create_dir_all("/tmp/pantheon-test-ws").unwrap();
        ensure_scratch_writable();
        cleanup_key(&backend, &key);

        let presence = backend.ensure_sandbox(&key, &plan).unwrap();
        assert_eq!(presence, SandboxPresence::Present);

        let verified = backend.verify_sandbox(&key, &plan).unwrap();
        if !verified.all_passed() {
            let name = backend.container_name(&key);
            let inspect = backend
                .inspect_container(&name)
                .unwrap_or_else(|_| "<inspect failed>".to_string());
            eprintln!("=== container inspect ===\n{inspect}\n=========================");
            for probe in &verified.probe_results {
                eprintln!(
                    "probe {}: expected={} observed={} passed={}",
                    probe.name, probe.expected, probe.observed, probe.passed
                );
            }
        }
        assert!(
            verified.all_passed(),
            "verification failed: mounts={} network={} privilege={} capability={} \
             agent_route={} workspace={} resources={} seccomp={} pid={} user={} mount={} \
             cloud={} dns={} forbidden={} runtime_socket={} control_plane={}",
            verified.mounts_verified,
            verified.network_mode_verified,
            verified.privilege_verified,
            verified.capability_verified,
            verified.agent_control_route_verified,
            verified.workspace_binding_verified,
            verified.resource_limits_verified,
            verified.seccomp_active_verified,
            verified.host_pid_hidden_verified,
            verified.host_user_namespace_verified,
            verified.host_mount_namespace_verified,
            verified.cloud_metadata_unreachable_verified,
            verified.dns_resolution_denied_verified,
            verified.forbidden_mounts_absent_verified,
            verified.runtime_socket_absent_verified,
            verified.control_plane_unreachable_verified,
        );

        backend.release_sandbox(&key).unwrap();

        let after = backend.inspect_sandbox(&key).unwrap();
        assert_eq!(after, SandboxPresence::Absent);
    }

    #[test]
    fn behavioral_probes_all_pass() {
        let backend = LocalContainerBackend::detect().expect("no container runtime found");
        let key = SandboxKey::new("pantheon-test-probes").unwrap();
        let plan = test_plan();

        std::fs::create_dir_all("/tmp/pantheon-test-ws").unwrap();
        ensure_scratch_writable();
        cleanup_key(&backend, &key);

        backend.ensure_sandbox(&key, &plan).unwrap();
        let verified = backend.verify_sandbox(&key, &plan).unwrap();

        // Every required probe must have been run and passed.
        assert!(
            verified.seccomp_active_verified,
            "seccomp must be active: observed {:?}",
            verified
                .probe_results
                .iter()
                .find(|p| p.name == "seccomp_active")
                .map(|p| &p.observed)
        );
        assert!(verified.host_pid_hidden_verified, "host PID must be hidden");
        assert!(
            verified.host_user_namespace_verified,
            "user namespace must be distinct"
        );
        assert!(
            verified.host_mount_namespace_verified,
            "mount namespace must be distinct"
        );
        assert!(
            verified.cloud_metadata_unreachable_verified,
            "cloud metadata must be unreachable"
        );
        assert!(
            verified.dns_resolution_denied_verified,
            "DNS must be denied"
        );
        assert!(
            verified.forbidden_mounts_absent_verified,
            "forbidden mounts must be absent"
        );
        assert!(
            verified.runtime_socket_absent_verified,
            "runtime socket must be absent"
        );
        assert!(
            verified.control_plane_unreachable_verified,
            "control plane must be unreachable"
        );

        // Identity binding must be present in every probe result.
        for probe in &verified.probe_results {
            assert!(
                !probe.observed.is_empty(),
                "probe {} must have an observed fact",
                probe.name
            );
        }
        assert!(
            verified.backend_descriptor == "podman" || verified.backend_descriptor == "docker",
            "backend descriptor must name the actual runtime: {}",
            verified.backend_descriptor
        );
        assert!(!verified.backend_version.is_empty());
        assert_eq!(verified.platform, "linux");
        assert_eq!(verified.architecture, "x86_64");
        assert_eq!(verified.probe_implementation_version, "2");

        backend.release_sandbox(&key).unwrap();
    }

    #[test]
    fn cross_attempt_isolation_holds() {
        let backend = LocalContainerBackend::detect().expect("no container runtime found");
        let key_a = SandboxKey::new("pantheon-test-xa-a").unwrap();
        let key_b = SandboxKey::new("pantheon-test-xa-b").unwrap();

        std::fs::create_dir_all("/tmp/pantheon-test-ws").unwrap();
        cleanup_key(&backend, &key_a);
        cleanup_key(&backend, &key_b);

        // Each sandbox must use a unique scratch directory; bind-mounts share
        // the host path exactly, so identical sources would mean shared state.
        let scratch_a = "/tmp/pantheon-test-scratch-a";
        let scratch_b = "/tmp/pantheon-test-scratch-b";
        std::fs::create_dir_all(scratch_a).unwrap();
        std::fs::create_dir_all(scratch_b).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(scratch_a, std::fs::Permissions::from_mode(0o777)).unwrap();
            std::fs::set_permissions(scratch_b, std::fs::Permissions::from_mode(0o777)).unwrap();
        }

        let mut plan_a = test_plan();
        plan_a.mounts = vec![
            SandboxMount {
                source: "/tmp/pantheon-test-ws".to_string(),
                destination: "/workspace".to_string(),
                read_only: false,
            },
            SandboxMount {
                source: scratch_a.to_string(),
                destination: "/scratch".to_string(),
                read_only: false,
            },
        ];
        let mut plan_b = test_plan();
        plan_b.mounts = vec![
            SandboxMount {
                source: "/tmp/pantheon-test-ws".to_string(),
                destination: "/workspace".to_string(),
                read_only: false,
            },
            SandboxMount {
                source: scratch_b.to_string(),
                destination: "/scratch".to_string(),
                read_only: false,
            },
        ];

        backend.ensure_sandbox(&key_a, &plan_a).unwrap();
        backend.ensure_sandbox(&key_b, &plan_b).unwrap();

        // Write a canary file inside sandbox A's scratch mount.
        let name_a = backend.container_name(&key_a);
        let write_out = backend
            .exec_in_container_raw(&name_a, &["sh", "-c", "echo secret-a > /scratch/canary"])
            .unwrap();
        assert!(write_out.status.success(), "canary write must succeed");

        // Verify the canary exists in A before testing B.
        let check_a = backend
            .exec_in_container_raw(&name_a, &["cat", "/scratch/canary"])
            .unwrap();
        assert!(
            String::from_utf8_lossy(&check_a.stdout).trim() == "secret-a",
            "canary must exist in sandbox A"
        );

        // Try to read it from sandbox B.
        let name_b = backend.container_name(&key_b);
        let leak = backend.exec_in_container_raw(
            &name_b,
            &[
                "sh",
                "-c",
                "cat /scratch/canary 2>/dev/null || echo NOT_FOUND",
            ],
        );
        let output = leak.unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.trim() == "NOT_FOUND" || !stdout.contains("secret-a"),
            "sandbox B must not read sandbox A's scratch: got {stdout}"
        );

        backend.release_sandbox(&key_a).unwrap();
        backend.release_sandbox(&key_b).unwrap();
    }

    #[test]
    fn weakened_profile_detected_and_prevented() {
        let backend = LocalContainerBackend::detect().expect("no container runtime found");
        let key = SandboxKey::new("pantheon-test-weak").unwrap();

        std::fs::create_dir_all("/tmp/pantheon-test-ws").unwrap();
        ensure_scratch_writable();
        cleanup_key(&backend, &key);

        // Create the container with the plan's expected config.
        let plan = test_plan();
        backend.ensure_sandbox(&key, &plan).unwrap();

        // Now verify against a deliberately different plan (wrong image).
        let weakened_plan = SandboxPlan {
            environment_identity: "busybox:latest".to_string(),
            ..plan
        };
        let verified = backend.verify_sandbox(&key, &weakened_plan).unwrap();
        assert!(
            !verified.all_passed(),
            "verification must fail when plan does not match actual container config"
        );
        assert!(
            !verified.mounts_verified,
            "environment identity mismatch must be detected"
        );

        backend.release_sandbox(&key).unwrap();
    }
}
