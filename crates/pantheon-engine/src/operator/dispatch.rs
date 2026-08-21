//! Dispatch desired state and the scheduling cycle, as Operator Control
//! serves them.
//!
//! `docs/architecture/operations/public-daemon-api-and-cli.md` ("Dispatch
//! control") is canonical: pause/resume mutate durable
//! `scheduler_state.dispatch_mode` through the normal command envelope, and
//! the status read distinguishes the operator's *desired* mode from the
//! *effective* ability to commit new Runs. The recovery barrier is reported
//! through readiness as unimplemented rather than asserted here; it is not a
//! gate this build can claim to evaluate.

use std::borrow::Borrow;

use pantheon_core::scheduling::DispatchMode;
use pantheon_store::{Store, StoreError};

use crate::operator::{CommandIdentity, OperatorError, OperatorService};
use crate::routing::ExecutorBackendPort;
pub use crate::scheduling::ScheduleOutcome;
use crate::scheduling::{SchedulingController, SchedulingError};

/// The dispatch view `GET /api/v1/dispatch` exposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchView {
    /// The durable operator desired state.
    pub desired_mode: DispatchMode,
    /// The scheduler singleton revision — the concurrency token an ETag is
    /// derived from and the expected revision a pause/resume must carry.
    pub revision: i64,
    /// Whether new T3 Run-intent commits are effectively possible right now.
    pub effective_can_dispatch: bool,
    /// The normalized set of current factual gates, never a second
    /// desired-state field.
    pub blocked_by: Vec<&'static str>,
}

impl<S: Borrow<Store>> OperatorService<'_, S> {
    /// The current dispatch view.
    ///
    /// # Errors
    ///
    /// [`OperatorError::Internal`] when durable state cannot be read.
    pub fn dispatch_status(&self) -> Result<DispatchView, OperatorError> {
        let state = self.store.scheduler_state()?;
        let mut blocked_by = Vec::new();
        if state.dispatch_mode == DispatchMode::Paused {
            blocked_by.push("operator-pause");
        }
        if !self.configuration_usable() {
            blocked_by.push("configuration");
        }
        Ok(DispatchView {
            desired_mode: state.dispatch_mode,
            revision: state.revision.get(),
            effective_can_dispatch: blocked_by.is_empty(),
            blocked_by,
        })
    }

    /// Commits `PAUSED` as the durable desired state.
    ///
    /// # Errors
    ///
    /// [`OperatorError::StaleRevision`] when `expected` is not the current
    /// singleton revision, or the mapped store failure.
    pub fn pause_dispatch(
        &self,
        command: &CommandIdentity,
        expected: i64,
    ) -> Result<DispatchView, OperatorError> {
        self.set_dispatch(command, DispatchMode::Paused, "dispatch.paused", expected)
    }

    /// Commits `RUNNING` as the durable desired state.
    ///
    /// # Errors
    ///
    /// [`OperatorError::StaleRevision`] when `expected` is not the current
    /// singleton revision, or the mapped store failure.
    pub fn resume_dispatch(
        &self,
        command: &CommandIdentity,
        expected: i64,
    ) -> Result<DispatchView, OperatorError> {
        self.set_dispatch(command, DispatchMode::Running, "dispatch.resumed", expected)
    }

    fn set_dispatch(
        &self,
        command: &CommandIdentity,
        mode: DispatchMode,
        event: &'static str,
        expected: i64,
    ) -> Result<DispatchView, OperatorError> {
        // The revision expectation is decided inside the command envelope, so
        // a genuine replay of an already-committed pause/resume reconciles to
        // its prior outcome even though the world has since moved.
        let committed = self.store.set_dispatch_mode(
            &command.command(event),
            mode,
            pantheon_store::Revision::new(expected),
        );
        if let Err(StoreError::RevisionConflict {
            table: "scheduler_state",
            ..
        }) = &committed
        {
            let current = self.store.scheduler_state()?;
            return Err(OperatorError::StaleRevision {
                detail: format!(
                    "dispatch desired state is at revision {}; the request carried {expected}",
                    current.revision.get()
                ),
            });
        }
        committed?;
        self.dispatch_status()
    }

    /// Whether a usable ConfigurationRevision is published and loaded.
    fn configuration_usable(&self) -> bool {
        self.configuration
            .snapshot()
            .map(|snapshot| snapshot.compiled().is_some())
            .unwrap_or(false)
    }

    /// Runs one full scheduling cycle with the composition's backends.
    ///
    /// This is the production entry point the daemon's supervised loop calls;
    /// every stage it performs is authoritative or side-effect-free, so a
    /// cycle that ends anywhere before T3 has written at most durable backoff.
    ///
    /// # Errors
    ///
    /// [`SchedulingError`] when authority cannot be read or refuses a
    /// non-contention mutation.
    pub fn schedule_once(
        &self,
        backends: &[ExecutorBackendPort<'_>],
    ) -> Result<ScheduleOutcome, SchedulingError> {
        SchedulingController::new(self.store, self.configuration).schedule_once(backends)
    }
}

impl From<SchedulingError> for OperatorError {
    fn from(err: SchedulingError) -> Self {
        match err {
            SchedulingError::Store(store) => store.into(),
            // A cycle cannot run without usable configuration. That is a
            // readiness fact about the daemon, not a defect in anything.
            SchedulingError::Configuration(detail) => Self::NotReady(detail.to_string()),
        }
    }
}
