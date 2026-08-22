//! The Agent Control gateway: the restricted worker-facing control surface.
//!
//! `docs/architecture/execution/agent-control-channel.md` is canonical for
//! this boundary. The rules that shape everything here:
//!
//! - **A separate surface and principal.** This gateway is not Operator
//!   Control: it dispatches a closed set of worker operations
//!   ([`AgentOperation`]-equivalent semantics: `artifact.seal`,
//!   `task.submit_result`, plus the minimal `session.describe` read those
//!   operations need to be buildable) against an Attempt-bound principal,
//!   never the operator's command-identity headers, and it exposes none of
//!   the operator verbs. The concrete transport that reaches it stays a
//!   backend-adapter concern (the channel contract is transport-neutral);
//!   what must not happen — an Agent credential reaching operator handlers,
//!   or operator routes growing worker verbs — is structurally impossible
//!   here because no code path connects the two.
//! - **Authority is derived, never supplied.** A request carries only its
//!   credential, an Attempt-scoped request ID, and bounded semantic payload
//!   (an output slot; slot-to-Artifact-digest mappings). Run, Task, Goal,
//!   Workspace, capture root, base revision and provenance are all derived
//!   server-side from durable state.
//! - **Raw bearer hygiene.** The bearer exists only inside the caller's
//!   [`Bearer`] value; this module immediately reduces it to the SHA-256
//!   verifier form the store already persists and never persists, logs or
//!   embeds the raw bytes anywhere else.
//!
//! Orchestration lives in this crate per the implementation map; transport
//! and persistence do not.

use std::fmt;
use std::path::PathBuf;

use pantheon_core::candidate::{CandidateError, CandidateResult};
use pantheon_core::config::Digest;
use pantheon_store::Store;
use pantheon_store::{
    AgentCredential, AgentOperation, AgentRequestOpened, AgentRequestState,
    AgentSessionDescription, CandidateSubmission, SealAuthority as StoreSealAuthority,
    SubmissionOutcome,
};

use crate::run::Bearer;
use crate::sealing::{
    ChangesetSealer, SealCommand, SealError, SealRequest, SealedArtifact, WorkspaceTreeCapture,
};
use crate::sealing::{ContentObjectStore, TrustedBaseReader};

/// What a worker presents on every request: which Attempt it is (routing)
/// and the raw bearer it was launched with (authentication material).
///
/// `Debug` inherits [`Bearer`]'s redaction — the raw bytes never render.
#[derive(Debug)]
pub struct WorkerCredential<'a> {
    pub attempt_id: &'a str,
    pub bearer: &'a Bearer,
}

impl WorkerCredential<'_> {
    /// The only credential-derived material that ever crosses into durable
    /// state: the SHA-256 verifier, exactly as T4/T4a/T4b persist it.
    #[must_use]
    pub fn verifier(&self) -> [u8; 32] {
        *Digest::of(self.bearer.expose().as_bytes()).as_bytes()
    }
}

/// Builds the store-facing view of one credential. The caller owns the
/// verifier bytes for as long as the view is used.
fn store_view<'a>(
    credential: &'a WorkerCredential<'_>,
    verifier: &'a [u8; 32],
) -> AgentCredential<'a> {
    AgentCredential {
        attempt_id: credential.attempt_id,
        verifier,
    }
}

/// Why one gateway operation could not proceed.
#[derive(Debug)]
pub enum AgentControlError {
    /// A typed refusal from the authoritative store.
    Store(pantheon_store::StoreError),
    /// A typed refusal from the sealing path.
    Seal(SealError),
    /// The request had already been durably recorded as definitively refused;
    /// same-ID retries reconcile this outcome rather than re-running work.
    RequestRefused { problem_code: &'static str },
    /// The proposed output mapping could not become a Candidate.
    Candidate(CandidateError),
}

impl fmt::Display for AgentControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(f, "{error}"),
            Self::Seal(error) => write!(f, "{error}"),
            Self::RequestRefused { problem_code } => {
                write!(f, "the agent request was durably refused ({problem_code})")
            }
            Self::Candidate(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AgentControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Seal(error) => Some(error),
            _ => None,
        }
    }
}

