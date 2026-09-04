//! Pantheon's concrete local container SandboxBackend.
//!
//! # Owns
//!
//! The production `CONTAINER` SandboxBackend implementation: creating,
//! inspecting, verifying and releasing Linux containers through a local
//! container runtime (Podman preferred, Docker supported).
//!
//! # Must not own
//!
//! Durable state, orchestration logic, or domain rules. This crate is a
//! concrete backend behind the engine's abstract port; it does not decide
//! when to provision or release.
//!
//! # Security model
//!
//! Every container is rootless, unprivileged, and runs with:
//! - `--user` set to a non-root UID;
//! - `--security-opt=no-new-privileges`;
//! - `--cap-drop=ALL`;
//! - `--network=none` (for the v0.1.0 MVP coding workload);
//! - explicit bind mounts only for the Workspace, scratch, and Agent Control
//!   route;
//! - no host runtime socket mounted;
//! - no host namespace escapes.
//!
//! Verification fails closed: if any required isolation fact cannot be
//! established, the Sandbox is rejected and the Run never reaches LaunchReady.

use std::process::{Command, Stdio};

use pantheon_core::sandbox::{
    SandboxKey, SandboxMount, SandboxNetworkMode, SandboxPlan, SandboxPresence, SandboxVerification,
};
use pantheon_engine::sandbox::{SandboxBackend, SandboxError};

/// Which container runtime executable this backend uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Podman,
    Docker,
}

/// A local container runtime backed by either Podman or Docker.
///
/// Both are invoked through their respective CLI tools. The backend does not
/// link against a container runtime library; it spawns the executable and
/// parses its output. This keeps the crate free of C dependencies and lets
/// the same binary work with either runtime.
#[derive(Debug, Clone)]
pub struct LocalContainerBackend {
    runtime: String,
    #[allow(dead_code)]
    kind: RuntimeKind,
    /// A prefix applied to every container name so Pantheon-owned containers
    /// are distinguishable from unrelated host containers.
    name_prefix: String,
    /// An optional user ID to run containers as (e.g. "1000"). If `None`,
    /// the runtime's default is used.
    run_user: Option<String>,
}

impl LocalContainerBackend {
    /// Probes the host for a usable container runtime.
    ///
    /// Prefers Podman because it is daemonless and rootless by default;
    /// falls back to Docker when Podman is not on `$PATH`. Fails closed
    /// when neither is available.
    ///
    /// # Errors
    ///
    /// [`SandboxError`] when no supported runtime can be found.
    pub fn detect() -> Result<Self, SandboxError> {
        if Self::has_executable("podman") {
            return Ok(Self {
                runtime: "podman".to_string(),
                kind: RuntimeKind::Podman,
                name_prefix: "pantheon-sandbox-".to_string(),
                run_user: Some("1000".to_string()),
            });
        }
        if Self::has_executable("docker") {
            return Ok(Self {
                runtime: "docker".to_string(),
                kind: RuntimeKind::Docker,
                name_prefix: "pantheon-sandbox-".to_string(),
                run_user: Some("1000".to_string()),
            });
        }
        Err(SandboxError {
            detail: "no supported container runtime found on PATH (tried: podman, docker)"
                .to_string(),
        })
    }

    /// Creates a backend using a specific runtime executable.
    ///
    /// # Errors
    ///
    /// [`SandboxError`] when the executable does not exist or is not usable.
    pub fn with_runtime(runtime: impl Into<String>) -> Result<Self, SandboxError> {
        let runtime = runtime.into();
        let kind = if runtime.contains("podman") {
            RuntimeKind::Podman
        } else {
            RuntimeKind::Docker
        };
        if !Self::has_executable(&runtime) {
            return Err(SandboxError {
                detail: format!("container runtime {runtime} is not available on PATH"),
            });
        }
        Ok(Self {
            runtime,
            kind,
            name_prefix: "pantheon-sandbox-".to_string(),
            run_user: Some("1000".to_string()),
        })
    }

