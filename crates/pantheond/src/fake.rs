//! The deterministic fake execution backend.
//!
//! **Test/fake infrastructure.** This module exists so the Scheduler can
//! commit real T3 intents and the Run Controller can exercise the full
//! restart-safe Attempt lifecycle — LaunchKey, AgentControlSession, the
//! durable contact boundary and same-lineage reconciliation — without any
//! production executor behind the ports. It makes no production isolation or
//! execution-readiness claim whatsoever; the strict container backend is
//! Issue #34 and the production local executor is Issue #35.
//!
//! Factual semantics it *does* provide, because the controller depends on
//! them:
//!
//! - `KEYED_IDEMPOTENT` launch semantics: exactly one logical lineage per
//!   `LaunchKey`, so repeated `ensureExecution` calls address one execution;
//! - keyed inspection by LaunchKey alone;
//! - deterministic state progression (`STARTING -> RUNNING -> EXITED`), one
//!   step per inspection;
//! - a Sandbox readiness gate that is factually ready for anything (and can
//!   be observed refusing nothing — it is a fake).
//!
//! The world lives in process memory. A daemon restart therefore loses it,
//! which is itself honest external-world behavior: the restarted controller
//! inventories durable nonterminal state, inspects by LaunchKey against the
//! (now empty) external world, receives proven absence, and reconciles
//! through the ordinary path.

use std::sync::Arc;

use pantheon_core::attempt::Observation;
use pantheon_core::execution::{
    BackendDescriptor, ControllerSafetyFacts, ExecutionOffer, ExecutionRequest, LaunchSemantics,
};
use pantheon_core::sandbox::{SandboxKey, SandboxPlan, SandboxPresence, SandboxVerification};
use pantheon_engine::routing::{BackendError, ExecutorBackend, ExecutorBackendPort};
use pantheon_engine::run::{ExecutionLauncher, LaunchPackage, LauncherFailure};
use pantheon_engine::sandbox::{SandboxBackend, SandboxError};

/// One logical external execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lineage {
    Starting,
    Running,
    Exited,
}

/// The fake backend's entire external world.
#[derive(Debug, Default)]
struct World {
    lineages: std::collections::BTreeMap<String, Lineage>,
}

impl World {
    fn advance(lin: Lineage) -> Lineage {
        match lin {
            Lineage::Starting => Lineage::Running,
            Lineage::Running => Lineage::Exited,
            Lineage::Exited => Lineage::Exited,
        }
    }

    fn observe(&self, lin: Lineage) -> Observation {
        match lin {
            Lineage::Starting => Observation::Starting,
            Lineage::Running => Observation::Running,
            Lineage::Exited => Observation::Exited,
        }
    }
}

/// The fake backend: routing descriptor, launcher and Sandbox gate in one
/// deliberately small object.
#[derive(Debug, Clone, Default)]
pub(crate) struct FakeExecutor {
    world: Arc<std::sync::Mutex<World>>,
}

impl FakeExecutor {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The port the Scheduler routes through.
    #[must_use]
    pub(crate) fn port(&self) -> ExecutorBackendPort<'_> {
        ExecutorBackendPort::new(
            self,
            ControllerSafetyFacts {
                isolation_guarantees: vec!["isolation.control-plane".to_string()],
                observational_launch_safe: false,
            },
        )
    }
}

impl ExecutorBackend for FakeExecutor {
    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            backend_id: "fake-local".to_string(),
            revision: 1,
            available_for_offers: true,
            placement: vec![],
            supported_execution_features: vec!["exec.shell".to_string()],
            context_capacity_tokens: 32_000,
            isolation_facts: vec!["isolation.control-plane".to_string()],
            resources: vec![],
            launch_semantics: LaunchSemantics::KeyedIdempotent,
        }
    }

    fn offer(&self, request: &ExecutionRequest) -> Result<Vec<ExecutionOffer>, BackendError> {
        Ok(vec![ExecutionOffer {
            request_digest: request.digest(),
            backend_id: "fake-local".to_string(),
            descriptor_revision: 1,
            descriptor_digest: self.describe().digest(),
            supported_execution_features: vec!["exec.shell".to_string()],
            context_capacity_tokens: 32_000,
            placement: vec![],
            isolation_facts: vec!["isolation.control-plane".to_string()],
            resources: vec![],
            launch_semantics: LaunchSemantics::KeyedIdempotent,
            offer_reference: format!("fake://{}", request.task_id),
        }])
    }
}

impl ExecutionLauncher for FakeExecutor {
    fn backend_id(&self) -> &str {
        "fake-local"
    }

    fn launch_semantics(&self) -> LaunchSemantics {
        LaunchSemantics::KeyedIdempotent
    }

    fn ensure_execution(
        &self,
        package: &LaunchPackage<'_>,
    ) -> Result<Observation, LauncherFailure> {
        let mut world = self.world.lock().expect("fake world");
        world
            .lineages
            .entry(package.launch_key.to_string())
            .or_insert(Lineage::Starting);
        Ok(Observation::Starting)
    }

    fn inspect_execution(&self, launch_key: &str) -> Result<Observation, LauncherFailure> {
        let mut world = self.world.lock().expect("fake world");
        let next = match world.lineages.get(launch_key).copied() {
            Some(lineage) => World::advance(lineage),
            // A keyed-idempotent backend can prove absence in its own
            // namespace; an unknown key means no lineage exists.
            None => return Ok(Observation::Absent),
        };
        world.lineages.insert(launch_key.to_string(), next);
        Ok(world.observe(next))
    }
}

impl SandboxBackend for FakeExecutor {
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

    fn release_sandbox(&self, _key: &SandboxKey) -> Result<(), SandboxError> {
        Ok(())
    }
}
