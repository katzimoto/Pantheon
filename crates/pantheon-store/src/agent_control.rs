//! Attempt-bound Agent Control persistence: the session authentication
//! fence, the durable `(attempt, request)` idempotency ledger, and the
//! authoritative Candidate submission transaction (T6).
//!
//! `docs/architecture/execution/agent-control-channel.md` (§7, §12, §13,
//! §18) and `docs/architecture/persistence-and-recovery/sqlite-persistence-
//! and-transactions.md` ("Agent Control", "Candidate submission transaction
//! (T6)") are canonical for what these transactions mean. The rules that
//! shape everything here:
//!
//! - **Authentication is not authorization.** A presented bearer proves only
//!   which Attempt is calling. Every authority fact — Run, Task, Goal,
//!   output ceiling, provenance — is re-derived server-side from the
//!   authenticated Attempt inside whichever transaction acts on it.
//! - **The RestoreGeneration fence precedes request-idempotency
//!   lookup/creation.** An old-generation restored session fails closed
//!   before any [`agent_requests`] row is read or written, even when the
//!   presented bearer still matches the stored verifier.
//! - **`(attempt_id, request_id)` is canonical request identity** after that
//!   fence. Same ID plus the same canonical request hash reconciles the
//!   recorded outcome; same ID plus a different hash fails closed. Credential
//!   revisions and generations deliberately do not enter request identity.
//! - **T6 is one serialized authoritative transaction.** Session, Attempt,
//!   Run, Task, Goal, specification, Artifact and provenance facts are all
//!   re-read inside it, and the Candidate rows, both lifecycle transitions,
//!   the request outcome and the Event commit together or not at all. No
//!   external effect ever happens inside it. Unlike operator commands, T6 is
//!   not itself under the command ledger: its worker-facing idempotency lives
//!   in `agent_requests`, and appending a second operator-command identity
//!   here would create exactly the cross-namespace masquerade the Agent
//!   Control contract forbids.
//!
//! Raw bearer material never reaches this crate: callers present the SHA-256
//! verifier of the raw bearer, the same form T4/T4a/T4b already persist.
//!
//! [`agent_requests`]: crate::agent_control::open_agent_request

use std::fmt;

use pantheon_core::candidate::CandidateResult;
use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value as Json;
use pantheon_core::planning::TaskSpec;

use crate::command::append_internal_event;
use crate::error::StoreError;
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

const REQUEST_TABLE: &str = "agent_requests";

/// The two consequential worker operations this schema knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentOperation {
    /// Seal the current Task Workspace into an authoritative Artifact.
    SealArtifact,
    /// Submit the Run's single immutable CandidateResult.
    SubmitResult,
}

impl AgentOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SealArtifact => "artifact.seal",
            Self::SubmitResult => "task.submit_result",
        }
    }
}

/// The credential a worker presents to one Agent Control operation: which
/// Attempt is calling (routing only — nothing is authorized by naming it)
/// and the SHA-256 verifier of the raw bearer it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCredential<'a> {
    /// The Attempt whose AgentControlSession authenticates this request.
    pub attempt_id: &'a str,
    /// SHA-256 of the raw bearer, computed by the caller that holds the raw
    /// bytes. This crate never sees them.
    pub verifier: &'a [u8; 32],
}

/// The durable state of one Agent Control request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRequestState {
    /// Durably begun; any external effect it drives is still being reconciled.
    Started,
    /// Committed successfully; `result_ref` names the durable outcome.
    Succeeded { result_ref: String },
    /// Definitively refused; `problem_code` names why.
    Failed { problem_code: String },
}

fn request_state_from(
    state: String,
    result_ref: Option<String>,
    problem_code: Option<String>,
) -> Result<AgentRequestState, StoreError> {
    match state.as_str() {
        "STARTED" => Ok(AgentRequestState::Started),
        "SUCCEEDED" => {
            let Some(result_ref) = result_ref else {
                return Err(StoreError::InvariantViolated(format!(
                    "a SUCCEEDED agent request stores no result ref ({state})"
                )));
            };
            Ok(AgentRequestState::Succeeded { result_ref })
        }
        "FAILED" => {
            let Some(problem_code) = problem_code else {
                return Err(StoreError::InvariantViolated(format!(
                    "a FAILED agent request stores no problem code ({state})"
                )));
            };
            Ok(AgentRequestState::Failed { problem_code })
        }
        other => Err(StoreError::InvariantViolated(format!(
            "agent_requests stores an unknown state {other:?}"
        ))),
    }
}

/// What opening a request found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRequestOpened {
    /// No prior row existed; a `STARTED` row was durably recorded before any
    /// external effect.
    Started,
    /// The exact identity and hash had already reached a recorded outcome;
    /// nothing was created or mutated.
    Reconciled(AgentRequestState),
}

