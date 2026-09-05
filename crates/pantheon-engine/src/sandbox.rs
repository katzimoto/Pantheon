//! The SandboxBackend port and Sandbox lifecycle controller.
//!
//! `docs/architecture/security/sandbox-broker-and-isolation.md` is canonical
//! for what a Sandbox is and what it must guarantee. This module owns the
//! abstract port concrete backends implement and the controller that
//! orchestrates durable identity, provisioning, verification, and release.

use std::fmt;

pub use pantheon_core::sandbox::{
    SandboxKey, SandboxPlan, SandboxPresence, SandboxProbeResult, SandboxVerification,
};
use pantheon_store::{
    Command, Committed, Revision, SandboxBinding, SandboxProbeEvidence, SandboxRecord, Store,
    StoreError,
};

/// Why a sandbox operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxError {
    pub detail: String,
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for SandboxError {}

/// The narrow port a concrete SandboxBackend must implement.
///
/// Implementations are responsible for factual physical isolation, not for
/// self-awarding guarantees. Pantheon verifies every claim before execution.
pub trait SandboxBackend: fmt::Debug {
    /// Ensure a Sandbox matching `key` and `plan` exists in the external
    /// runtime. Idempotent: repeating with the same key must address the
    /// same logical Sandbox.
    fn ensure_sandbox(
        &self,
        key: &SandboxKey,
        plan: &SandboxPlan,
    ) -> Result<SandboxPresence, SandboxError>;

    /// Inspect whether a Sandbox keyed by `key` currently exists.
    fn inspect_sandbox(&self, key: &SandboxKey) -> Result<SandboxPresence, SandboxError>;

    /// Request release of the Sandbox keyed by `key`. The caller must
    /// separately verify external absence before declaring the Sandbox
    /// Released.
    fn release_sandbox(&self, key: &SandboxKey) -> Result<(), SandboxError>;

    /// Verify that the external Sandbox matches the durable plan.
    ///
    /// Returns a [`SandboxVerification`] with every required fact checked.
    /// A detected invariant violation is a system/security failure and the
    /// verification fails closed.
    fn verify_sandbox(
        &self,
        key: &SandboxKey,
        plan: &SandboxPlan,
    ) -> Result<SandboxVerification, SandboxError>;
}

/// Orchestrates Sandbox lifecycle for one Run.
#[derive(Debug)]
pub struct SandboxController<'store> {
    store: &'store Store,
}

impl<'store> SandboxController<'store> {
    #[must_use]
    pub const fn new(store: &'store Store) -> Self {
        Self { store }
    }

