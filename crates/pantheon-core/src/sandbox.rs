//! The Sandbox vocabulary.
//!
//! `docs/architecture/security/sandbox-broker-and-isolation.md` is canonical
//! for what a Sandbox is. This module holds only the pure-computation parts:
//! the lifecycle phase domain, the separate factual record of external
//! presence, the immutable SandboxKey, and the provider-neutral SandboxPlan.
//!
//! Nothing here touches a container runtime, a process, or a database.

use std::fmt;

use crate::config::Digest;

/// The canonical Sandbox lifecycle phases.
///
/// All six from the canonical contract's "Sandbox lifecycle and external
/// identity" section are represented, not only the ones a current mission
/// drives. Naming the full domain now avoids a later migration that rewrites
/// stored history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPhase {
    /// Durable Sandbox identity and intention exist. No provisioning side
    /// effect has been attempted, which is exactly what makes this phase
    /// worth distinguishing from [`Self::Preparing`]: recovery can conclude
    /// from it that no external runtime exists.
    Requested,
    /// Provisioning has been authorized and may have begun. From here on
    /// external state may exist whatever happens next.
    Preparing,
    /// Provisioning completed and was verified. The Sandbox may host
    /// execution for its holder.
    Ready,
    /// Release has been authorized and external state may still exist.
    Releasing,
    /// External state is established absent and the holder's Sandbox slot is
    /// free.
    Released,
    /// A controller operation on this Sandbox failed. `Error` is a lifecycle
    /// fact and says nothing about whether external state exists — see
    /// [`SandboxPresence`].
    Error,
}

impl SandboxPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "Requested",
            Self::Preparing => "Preparing",
            Self::Ready => "Ready",
            Self::Releasing => "Releasing",
            Self::Released => "Released",
            Self::Error => "Error",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "Requested" => Self::Requested,
            "Preparing" => Self::Preparing,
            "Ready" => Self::Ready,
            "Releasing" => Self::Releasing,
            "Released" => Self::Released,
            "Error" => Self::Error,
            _ => return None,
        })
    }
}

impl fmt::Display for SandboxPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The strongest current factual observation about the Sandbox's external
/// runtime existence.
///
/// Deliberately a separate dimension from [`SandboxPhase`], for the reason
/// the canonical contract gives: a controller error is not proof that an
/// external effect did not happen. A failed `podman create` leaves `Unknown`,
/// not `Absent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPresence {
    /// Verified to exist and to match the durable Sandbox binding.
    Present,
    /// Established not to exist.
    Absent,
    /// Not established either way. Never inferred from an error.
    Unknown,
}

impl SandboxPresence {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "Present",
            Self::Absent => "Absent",
            Self::Unknown => "Unknown",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "Present" => Self::Present,
            "Absent" => Self::Absent,
            "Unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

impl fmt::Display for SandboxPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An immutable durable identity for one SandboxInstance.
///
/// Created before any provisioning side effect, so recovery can reconcile
/// the same key after crash or restart instead of blindly creating another
/// Sandbox for the holder.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SandboxKey(String);

impl SandboxKey {
    /// Accepts a sandbox key identifier.
    ///
    /// # Errors
    ///
    /// Fails when `value` is empty or longer than 128 characters.
    pub fn new(value: impl Into<String>) -> Result<Self, SandboxKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SandboxKeyError::Empty);
        }
        if value.len() > 128 {
            return Err(SandboxKeyError::TooLong);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SandboxKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a sandbox key was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxKeyError {
    Empty,
    TooLong,
}

impl fmt::Display for SandboxKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("sandbox key must not be empty"),
            Self::TooLong => f.write_str("sandbox key must not exceed 128 characters"),
        }
    }
}

impl std::error::Error for SandboxKeyError {}

/// One mount entry in a provider-neutral SandboxPlan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMount {
    /// Host path or source to expose.
    pub source: String,
    /// Path inside the Sandbox where the source appears.
    pub destination: String,
    /// Whether the mount is read-only inside the Sandbox.
    pub read_only: bool,
}

/// The network mode a Sandbox enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxNetworkMode {
    /// No external network. Agent Control may still be reachable through a
    /// narrowly scoped transport.
    None,
    /// Arbitrary worker egress unavailable; authorized external operations
    /// run through Pantheon-owned brokers.
    Brokered,
}

impl SandboxNetworkMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Brokered => "BROKERED",
        }
    }
}

/// A provider-neutral immutable plan for one SandboxInstance.
///
/// Pantheon creates this before execution and the SandboxBackend must prove
/// it can satisfy the plan through factual mechanisms, not profile-name
/// assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    /// Immutable digest of the SandboxProfile that authorized this plan.
    pub sandbox_profile_digest: Digest,
    /// Immutable content identity of the execution environment — an image
    /// digest rather than a mutable tag. Opaque to core.
    pub environment_identity: String,
    /// Explicit filesystem mounts the Sandbox exposes.
    pub mounts: Vec<SandboxMount>,
    /// Network mode the Sandbox must enforce.
    pub network_mode: SandboxNetworkMode,
    /// Resource claims the Sandbox must satisfy.
    pub cpu_limit_millicores: Option<u32>,
    pub memory_limit_mb: Option<u32>,
}

impl SandboxPlan {
    #[must_use]
    pub fn digest(&self) -> Digest {
        use crate::config::canonical::Value;
        Value::object([
            (
                "sandboxProfileDigest",
                Value::string(self.sandbox_profile_digest.to_string()),
            ),
            (
                "environmentIdentity",
                Value::string(&self.environment_identity),
            ),
            (
                "mounts",
                Value::array(self.mounts.iter().map(|mount| {
                    Value::object([
                        ("source", Value::string(&mount.source)),
                        ("destination", Value::string(&mount.destination)),
                        ("readOnly", Value::Bool(mount.read_only)),
                    ])
                })),
            ),
            ("networkMode", Value::string(self.network_mode.as_str())),
            (
                "cpuLimitMillicores",
                match self.cpu_limit_millicores {
                    Some(v) => Value::Integer(i64::from(v)),
                    None => Value::Null,
                },
            ),
            (
                "memoryLimitMb",
                match self.memory_limit_mb {
                    Some(v) => Value::Integer(i64::from(v)),
                    None => Value::Null,
                },
            ),
        ])
        .digest()
    }
}

/// Facts established by Sandbox verification before execution is permitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxVerification {
    pub sandbox_key: SandboxKey,
    pub holder_id: String,
    pub environment_identity: String,
    pub mounts_verified: bool,
    pub network_mode_verified: bool,
    pub privilege_verified: bool,
    pub capability_verified: bool,
    pub agent_control_route_verified: bool,
    pub workspace_binding_verified: bool,
    pub resource_limits_verified: bool,
}

impl SandboxVerification {
    /// Whether every required verification fact passed.
    #[must_use]
    pub const fn all_passed(&self) -> bool {
        self.mounts_verified
            && self.network_mode_verified
            && self.privilege_verified
            && self.capability_verified
            && self.agent_control_route_verified
            && self.workspace_binding_verified
            && self.resource_limits_verified
    }
}

#[cfg(test)]
mod tests;
