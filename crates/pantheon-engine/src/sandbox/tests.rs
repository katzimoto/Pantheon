use std::sync::atomic::{AtomicU64, Ordering};

use pantheon_core::config::Digest;
use pantheon_core::sandbox::{
    SandboxKey, SandboxMount, SandboxNetworkMode, SandboxPhase, SandboxPlan, SandboxPresence,
    SandboxVerification,
};
use pantheon_store::{Command, Committed, Store};

use crate::sandbox::{SandboxBackend, SandboxController, SandboxError};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_store() -> (Store, std::path::PathBuf) {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "pantheon-engine-sandbox-test-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("store.db");
    let store = Store::open(&path).expect("open");
    (store, dir)
}

fn command<'a>(epoch: &'a str, id: &'a str) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: &[0u8; 32],
        event_type: "test",
    }
}

fn test_plan() -> SandboxPlan {
    SandboxPlan {
        sandbox_profile_digest: Digest::of(b"profile"),
        environment_identity: "env-1".to_string(),
        mounts: vec![SandboxMount {
            source: "/tmp/src".to_string(),
            destination: "/dst".to_string(),
            read_only: true,
        }],
        network_mode: SandboxNetworkMode::None,
        cpu_limit_millicores: Some(500),
        memory_limit_mb: Some(512),
    }
}

#[derive(Debug)]
struct FakeBackend {
    refuses_ensure: bool,
    refuses_verify: bool,
}

impl SandboxBackend for FakeBackend {
    fn ensure_sandbox(
        &self,
        _key: &SandboxKey,
        _plan: &SandboxPlan,
    ) -> Result<SandboxPresence, SandboxError> {
        if self.refuses_ensure {
            Err(SandboxError {
                detail: "injected ensure failure".to_string(),
            })
        } else {
            Ok(SandboxPresence::Present)
        }
    }

    fn inspect_sandbox(&self, _key: &SandboxKey) -> Result<SandboxPresence, SandboxError> {
        Ok(SandboxPresence::Present)
    }

    fn verify_sandbox(
        &self,
        key: &SandboxKey,
        plan: &SandboxPlan,
    ) -> Result<SandboxVerification, SandboxError> {
        if self.refuses_verify {
            Err(SandboxError {
                detail: "injected verify failure".to_string(),
            })
        } else {
            Ok(SandboxVerification {
                sandbox_key: key.clone(),
                holder_id: "fake".to_string(),
                environment_identity: plan.environment_identity.clone(),
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
                backend_descriptor: "fake".to_string(),
                backend_version: "0.0.0".to_string(),
                platform: std::env::consts::OS.to_string(),
                architecture: std::env::consts::ARCH.to_string(),
                probe_implementation_version: "1".to_string(),
            })
        }
    }

    fn release_sandbox(&self, _key: &SandboxKey) -> Result<(), SandboxError> {
        Ok(())
    }
}

#[test]
fn provision_creates_durable_sandbox() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");

    let record = controller
        .provision(&cmd, "run-1", &plan, &backend)
        .unwrap();

    assert_eq!(record.run_id, "run-1");
    assert_eq!(record.phase, SandboxPhase::Ready);
}

#[test]
fn provision_fails_closed_on_ensure_failure() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: true,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");

    let err = controller
        .provision(&cmd, "run-1", &plan, &backend)
        .unwrap_err();
    assert!(matches!(
        err,
        crate::sandbox::SandboxControllerError::ProvisioningFailed { .. }
    ));

    // Sandbox should be in Error phase
    let found = store.sandbox_for_run("run-1").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().phase, SandboxPhase::Error);
}

#[test]
fn provision_fails_closed_on_verify_failure() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: true,
    };
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");

    let err = controller
        .provision(&cmd, "run-1", &plan, &backend)
        .unwrap_err();
    assert!(matches!(
        err,
        crate::sandbox::SandboxControllerError::ProvisioningFailed { .. }
    ));
}