/// One declared output slot of the Task an authenticated Attempt serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredOutput {
    pub name: String,
    pub kind: String,
    pub required: bool,
}

/// The minimal description an authenticated worker may read about its own
/// execution lineage: enough to build a well-formed submission, nothing more.
///
/// There is no bearer or verifier material here, and no other Task's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionDescription {
    pub session_id: String,
    pub attempt_id: String,
    pub run_id: String,
    pub task_id: String,
    pub task_phase: String,
    /// The Task status revision a `task.submit_result` CAS expectation is
    /// formed from.
    pub task_revision: Revision,
    pub outputs: Vec<DeclaredOutput>,
}

/// The authoritative input to T6 Candidate submission.
///
/// Only three things come from the worker: the credential, the Attempt-scoped
/// request ID, and the proposed output mapping — plus the Task status
/// revision observed for CAS. Everything authoritative is derived and
/// re-proven inside the transaction.
#[derive(Debug)]
pub struct CandidateSubmission<'a> {
    pub credential: AgentCredential<'a>,
    pub request_id: &'a str,
    /// Digest over the normalized operation and semantic payload — never the
    /// bearer, never secret-derived material.
    pub request_hash: &'a [u8; 32],
    /// The pure Candidate vocabulary instance binding the exact Task, Run and
    /// output mapping. Its embedded ids must equal what the authenticated
    /// lineage derives; the transaction re-proves that rather than trusting
    /// the caller-side derivation.
    pub candidate: &'a CandidateResult,
    /// The Task status revision the caller observed; CAS-checked inside T6.
    pub expected_task_revision: Revision,
}

/// What a committed T6 produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateCommitted {
    pub candidate_digest: String,
    pub task_revision: Revision,
    pub run_revision: Revision,
}

/// Whether this call executed the submission or reconciled an identical
/// committed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionOutcome {
    pub committed: CandidateCommitted,
    pub reconciled: bool,
}

/// Why a session failed its fences, decided deterministically so error text
/// cannot be used to probe rows.
enum SessionFacts {
    Authorized,
    Absent,
    GenerationFenced,
    NotActive(String),
    VerifierMismatch,
}

