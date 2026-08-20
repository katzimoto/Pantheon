//! Configuration publication: compiling candidates, activating them, and
//! keeping the process-local snapshot consistent with durable authority.
//!
//! `docs/architecture/operations/configuration-and-policy-revisions.md` §10
//! names the hazard this module exists to prevent:
//!
//! > Pantheon must prevent requests from observing a database-active new
//! > revision while process-local compiled configuration still points at the
//! > old revision.
//!
//! and §19 names the other one: restart loads the durable active revision and
//! reconciles source drift *separately*, because "ordinary restart is not
//! configuration deployment".
//!
//! # The publication barrier
//!
//! [`ConfigurationAuthority::activate`] holds the publication lock across both
//! the commit and the in-memory swap, so there is no window in which the
//! database says one revision is active and a caller can still read the
//! previous snapshot. The lock is taken before the commit rather than after,
//! because taking it afterwards is exactly the window it is supposed to close.
//!
//! # Why source is never authority
//!
//! On startup the durable revision is the authority. Source files are consulted
//! only to *recover the compiled semantics* of that same revision, and only
//! when they compile to a byte-identical content digest. If they do not, the
//! installation is drifted: the durable identity still governs, the source is
//! not activated, and [`ConfigurationAuthority::status`] reports the drift for
//! an operator to act on deliberately.

use std::sync::{Mutex, PoisonError};

use pantheon_core::config::compile::compile;
use pantheon_core::config::revision::source_set_digest;
use pantheon_core::config::{CompiledConfiguration, ConfigError, Digest};
use pantheon_store::{ActiveConfiguration, Command, Committed, Store, StoreError};

/// A named configuration source file and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub name: String,
    pub text: String,
}

/// The operator's configuration source inputs.
///
/// A set rather than a single file so provenance can record what was supplied,
/// but the MVP compiles exactly one shape — the mission excludes arbitrary
/// layering and deep merge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceSet {
    files: Vec<SourceFile>,
}

impl SourceSet {
    /// Builds a source set from one primary document.
    #[must_use]
    pub fn single(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            files: vec![SourceFile {
                name: name.into(),
                text: text.into(),
            }],
        }
    }

    /// The provenance digest of this source set.
    #[must_use]
    pub fn digest(&self) -> Digest {
        let pairs: Vec<(String, String)> = self
            .files
            .iter()
            .map(|file| (file.name.clone(), file.text.clone()))
            .collect();
        source_set_digest(&pairs)
    }

    /// Compiles the source set into a candidate.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when the source is malformed, invalid, or internally
    /// inconsistent. Compilation is pure, so a rejected candidate cannot have
    /// disturbed anything.
    pub fn compile(&self) -> Result<CompiledConfiguration, ConfigError> {
        let primary = self.files.first().ok_or(ConfigError::MissingField {
            path: "sources".to_string(),
        })?;
        compile(&primary.text)
    }
}

/// What the installation's configuration currently is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigurationStatus {
    /// No revision has ever been activated. The daemon is not ready for
    /// authority-bearing work.
    Uninitialized,
    /// The active revision matches the source set on disk.
    Active { active: ActiveConfiguration },
    /// The active revision governs, and the source set on disk differs from
    /// the one it was compiled from. Diagnosable, never self-applying.
    Drifted {
        active: ActiveConfiguration,
        source_set_digest: Digest,
    },
}

impl ConfigurationStatus {
    /// The active revision, if one exists.
    #[must_use]
    pub const fn active(&self) -> Option<&ActiveConfiguration> {
        match self {
            Self::Uninitialized => None,
            Self::Active { active } | Self::Drifted { active, .. } => Some(active),
        }
    }

    /// Whether the source set differs from the active revision's.
    #[must_use]
    pub const fn is_drifted(&self) -> bool {
        matches!(self, Self::Drifted { .. })
    }
}

/// A failure at the configuration control boundary.
#[derive(Debug)]
pub enum ConfigurationError {
    /// The candidate is not valid configuration. The active revision is
    /// untouched — compilation never writes.
    Invalid(ConfigError),
    /// The durable store rejected or could not perform the activation.
    Store(StoreError),
    /// The published snapshot could not be reached.
    Unavailable(String),
}

impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(err) => write!(f, "invalid configuration candidate: {err}"),
            Self::Store(err) => write!(f, "configuration store failure: {err}"),
            Self::Unavailable(detail) => write!(f, "configuration unavailable: {detail}"),
        }
    }
}

impl std::error::Error for ConfigurationError {}

impl From<ConfigError> for ConfigurationError {
    fn from(err: ConfigError) -> Self {
        Self::Invalid(err)
    }
}

impl From<StoreError> for ConfigurationError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

/// The published process-local snapshot.
///
/// The identity and the semantics are constructed together and checked against
/// each other, so a snapshot carrying one revision's identity and another's
/// compiled configuration is unrepresentable. That pairing is the whole point:
/// a mixed-generation snapshot is precisely the state the publication barrier
/// exists to prevent, and making it a constructor invariant means no future
/// publication path can reintroduce it by ordering its writes badly.
#[derive(Debug, Clone)]
pub struct Snapshot {
    active: ActiveConfiguration,
    compiled: Option<CompiledConfiguration>,
}

impl Snapshot {
    /// Pairs an identity with the semantics of that same revision.
    ///
    /// # Errors
    ///
    /// [`ConfigurationError::Unavailable`] when `compiled` is not the
    /// configuration `active` identifies.
    fn new(
        active: ActiveConfiguration,
        compiled: Option<CompiledConfiguration>,
    ) -> Result<Self, ConfigurationError> {
        if let Some(candidate) = &compiled
            && candidate.revision_digest() != active.content_digest
        {
            {
                return Err(ConfigurationError::Unavailable(format!(
                    "refusing to publish semantics for {} under revision {} identity",
                    candidate.revision_digest(),
                    active.content_digest
                )));
            }
        }
        Ok(Self { active, compiled })
    }