#[test]
fn provision_is_idempotent() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");
    let cmd1 = command(epoch.as_str(), "cmd-1");
    let cmd2 = command(epoch.as_str(), "cmd-2");

    let first = controller
        .provision(&cmd1, "run-1", &plan, &backend)
        .unwrap();
    let second = controller
        .provision(&cmd2, "run-1", &plan, &backend)
        .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(first.revision.get(), second.revision.get());
}

#[test]
fn reconcile_updates_presence() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");

    let record = controller
        .provision(&cmd, "run-1", &plan, &backend)
        .unwrap();

    let cmd2 = command(epoch.as_str(), "cmd-2");
    let reconciled = controller.reconcile(&cmd2, &record, &backend).unwrap();

    assert_eq!(reconciled.record.id, record.id);
    assert_eq!(reconciled.presence, SandboxPresence::Present);
}

#[test]
fn release_lifecycle_completes() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");

    let record = controller
        .provision(&cmd, "run-1", &plan, &backend)
        .unwrap();
    let key = SandboxKey::new(&record.id).unwrap();

    let cmd2 = command(epoch.as_str(), "cmd-2");
    let releasing = controller
        .begin_release(&cmd2, &key, record.revision, &backend)
        .unwrap();
    assert_eq!(releasing.phase, SandboxPhase::Releasing);

    let cmd3 = command(epoch.as_str(), "cmd-3");
    let released = controller
        .complete_release(&cmd3, &key, releasing.revision, SandboxPresence::Absent)
        .unwrap();
    assert_eq!(released.phase, SandboxPhase::Released);
}

#[test]
fn provision_recovers_preparing_to_ready() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");

    // Manually create a sandbox in Preparing phase to simulate a crash.
    let key = SandboxKey::new("sandbox-run-prep").unwrap();
    let digest = plan.digest();
    let binding = pantheon_store::SandboxBinding {
        run_id: "run-prep",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };
    let cmd_create = command(epoch.as_str(), "create-prep");
    let created = store.create_sandbox(&cmd_create, &key, &binding).unwrap();
    let record = match created {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };

    let cmd_begin = command(epoch.as_str(), "begin-prep");
    store
        .begin_sandbox_preparation(&cmd_begin, &key, record.revision)
        .unwrap();

    // Provision must recover from Preparing to Ready without overlapping.
    let cmd_provision = command(epoch.as_str(), "provision-prep");
    let record = controller
        .provision(&cmd_provision, "run-prep", &plan, &backend)
        .unwrap();
    assert_eq!(record.phase, SandboxPhase::Ready);
}

#[test]
fn provision_refuses_error_phase() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");

    // Manually create a sandbox and fail it.
    let key = SandboxKey::new("sandbox-run-err").unwrap();
    let digest = plan.digest();
    let binding = pantheon_store::SandboxBinding {
        run_id: "run-err",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };
    let cmd_create = command(epoch.as_str(), "create-err");
    let created = store.create_sandbox(&cmd_create, &key, &binding).unwrap();
    let record = match created {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };

    let cmd_fail = command(epoch.as_str(), "fail-err");
    store
        .fail_sandbox(&cmd_fail, &key, record.revision, SandboxPresence::Unknown)
        .unwrap();

    let cmd_provision = command(epoch.as_str(), "provision-err");
    let err = controller
        .provision(&cmd_provision, "run-err", &plan, &backend)
        .unwrap_err();
    assert!(matches!(
        err,
        crate::sandbox::SandboxControllerError::ProvisioningFailed { .. }
    ));
}