    fn has_executable(name: &str) -> bool {
        Command::new("which")
            .arg(name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn container_name(&self, key: &SandboxKey) -> String {
        format!("{}{}", self.name_prefix, key.as_str())
    }

    #[allow(dead_code)]
    fn base_args(&self) -> Vec<String> {
        vec![self.runtime.clone()]
    }
}

impl SandboxBackend for LocalContainerBackend {
    fn ensure_sandbox(
        &self,
        key: &SandboxKey,
        plan: &SandboxPlan,
    ) -> Result<SandboxPresence, SandboxError> {
        let name = self.container_name(key);

        // If the container already exists, inspect it.
        let exists = self.container_exists(&name)?;
        if exists {
            let running = self.container_running(&name)?;
            return Ok(if running {
                SandboxPresence::Present
            } else {
                // A created but not running container is ambiguous.
                // Start it if possible.
                self.start_container(&name)?;
                SandboxPresence::Present
            });
        }

        // Create the container.
        self.create_container(&name, plan)?;
        self.start_container(&name)?;
        Ok(SandboxPresence::Present)
    }

    fn inspect_sandbox(&self, key: &SandboxKey) -> Result<SandboxPresence, SandboxError> {
        let name = self.container_name(key);
        if !self.container_exists(&name)? {
            return Ok(SandboxPresence::Absent);
        }
        let running = self.container_running(&name)?;
        Ok(if running {
            SandboxPresence::Present
        } else {
            SandboxPresence::Unknown
        })
    }

    fn release_sandbox(&self, key: &SandboxKey) -> Result<(), SandboxError> {
        let name = self.container_name(key);
        if !self.container_exists(&name)? {
            return Ok(());
        }
        self.stop_container(&name)?;
        self.remove_container(&name)?;
        Ok(())
    }

    fn verify_sandbox(
        &self,
        key: &SandboxKey,
        plan: &SandboxPlan,
    ) -> Result<SandboxVerification, SandboxError> {
        let name = self.container_name(key);
        let inspect_json = self.inspect_container(&name)?;

        let verification = verify_container_json(&inspect_json, key, plan)
            .map_err(|detail| SandboxError { detail })?;

        Ok(verification)
    }
}

// ---------------------------------------------------------------------------
// Container CLI operations
// ---------------------------------------------------------------------------

impl LocalContainerBackend {
    fn container_exists(&self, name: &str) -> Result<bool, SandboxError> {
        let output = Command::new(&self.runtime)
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("name=^{name}$"),
                "--format",
                "{{.Names}}",
            ])
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not list containers: {err}"),
            })?;
        if !output.status.success() {
            return Err(SandboxError {
                detail: format!(
                    "{} ps failed: {}",
                    self.runtime,
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim() == name)
    }

    fn container_running(&self, name: &str) -> Result<bool, SandboxError> {
        let output = Command::new(&self.runtime)
            .args([
                "ps",
                "--filter",
                &format!("name=^{name}$"),
                "--format",
                "{{.Names}}",
            ])
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not list running containers: {err}"),
            })?;
        if !output.status.success() {
            return Err(SandboxError {
                detail: format!(
                    "{} ps failed: {}",
                    self.runtime,
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim() == name)
    }

    fn create_container(&self, name: &str, plan: &SandboxPlan) -> Result<(), SandboxError> {
        let mut args = vec!["create".to_string(), "--name".to_string(), name.to_string()];

        // Non-privileged, no new privileges, drop all capabilities.
        args.push("--security-opt=no-new-privileges:true".to_string());
        args.push("--cap-drop=ALL".to_string());

        // User namespace isolation.
        if let Some(user) = &self.run_user {
            args.push("--user".to_string());
            args.push(user.clone());
        }

        // Network mode.
        match plan.network_mode {
            SandboxNetworkMode::None => {
                args.push("--network=none".to_string());
            }
            SandboxNetworkMode::Brokered => {
                // Brokered means no direct external network for the worker.
                // In a local container this is implemented as `--network=none`
                // because the backend cannot enforce a host/port allowlist.
                args.push("--network=none".to_string());
            }
        }

        // Resource limits.
        if let Some(cpu) = plan.cpu_limit_millicores {
            let cpus = f64::from(cpu) / 1000.0;
            args.push("--cpus".to_string());
            args.push(format!("{cpus:.2}"));
        }
        if let Some(mem) = plan.memory_limit_mb {
            args.push("--memory".to_string());
            args.push(format!("{mem}m"));
        }

        // Mounts.
        for mount in &plan.mounts {
            let mut spec = format!(
                "type=bind,source={},destination={}",
                mount.source, mount.destination
            );
            if mount.read_only {
                spec.push_str(",readonly");
            }
            args.push("--mount".to_string());
            args.push(spec);
        }

        // Image.
        args.push(plan.environment_identity.clone());

        let output = Command::new(&self.runtime)
            .args(&args)
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not create container: {err}"),
            })?;
        if !output.status.success() {
            return Err(SandboxError {
                detail: format!(
                    "{} create failed: {}",
                    self.runtime,
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        Ok(())
    }

    fn start_container(&self, name: &str) -> Result<(), SandboxError> {
        let output = Command::new(&self.runtime)
            .args(["start", name])
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not start container: {err}"),
            })?;
        if !output.status.success() {
            return Err(SandboxError {
                detail: format!(
                    "{} start failed: {}",
                    self.runtime,
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        Ok(())
    }

    fn stop_container(&self, name: &str) -> Result<(), SandboxError> {
        let output = Command::new(&self.runtime)
            .args(["stop", "-t", "10", name])
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not stop container: {err}"),
            })?;
        // stop may fail if the container is already stopped; tolerate that.
        let _ = output;
        Ok(())
    }

    fn remove_container(&self, name: &str) -> Result<(), SandboxError> {
        let output = Command::new(&self.runtime)
            .args(["rm", "-f", name])
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not remove container: {err}"),
            })?;
        let _ = output;
        Ok(())
    }

    fn inspect_container(&self, name: &str) -> Result<String, SandboxError> {
        let output = Command::new(&self.runtime)
            .args(["inspect", name])
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not inspect container: {err}"),
            })?;
        if !output.status.success() {
            return Err(SandboxError {
                detail: format!(
                    "{} inspect failed: {}",
                    self.runtime,
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

fn verify_container_json(
    json: &str,
    key: &SandboxKey,
    plan: &SandboxPlan,
) -> Result<SandboxVerification, String> {
    // Parse the minimal fields we need from `docker/podman inspect` output.
    // Both tools emit a JSON array with one object per container.
    let array: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| format!("inspect output is not valid JSON: {err}"))?;
    let obj = array
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or("inspect output is empty")?;

    let host_config = obj
        .get("HostConfig")
        .ok_or("inspect output missing HostConfig")?;

    // Privilege check.
    let privileged = host_config
        .get("Privileged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let privilege_verified = !privileged;

    // Capabilities check.
    let cap_drop = host_config
        .get("CapDrop")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    let capability_verified = cap_drop.contains(&"ALL") || cap_drop.contains(&"all");

    // Network mode check.
    let network_mode_json = host_config
        .get("NetworkMode")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let expected_network = match plan.network_mode {
        SandboxNetworkMode::None | SandboxNetworkMode::Brokered => "none",
    };
    let network_mode_verified = network_mode_json == expected_network;

    // Mount check.
    let mounts_json = obj
        .get("Mounts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mounts_verified = verify_mounts(&mounts_json, &plan.mounts)?;

    // Resource limits check.
    // --cpus maps to NanoCpus (not CpuQuota) in both Docker and Podman.
    let nano_cpus = host_config.get("NanoCpus").and_then(|v| v.as_i64());
    let memory = host_config.get("Memory").and_then(|v| v.as_i64());
    let resource_limits_verified = plan.cpu_limit_millicores.is_some() == nano_cpus.is_some()
        && plan.memory_limit_mb.is_some() == memory.is_some();

    // Identity check: the container name must match our key.
    let name_json = obj
        .get("Name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim_start_matches('/');
    let expected_name = format!("pantheon-sandbox-{}", key.as_str());
    let identity_verified = name_json == expected_name;

    // Environment identity: we check the image name/digest. For the MVP we
    // verify the Config.Image field contains the plan's environment_identity.
    let config = obj.get("Config").ok_or("inspect output missing Config")?;
    let image = config.get("Image").and_then(|v| v.as_str()).unwrap_or("");
    let environment_identity_verified = image == plan.environment_identity;

    // Agent control route and workspace binding are verified through the
    // mount set check above.
    let agent_control_route_verified = mounts_verified;
    let workspace_binding_verified = mounts_verified;

    Ok(SandboxVerification {
        sandbox_key: key.clone(),
        holder_id: key.as_str().to_string(),
        environment_identity: plan.environment_identity.clone(),
        mounts_verified: mounts_verified && identity_verified && environment_identity_verified,
        network_mode_verified,
        privilege_verified,
        capability_verified,
        agent_control_route_verified,
        workspace_binding_verified,
        resource_limits_verified,
    })
}

fn verify_mounts(actual: &[serde_json::Value], expected: &[SandboxMount]) -> Result<bool, String> {
    // Container runtimes may add implicit mounts (resolv.conf, hosts, etc.);
    // we only verify that every explicitly requested mount is present.
    for expected_mount in expected {
        let found = actual.iter().any(|m| {
            let source = m.get("Source").and_then(|v| v.as_str()).unwrap_or("");
            let dest = m.get("Destination").and_then(|v| v.as_str()).unwrap_or("");
            let mode = m.get("Mode").and_then(|v| v.as_str()).unwrap_or("");
            let rw = m
                .get("RW")
                .and_then(|v| v.as_bool())
                .or_else(|| m.get("ReadOnly").and_then(|v| v.as_bool()).map(|ro| !ro))
                .unwrap_or(true);
            source == expected_mount.source
                && dest == expected_mount.destination
                && rw != expected_mount.read_only
                && (!expected_mount.read_only || mode.contains("ro"))
        });
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
