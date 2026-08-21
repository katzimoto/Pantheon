//! The Task-owned Workspace vocabulary.
//!
//! `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`
//! is canonical for what a Workspace is. This module holds only the parts that
//! are pure computation: the lifecycle phase domain, the separate factual
//! record of whether materialization exists, and the two base identities a
//! Workspace binds — the requested, possibly mutable ref and the immutable
//! object identity it resolved to.
//!
//! Nothing here touches a filesystem, a repository or a database. Resolving a
//! requested base is an external effect and belongs behind the engine's port;
//! *deciding whether a resolved identity is well formed* is computation and
//! belongs here.

use std::fmt;

/// The canonical Workspace lifecycle phases.
///
/// All seven from the canonical contract's "Workspace phases" section are
/// represented, not only the ones a current mission drives. The MVP
/// materialization path writes `Requested`, `Materializing`, `Ready` and
/// `Error`; `Frozen`, `Releasing` and `Released` arrive with candidate
/// capture and Workspace release. Naming the full domain now is the same
/// choice [`crate::planning::TaskPhase`] made: a lifecycle that a later
/// mission has to replace, and a migration that rewrites stored history, are
/// worse than four unreached variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePhase {
    /// Durable Workspace identity and intention exist. No filesystem or Git
    /// side effect has been attempted, which is exactly what makes this
    /// phase worth distinguishing from [`Self::Materializing`]: recovery can
    /// conclude from it that no external state exists.
    Requested,
    /// Materialization has been authorized and may have begun. From here on
    /// external state may exist whatever happens next.
    Materializing,
    /// Materialization completed and was verified. The current Task
    /// execution owner may mutate inside the Workspace.
    Ready,
    /// Mutation authority is suspended while authoritative candidate, yield
    /// or finalization state must not change.
    Frozen,
    /// Release has been authorized and external state may still exist.
    Releasing,
    /// External state is established absent and the Task's Workspace slot is
    /// free.
    Released,
    /// A controller operation on this Workspace failed. `Error` is a
    /// lifecycle fact and says nothing about whether external state exists —
    /// see [`Materialization`].
    Error,
}

impl WorkspacePhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "Requested",
            Self::Materializing => "Materializing",
            Self::Ready => "Ready",
            Self::Frozen => "Frozen",
            Self::Releasing => "Releasing",
            Self::Released => "Released",
            Self::Error => "Error",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "Requested" => Self::Requested,
            "Materializing" => Self::Materializing,
            "Ready" => Self::Ready,
            "Frozen" => Self::Frozen,
            "Releasing" => Self::Releasing,
            "Released" => Self::Released,
            "Error" => Self::Error,
            _ => return None,
        })
    }

    /// Whether this Workspace has ever been handed to an execution owner.
    ///
    /// This is the predicate that decides whether partial external state may
    /// be discarded and rebuilt. A Workspace that has never reached `Ready`
    /// has never been mutable to any worker, so whatever exists at its path
    /// is controller-owned scratch. Once it has been `Ready`, its filesystem
    /// state may hold unsealed work and is never silently recreated.
    #[must_use]
    pub const fn has_been_mutable(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Frozen | Self::Releasing | Self::Released
        )
    }
}

impl fmt::Display for WorkspacePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The strongest current factual observation about the Workspace's external
/// filesystem/Git materialization.
///
/// Deliberately a separate dimension from [`WorkspacePhase`], for the reason
/// `docs/architecture/security/sandbox-broker-and-isolation.md` gives for
/// keeping Sandbox lifecycle and observed presence apart: a controller error
/// is not proof that an external effect did not happen. A failed `git init`
/// leaves `Unknown`, not `Absent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Materialization {
    /// Verified to exist and to match the durable Workspace binding.
    Present,
    /// Established not to exist.
    Absent,
    /// Not established either way. Never inferred from an error.
    Unknown,
}