#[test]
fn reconcile_preserves_error_sandbox() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FakeBackend {
        refuses_ensure: false,
        refuses_verify: false,
    };
    let epoch = store.restore_generation().expect("generation");

    // Create and fail a sandbox.
    let key = SandboxKey::new("sandbox-run-rec").unwrap();
    let digest = plan.digest();
    let binding = pantheon_store::SandboxBinding {
        run_id: "run-rec",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };
    let cmd_create = command(epoch.as_str(), "create-rec");
    let created = store.create_sandbox(&cmd_create, &key, &binding).unwrap();
    let record = match created {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    let cmd_fail = command(epoch.as_str(), "fail-rec");
    store
        .fail_sandbox(&cmd_fail, &key, record.revision, SandboxPresence::Unknown)
        .unwrap();

    // Reconcile must not overwrite Error or provision a replacement.
    let failed = store.sandbox_for_run("run-rec").unwrap().unwrap();
    assert_eq!(failed.phase, SandboxPhase::Error);

    let cmd_reconcile = command(epoch.as_str(), "reconcile-rec");
    let reconciled = controller
        .reconcile(&cmd_reconcile, &failed, &backend)
        .unwrap();
    assert_eq!(reconciled.record.phase, SandboxPhase::Error);
    assert_eq!(reconciled.presence, SandboxPresence::Present);
}

#[derive(Debug)]
struct FailingProbeBackend;

impl SandboxBackend for FailingProbeBackend {
    fn ensure_sandbox(
        &self,
        _key: &SandboxKey,
        _plan: &SandboxPlan,
    ) -> Result<SandboxPresence, SandboxError> {
        Ok(SandboxPresence::Present)
    }

    fn inspect_sandbox(&self, _key: &SandboxKey) -> Result<SandboxPresence, SandboxError> {
        Ok(SandboxPresence::Present)
    }

    fn verify_sandbox(
        &self,
        key: &SandboxKey,
        plan: &SandboxPlan,
    ) -> Result<SandboxVerification, SandboxError> {
        Ok(SandboxVerification {
            sandbox_key: key.clone(),
            holder_id: "fake".to_string(),
            environment_identity: plan.environment_identity.clone(),
            mounts_verified: true,
            network_mode_verified: true,
            privilege_verified: true,
            capability_verified: true,
            agent_control_route_verified: true,
            workspace_binding_verified: true,
            resource_limits_verified: true,
            seccomp_active_verified: false,
            host_pid_hidden_verified: true,
            host_user_namespace_verified: true,
            host_mount_namespace_verified: true,
            cloud_metadata_unreachable_verified: true,
            dns_resolution_denied_verified: true,
            forbidden_mounts_absent_verified: true,
            runtime_socket_absent_verified: true,
            cross_attempt_isolation_verified: true,
            control_plane_unreachable_verified: true,
            probe_results: vec![pantheon_core::sandbox::SandboxProbeResult {
                name: "seccomp_active".to_string(),
                expected: "Seccomp: 2".to_string(),
                observed: "Seccomp: 0".to_string(),
                passed: false,
            }],
            backend_descriptor: "fake".to_string(),
            backend_version: "0.0.0".to_string(),
            platform: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            probe_implementation_version: "1".to_string(),
        })
    }

    fn release_sandbox(&self, _key: &SandboxKey) -> Result<(), SandboxError> {
        Ok(())
    }
}

#[test]
fn failed_probe_prevents_launch_and_records_evidence() {
    let (store, _dir) = temp_store();
    let controller = SandboxController::new(&store);
    let plan = test_plan();
    let backend = FailingProbeBackend;
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-probe");

    let err = controller
        .provision(&cmd, "run-probe", &plan, &backend)
        .unwrap_err();
    assert!(matches!(
        err,
        crate::sandbox::SandboxControllerError::VerificationFailed { .. }
    ));

    // The sandbox should be in Error phase.
    let record = store.sandbox_for_run("run-probe").unwrap().unwrap();
    assert_eq!(record.phase, SandboxPhase::Error);

    // Probe evidence should be persisted.
    let evidence = store.sandbox_probe_results(record.id.as_str()).unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].probe_name, "seccomp_active");
    assert_eq!(evidence[0].expected, "Seccomp: 2");
    assert_eq!(evidence[0].observed, "Seccomp: 0");
    assert!(!evidence[0].passed);
}
