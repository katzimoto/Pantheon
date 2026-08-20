//! System metadata and readiness.
//!
//! `docs/architecture/operations/public-daemon-api-and-cli.md` ("Health and
//! readiness") defines readiness as *recovery barrier passed + active
//! configuration published + control plane safe for new authority-bearing
//! work*. Only the middle conjunct is a fact this build can establish. The
//! other two are reported as unimplemented components rather than asserted,
//! because a readiness endpoint that claims a barrier no code enforces is
//! worse than one that admits the gap.

use std::borrow::Borrow;

use pantheon_core::config::Digest;
use pantheon_store::{Cursor, Store};

use crate::configuration::ConfigurationError;
use crate::operator::{API_VERSIONS, OperatorError, OperatorService};

/// The daemon's own version, from its build.
const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What `GET /api/v1/system` reports.
///
/// Deliberately absent: an installation identity. The contract lists one, but
/// the only installation-scoped identifier Pantheon durably holds is the
/// RestoreGeneration, and that rotates on disaster restore — exactly the
/// property an installation identity must not have. Aliasing the two would
/// make the distinction the same contract calls load-bearing unobservable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemView {
    pub daemon_version: &'static str,
    pub api_versions: &'static [&'static str],
    /// The schema version the open database is migrated to.
    pub schema_version: i64,
    /// The current RestoreGeneration. This *is* the `commandEpoch` a mutation
    /// must carry, so a client reads it here before issuing one.
    pub command_epoch: String,
    /// Event Journal continuity, which rotates independently of the
    /// RestoreGeneration.
    pub journal: JournalView,
    pub active_configuration: Option<ActiveConfigurationView>,
    pub readiness: ReadinessReport,
}

/// Event Journal continuity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalView {
    pub epoch: String,
    /// The last committed sequence, or `None` when nothing has been committed
    /// in this history. Reporting `0` would be a sequence, and sequences start
    /// at 1.
    pub latest_sequence: Option<i64>,
}

impl JournalView {
    fn from_head(head: &Cursor) -> Self {
        Self {
            epoch: head.journal_epoch.clone(),
            latest_sequence: (head.sequence >= 1).then_some(head.sequence),
        }
    }
}

/// The published ConfigurationRevision's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveConfigurationView {
    pub activation_sequence: i64,
    pub content_digest: Digest,
    /// Whether the compiled semantics of that revision are loaded. `false`
    /// means the durable revision is active but its source drifted, so
    /// identity governs and nothing can be planned against it.
    pub semantics_loaded: bool,
}

/// Whether the control plane could safely take on new authority-bearing work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessReport {
    pub ready: bool,
    pub components: Vec<ReadinessComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessComponent {
    pub name: &'static str,
    pub state: ComponentState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentState {
    Satisfied,
    Unsatisfied,
    /// The architecture requires this conjunct and no code establishes it yet.
    /// It never counts toward readiness in either direction; it is reported so
    /// the altitude of the `ready` claim stays visible.
    Unimplemented,
}

impl ComponentState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Unsatisfied => "unsatisfied",
            Self::Unimplemented => "unimplemented",
        }
    }
}

impl<S: Borrow<Store>> OperatorService<'_, S> {
    /// The system metadata view.
    ///
    /// # Errors
    ///
    /// [`OperatorError::Internal`] when durable state cannot be read.
    pub fn system(&self) -> Result<SystemView, OperatorError> {
        let head = self.store.journal_head()?;
        Ok(SystemView {
            daemon_version: DAEMON_VERSION,
            api_versions: API_VERSIONS,
            schema_version: self.store.schema_version()?,
            command_epoch: self.store.restore_generation()?.as_str().to_string(),
            journal: JournalView::from_head(&head),
            active_configuration: self.active_configuration(),
            readiness: self.readiness(),
        })
    }

    /// Whether the process itself is functioning.
    ///
    /// Deliberately touches nothing durable. Liveness answers "should this
    /// process be restarted", and a database that is merely unreachable is a
    /// readiness fact, not a reason to kill a daemon that is still serving.
    #[must_use]
    pub const fn live(&self) -> bool {
        true
    }

    /// Whether new authority-bearing work could safely be admitted.
    #[must_use]
    pub fn readiness(&self) -> ReadinessReport {
        let configuration = match self.configuration.snapshot() {
            Ok(snapshot) if snapshot.compiled().is_some() => ReadinessComponent {
                name: "active-configuration",
                state: ComponentState::Satisfied,
                detail: None,
            },
            Ok(_) => ReadinessComponent {
                name: "active-configuration",
                state: ComponentState::Unsatisfied,
                detail: Some(
                    "the active revision's source has drifted, so its semantics are not loaded"
                        .to_string(),
                ),
            },
            Err(ConfigurationError::Unavailable(detail)) => ReadinessComponent {
                name: "active-configuration",
                state: ComponentState::Unsatisfied,
                detail: Some(detail),
            },
            Err(err) => ReadinessComponent {
                name: "active-configuration",
                state: ComponentState::Unsatisfied,
                detail: Some(err.to_string()),
            },
        };

        let components = vec![
            ReadinessComponent {
                name: "recovery-barrier",
                state: ComponentState::Unimplemented,
                detail: Some(
                    "no startup recovery barrier exists in this build; readiness does not \
                     assert one was passed"
                        .to_string(),
                ),
            },
            configuration,
            ReadinessComponent {
                name: "dispatch",
                state: ComponentState::Unimplemented,
                detail: Some("no scheduler or dispatch surface exists in this build".to_string()),
            },
        ];

        // `ready` is the conjunction over the components this build actually
        // establishes. An unimplemented component neither satisfies nor
        // blocks: pretending either way would make the flag say something the
        // code cannot back.
        let ready = components
            .iter()
            .all(|component| component.state != ComponentState::Unsatisfied);
        ReadinessReport { ready, components }
    }

    fn active_configuration(&self) -> Option<ActiveConfigurationView> {
        let snapshot = self.configuration.snapshot().ok()?;
        Some(ActiveConfigurationView {
            activation_sequence: snapshot.active().activation_sequence,
            content_digest: snapshot.active().content_digest,
            semantics_loaded: snapshot.compiled().is_some(),
        })
    }
}