impl From<pantheon_store::StoreError> for AgentControlError {
    fn from(error: pantheon_store::StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<SealError> for AgentControlError {
    fn from(error: SealError) -> Self {
        Self::Seal(error)
    }
}

impl From<CandidateError> for AgentControlError {
    fn from(error: CandidateError) -> Self {
        Self::Candidate(error)
    }
}

/// How one `artifact.seal` request resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSealOutcome {
    /// This call drove the seal to publication.
    Executed(SealedArtifact),
    /// An identical prior request had already succeeded; the recorded result
    /// was returned without re-running any capture.
    Reconciled(Digest),
    /// An identical prior request had already been definitively refused.
    Refused(&'static str),
}

/// A committed `task.submit_result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedCandidate {
    pub candidate_digest: String,
    /// True when an identical prior request was reconciled instead of
    /// executed.
    pub reconciled: bool,
}

/// The restricted worker operations, dispatched for one authenticated
/// Attempt at a time.
pub struct AgentControlGateway<'a> {
    store: &'a Store,
    capture: &'a dyn WorkspaceTreeCapture,
    base: &'a dyn TrustedBaseReader,
    objects: &'a dyn ContentObjectStore,
    workspace_root: PathBuf,
}

/// One `task.submit_result` request body.
#[derive(Debug)]
pub struct SubmitResultRequest<'a> {
    /// The Attempt-scoped idempotency identity of this request.
    pub request_id: &'a str,
    /// The Task status revision the caller observed through
    /// the Task status revision observed through
    /// [`AgentControlGateway::describe`](AgentControlGateway::describe);
    /// CAS-checked authoritatively inside T6.
    pub expected_task_revision: pantheon_store::Revision,
    /// The normalized output mapping: declared slot name → sealed Artifact
    /// digest. Slot names are references to the Task's own contract, not
    /// authority; Artifact digests name existing content, not ownership.
    pub outputs: Vec<(String, Digest)>,
}