    /// Creates durable Sandbox identity for a Run and begins provisioning.
    ///
    /// The SandboxKey is derived deterministically from the Run ID so
    /// recovery can reconcile the same key after restart.
    pub fn provision(
        &self,
        command: &Command<'_>,
        run_id: &str,
        plan: &SandboxPlan,
        backend: &dyn SandboxBackend,
    ) -> Result<SandboxRecord, SandboxControllerError> {
        let key = derive_sandbox_key(run_id);
        let plan_digest = plan.digest();
        let binding = SandboxBinding {
            run_id,
            sandbox_plan_digest: plan_digest.as_bytes(),
            environment_identity: &plan.environment_identity,
        };

        let record = match self.store.create_sandbox(command, &key, &binding) {
            Ok(Committed::Executed { value, .. }) => value,
            Ok(Committed::Replayed { .. }) => self
                .store
                .sandbox_for_run(run_id)
                .map_err(SandboxControllerError::Store)?
                .ok_or_else(|| {
                    SandboxControllerError::Invariant(format!(
                        "sandbox for run {run_id} reported replayed but not found"
                    ))
                })?,
            Err(StoreError::SandboxAlreadyCurrent { .. }) => self
                .store
                .sandbox_for_run(run_id)
                .map_err(SandboxControllerError::Store)?
                .ok_or_else(|| {
                    SandboxControllerError::Invariant(format!(
                        "sandbox for run {run_id} is current but not found"
                    ))
                })?,
            Err(other) => return Err(SandboxControllerError::Store(other)),
        };

        match record.phase {
            pantheon_core::sandbox::SandboxPhase::Ready => Ok(record),
            pantheon_core::sandbox::SandboxPhase::Error => {
                Err(SandboxControllerError::ProvisioningFailed {
                    sandbox_id: key.as_str().to_string(),
                    detail: "sandbox is in Error phase from a previous provisioning attempt"
                        .to_string(),
                })
            }
            pantheon_core::sandbox::SandboxPhase::Releasing => {
                Err(SandboxControllerError::ProvisioningFailed {
                    sandbox_id: key.as_str().to_string(),
                    detail: "sandbox is in Releasing phase".to_string(),
                })
            }
            pantheon_core::sandbox::SandboxPhase::Requested
            | pantheon_core::sandbox::SandboxPhase::Preparing => {
                let record = if record.phase == pantheon_core::sandbox::SandboxPhase::Requested {
                    let epoch = self
                        .store
                        .restore_generation()
                        .map_err(SandboxControllerError::Store)?;
                    let cmd_begin = Command {
                        epoch: epoch.as_str(),
                        id: &format!("{}-begin-preparation", command.id),
                        request_hash: command.request_hash,
                        event_type: "sandbox.preparation.begun",
                    };
                    let begun = self
                        .store
                        .begin_sandbox_preparation(&cmd_begin, &key, record.revision)
                        .map_err(SandboxControllerError::Store)?;
                    match begun {
                        Committed::Executed { value, .. } => value,
                        Committed::Replayed { .. } => self
                            .store
                            .sandbox_for_run(run_id)
                            .map_err(SandboxControllerError::Store)?
                            .ok_or_else(|| {
                                SandboxControllerError::Invariant(format!(
                                    "sandbox for run {run_id} reported replayed but not found"
                                ))
                            })?,
                    }
                } else {
                    record
                };

                let epoch = self
                    .store
                    .restore_generation()
                    .map_err(SandboxControllerError::Store)?;

                let presence = match backend.ensure_sandbox(&key, plan) {
                    Ok(p) => p,
                    Err(err) => {
                        let cmd_fail = Command {
                            epoch: epoch.as_str(),
                            id: &format!("{}-fail", command.id),
                            request_hash: command.request_hash,
                            event_type: "sandbox.failed",
                        };
                        let _ = self.store.fail_sandbox(
                            &cmd_fail,
                            &key,
                            record.revision,
                            SandboxPresence::Unknown,
                        );
                        return Err(SandboxControllerError::ProvisioningFailed {
                            sandbox_id: key.as_str().to_string(),
                            detail: err.detail,
                        });
                    }
                };

                if presence == SandboxPresence::Present {
                    let verified = backend.verify_sandbox(&key, plan).map_err(|err| {
                        SandboxControllerError::ProvisioningFailed {
                            sandbox_id: key.as_str().to_string(),
                            detail: err.detail,
                        }
                    })?;

                    if !verified.all_passed() {
                        let _ = self.record_probe_evidence(command, &key, &verified);
                        let cmd_fail = Command {
                            epoch: epoch.as_str(),
                            id: &format!("{}-fail", command.id),
                            request_hash: command.request_hash,
                            event_type: "sandbox.failed",
                        };
                        let _ = self.store.fail_sandbox(
                            &cmd_fail,
                            &key,
                            record.revision,
                            SandboxPresence::Unknown,
                        );
                        return Err(SandboxControllerError::VerificationFailed {
                            sandbox_id: key.as_str().to_string(),
                        });
                    }

                    let cmd_complete = Command {
                        epoch: epoch.as_str(),
                        id: &format!("{}-complete-preparation", command.id),
                        request_hash: command.request_hash,
                        event_type: "sandbox.preparation.completed",
                    };
                    let completed = self
                        .store
                        .complete_sandbox_preparation(&cmd_complete, &key, record.revision)
                        .map_err(SandboxControllerError::Store)?;
                    Ok(match completed {
                        Committed::Executed { value, .. } => value,
                        Committed::Replayed { .. } => self
                            .store
                            .sandbox_for_run(run_id)
                            .map_err(SandboxControllerError::Store)?
                            .ok_or_else(|| {
                                SandboxControllerError::Invariant(format!(
                                    "sandbox for run {run_id} reported replayed but not found"
                                ))
                            })?,
                    })
                } else {
                    let cmd_fail = Command {
                        epoch: epoch.as_str(),
                        id: &format!("{}-fail", command.id),
                        request_hash: command.request_hash,
                        event_type: "sandbox.failed",
                    };
                    let _ = self
                        .store
                        .fail_sandbox(&cmd_fail, &key, record.revision, presence);
                    Err(SandboxControllerError::ProvisioningFailed {
                        sandbox_id: key.as_str().to_string(),
                        detail: format!("sandbox did not reach Present: {presence}"),
                    })
                }
            }
            pantheon_core::sandbox::SandboxPhase::Released => {
                Err(SandboxControllerError::Invariant(format!(
                    "sandbox for run {run_id} is current but Released"
                )))
            }
        }
    }

    /// Reconciles an existing non-Released Sandbox against external state.
    ///
    /// Recovery calls this for every Sandbox in the inventory.
    pub fn reconcile(
        &self,
        command: &Command<'_>,
        record: &SandboxRecord,
        backend: &dyn SandboxBackend,
    ) -> Result<ReconciledSandbox, SandboxControllerError> {
        let key = SandboxKey::new(&record.id).map_err(|err| {
            SandboxControllerError::Invariant(format!("invalid stored key: {err}"))
        })?;

        let presence = backend.inspect_sandbox(&key).map_err(|err| {
            SandboxControllerError::InspectionFailed {
                sandbox_id: record.id.clone(),
                detail: err.detail,
            }
        })?;

        let updated = self
            .store
            .update_sandbox_presence(command, &key, record.revision, presence)
            .map_err(SandboxControllerError::Store)?;

        let current = match updated {
            Committed::Executed { value, .. } => value,
            Committed::Replayed { .. } => {
                return self
                    .store
                    .sandbox_for_run(&record.run_id)
                    .map_err(SandboxControllerError::Store)?
                    .map(|r| ReconciledSandbox {
                        record: r,
                        presence,
                    })
                    .ok_or_else(|| {
                        SandboxControllerError::Invariant(format!(
                            "sandbox for run {} reported replayed but not found",
                            record.run_id
                        ))
                    });
            }
        };

        Ok(ReconciledSandbox {
            record: current,
            presence,
        })
    }

