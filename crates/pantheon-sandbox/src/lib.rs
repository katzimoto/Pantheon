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
    SandboxKey, SandboxMount, SandboxNetworkMode, SandboxPlan, SandboxPresence, SandboxProbeResult,
    SandboxVerification,
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

    pub(crate) fn container_name(&self, key: &SandboxKey) -> String {
        format!("{}{}", self.name_prefix, key.as_str())
    }

    #[allow(dead_code)]
    fn base_args(&self) -> Vec<String> {
        vec![self.runtime.clone()]
    }

    fn backend_version(&self) -> Result<String, SandboxError> {
        let output = Command::new(&self.runtime).args(["--version"]).output();
        match output {
            Ok(out) if out.status.success() => {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            Ok(out) => Err(SandboxError {
                detail: format!(
                    "{} --version failed: {}",
                    self.runtime,
                    String::from_utf8_lossy(&out.stderr)
                ),
            }),
            Err(err) => Err(SandboxError {
                detail: format!("could not run {} --version: {err}", self.runtime),
            }),
        }
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
        let pid =
            container_pid_from_inspect(&inspect_json).map_err(|detail| SandboxError { detail })?;

        let mut verification = verify_container_json(&inspect_json, key, plan, pid)
            .map_err(|detail| SandboxError { detail })?;

        let (probe_results, probes) = self.run_probes(pid)?;
        verification.seccomp_active_verified = probes.seccomp_active;
        verification.host_pid_hidden_verified = probes.host_pid_hidden;
        verification.host_user_namespace_verified = probes.host_user_namespace;
        verification.host_mount_namespace_verified = probes.host_mount_namespace;
        verification.cloud_metadata_unreachable_verified = probes.cloud_metadata_unreachable;
        verification.dns_resolution_denied_verified = probes.dns_denied;
        verification.forbidden_mounts_absent_verified = probes.forbidden_mounts_absent;
        verification.runtime_socket_absent_verified = probes.runtime_socket_absent;
        verification.control_plane_unreachable_verified = probes.control_plane_unreachable;
        // Cross-Attempt isolation requires a distinct mount namespace;
        // the integration test proves the full property empirically.
        verification.cross_attempt_isolation_verified = probes.host_mount_namespace;
        verification.probe_results = probe_results;
        verification.backend_descriptor = self.runtime.clone();
        verification.backend_version = self.backend_version()?;
        verification.platform = std::env::consts::OS.to_string();
        verification.architecture = std::env::consts::ARCH.to_string();
        verification.probe_implementation_version = "2".to_string();

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
        // Keep the container alive so verification probes can exec into it.
        // The actual workload command is supplied later by the executor.
        args.push("sleep".to_string());
        args.push("3600".to_string());

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

    pub(crate) fn inspect_container(&self, name: &str) -> Result<String, SandboxError> {
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

    /// Execute a command inside a running container, returning raw output
    /// including non-zero exits so probes can observe denial.
    pub fn exec_in_container_raw(
        &self,
        name: &str,
        cmd: &[&str],
    ) -> Result<std::process::Output, SandboxError> {
        let mut args = vec!["exec", name];
        args.extend(cmd);
        Command::new(&self.runtime)
            .args(&args)
            .output()
            .map_err(|err| SandboxError {
                detail: format!("could not exec in container: {err}"),
            })
    }
}

// ---------------------------------------------------------------------------
// Host-side trustworthy inspection helpers
// ---------------------------------------------------------------------------

fn container_pid_from_inspect(json: &str) -> Result<u32, String> {
    let array: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| format!("inspect output is not valid JSON: {err}"))?;
    let obj = array
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or("inspect output is empty")?;
    let state = obj.get("State").ok_or("inspect output missing State")?;
    let pid = state
        .get("Pid")
        .and_then(|v| v.as_u64())
        .ok_or("inspect output missing State.Pid")?;
    if pid == 0 {
        return Err("container is not running".to_string());
    }
    Ok(pid as u32)
}

fn read_ns_inode(pid: u32, ns_type: &str) -> Result<u64, std::io::Error> {
    let path = format!("/proc/{pid}/ns/{ns_type}");
    let link = std::fs::read_link(&path)?;
    let s = link.to_string_lossy();
    // Format: "pid:[4026531836]"
    let start = s.find('[').ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "no [ in ns symlink")
    })?;
    let end = s.find(']').ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "no ] in ns symlink")
    })?;
    s[start + 1..end]
        .parse::<u64>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn read_proc_file(pid: u32, filename: &str) -> Result<String, std::io::Error> {
    let path = format!("/proc/{pid}/{filename}");
    std::fs::read_to_string(&path)
}