impl<'a> AgentControlGateway<'a> {
    /// Composes the gateway over the same ports the controller-side sealer
    /// uses. No concrete implementation is named here.
    pub fn new(
        store: &'a Store,
        capture: &'a dyn WorkspaceTreeCapture,
        base: &'a dyn TrustedBaseReader,
        objects: &'a dyn ContentObjectStore,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            store,
            capture,
            base,
            objects,
            workspace_root: workspace_root.into(),
        }
    }

    /// The minimal description behavior the two consequential operations
    /// require: the worker learns its own Task, its current status revision
    /// (the CAS expectation), and the declared output slots. Nothing else —
    /// no other Task's state, no host paths, no credential material.
    ///
    /// # Errors
    ///
    /// Every authentication or authority fence failure surfaces as
    /// [`AgentControlError::Store`]; nothing is ever created.
    pub fn describe(
        &self,
        credential: &WorkerCredential<'_>,
    ) -> Result<AgentSessionDescription, AgentControlError> {
        let verifier = credential.verifier();
        Ok(self
            .store
            .describe_agent_session(store_view(credential, &verifier))?)
    }

    /// `artifact.seal`: seals the authenticated Task's current Workspace
    /// content for one declared output slot into an immutable Artifact.
    ///
    /// Ordering: authenticate + fence + open/reconcile the durable request
    /// row *before* any external effect; derive lineage and seal authority
    /// server-side; delegate to the existing Run-authorized sealing path
    /// (freeze-or-revalidate under Run authority, confined no-follow capture,
    /// CAS-first publication, trusted-base preimages, scope enforcement);
    /// then record success through the same session fence. Every crash window
    /// converges: the frozen fence, content-addressed identities and the
    /// deterministic command identity make a retry land on the same Artifact
    /// and the same ProductionRecord, and a recorded outcome short-circuits
    /// without touching storage again.
    ///
    /// # Errors
    ///
    /// Typed refusals from the fences above; a definitive seal failure is
    /// also recorded (`FAILED`) so retries reconcile instead of repeating
    /// uncertain external work.
    pub fn seal_artifact(
        &self,
        credential: &WorkerCredential<'_>,
        request_id: &str,
        output_slot: &str,
    ) -> Result<AgentSealOutcome, AgentControlError> {
        let request_hash = canonical_request_hash(
            "artifact.seal",
            credential.attempt_id,
            request_id,
            &[("outputSlot", output_slot.into())],
        );

        let verifier = credential.verifier();
        match self.store.open_agent_request(
            store_view(credential, &verifier),
            AgentOperation::SealArtifact,
            request_id,
            &request_hash,
        )? {
            AgentRequestOpened::Reconciled(AgentRequestState::Succeeded { result_ref }) => {
                return parse_recorded_digest(&result_ref).map(AgentSealOutcome::Reconciled);
            }
            AgentRequestOpened::Reconciled(AgentRequestState::Failed { problem_code }) => {
                let code: &'static str =
                    recorded_problem_code(&problem_code).unwrap_or("recorded-refusal");
                return Ok(AgentSealOutcome::Refused(code));
            }
            // STARTED (freshly opened or left by a crash mid-flight):
            // drive the seal to convergence below.
            AgentRequestOpened::Started | AgentRequestOpened::Reconciled(_) => {}
        }

        let sealed = self.drive_seal(credential, request_hash, request_id, output_slot);

        if let Err(error) = sealed {
            // Record the definitive refusal through the same fence, so a
            // same-ID retry reconciles instead of re-running capture. The
            // record itself fails closed if the session was fenced mid-flight.
            let fail_verifier = credential.verifier();
            let _ = self.store.fail_agent_request(
                store_view(credential, &fail_verifier),
                request_id,
                problem_code_of(&error),
            );
            return Err(error);
        }
        let sealed = sealed.expect("checked above");

        let complete_verifier = credential.verifier();
        self.store.complete_agent_request(
            store_view(credential, &complete_verifier),
            request_id,
            &sealed.artifact_digest.to_string(),
        )?;
        Ok(AgentSealOutcome::Executed(sealed))
    }

    /// `task.submit_result`: submits the Run's single immutable
    /// CandidateResult and moves execution into finalization/evaluation in
    /// one authoritative transaction.
    ///
    /// The Candidate vocabulary instance is built here — binding the exact
    /// Task and Run ids the authenticated session derives — and handed to the
    /// store's T6, which re-proves every fact before committing anything.
    ///
    /// # Errors
    ///
    /// Typed refusals from T6; nothing partial survives any of them.
    pub fn submit_result(
        &self,
        credential: &WorkerCredential<'_>,
        request: &SubmitResultRequest<'_>,
    ) -> Result<SubmittedCandidate, AgentControlError> {
        // Identity, not currency: the payload must bind the same Task and Run
        // the authenticated lineage derives, but a committed replay must stay
        // constructible after the original commit moved the lifecycle. T6
        // alone decides whether this request may act.
        let verifier = credential.verifier();
        let context = self
            .store
            .agent_submission_context(store_view(credential, &verifier))?;
        let candidate = CandidateResult::new(context.task_id.clone(), context.run_id.clone(), {
            request.outputs.iter().cloned()
        })?;

        let mut pairs: Vec<(String, String)> = Vec::with_capacity(request.outputs.len() + 2);
        pairs.push(("attempt".into(), credential.attempt_id.to_string()));
        pairs.push(("request".into(), request.request_id.to_string()));
        let request_hash = canonical_request_hash_pairs("task.submit_result", &pairs, &candidate);

        let verifier = credential.verifier();
        let submission = CandidateSubmission {
            credential: store_view(credential, &verifier),
            request_id: request.request_id,
            request_hash: &request_hash,
            candidate: &candidate,
            expected_task_revision: request.expected_task_revision,
        };
        let SubmissionOutcome {
            committed,
            reconciled,
        } = self.store.submit_candidate(&submission)?;

        Ok(SubmittedCandidate {
            candidate_digest: committed.candidate_digest,
            reconciled,
        })
    }

    /// Drives the sealing path once the request row is durably present.
    fn drive_seal(
        &self,
        credential: &WorkerCredential<'_>,
        request_hash: [u8; 32],
        request_id: &str,
        output_slot: &str,
    ) -> Result<SealedArtifact, AgentControlError> {
        // Current authority facts, derived server-side. `describe` proves the
        // whole lineage is still live and returns exactly the identities the
        // seal needs.
        let description = self.describe(credential)?;
        let run_view = self
            .store
            .run_execution_view(&description.run_id)?
            .ok_or_else(|| pantheon_store::StoreError::RunNotFound {
                run_id: description.run_id.clone(),
            })?;

        let generation = self.store.restore_generation()?;
        let command_id = internal_command_id(credential.attempt_id, request_id, &request_hash);
        let authority = StoreSealAuthority {
            run_id: description.run_id.clone(),
            expected_run_revision: run_view.revision,
        };

        let sealer = ChangesetSealer::new(
            self.store,
            self.capture,
            self.base,
            self.objects,
            self.workspace_root.clone(),
        );
        let sealed = sealer.seal(
            &SealCommand {
                epoch: generation.as_str(),
                id: &command_id,
                request_hash: &request_hash,
            },
            &SealRequest {
                task_id: &description.task_id,
                output_slot,
                authority,
                producer_attempt_id: Some(credential.attempt_id),
            },
        )?;
        Ok(sealed)
    }
}