    /// Begins release of a Sandbox.
    pub fn begin_release(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
        backend: &dyn SandboxBackend,
    ) -> Result<SandboxRecord, SandboxControllerError> {
        let record = self
            .store
            .begin_sandbox_release(command, sandbox_key, expected)
            .map_err(SandboxControllerError::Store)?;

        if let Err(err) = backend.release_sandbox(sandbox_key) {
            // Release request failed; the Sandbox remains Releasing+Unknown.
            // Recovery will retry.
            return Err(SandboxControllerError::ReleaseFailed {
                sandbox_id: sandbox_key.as_str().to_string(),
                detail: err.detail,
            });
        }

        Ok(match record {
            Committed::Executed { value, .. } => value,
            Committed::Replayed { .. } => self
                .store
                .sandbox_by_id(sandbox_key.as_str())
                .map_err(SandboxControllerError::Store)?
                .ok_or_else(|| {
                    SandboxControllerError::Invariant(format!(
                        "sandbox {} reported replayed but not found",
                        sandbox_key.as_str()
                    ))
                })?,
        })
    }

    /// Completes release once external absence is established.
    pub fn complete_release(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
        observed: SandboxPresence,
    ) -> Result<SandboxRecord, SandboxControllerError> {
        let record = self
            .store
            .complete_sandbox_release(command, sandbox_key, expected, observed)
            .map_err(SandboxControllerError::Store)?;
        Ok(match record {
            Committed::Executed { value, .. } => value,
            Committed::Replayed { .. } => self
                .store
                .sandbox_by_id(sandbox_key.as_str())
                .map_err(SandboxControllerError::Store)?
                .ok_or_else(|| {
                    SandboxControllerError::Invariant(format!(
                        "sandbox {} reported replayed but not found",
                        sandbox_key.as_str()
                    ))
                })?,
        })
    }

    /// Records controller-owned probe evidence for every probe result.
    fn record_probe_evidence(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        verification: &SandboxVerification,
    ) -> Result<(), SandboxControllerError> {
        for result in &verification.probe_results {
            let evidence = SandboxProbeEvidence {
                sandbox_id: sandbox_key.as_str(),
                probe_name: &result.name,
                expected: &result.expected,
                observed: &result.observed,
                passed: result.passed,
                backend_descriptor: &verification.backend_descriptor,
                backend_version: &verification.backend_version,
                platform: &verification.platform,
                architecture: &verification.architecture,
                probe_implementation_version: &verification.probe_implementation_version,
            };
            let cmd = Command {
                epoch: command.epoch,
                id: &format!("{}-probe-{}", command.id, result.name),
                request_hash: command.request_hash,
                event_type: "sandbox.probe.recorded",
            };
            let _ = self.store.record_sandbox_probe_evidence(&cmd, &evidence);
        }
        Ok(())
    }
}

/// The result of reconciling one Sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledSandbox {
    pub record: SandboxRecord,
    pub presence: SandboxPresence,
}

/// A failure along the Sandbox control path.
#[derive(Debug)]
pub enum SandboxControllerError {
    Store(StoreError),
    ProvisioningFailed { sandbox_id: String, detail: String },
    VerificationFailed { sandbox_id: String },
    InspectionFailed { sandbox_id: String, detail: String },
    ReleaseFailed { sandbox_id: String, detail: String },
    Invariant(String),
}

impl fmt::Display for SandboxControllerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(err) => write!(f, "sandbox controller store failure: {err}"),
            Self::ProvisioningFailed { sandbox_id, detail } => {
                write!(f, "sandbox {sandbox_id} provisioning failed: {detail}")
            }
            Self::VerificationFailed { sandbox_id } => {
                write!(f, "sandbox {sandbox_id} verification failed closed")
            }
            Self::InspectionFailed { sandbox_id, detail } => {
                write!(f, "sandbox {sandbox_id} inspection failed: {detail}")
            }
            Self::ReleaseFailed { sandbox_id, detail } => {
                write!(f, "sandbox {sandbox_id} release failed: {detail}")
            }
            Self::Invariant(detail) => write!(f, "sandbox controller invariant: {detail}"),
        }
    }
}

impl std::error::Error for SandboxControllerError {}

impl From<StoreError> for SandboxControllerError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

fn derive_sandbox_key(run_id: &str) -> SandboxKey {
    // Deterministic: the same Run always gets the same SandboxKey,
    // so recovery reconciles rather than duplicates.
    SandboxKey::new(format!("sandbox-{run_id}")).expect("derived key is valid")
}

#[cfg(test)]
mod tests;