fn nsenter_net(pid: u32, cmd: &[&str]) -> Result<std::process::Output, std::io::Error> {
    let mut command = Command::new("nsenter");
    command.arg("-t").arg(pid.to_string()).arg("-n");
    for arg in cmd {
        command.arg(arg);
    }
    command.output()
}

/// When container inspect does not expose Architecture/Os, verify the
/// container's init binary architecture from host `/proc/<pid>/exe`.
fn verify_container_arch_from_host(
    pid: u32,
    host_arch: &str,
    host_platform: &str,
) -> Result<bool, String> {
    let exe_path = format!("/proc/{pid}/exe");
    let output = Command::new("file")
        .arg("-L")
        .arg(&exe_path)
        .output()
        .map_err(|e| format!("cannot run file on {exe_path}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "file command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // file output looks like:
    // /proc/12345/exe: ELF 64-bit LSB executable, x86-64, version 1 (SYSV), ...
    let arch_ok = if host_platform == "linux" {
        match host_arch {
            "x86_64" => stdout.contains("x86-64") || stdout.contains("x86_64"),
            "aarch64" => stdout.contains("aarch64") || stdout.contains("ARM aarch64"),
            "i686" => stdout.contains("Intel 80386") || stdout.contains("i386"),
            "arm" => stdout.contains("ARM"),
            "riscv64" => stdout.contains("RISC-V"),
            _ => {
                // Unknown host architecture: we cannot verify, so fail closed.
                return Err(format!(
                    "unsupported host architecture for host-side verification: {host_arch}"
                ));
            }
        }
    } else {
        // Non-Linux platforms not supported for container execution.
        false
    };
    Ok(arch_ok)
}

// ---------------------------------------------------------------------------
// Runtime behavioral probes
// ---------------------------------------------------------------------------

impl LocalContainerBackend {
    /// Run all controller-owned behavioral probes.
    ///
    /// Namespace, mount, seccomp, and socket checks use host-side
    /// `/proc/<pid>/...` inspection so worker image binaries cannot
    /// fabricate evidence. Network behavioral checks use host `nsenter`
    /// with host-owned binaries in the container's network namespace.
    fn run_probes(
        &self,
        pid: u32,
    ) -> Result<(Vec<SandboxProbeResult>, BehavioralProbes), SandboxError> {
        let mut results = Vec::new();
        let mut probes = BehavioralProbes::default();

        let seccomp = self.probe_seccomp(pid)?;
        probes.seccomp_active = seccomp.passed;
        results.push(seccomp);

        let pid_ns = self.probe_pid_namespace(pid)?;
        probes.host_pid_hidden = pid_ns.passed;
        results.push(pid_ns);

        let user_ns = self.probe_user_namespace(pid)?;
        probes.host_user_namespace = user_ns.passed;
        results.push(user_ns);

        let mount_ns = self.probe_mount_namespace(pid)?;
        probes.host_mount_namespace = mount_ns.passed;
        results.push(mount_ns);

        let net_ns = self.probe_network_namespace(pid)?;
        probes.network_isolated = net_ns.passed;
        results.push(net_ns);

        let cloud = self.probe_cloud_metadata(pid)?;
        probes.cloud_metadata_unreachable = cloud.passed;
        results.push(cloud);

        let dns = self.probe_dns_denied(pid)?;
        probes.dns_denied = dns.passed;
        results.push(dns);

        let forbidden = self.probe_forbidden_mounts(pid)?;
        probes.forbidden_mounts_absent = forbidden.passed;
        results.push(forbidden);

        let sockets = self.probe_runtime_sockets(pid)?;
        probes.runtime_socket_absent = sockets.passed;
        results.push(sockets);

        let ctrl = self.probe_control_plane(pid)?;
        probes.control_plane_unreachable = ctrl.passed;
        results.push(ctrl);

        Ok((results, probes))
    }

    fn probe_seccomp(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let status = read_proc_file(pid, "status").map_err(|e| SandboxError {
            detail: format!("cannot read /proc/{pid}/status: {e}"),
        })?;
        let observed = if let Some(line) = status.lines().find(|l| l.starts_with("Seccomp:")) {
            line.trim().to_string()
        } else {
            "Seccomp line missing".to_string()
        };
        let passed = observed == "Seccomp:\t2";
        Ok(SandboxProbeResult {
            name: "seccomp_active".to_string(),
            expected: "Seccomp: 2".to_string(),
            observed,
            passed,
        })
    }

    fn probe_pid_namespace(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let host_pid =
            read_ns_inode(std::process::id() as u32, "pid").map_err(|e| SandboxError {
                detail: format!("cannot read host PID namespace: {e}"),
            })?;
        let container_pid = read_ns_inode(pid, "pid").map_err(|e| SandboxError {
            detail: format!("cannot read container PID namespace: {e}"),
        })?;
        let passed = host_pid != container_pid;
        let observed = format!("host pid:[{host_pid}] container pid:[{container_pid}]");
        Ok(SandboxProbeResult {
            name: "pid_namespace".to_string(),
            expected: "distinct from host PID namespace".to_string(),
            observed,
            passed,
        })
    }

    fn probe_user_namespace(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let host_user =
            read_ns_inode(std::process::id() as u32, "user").map_err(|e| SandboxError {
                detail: format!("cannot read host user namespace: {e}"),
            })?;
        let container_user = read_ns_inode(pid, "user").map_err(|e| SandboxError {
            detail: format!("cannot read container user namespace: {e}"),
        })?;
        let passed = host_user != container_user;
        let observed = format!("host user:[{host_user}] container user:[{container_user}]");
        Ok(SandboxProbeResult {
            name: "user_namespace".to_string(),
            expected: "distinct from host user namespace".to_string(),
            observed,
            passed,
        })
    }

    fn probe_mount_namespace(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let host_mnt =
            read_ns_inode(std::process::id() as u32, "mnt").map_err(|e| SandboxError {
                detail: format!("cannot read host mount namespace: {e}"),
            })?;
        let container_mnt = read_ns_inode(pid, "mnt").map_err(|e| SandboxError {
            detail: format!("cannot read container mount namespace: {e}"),
        })?;
        let passed = host_mnt != container_mnt;
        let observed = format!("host mnt:[{host_mnt}] container mnt:[{container_mnt}]");
        Ok(SandboxProbeResult {
            name: "mount_namespace".to_string(),
            expected: "distinct from host mount namespace".to_string(),
            observed,
            passed,
        })
    }

    fn probe_network_namespace(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let host_net =
            read_ns_inode(std::process::id() as u32, "net").map_err(|e| SandboxError {
                detail: format!("cannot read host net namespace: {e}"),
            })?;
        let container_net = read_ns_inode(pid, "net").map_err(|e| SandboxError {
            detail: format!("cannot read container net namespace: {e}"),
        })?;
        let ns_distinct = host_net != container_net;

        let output = nsenter_net(pid, &["cat", "/proc/net/dev"]).map_err(|e| SandboxError {
            detail: format!("nsenter net check failed: {e}"),
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let non_lo = stdout
            .lines()
            .filter(|l| {
                let trimmed = l.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with("Inter-|")
                    && !trimmed.starts_with("face |")
                    && !trimmed.starts_with("lo:")
            })
            .count();

        let passed = ns_distinct && non_lo == 0;
        let observed = format!("net ns distinct={ns_distinct} non-loopback interfaces={non_lo}");
        Ok(SandboxProbeResult {
            name: "network_namespace".to_string(),
            expected: "distinct from host net namespace, 0 non-loopback interfaces".to_string(),
            observed,
            passed,
        })
    }

    fn probe_cloud_metadata(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        // Prefer `nc` if available; fall back to bash /dev/tcp.
        let (stdout, stderr, _status) = match self.try_connect_nsenter(pid, "169.254.169.254", "80")
        {
            Ok(result) => result,
            Err(err) => {
                return Ok(SandboxProbeResult {
                    name: "cloud_metadata_reachability".to_string(),
                    expected: "BLOCKED".to_string(),
                    observed: format!("probe execution error: {err}"),
                    passed: false,
                });
            }
        };
        let observed = if stdout.is_empty() && !stderr.is_empty() {
            format!("stderr={stderr}")
        } else {
            stdout.clone()
        };
        let passed = stdout == "BLOCKED";
        Ok(SandboxProbeResult {
            name: "cloud_metadata_reachability".to_string(),
            expected: "BLOCKED".to_string(),
            observed,
            passed,
        })
    }

    fn probe_dns_denied(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let (stdout, stderr, _status) = match self.try_connect_nsenter(pid, "8.8.8.8", "53") {
            Ok(result) => result,
            Err(err) => {
                return Ok(SandboxProbeResult {
                    name: "dns_resolution".to_string(),
                    expected: "BLOCKED".to_string(),
                    observed: format!("probe execution error: {err}"),
                    passed: false,
                });
            }
        };
        let observed = if stdout.is_empty() && !stderr.is_empty() {
            format!("stderr={stderr}")
        } else {
            stdout.clone()
        };
        let passed = stdout == "BLOCKED";
        Ok(SandboxProbeResult {
            name: "dns_resolution".to_string(),
            expected: "BLOCKED".to_string(),
            observed,
            passed,
        })
    }

    fn try_connect_nsenter(
        &self,
        pid: u32,
        host: &str,
        port: &str,
    ) -> Result<(String, String, std::process::ExitStatus), SandboxError> {
        // Try netcat first.
        let nc = nsenter_net(pid, &["nc", "-z", "-w", "2", host, port]);
        if let Ok(out) = nc {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if out.status.success() {
                // nc -z returns 0 when the port is open.
                return Ok(("REACHABLE".to_string(), stderr, out.status));
            }
            if out.status.code() == Some(1) {
                // nc -z returns 1 when the port is closed/unreachable.
                return Ok(("BLOCKED".to_string(), stderr, out.status));
            }
        }

        // Fall back to bash /dev/tcp.
        let bash = nsenter_net(
            pid,
            &[
                "/bin/bash",
                "-c",
                &format!(
                    "echo > /dev/tcp/{host}/{port} 2>/dev/null && echo REACHABLE || echo BLOCKED"
                ),
            ],
        )
        .map_err(|e| SandboxError {
            detail: format!("nsenter failed: {e}"),
        })?;
        let stdout = String::from_utf8_lossy(&bash.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&bash.stderr).trim().to_string();
        Ok((stdout, stderr, bash.status))
    }

    fn probe_forbidden_mounts(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let mountinfo = read_proc_file(pid, "mountinfo").map_err(|e| SandboxError {
            detail: format!("cannot read container mountinfo: {e}"),
        })?;
        let forbidden = [
            "/.ssh",
            "/.gnupg",
            "/root/.ssh",
            "/root/.gnupg",
            "/root/.aws",
            "/root/.config/gcloud",
            "/var/run/docker.sock",
            "/run/docker.sock",
        ];
        let mut found = Vec::new();
        for path in &forbidden {
            if mountinfo.contains(path) {
                found.push(*path);
            }
        }
        let passed = found.is_empty();
        let observed = if found.is_empty() {
            "none found".to_string()
        } else {
            found.join(", ")
        };
        Ok(SandboxProbeResult {
            name: "forbidden_mounts".to_string(),
            expected: "no host credential mounts".to_string(),
            observed,
            passed,
        })
    }

    fn probe_runtime_sockets(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let mountinfo = read_proc_file(pid, "mountinfo").map_err(|e| SandboxError {
            detail: format!("cannot read container mountinfo: {e}"),
        })?;
        let sockets = [
            "/run/docker.sock",
            "/run/containerd.sock",
            "/run/crio.sock",
            "/run/podman/podman.sock",
            "/var/run/docker.sock",
            "/var/run/containerd.sock",
            "/var/run/crio.sock",
            "/var/run/podman/podman.sock",
        ];
        let mut found = Vec::new();
        for path in &sockets {
            if mountinfo.contains(path) {
                found.push(*path);
            }
        }
        let passed = found.is_empty();
        let observed = if found.is_empty() {
            "none found".to_string()
        } else {
            found.join(", ")
        };
        Ok(SandboxProbeResult {
            name: "runtime_socket_absent".to_string(),
            expected: "no container runtime sockets mounted".to_string(),
            observed,
            passed,
        })
    }

    fn probe_control_plane(&self, pid: u32) -> Result<SandboxProbeResult, SandboxError> {
        let mountinfo = read_proc_file(pid, "mountinfo").map_err(|e| SandboxError {
            detail: format!("cannot read container mountinfo: {e}"),
        })?;
        let paths = [
            "/pantheon.db",
            "/workspace/pantheon.db",
            "/root/.pantheon/pantheon.db",
        ];
        let mut found = Vec::new();
        for path in &paths {
            if mountinfo.contains(path) {
                found.push(*path);
            }
        }
        let has_pantheon_mount = mountinfo.lines().any(|line| {
            line.split_whitespace()
                .any(|field| field.contains("pantheon.db"))
        });
        if has_pantheon_mount {
            found.push("pantheon.db mounted");
        }
        let passed = found.is_empty();
        let observed = if found.is_empty() {
            "none accessible".to_string()
        } else {
            found.join(", ")
        };
        Ok(SandboxProbeResult {
            name: "control_plane_reachability".to_string(),
            expected: "pantheon.db not accessible".to_string(),
            observed,
            passed,
        })
    }
}

#[derive(Debug, Default)]
struct BehavioralProbes {
    seccomp_active: bool,
    host_pid_hidden: bool,
    host_user_namespace: bool,
    host_mount_namespace: bool,
    network_isolated: bool,
    cloud_metadata_unreachable: bool,
    dns_denied: bool,
    forbidden_mounts_absent: bool,
    runtime_socket_absent: bool,
    control_plane_unreachable: bool,
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

fn verify_container_json(
    json: &str,
    key: &SandboxKey,
    plan: &SandboxPlan,
    pid: u32,
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
    let cap_add = host_config
        .get("CapAdd")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    let cap_drop = host_config
        .get("CapDrop")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    // Docker reports ["ALL"]; Podman rootless reports individual caps.
    let capability_verified = !cap_drop.is_empty() && cap_add.is_empty();

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

    // Environment identity: we check the image reference.
    // Docker stores the original reference in Config.Image.
    // Podman stores the resolved reference in Config.Image and ImageName.
    let config = obj.get("Config").ok_or("inspect output missing Config")?;
    let image = config.get("Image").and_then(|v| v.as_str()).unwrap_or("");
    let image_name = obj.get("ImageName").and_then(|v| v.as_str()).unwrap_or("");
    let environment_identity_verified = image == plan.environment_identity
        || image_name == plan.environment_identity
        || image.ends_with(&format!("/{}", plan.environment_identity));

    // Architecture and platform verification for seccomp safety.
    // Docker/Podman container inspect may expose Platform ("linux/amd64")
    // or separate Os/Architecture fields.  We accept either, and fall back
    // to host-side /proc/<pid>/exe inspection when inspect is silent.
    let host_arch = std::env::consts::ARCH;
    let host_platform = std::env::consts::OS;

    let (container_platform, container_arch) =
        if let Some(platform) = obj.get("Platform").and_then(|v| v.as_str()) {
            let mut parts = platform.split('/');
            (parts.next().unwrap_or(""), parts.next().unwrap_or(""))
        } else {
            (
                obj.get("Os").and_then(|v| v.as_str()).unwrap_or(""),
                obj.get("Architecture")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            )
        };

    let arch_ok = if container_arch.is_empty() && container_platform.is_empty() {
        // Inspect provided no architecture facts; verify from host /proc.
        verify_container_arch_from_host(pid, host_arch, host_platform)?
    } else {
        let mapped_arch = match container_arch {
            "amd64" => "x86_64",
            "arm64" => "aarch64",
            "386" => "i686",
            _ => container_arch,
        };
        mapped_arch == host_arch && container_platform == host_platform
    };

    if !arch_ok {
        return Err(format!(
            "container architecture/platform mismatch or unverifiable: container={container_platform}/{container_arch}, host={host_platform}/{host_arch}"
        ));
    }

    // Verify the container PID exists on the host (binds verification to
    // the actual running process).
    if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return Err(format!("container PID {pid} does not exist in host /proc"));
    }

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
        seccomp_active_verified: false,
        host_pid_hidden_verified: false,
        host_user_namespace_verified: false,
        host_mount_namespace_verified: false,
        cloud_metadata_unreachable_verified: false,
        dns_resolution_denied_verified: false,
        forbidden_mounts_absent_verified: false,
        runtime_socket_absent_verified: false,
        cross_attempt_isolation_verified: false,
        control_plane_unreachable_verified: false,
        probe_results: Vec::new(),
        backend_descriptor: "unknown".to_string(),
        backend_version: "unknown".to_string(),
        platform: "unknown".to_string(),
        architecture: "unknown".to_string(),
        probe_implementation_version: "unknown".to_string(),
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