/// The canonical request hash: SHA-256 over the normalized operation and its
/// semantic payload. Never over the bearer or any secret-derived material —
/// the credential participates in authentication, not in request identity.
pub(crate) fn canonical_request_hash(
    operation: &str,
    attempt_id: &str,
    request_id: &str,
    fields: &[(&str, String)],
) -> [u8; 32] {
    use pantheon_core::config::canonical::Value;
    let mut pairs: Vec<(String, Value)> = vec![
        ("attempt".to_string(), Value::string(attempt_id)),
        ("operation".to_string(), Value::string(operation)),
        ("request".to_string(), Value::string(request_id)),
    ];
    for (key, value) in fields {
        pairs.push(((*key).to_string(), Value::string(value)));
    }
    *Digest::of(Value::object(pairs).to_canonical_bytes().as_slice()).as_bytes()
}

fn canonical_request_hash_pairs(
    operation: &str,
    header: &[(String, String)],
    candidate: &CandidateResult,
) -> [u8; 32] {
    use pantheon_core::config::canonical::Value;
    let mut object: Vec<(String, Value)> =
        vec![("operation".to_string(), Value::string(operation))];
    object.extend(
        header
            .iter()
            .map(|(key, value)| (key.clone(), Value::string(value))),
    );
    object.push((
        "candidate".to_string(),
        Value::string(candidate.to_canonical_json()),
    ));
    *Digest::of(Value::object(object).to_canonical_bytes().as_slice()).as_bytes()
}

/// Derives the namespaced internal command identity for one agent request's
/// sealing sub-operations: server-generated Attempt id plus a digest of the
/// request id. Caller text never enters another authority namespace raw, and
/// the derivation is stable across restarts so publication replays converge.
pub(crate) fn internal_command_id(
    attempt_id: &str,
    request_id: &str,
    request_hash: &[u8; 32],
) -> String {
    let _ = request_id; // covered by the hash below
    let short = Digest::of(request_hash).to_hex();
    format!("acs:{attempt_id}:{}", &short[..16])
}

fn parse_recorded_digest(result_ref: &str) -> Result<Digest, AgentControlError> {
    Digest::from_display(result_ref).ok_or_else(|| {
        AgentControlError::Store(pantheon_store::StoreError::InvariantViolated(format!(
            "a recorded agent-request outcome stores an unparsable ref {result_ref}"
        )))
    })
}

/// Maps a recorded problem code back onto the static domain it came from.
fn recorded_problem_code(recorded: &str) -> Option<&'static str> {
    Some(match recorded {
        "output-slot-invalid" => "output-slot-invalid",
        "workspace-state" => "workspace-state",
        "scope-violated" => "scope-violated",
        "ceilings-exceeded" => "ceilings-exceeded",
        "capture-failed" => "capture-failed",
        "task-unusable" => "task-unusable",
        "internal" => "internal",
        "authority-stale" => "authority-stale",
        "invalid-candidate" => "invalid-candidate",
        _ => return None,
    })
}

/// Maps a definitive seal failure onto the bounded problem-code domain the
/// request ledger stores.
fn problem_code_of(error: &AgentControlError) -> &'static str {
    match error {
        AgentControlError::Seal(SealError::OutputSlotInvalid { .. }) => "output-slot-invalid",
        AgentControlError::Seal(SealError::WorkspaceState { .. }) => "workspace-state",
        AgentControlError::Seal(SealError::ScopeViolated { .. }) => "scope-violated",
        AgentControlError::Seal(SealError::CeilingsExceeded { .. }) => "ceilings-exceeded",
        AgentControlError::Seal(SealError::Capture(_)) => "capture-failed",
        AgentControlError::Seal(SealError::TaskUnusable { .. }) => "task-unusable",
        AgentControlError::Seal(SealError::CommandIdentityTooLong { .. })
        | AgentControlError::Seal(SealError::Store(_)) => "internal",
        AgentControlError::Store(_) => "authority-stale",
        AgentControlError::Candidate(_) => "invalid-candidate",
        AgentControlError::RequestRefused { .. } => "already-refused",
    }
}

#[cfg(test)]
mod tests;