    /// The identity of the published revision.
    #[must_use]
    pub const fn active(&self) -> &ActiveConfiguration {
        &self.active
    }

    /// The compiled semantics of the published revision.
    ///
    /// `None` when the durable revision is active but its source is drifted,
    /// so the semantics could not be recovered — identity still governs.
    #[must_use]
    pub const fn compiled(&self) -> Option<&CompiledConfiguration> {
        self.compiled.as_ref()
    }
}

/// Owns the process-local view of configuration authority.
#[derive(Debug)]
pub struct ConfigurationAuthority<'store> {
    store: &'store Store,
    /// The publication barrier. Held across commit and swap so the two cannot
    /// be observed out of step.
    published: Mutex<Option<Snapshot>>,
}

impl<'store> ConfigurationAuthority<'store> {
    /// Creates an authority with nothing published yet.
    #[must_use]
    pub const fn new(store: &'store Store) -> Self {
        Self {
            store,
            published: Mutex::new(None),
        }
    }

    /// Loads durable authority at startup and publishes it.
    ///
    /// `sources` is consulted only to recover the compiled semantics of the
    /// revision the database already says is active. It is never activated.
    ///
    /// # Errors
    ///
    /// [`ConfigurationError`] when durable state cannot be read or interpreted.
    /// Failing closed here is deliberate: serving authority-bearing work
    /// against a revision Pantheon cannot interpret is worse than not serving.
    pub fn load(&self, sources: &SourceSet) -> Result<ConfigurationStatus, ConfigurationError> {
        let pointer = self.store.configuration_pointer()?;
        let Some(active) = pointer.active else {
            *self.lock()? = None;
            return Ok(ConfigurationStatus::Uninitialized);
        };

        // Recompile the source only to see whether it *is* the active
        // revision. A mismatch is drift, never an activation.
        let recovered = sources
            .compile()
            .ok()
            .filter(|candidate| candidate.revision_digest() == active.content_digest);
        let source_digest = sources.digest();
        let drifted = recovered.is_none() || source_digest != active.source_set_digest;

        *self.lock()? = Some(Snapshot::new(active.clone(), recovered)?);

        Ok(if drifted {
            ConfigurationStatus::Drifted {
                active,
                source_set_digest: source_digest,
            }
        } else {
            ConfigurationStatus::Active { active }
        })
    }

    /// Compiles `sources` and activates the result as the new authority.
    ///
    /// The publication lock is held across the durable commit and the snapshot
    /// swap, so no caller can observe a database-active revision that the
    /// process has not published.
    ///
    /// # Errors
    ///
    /// [`ConfigurationError::Invalid`] when the candidate does not compile —
    /// nothing is written. [`ConfigurationError::Store`] when the activation
    /// is rejected, in which case the previous revision remains completely
    /// authoritative and the published snapshot is left untouched.
    pub fn activate(
        &self,
        command: &Command<'_>,
        sources: &SourceSet,
    ) -> Result<Committed<ActiveConfiguration>, ConfigurationError> {
        // Compile before taking the barrier: it is pure, it is the expensive
        // part, and a rejected candidate must not stall authority-bearing work.
        let compiled = sources.compile()?;
        let source_digest = sources.digest();

        let mut published = self.lock()?;
        let expected = self.store.configuration_pointer()?.revision;

        let committed =
            self.store
                .activate_configuration(command, &compiled, source_digest, expected)?;

        // Same critical section as the commit. A failure above returns without
        // touching the snapshot, which is what leaves the old revision whole.
        let active = match &committed {
            Committed::Executed { value, .. } => value.clone(),
            // A replay changed nothing durably, so the published snapshot must
            // not move either. Re-read rather than assume.
            Committed::Replayed { .. } => {
                let pointer = self.store.configuration_pointer()?;
                match pointer.active {
                    Some(active) => active,
                    None => return Ok(committed),
                }
            }
        };
        // Constructed after the commit and checked against the identity the
        // database actually recorded, so a divergent publication is a typed
        // failure rather than a silently mixed snapshot.
        *published = Some(Snapshot::new(active, Some(compiled))?);

        Ok(committed)
    }

    /// The currently published snapshot.
    ///
    /// # Errors
    ///
    /// [`ConfigurationError::Unavailable`] when nothing has been published, or
    /// when the barrier was poisoned by a panic.
    pub fn snapshot(&self) -> Result<Snapshot, ConfigurationError> {
        self.lock()?.clone().ok_or_else(|| {
            ConfigurationError::Unavailable(
                "no configuration has been published; the daemon is not ready".to_string(),
            )
        })
    }

    /// The durable status, recomputed against `sources`.
    ///
    /// # Errors
    ///
    /// [`ConfigurationError`] when durable state cannot be read.
    pub fn status(&self, sources: &SourceSet) -> Result<ConfigurationStatus, ConfigurationError> {
        let pointer = self.store.configuration_pointer()?;
        let Some(active) = pointer.active else {
            return Ok(ConfigurationStatus::Uninitialized);
        };
        let source_digest = sources.digest();
        Ok(if source_digest == active.source_set_digest {
            ConfigurationStatus::Active { active }
        } else {
            ConfigurationStatus::Drifted {
                active,
                source_set_digest: source_digest,
            }
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Option<Snapshot>>, ConfigurationError> {
        self.published.lock().map_err(|err: PoisonError<_>| {
            let _ = err;
            ConfigurationError::Unavailable(
                "the configuration publication barrier is poisoned".to_string(),
            )
        })
    }
}

#[cfg(test)]
mod tests;