/// Reads and judges the three authentication facts in one statement, through
/// the writer so the decision belongs to the acting transaction's snapshot.
///
/// The generation fence is evaluated *before* the verifier comparison: an
/// old-generation restored session is fenced regardless of what material a
/// caller presents, and must not even confirm whether a bearer would have
/// matched.
fn judge_session(
    writer: &Writer<'_>,
    credential: &AgentCredential<'_>,
) -> Result<SessionFacts, StoreError> {
    let facts: Option<(String, i64)> = writer.query_optional(
        "SELECT state,
                (restore_generation
                 = (SELECT restore_generation FROM system_state WHERE id = 1))
         FROM agent_control_sessions WHERE attempt_id = ?1",
        &[Value::from(credential.attempt_id)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some((state, generation_current)) = facts else {
        return Ok(SessionFacts::Absent);
    };
    if generation_current == 0 {
        return Ok(SessionFacts::GenerationFenced);
    }
    if state != "ACTIVE" {
        return Ok(SessionFacts::NotActive(state));
    }
    let matches: Option<i64> = writer.query_optional(
        "SELECT credential_hash = ?2 FROM agent_control_sessions WHERE attempt_id = ?1",
        &[
            Value::from(credential.attempt_id),
            Value::Blob(credential.verifier.as_slice().to_vec()),
        ],
        |row| row.get(0),
    )?;
    match matches {
        Some(1) => Ok(SessionFacts::Authorized),
        Some(_) => Ok(SessionFacts::VerifierMismatch),
        None => Ok(SessionFacts::Absent),
    }
}

/// Applies the authentication fence inside a write transaction: authorized
/// passes; anything else fails closed with nothing written.
///
/// # Errors
///
/// [`StoreError::AgentControlUnauthorized`] on every failure class.
fn authenticate_agent_session(
    writer: &Writer<'_>,
    credential: &AgentCredential<'_>,
) -> Result<(), StoreError> {
    let unauthorized = |reason: &'static str| StoreError::AgentControlUnauthorized {
        attempt_id: credential.attempt_id.to_string(),
        reason,
    };
    match judge_session(writer, credential)? {
        SessionFacts::Authorized => Ok(()),
        SessionFacts::Absent => Err(unauthorized(
            "no AgentControlSession exists for the Attempt",
        )),
        SessionFacts::GenerationFenced => Err(unauthorized(
            "the session belongs to an older RestoreGeneration",
        )),
        SessionFacts::NotActive(state) => Err(unauthorized(match state.as_str() {
            "REVOKED" => "the session is REVOKED",
            _ => "the session is not ACTIVE",
        })),
        SessionFacts::VerifierMismatch => Err(unauthorized(
            "the presented bearer does not match the session verifier",
        )),
    }
}

impl Store {
    /// Describes the execution lineage behind an authenticated session.
    ///
    /// Read-only, so it performs no request-ledger work and creates nothing.
    /// Multi-statement by necessity — session, Attempt, Run, Task and the
    /// frozen specification must agree — so it runs inside one explicit read
    /// transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError::AgentControlUnauthorized`] on any failed authentication
    /// or authority fence; [`StoreError`] when durable state cannot be read
    /// or interpreted.
    pub fn describe_agent_session(
        &self,
        credential: AgentCredential<'_>,
    ) -> Result<AgentSessionDescription, StoreError> {
        self.read_snapshot(|conn| {
            let facts: Option<(String, i64)> = conn
                .query_row(
                    "SELECT state,
                            (restore_generation
                             = (SELECT restore_generation FROM system_state WHERE id = 1))
                     FROM agent_control_sessions WHERE attempt_id = ?1",
                    rusqlite::params![credential.attempt_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            let Some((state, generation_current)) = facts else {
                return Err(StoreError::AgentControlUnauthorized {
                    attempt_id: credential.attempt_id.to_string(),
                    reason: "no AgentControlSession exists for the Attempt",
                });
            };
            if generation_current == 0 {
                return Err(StoreError::AgentControlUnauthorized {
                    attempt_id: credential.attempt_id.to_string(),
                    reason: "the session belongs to an older RestoreGeneration",
                });
            }
            if state != "ACTIVE" {
                return Err(StoreError::AgentControlUnauthorized {
                    attempt_id: credential.attempt_id.to_string(),
                    reason: "the session is not ACTIVE",
                });
            }
            let verified: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM agent_control_sessions
                     WHERE attempt_id = ?1 AND credential_hash = ?2",
                    rusqlite::params![credential.attempt_id, credential.verifier.as_slice()],
                    |row| row.get(0),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            if verified.is_none() {
                return Err(StoreError::AgentControlUnauthorized {
                    attempt_id: credential.attempt_id.to_string(),
                    reason: "the presented bearer does not match the session verifier",
                });
            }

            let lineage: Option<(String, String, String, String, i64, Vec<u8>)> = conn
                .query_row(
                    "SELECT acs.id, rs.run_id, t.id, t.phase, t.revision, t.spec_digest
                     FROM agent_control_sessions acs
                     JOIN attempts a ON a.id = acs.attempt_id
                     JOIN attempt_status st ON st.attempt_id = a.id AND st.terminal = 0
                     JOIN run_status rs ON rs.run_id = a.run_id
                         AND rs.phase = 'Active'
                         AND rs.current_attempt_id = acs.attempt_id
                     JOIN tasks t ON t.id = rs.task_id
                         AND t.phase = 'Active'
                         AND t.active_run_id = a.run_id
                     WHERE acs.attempt_id = ?1",
                    rusqlite::params![credential.attempt_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            let Some((session_id, run_id, task_id, phase, revision, spec_digest)) = lineage else {
                return Err(StoreError::AgentControlUnauthorized {
                    attempt_id: credential.attempt_id.to_string(),
                    reason: "the Attempt no longer carries current Run/Task authority",
                });
            };

            let spec_json: String = conn.query_row(
                "SELECT canonical_json FROM task_specs WHERE digest = ?1",
                rusqlite::params![spec_digest],
                |row| row.get(0),
            )?;
            let spec = TaskSpec::from_canonical_json(&spec_json).map_err(|error| {
                StoreError::CandidateInvalid {
                    detail: error.to_string(),
                }
            })?;

            Ok(AgentSessionDescription {
                session_id,
                attempt_id: credential.attempt_id.to_string(),
                run_id,
                task_id,
                task_phase: phase,
                task_revision: Revision::new(revision),
                outputs: spec
                    .outputs
                    .iter()
                    .map(|output| DeclaredOutput {
                        name: output.name.clone(),
                        kind: output.kind.clone(),
                        required: output.required,
                    })
                    .collect(),
            })
        })
    }

    /// Opens (or reconciles) the durable record of one Agent Control request:
    /// authenticate, fence the generation, then look up or create the
    /// `(attempt_id, request_id)` row — in that order, in one transaction.
    ///
    /// Used by requests whose effects span transactions (`artifact.seal`
    /// captures outside SQLite): the `STARTED` row is durably represented
    /// before any multi-step external effect begins.
    ///
    /// # Errors
    ///
    /// [`StoreError::AgentControlUnauthorized`] on a failed fence;
    /// [`StoreError::AgentRequestConflict`] when the identity was used with a
    /// different canonical request hash; plus ordinary storage failures.
    pub fn open_agent_request(
        &self,
        credential: AgentCredential<'_>,
        operation: AgentOperation,
        request_id: &str,
        request_hash: &[u8; 32],
    ) -> Result<AgentRequestOpened, StoreError> {
        self.write(|writer| {
            authenticate_agent_session(writer, &credential)?;
            let existing_raw: Option<(Vec<u8>, String, Option<String>, Option<String>)> = writer
                .query_optional(
                    "SELECT request_hash, state, result_ref, problem_code
                     FROM agent_requests WHERE attempt_id = ?1 AND request_id = ?2",
                    &[Value::from(credential.attempt_id), Value::from(request_id)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
            let existing = match existing_raw {
                Some((hash, state, result_ref, problem_code)) => {
                    Some((hash, request_state_from(state, result_ref, problem_code)?))
                }
                None => None,
            };
            if let Some((hash, state)) = existing {
                if hash.as_slice() != request_hash.as_slice() {
                    return writer.fail(StoreError::AgentRequestConflict {
                        attempt_id: credential.attempt_id.to_string(),
                        request_id: request_id.to_string(),
                    });
                }
                return Ok(AgentRequestOpened::Reconciled(state));
            }
            writer.execute(
                "INSERT INTO agent_requests
                     (attempt_id, request_id, request_hash, operation, state,
                      result_ref, problem_code, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'STARTED', NULL, NULL, ?5, ?5)",
                &[
                    Value::from(credential.attempt_id),
                    Value::from(request_id),
                    Value::Blob(request_hash.as_slice().to_vec()),
                    Value::from(operation.as_str()),
                    Value::Integer(crate::scheduling::now(writer)?),
                ],
            )?;
            Ok(AgentRequestOpened::Started)
        })
    }

    /// Records a request's successful outcome, refusing through the same
    /// authentication fence the request opened under.
    ///
    /// A session fenced mid-flight (revoked, old-generation, stale Attempt)
    /// cannot record success: the row stays `STARTED` inert historical
    /// evidence, unreachable because every later touch re-fences first.
    ///
    /// Finding the outcome already recorded is reconciliation, not error.
    ///
    /// # Errors
    ///
    /// [`StoreError::AgentControlUnauthorized`] on a failed fence;
    /// [`StoreError::InvariantViolated`] when the row is absent or terminally
    /// failed — states success cannot legitimately overwrite.
    pub fn complete_agent_request(
        &self,
        credential: AgentCredential<'_>,
        request_id: &str,
        result_ref: &str,
    ) -> Result<(), StoreError> {
        self.write(|writer| {
            authenticate_agent_session(writer, &credential)?;
            let updated = writer.execute(
                "UPDATE agent_requests
                 SET state = 'SUCCEEDED', result_ref = ?3, problem_code = NULL,
                     updated_at = ?4
                 WHERE attempt_id = ?1 AND request_id = ?2 AND state = 'STARTED'",
                &[
                    Value::from(credential.attempt_id),
                    Value::from(request_id),
                    Value::from(result_ref),
                    Value::Integer(crate::scheduling::now(writer)?),
                ],
            )?;
            if updated > 0 {
                return Ok(());
            }
            let current: Option<String> = writer.query_optional(
                "SELECT state FROM agent_requests WHERE attempt_id = ?1 AND request_id = ?2",
                &[Value::from(credential.attempt_id), Value::from(request_id)],
                |row| row.get(0),
            )?;
            match current.as_deref() {
                Some("SUCCEEDED") => Ok(()),
                Some(other) => writer.fail(StoreError::InvariantViolated(format!(
                    "agent request {request_id} is {other}; success cannot overwrite it"
                ))),
                None => writer.fail(StoreError::InvariantViolated(format!(
                    "agent request {request_id} has no row to complete"
                ))),
            }
        })
    }

    /// Records a request's definitive refusal, through the same
    /// authentication fence. Same-ID retries reconcile the refusal instead of
    /// re-running the work; a genuinely fresh attempt mints a new request ID.
    pub fn fail_agent_request(
        &self,
        credential: AgentCredential<'_>,
        request_id: &str,
        problem_code: &'static str,
    ) -> Result<(), StoreError> {
        self.write(|writer| {
            authenticate_agent_session(writer, &credential)?;
            writer.execute(
                "UPDATE agent_requests
                 SET state = 'FAILED', problem_code = ?3, result_ref = NULL,
                     updated_at = ?4
                 WHERE attempt_id = ?1 AND request_id = ?2 AND state = 'STARTED'",
                &[
                    Value::from(credential.attempt_id),
                    Value::from(request_id),
                    Value::from(problem_code),
                    Value::Integer(crate::scheduling::now(writer)?),
                ],
            )?;
            Ok(())
        })
    }

    /// Commits Candidate submission (T6): one serialized authoritative
    /// transaction that re-reads every precondition and writes every effect,
    /// or nothing.
    ///
    /// Deliberately absent from its effects: any acceptance decision, Task
    /// success, Run terminalization or slot release. The Run stays nonterminal
    /// in `Finalizing`, keeping the Task's unique live-Run slot while
    /// evaluation proceeds; only Pantheon-owned acceptance may later succeed
    /// the Task.
    ///
    /// # Errors
    ///
    /// [`StoreError::AgentControlUnauthorized`] on a failed session fence,
    /// [`StoreError::AgentRequestConflict`] on a reused request identity with
    /// different semantics, [`StoreError::RevisionConflict`] on a stale Task
    /// CAS, [`StoreError::CandidateExists`] for a second different Candidate
    /// on one Run, [`StoreError::CandidateInvalid`] /
    /// [`StoreError::CandidateProvenanceInvalid`] for structural or provenance
    /// refusals, and [`StoreError::SubmissionStaleAuthority`] when the
    /// lifecycle moved underneath. Nothing is written on any refusal.
    #[allow(clippy::too_many_lines)]
    pub fn submit_candidate(
        &self,
        submission: &CandidateSubmission<'_>,
    ) -> Result<SubmissionOutcome, StoreError> {
        self.write(|writer| apply_candidate_submission(writer, submission))
    }
}

#[allow(clippy::too_many_lines)]
fn apply_candidate_submission(
    writer: &Writer<'_>,
    submission: &CandidateSubmission<'_>,
) -> Result<SubmissionOutcome, StoreError> {
    // ---- Authentication and the restore-generation fence, ahead of
    // ---- everything else — including request-ledger lookups.
    authenticate_agent_session(writer, &submission.credential)?;

    // ---- Request idempotency, after the fence. ----
    let existing_raw: Option<(Vec<u8>, String, Option<String>, Option<String>)> = writer
        .query_optional(
            "SELECT request_hash, state, result_ref, problem_code
             FROM agent_requests WHERE attempt_id = ?1 AND request_id = ?2",
            &[
                Value::from(submission.credential.attempt_id),
                Value::from(submission.request_id),
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    let existing = match existing_raw {
        Some((hash, state, result_ref, problem_code)) => {
            Some((hash, request_state_from(state, result_ref, problem_code)?))
        }
        None => None,
    };
    let mut prior_row = false;
    if let Some((hash, state)) = existing {
        if hash.as_slice() != submission.request_hash.as_slice() {
            return writer.fail(StoreError::AgentRequestConflict {
                attempt_id: submission.credential.attempt_id.to_string(),
                request_id: submission.request_id.to_string(),
            });
        }
        match state {
            AgentRequestState::Succeeded { result_ref } => {
                // Replay of a committed submission: reconcile to the same
                // Candidate, parsed back from the recorded outcome rather
                // than recomputed, so the ledger stays the single source of
                // truth even across a restart.
                let digest = Digest::from_display(&result_ref).ok_or_else(|| {
                    StoreError::InvariantViolated(format!(
                        "agent request stores an unparsable result ref {result_ref}"
                    ))
                })?;
                let committed = load_committed_outcome(writer, submission.candidate, &digest)?;
                return Ok(SubmissionOutcome {
                    committed,
                    reconciled: true,
                });
            }
            // STARTED/FAILED rows cannot legitimately exist for this purely
            // relational operation — its whole effect commits atomically
            // below — but recovery treats them as resumable rather than
            // poisoned: the final write updates whatever row is present.
            AgentRequestState::Started | AgentRequestState::Failed { .. } => prior_row = true,
        }
    }

    // ---- Current authority, derived from the authenticated Attempt. ----
    let attempt: Option<(String, i64)> = writer.query_optional(
        "SELECT a.run_id, st.terminal FROM attempts a
         JOIN attempt_status st ON st.attempt_id = a.id
         WHERE a.id = ?1",
        &[Value::from(submission.credential.attempt_id)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let run_id = match attempt {
        Some((run_id, 0)) => run_id,
        Some((_, terminal)) => {
            return writer.fail(StoreError::SubmissionStaleAuthority {
                attempt_id: submission.credential.attempt_id.to_string(),
                detail: format!("the Attempt is terminal ({terminal})"),
            });
        }
        None => {
            return writer.fail(StoreError::SubmissionStaleAuthority {
                attempt_id: submission.credential.attempt_id.to_string(),
                detail: "the Attempt does not exist".to_string(),
            });
        }
    };

    let run: Option<(String, String, Option<String>, i64, Option<String>)> = writer
        .query_optional(
            "SELECT task_id, phase, active_slot, revision, current_attempt_id
         FROM run_status WHERE run_id = ?1",
            &[Value::from(run_id.as_str())],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
    let Some((task_id, run_phase, active_slot, run_revision, current_attempt)) = run else {
        return writer.fail(StoreError::SubmissionStaleAuthority {
            attempt_id: submission.credential.attempt_id.to_string(),
            detail: "the Attempt's Run has no status row".to_string(),
        });
    };
    if run_phase != "Active" || active_slot.is_none() {
        return writer.fail(StoreError::SubmissionStaleAuthority {
            attempt_id: submission.credential.attempt_id.to_string(),
            detail: format!("the Run is {run_phase}, not Active"),
        });
    }
    if current_attempt.as_deref() != Some(submission.credential.attempt_id) {
        return writer.fail(StoreError::SubmissionStaleAuthority {
            attempt_id: submission.credential.attempt_id.to_string(),
            detail: "the Attempt is no longer the Run's current one".to_string(),
        });
    }

    let task: Option<(String, Option<String>, i64, Vec<u8>)> = writer.query_optional(
        "SELECT t.phase, t.active_run_id, t.revision, t.spec_digest
         FROM tasks t WHERE t.id = ?1",
        &[Value::from(task_id.as_str())],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let Some((task_phase, active_run, task_revision, spec_digest)) = task else {
        return writer.fail(StoreError::SubmissionStaleAuthority {
            attempt_id: submission.credential.attempt_id.to_string(),
            detail: "the Run's Task no longer exists".to_string(),
        });
    };
    if task_phase != "Active" {
        // Includes the cancellation/supersession/finalization fence: any such
        // commitment moves the Task out of Active first, and this CAS-bound
        // read sees it. Losing the race is the canonical outcome.
        return writer.fail(StoreError::SubmissionStaleAuthority {
            attempt_id: submission.credential.attempt_id.to_string(),
            detail: format!("the Task is {task_phase}, not Active"),
        });
    }
    if active_run.as_deref() != Some(run_id.as_str()) {
        return writer.fail(StoreError::SubmissionStaleAuthority {
            attempt_id: submission.credential.attempt_id.to_string(),
            detail: "the Task's responsible-Run pointer moved".to_string(),
        });
    }
    let goal_phase: Option<String> = writer.query_optional(
        "SELECT phase FROM goals
         WHERE id = (SELECT goal_id FROM tasks WHERE id = ?1)",
        &[Value::from(task_id.as_str())],
        |row| row.get(0),
    )?;
    if !matches!(
        goal_phase.as_deref(),
        Some("Planning") | Some("Active") | Some("Evaluating")
    ) {
        return writer.fail(StoreError::SubmissionStaleAuthority {
            attempt_id: submission.credential.attempt_id.to_string(),
            detail: format!(
                "the Goal is {}; Task submission authority is gone",
                goal_phase.unwrap_or_else(|| "absent".to_string())
            ),
        });
    }
    if task_revision != submission.expected_task_revision.get() {
        return writer.fail(StoreError::RevisionConflict {
            table: "tasks",
            id: task_id.clone(),
            expected: submission.expected_task_revision.get(),
            actual: Some(task_revision),
        });
    }

    // ---- Identity honesty: the submitted Candidate must bind exactly what
    // ---- the authenticated lineage derives. ----
    if submission.candidate.run_id() != run_id || submission.candidate.task_id() != task_id {
        return writer.fail(StoreError::CandidateInvalid {
            detail: format!(
                "the candidate binds task {} run {}, but the authenticated lineage \
                 derives task {task_id} run {run_id}",
                submission.candidate.task_id(),
                submission.candidate.run_id(),
            ),
        });
    }

    // ---- Specification validation. ----
    let spec_json: String = writer
        .query_optional(
            "SELECT canonical_json FROM task_specs WHERE digest = ?1",
            &[Value::Blob(spec_digest)],
            |row| row.get(0),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated(
                "the task's stored specification is not present".to_string(),
            )
        })?;
    let spec = TaskSpec::from_canonical_json(&spec_json).map_err(|error| {
        StoreError::CandidateInvalid {
            detail: error.to_string(),
        }
    })?;
    submission
        .candidate
        .validate_against_spec(&spec)
        .map_err(|error| StoreError::CandidateInvalid {
            detail: error.to_string(),
        })?;

    // ---- Per-output Artifact and provenance validation. ----
    for output in submission.candidate.outputs() {
        let artifact: Option<(String, String)> = writer.query_optional(
            "SELECT artifact_kind, canonical_json FROM artifacts WHERE digest = ?1",
            &[Value::Blob(output.artifact.as_bytes().to_vec())],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let Some((kind, json)) = artifact else {
            return writer.fail(StoreError::CandidateInvalid {
                detail: format!(
                    "output {:?} references artifact {} which does not exist",
                    output.slot, output.artifact
                ),
            });
        };
        let slot = declared_slot(&spec, &output.slot);
        pantheon_core::candidate::kind_permitted(slot, &kind).map_err(|error| {
            StoreError::CandidateInvalid {
                detail: error.to_string(),
            }
        })?;
        ensure_artifact_complete(writer, output, &json)?;

        // Content is not ownership: the Artifact must carry a ProductionRecord
        // binding it to THIS Run and THIS exact slot.
        let provenance: Option<Vec<u8>> = writer.query_optional(
            "SELECT artifact_digest FROM production_records
             WHERE run_id = ?1 AND output_slot = ?2",
            &[
                Value::from(run_id.as_str()),
                Value::from(output.slot.as_str()),
            ],
            |row| row.get(0),
        )?;
        match provenance {
            Some(digest) if digest == output.artifact.as_bytes().to_vec() => {}
            Some(digest) => {
                return writer.fail(StoreError::CandidateProvenanceInvalid {
                    detail: format!(
                        "run {run_id} slot {:?} produced {}, not {}",
                        output.slot,
                        display_digest(&digest),
                        output.artifact
                    ),
                });
            }
            None => {
                return writer.fail(StoreError::CandidateProvenanceInvalid {
                    detail: format!(
                        "run {run_id} has no production record for slot {:?}",
                        output.slot
                    ),
                });
            }
        }
    }

    // ---- Immutable Candidate: create once. A Candidate already existing
    // ---- for this Run cannot reach this point through the request ledger
    // ---- (an exact replay reconciled above) or through lifecycle (the
    // ---- committing Run left Active with its Candidate); reaching it means
    // ---- a second submission slipped past every fence, so it is refused as
    // ---- the typed form of the database's own one-per-Run constraint.
    let digest_bytes = submission.candidate.digest().as_bytes().to_vec();
    let canonical_json = submission.candidate.to_canonical_json();
    let now = crate::scheduling::now(writer)?;
    let existing_candidate: Option<String> = writer.query_optional(
        "SELECT canonical_json FROM candidates WHERE run_id = ?1",
        &[Value::from(run_id.as_str())],
        |row| row.get(0),
    )?;
    if existing_candidate.is_some() {
        return writer.fail(StoreError::CandidateExists {
            run_id: run_id.clone(),
            candidate_digest: submission.candidate.digest().to_string(),
        });
    }
    writer.execute(
        "INSERT INTO candidates (digest, task_id, run_id, canonical_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            Value::Blob(digest_bytes.clone()),
            Value::from(task_id.as_str()),
            Value::from(run_id.as_str()),
            Value::from(canonical_json.as_str()),
            Value::Integer(now),
        ],
    )?;
    for output in submission.candidate.outputs() {
        writer.execute(
            "INSERT INTO candidate_outputs
                 (candidate_digest, output_slot, artifact_digest, production_run_id)
             VALUES (?1, ?2, ?3, ?4)",
            &[
                Value::Blob(digest_bytes.clone()),
                Value::from(output.slot.as_str()),
                Value::Blob(output.artifact.as_bytes().to_vec()),
                Value::from(run_id.as_str()),
            ],
        )?;
    }

    // ---- Lifecycle: both transitions in this same transaction. The Run
    // ---- keeps its global execution slot while Finalizing. ----
    let new_run_revision = writer.update_revisioned_by(
        "run_status",
        "run_id",
        &run_id,
        Revision::new(run_revision),
        &[
            ("phase", Value::from("Finalizing")),
            ("terminal_target", Value::from("Completed")),
            ("candidate_digest", Value::Blob(digest_bytes)),
            ("updated_at", Value::Integer(now)),
        ],
    )?;
    let new_task_revision = writer.update_revisioned(
        "tasks",
        &task_id,
        submission.expected_task_revision,
        &[("phase", Value::from("Evaluating"))],
    )?;

    // ---- Request outcome, committed with everything above. ----
    let result_ref = submission.candidate.digest().to_string();
    if prior_row {
        writer.execute(
            "UPDATE agent_requests
             SET state = 'SUCCEEDED', result_ref = ?3, problem_code = NULL, updated_at = ?4
             WHERE attempt_id = ?1 AND request_id = ?2",
            &[
                Value::from(submission.credential.attempt_id),
                Value::from(submission.request_id),
                Value::from(result_ref.as_str()),
                Value::Integer(now),
            ],
        )?;
    } else {
        writer.execute(
            "INSERT INTO agent_requests
                 (attempt_id, request_id, request_hash, operation, state,
                  result_ref, problem_code, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'task.submit_result', 'SUCCEEDED', ?4, NULL, ?5, ?5)",
            &[
                Value::from(submission.credential.attempt_id),
                Value::from(submission.request_id),
                Value::Blob(submission.request_hash.as_slice().to_vec()),
                Value::from(result_ref.as_str()),
                Value::Integer(now),
            ],
        )?;
    }

    append_internal_event(writer, "candidate.submitted")?;

    Ok(SubmissionOutcome {
        committed: CandidateCommitted {
            candidate_digest: result_ref,
            task_revision: new_task_revision,
            run_revision: new_run_revision,
        },
        reconciled: false,
    })
}

/// Loads the committed truth a reconciled replay reports: the Candidate must
/// still exist, and the reported revisions are the rows' *current* values,
/// since the original request already moved them.
fn load_committed_outcome(
    writer: &Writer<'_>,
    candidate: &CandidateResult,
    digest: &Digest,
) -> Result<CandidateCommitted, StoreError> {
    let exists: Option<String> = writer.query_optional(
        "SELECT canonical_json FROM candidates WHERE digest = ?1 AND run_id = ?2",
        &[
            Value::Blob(digest.as_bytes().to_vec()),
            Value::from(candidate.run_id()),
        ],
        |row| row.get(0),
    )?;
    match exists {
        Some(stored_json) if stored_json == candidate.to_canonical_json() => {}
        Some(_) => {
            return writer.fail(StoreError::ContentIdentityConflict {
                table: REQUEST_TABLE,
                id: digest.to_string(),
                detail: "a recorded agent-request outcome names a candidate whose \
                         content disagrees"
                    .to_string(),
            });
        }
        None => {
            return writer.fail(StoreError::InvariantViolated(format!(
                "an agent request references candidate {} which is not stored",
                digest
            )));
        }
    }
    let task_revision: i64 = writer
        .query_optional(
            "SELECT revision FROM tasks WHERE id = ?1",
            &[Value::from(candidate.task_id())],
            |row| row.get(0),
        )?
        .unwrap_or_default();
    let run_revision: i64 = writer
        .query_optional(
            "SELECT revision FROM run_status WHERE run_id = ?1",
            &[Value::from(candidate.run_id())],
            |row| row.get(0),
        )?
        .unwrap_or_default();
    Ok(CandidateCommitted {
        candidate_digest: digest.to_string(),
        task_revision: Revision::new(task_revision),
        run_revision: Revision::new(run_revision),
    })
}

fn declared_slot<'a>(spec: &'a TaskSpec, name: &str) -> &'a pantheon_core::planning::TaskOutput {
    spec.outputs
        .iter()
        .find(|output| output.name == name)
        .expect("validate_against_spec already proved the slot is declared")
}

fn display_digest(bytes: &[u8]) -> String {
    match <[u8; 32]>::try_from(bytes) {
        Ok(bytes) => Digest::from_bytes(bytes).to_string(),
        Err(_) => "<malformed-digest>".to_string(),
    }
}

/// Verifies an Artifact is structurally complete: its stored manifest parses,
/// and every payload the manifest names is present as a member Blob. Extra
/// retained members are harmless; a missing payload is not — the Artifact's
/// own semantics could not be recovered from it.
fn ensure_artifact_complete(
    writer: &Writer<'_>,
    output: &pantheon_core::candidate::CandidateOutput,
    canonical_json: &str,
) -> Result<(), StoreError> {
    let invalid = |detail: String| StoreError::CandidateInvalid { detail };
    let manifest = pantheon_core::config::parse::parse(canonical_json).map_err(|error| {
        invalid(format!(
            "artifact {} does not parse: {error}",
            output.artifact
        ))
    })?;
    let Json::Array(entries) = manifest.get("entries").unwrap_or(&Json::Null) else {
        return Err(invalid(format!(
            "artifact {} has no readable entries",
            output.artifact
        )));
    };
    let mut referenced: Vec<Vec<u8>> = Vec::new();
    for entry in entries {
        for side in ["before", "after"] {
            let Some(Json::String(blob)) = entry.get(side).and_then(|state| state.get("blob"))
            else {
                continue;
            };
            let Some(digest) = Digest::from_display(blob) else {
                return Err(invalid(format!(
                    "artifact {} has an unparsable {side} blob reference",
                    output.artifact
                )));
            };
            referenced.push(digest.as_bytes().to_vec());
        }
    }
    referenced.sort();
    referenced.dedup();

    let mut members: Vec<Vec<u8>> = writer.query_all(
        "SELECT blob_digest FROM artifact_members WHERE artifact_digest = ?1",
        &[Value::Blob(output.artifact.as_bytes().to_vec())],
        |row| row.get(0),
    )?;
    members.sort();

    for referenced_digest in &referenced {
        if !members.binary_search(referenced_digest).is_ok() {
            return Err(invalid(format!(
                "artifact {} is incomplete: the manifest references payload \
                 absent from its stored members",
                output.artifact
            )));
        }
    }
    Ok(())
}

impl fmt::Display for DeclaredOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.kind)
    }
}

#[cfg(test)]
mod tests;