impl Materialization {
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

impl fmt::Display for Materialization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a base identity was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseError {
    /// A resolved base is not a lowercase hexadecimal object name of a
    /// supported width.
    NotAnObjectName(String),
    /// A requested base is not a usable ref name.
    NotARefName { value: String, reason: &'static str },
}

impl fmt::Display for BaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnObjectName(value) => write!(
                f,
                "{value:?} is not a lowercase hexadecimal object name of 40 or 64 characters"
            ),
            Self::NotARefName { value, reason } => {
                write!(f, "{value:?} is not a usable ref name: {reason}")
            }
        }
    }
}

impl std::error::Error for BaseError {}

/// The immutable object identity a Workspace is bound to.
///
/// Both widths Git defines are accepted: 40 hexadecimal characters for a
/// SHA-1 repository and 64 for a SHA-256 one. Pantheon does not choose the
/// repository's object format, so refusing the wider one would reject a valid
/// repository for no reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedBase(String);

impl ResolvedBase {
    /// Accepts a resolved object name.
    ///
    /// # Errors
    ///
    /// [`BaseError::NotAnObjectName`] when `value` is not lowercase
    /// hexadecimal of a supported width. Uppercase is refused rather than
    /// normalized: the durable binding is compared for equality against what
    /// a repository reports, and two spellings of one identity would make
    /// that comparison depend on who wrote the row.
    pub fn parse(value: &str) -> Result<Self, BaseError> {
        let supported_width = matches!(value.len(), 40 | 64);
        let canonical = value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if supported_width && canonical {
            Ok(Self(value.to_string()))
        } else {
            Err(BaseError::NotAnObjectName(value.to_string()))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResolvedBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The requested, possibly mutable base a Workspace was asked for.
///
/// Validated as a ref *name*, not as a general revision expression. The
/// Workspace records what a human asked for and resolves it exactly once; a
/// revision expression such as `main@{yesterday}` would make the requested
/// side of that pair depend on when it was read, and `HEAD~3` would make it
/// depend on state the record does not carry.
///
/// The rules below are the subset of `git check-ref-format` that keeps a
/// value from being reinterpreted as something other than a ref name. They
/// are a validation boundary, not a security boundary: nothing downstream may
/// treat a value that passed this check as trusted for any other purpose.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestedBase(String);

impl RequestedBase {
    /// The longest requested base Pantheon accepts.
    ///
    /// Ref names have no length limit in Git, but a durable column does, and
    /// a bound stated here fails a caller's input at the boundary rather than
    /// as a `CHECK` violation halfway through a transaction.
    pub const MAX_LEN: usize = 255;

    /// Accepts a requested base ref name.
    ///
    /// # Errors
    ///
    /// [`BaseError::NotARefName`] naming which rule the value broke.
    pub fn parse(value: &str) -> Result<Self, BaseError> {
        let refuse = |reason: &'static str| {
            Err(BaseError::NotARefName {
                value: value.to_string(),
                reason,
            })
        };

        if value.is_empty() {
            return refuse("it is empty");
        }
        if value.len() > Self::MAX_LEN {
            return refuse("it is longer than 255 characters");
        }
        // A leading `-` would be read as an option by any command the value
        // is ever passed to, whatever quoting is used.
        if value.starts_with('-') {
            return refuse("it starts with '-'");
        }
        if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
            return refuse("it has an empty path component");
        }
        if value.ends_with('.') || value.ends_with(".lock") || value.contains("/.") {
            return refuse("a component starts or ends with a reserved character");
        }
        if value.contains("..") || value.contains("@{") {
            return refuse("it contains a revision-expression sequence");
        }
        for ch in value.chars() {
            if ch.is_ascii_control() || ch == '\u{7f}' {
                return refuse("it contains a control character");
            }
            if matches!(ch, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\') {
                return refuse("it contains a character Git forbids in a ref name");
            }
        }
        Ok(Self(value.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestedBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of the Task input a coding Workspace materializes.
///
/// `docs/architecture/tasks/task-object.md` names the repository input
/// `repository` in both of its worked examples, and the reference it carries
/// is documented as opaque — so Pantheon binds the Workspace by the input's
/// *name* and records the reference verbatim rather than parsing a URI shape
/// no contract defines.
pub const REPOSITORY_INPUT: &str = "repository";

#[cfg(test)]
mod tests;
