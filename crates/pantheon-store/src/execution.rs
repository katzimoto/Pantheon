//! Durable Attempt lineage: T4 Attempt creation, T4a pre-contact credential
//! rekey, T4b launch-contact marker, observation recording and the reads the
//! Run Controller reconciles through.
//!
//! `docs/architecture/persistence-and-recovery/sqlite-persistence-and-
//! transactions.md` ("Attempt and launch-contact state", "Agent Control",
//! named families T4/T4a/T4b/T8) is canonical for what these transactions
//! mean. The relational shape carries the safety claims:
//!
//! - [`Store::create_attempt`] commits the Attempt, its ordinal, its globally
//!   unique LaunchKey, its nonterminal `attempt_status`, its
//!   `agent_control_sessions` row bound to the *transaction's* current
//!   RestoreGeneration, and the `run_status.current_attempt_id` pointer — in
//!   one authoritative transaction, under a command identity whose request
//!   hash deliberately excludes the random launch material, so a retry after
//!   a lost response replays instead of minting a second lineage;
//! - `one_nonterminal_attempt_per_run` is a real partial unique index;
//! - `attempt_status.run_id` and `run_status.current_attempt_id` are held
//!   holder-safe by composite foreign keys;
//! - T4a rotates only the session verifier/revision, and only while the
//!   Attempt is durably `NOT_CONTACTED` in the current generation;
//! - T4b is the monotonic boundary whose commit precedes any external
//!   contact and freezes the credential revision;
//! - terminalization requires the contact marker, because a lineage
//!   Pantheon never contacted cannot definitively end.
//!
//! Nothing here performs an external effect: no process, backend, network,
//! sandbox or filesystem call happens inside any of these transactions.

use pantheon_core::attempt::{LaunchContactState, Observation};

use crate::command::Committed;
use crate::error::StoreError;
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

const ATTEMPT_STATUS_TABLE: &str = "attempt_status";
const SESSION_TABLE: &str = "agent_control_sessions";

/// An immutable Attempt identity row, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRecord {
    pub id: String,
    pub run_id: String,
    pub ordinal: i64,
    pub launch_key: String,
}

/// One Attempt-scoped Agent Control identity, minus every secret-derived
/// field. The stored credential verifier is readable through tests and audit
/// paths, never through this control-plane view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentControlSessionView {
    pub id: String,
    pub attempt_id: String,
    /// The installation generation this session is bound to, immutably.
    pub restore_generation: String,
    pub credential_revision: i64,
    pub state: String,
}

/// Everything one Run Controller reconciliation step reads, as one snapshot.
///
/// Every field comes out of a single explicit read transaction, so the Run's
/// phase, its attachment and its Attempt lineage describe the same moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunExecutionView {
    pub run_id: String,
    pub task_id: String,
    pub binding_digest: pantheon_core::config::Digest,
    pub source_snapshot_digest: pantheon_core::config::Digest,
    pub phase: String,
    pub revision: Revision,
    pub current_attempt_id: Option<String>,
    /// The digest of the ContextPlan attached through the one-time relation,
    /// when preparation has reached ContextReady.
    pub context_plan_digest: Option<pantheon_core::config::Digest>,
    /// The current Attempt lineage, when one exists.
    pub attempt: Option<AttemptLineageView>,
}

/// The current Attempt of a Run: identity, status and session, consistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptLineageView {
    pub attempt: AttemptRecord,
    pub observed_execution: Observation,
    pub terminal: bool,
    pub launch_contact_state: LaunchContactState,
    pub status_revision: Revision,
    pub session: AgentControlSessionView,
}

/// One restart-inventory entry: a nonterminal Run and its current Attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInventoryEntry {
    pub run_id: String,
    pub task_id: String,
    pub phase: String,
    pub revision: Revision,
    pub current_attempt_id: Option<String>,
}

/// What T4/T8 creates. `restore_generation` reports the installation
/// generation the new session bound, as read inside the committing
/// transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptCreated {
    pub attempt_id: String,
    pub run_id: String,
    pub ordinal: i64,
    pub launch_key: String,
    pub session_id: String,
    pub restore_generation: String,
}

/// The authoritative input to T4/T8 Attempt creation.
///
/// Identity fields are supplied by the controller; the store persists them
/// and refuses inconsistency, but generates none of them. The credential
/// verifier is the SHA-256 of the raw bearer — the bearer itself never
/// enters this crate.
#[derive(Debug, Clone)]
pub struct AttemptCreation<'a> {
    pub run_id: &'a str,
    pub attempt_id: &'a str,
    pub launch_key: &'a str,
    pub session_id: &'a str,
    pub credential_verifier: &'a [u8; 32],
    /// The `run_status` revision the caller observed before deciding T4 was
    /// permitted; revalidated inside the transaction.
    pub expected_run_status_revision: Revision,
}

impl Store {
    /// Commits one T4/T8 Attempt creation.
    ///
    /// Inside one authoritative transaction this re-reads the Run, requires
    /// it Active at the expected revision, requires the one-time ContextPlan
    /// attachment (an Attempt before LaunchReady is refused), requires zero
    /// nonterminal Attempts, assigns the next Run-local ordinal, inserts the
    /// immutable Attempt with its unique LaunchKey, opens its nonterminal
    /// status at `NOT_CONTACTED`, creates the Attempt-scoped
    /// AgentControlSession bound to the current RestoreGeneration with
    /// credential revision 1, moves the Run's current-Attempt pointer, and
    /// appends the Event — all under the caller's command identity.
    ///
    /// Command replay semantics carry the recovery story: the request hash
    /// covers `(run, ordinal)` only, never the random launch material, so a
    /// retry after a crash between commit and response replays to the same
    /// committed result while a genuinely different next Attempt derives a
    /// different command.
    ///
    /// # Errors
    ///
    /// - [`StoreError::AttemptNotLaunchReady`] when the Run has no attached
    ///   ContextPlan, is not Active, or already owns a nonterminal Attempt;
    /// - [`StoreError::RevisionConflict`] when the Run's status moved since
    ///   the caller observed it;
    /// - plus the command envelope's failures. Nothing is written on failure.
    pub fn create_attempt(
        &self,
        command: &crate::command::Command<'_>,
        creation: &AttemptCreation<'_>,
    ) -> Result<Committed<AttemptCreated>, StoreError> {
        self.execute_command(command, |writer| apply_attempt_creation(writer, creation))
    }

    /// Recovers a lost pre-contact bearer by rotating the *same*
    /// AgentControlSession's verifier (T4a).
    ///
    /// Permitted only while every precondition holds on the transaction's own
    /// snapshot: the session is ACTIVE in the current RestoreGeneration, the
    /// parent Attempt is current, nonterminal and durably `NOT_CONTACTED`,
    /// and the expected credential revision is still current. It increments
    /// the revision, replaces the verifier, records non-secret provenance and
    /// appends the rekey Event. It never touches the Attempt, the LaunchKey,
    /// the session identity or `restore_generation`.
    ///
    /// This is controller recovery bookkeeping, deliberately *not* under a
    /// fixed command identity: each invocation with lost bearer material is
    /// genuinely new work, and repeating it after another crash must rotate
    /// again rather than replay.
    ///
    /// # Errors
    ///
    /// - [`StoreError::AgentControlRekeyForbidden`] when any precondition
    ///   fails — including `CONTACT_MAY_HAVE_OCCURRED`, where the credential
    ///   is frozen forever for this Attempt;
    /// - [`StoreError::RevisionConflict`] when the expected credential
    ///   revision is stale. Nothing is written on failure.
    pub fn rekey_agent_control_session(
        &self,
        attempt_id: &str,
        new_verifier: &[u8; 32],
        expected_credential_revision: i64,
    ) -> Result<i64, StoreError> {
        self.write(|writer| {
            apply_rekey(
                writer,
                attempt_id,
                new_verifier,
                expected_credential_revision,
            )
        })
    }

    /// Commits the durable launch-contact marker (T4b):
    /// `NOT_CONTACTED -> CONTACT_MAY_HAVE_OCCURRED`.
    ///
    /// The controller calls this immediately before the first external
    /// `ensureExecution` contact and commits it before invoking the backend.
    /// Inside the transaction it re-proves current authority: the Attempt is
    /// current and nonterminal under an Active Run, the session is ACTIVE in
    /// the current generation at exactly the expected credential revision,
    /// and the marker has not been crossed. On success it records initiation
    /// time and controller-epoch provenance and appends the Event.
    ///
    /// A lost response is safe to retry: finding the marker already set is
    /// reconciled as the committed outcome, not an error.
    ///
    /// # Errors
    ///
    /// - [`StoreError::LaunchContactStaleAuthority`] when the Attempt is no
    ///   longer current, the Run is no longer Active, the session is not the
    ///   current-generation ACTIVE session at the expected revision;
    /// - [`StoreError::RevisionConflict`] when the status row moved.
    pub fn mark_launch_contact(
        &self,
        run_id: &str,
        attempt_id: &str,
        controller_epoch: &str,
        expected_status_revision: Revision,
        expected_credential_revision: i64,
    ) -> Result<(), StoreError> {
        self.write(|writer| {
            apply_launch_contact(
                writer,
                run_id,
                attempt_id,
                controller_epoch,
                expected_status_revision,
                expected_credential_revision,
            )
        })
    }

    /// Records a normalized execution observation for one Attempt.
    ///
    /// Non-terminal observations (`STARTING`, `RUNNING`, `UNKNOWN`,
    /// pre-contact `ABSENT`) update the factual view under CAS and stamp
    /// `started_at` the first time the lineage is seen alive. Terminal
    /// updates are refused unless the contact marker is durably
    /// `CONTACT_MAY_HAVE_OCCURRED`: a lineage Pantheon provably never
    /// launched cannot have ended. Terminalization marks the Attempt ended,
    /// stamps `finished_at`, revokes its AgentControlSession, and appends the
    /// Event — one transaction.
    ///
    /// # Errors
    ///
    /// - [`StoreError::InvariantViolated`] when a terminal update arrives
    ///   while still `NOT_CONTACTED`;
    /// - [`StoreError::RevisionConflict`] when the status row moved.
    pub fn record_execution_observation(
        &self,
        attempt_id: &str,
        expected_status_revision: Revision,
        update: ObservationUpdate,
    ) -> Result<Revision, StoreError> {
        self.write(|writer| apply_observation(writer, attempt_id, expected_status_revision, update))
    }

    /// Concludes a Run that produced no usable execution: preparation
    /// failures and Recovery Policy exhaustion land here.
    ///
    /// Moves the Run directly to a terminal phase with its durable
    /// terminal target recorded in the same commit, releasing the global
    /// execution slot. The full Finalizing ceremony (obligations, resource
    /// settlement) belongs to the integrated startup-recovery mission; the
    /// durable *why* is preserved now so that mission inherits intent, not a
    /// guess.
    ///
    /// # Errors
    ///
    /// - [`StoreError::InvariantViolated`] for a target outside the
    ///   concluded-without-Candidate set;
    /// - [`StoreError::RevisionConflict`] when the Run's status moved.
    pub fn conclude_run(
        &self,
        run_id: &str,
        terminal_target: &'static str,
        expected_run_status_revision: Revision,
    ) -> Result<(), StoreError> {
        self.write(|writer| {
            apply_conclude_run(
                writer,
                run_id,
                terminal_target,
                expected_run_status_revision,
            )
        })
    }

    /// Reads everything one Run Controller reconciliation step needs, as one
    /// consistent snapshot.
    ///
    /// Multi-statement by necessity — Run, status, attachment and Attempt
    /// lineage must agree — so this runs inside one explicit read
    /// transaction rather than autocommit.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read or interpreted.
    pub fn run_execution_view(&self, run_id: &str) -> Result<Option<RunExecutionView>, StoreError> {
        self.read_snapshot(|conn| {
            let Some((task_id, binding, snapshot, phase, revision, current)) = conn
                .query_row(
                    "SELECT r.task_id, r.binding_digest,
                            r.context_source_snapshot_digest, s.phase, s.revision,
                            s.current_attempt_id
                     FROM runs r JOIN run_status s ON s.run_id = r.id
                     WHERE r.id = ?1",
                    rusqlite::params![run_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?
            else {
                return Ok(None);
            };

            let context_plan_digest: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT context_plan_digest FROM run_context_plans WHERE run_id = ?1",
                    rusqlite::params![run_id],
                    |row| row.get(0),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;

            let attempt = load_current_lineage(conn, run_id)?;

            Ok(Some(RunExecutionView {
                run_id: run_id.to_string(),
                task_id,
                binding_digest: crate::context::digest(&binding, "binding_digest")?,
                source_snapshot_digest: crate::context::digest(
                    &snapshot,
                    "context_source_snapshot_digest",
                )?,
                phase,
                revision: Revision::new(revision),
                current_attempt_id: current,
                context_plan_digest: context_plan_digest
                    .map(|bytes| crate::context::digest(&bytes, "context_plan_digest"))
                    .transpose()?,
                attempt,
            }))
        })
    }

    /// Inventories every nonterminal Run and its current Attempt pointer.
    ///
    /// Restart reconciliation walks exactly this list and continues each Run
    /// through the ordinary [`Self::run_execution_view`] path; there is no
    /// separate startup-only repair rule.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read.
    pub fn nonterminal_run_inventory(&self) -> Result<Vec<RunInventoryEntry>, StoreError> {
        self.read_snapshot(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, phase, revision, current_attempt_id
                 FROM run_status
                 WHERE phase IN ('Active', 'Finalizing')
                 ORDER BY run_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(RunInventoryEntry {
                    run_id: row.get(0)?,
                    task_id: row.get(1)?,
                    phase: row.get(2)?,
                    revision: Revision::new(row.get(3)?),
                    current_attempt_id: row.get(4)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::Sqlite)
        })
    }

    /// The Workspace readiness facts a Run's frozen snapshot names.
    ///
    /// WorkspaceReady is judged against exactly the Workspace the Run froze
    /// at T3 — never against whatever else the Task might own today.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read.
    pub fn workspace_readiness(
        &self,
        run_id: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT w.phase, w.materialization
                 FROM runs r
                 JOIN context_source_snapshots s ON s.digest = r.context_source_snapshot_digest
                 JOIN workspaces w ON w.id = s.workspace_id
                 WHERE r.id = ?1",
                rusqlite::params![run_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Sqlite(other)),
            })
        })
    }

    /// How many Attempts this Run has ever created, terminal or not.
    ///
    /// Recovery Policy reads this to bound same-Run execution retries.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read.
    pub fn attempt_history_count(&self, run_id: &str) -> Result<i64, StoreError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM attempts WHERE run_id = ?1",
                rusqlite::params![run_id],
                |row| row.get(0),
            )
            .map_err(StoreError::Sqlite)
        })
    }

    /// The stored canonical form of one immutable ExecutionBinding, by
    /// digest. The caller decodes and verifies the fields it acts on.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read.
    pub fn binding_canonical_json(
        &self,
        digest: pantheon_core::config::Digest,
    ) -> Result<Option<String>, StoreError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT canonical_json FROM execution_bindings WHERE digest = ?1",
                rusqlite::params![digest.as_bytes().to_vec()],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Sqlite(other)),
            })
        })
    }

    /// The current revision of one Attempt's status row.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read or the Attempt does
    /// not exist.
    pub fn attempt_status_revision(&self, attempt_id: &str) -> Result<Revision, StoreError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT revision FROM attempt_status WHERE attempt_id = ?1",
                rusqlite::params![attempt_id],
                |row| row.get::<_, i64>(0),
            )
            .map(Revision::new)
            .map_err(StoreError::Sqlite)
        })
    }
}

/// Loads the single nonterminal Attempt lineage for `run_id`, if any.
///
/// The partial unique index guarantees at most one; two rows would be schema
/// tampering and fail closed.
fn load_current_lineage(
    conn: &rusqlite::Connection,
    run_id: &str,
) -> Result<Option<AttemptLineageView>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.ordinal, a.launch_key,
                st.observed_execution, st.terminal, st.launch_contact_state, st.revision,
                acs.id, acs.restore_generation, acs.credential_revision, acs.state
         FROM attempt_status st
         JOIN attempts a ON a.id = st.attempt_id
         JOIN agent_control_sessions acs ON acs.attempt_id = a.id
         WHERE st.run_id = ?1 AND st.terminal = 0",
    )?;
    let mut rows = stmt
        .query_map(rusqlite::params![run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, String>(10)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    match rows.len() {
        0 => Ok(None),
        1 => {
            let (
                attempt_id,
                ordinal,
                launch_key,
                observed,
                terminal,
                contact,
                status_revision,
                session_id,
                generation,
                credential_revision,
                session_state,
            ) = rows.pop().expect("exactly one row");
            let observed = Observation::parse(&observed).ok_or_else(|| {
                StoreError::InvariantViolated(format!(
                    "attempt {attempt_id} stores unparsable observation {observed}"
                ))
            })?;
            let contact = LaunchContactState::parse(&contact).ok_or_else(|| {
                StoreError::InvariantViolated(format!(
                    "attempt {attempt_id} stores unparsable contact state"
                ))
            })?;
            if session_state != "ACTIVE" {
                return Err(StoreError::InvariantViolated(format!(
                    "nonterminal attempt {attempt_id} has a {session_state} session"
                )));
            }
            Ok(Some(AttemptLineageView {
                attempt: AttemptRecord {
                    id: attempt_id.clone(),
                    run_id: run_id.to_string(),
                    ordinal,
                    launch_key,
                },
                observed_execution: observed,
                terminal: terminal != 0,
                launch_contact_state: contact,
                status_revision: Revision::new(status_revision),
                session: AgentControlSessionView {
                    id: session_id,
                    attempt_id: attempt_id.clone(),
                    restore_generation: generation,
                    credential_revision,
                    state: session_state,
                },
            }))
        }
        n => Err(StoreError::InvariantViolated(format!(
            "run {run_id} has {n} nonterminal attempts; the partial unique index \
             makes this unreachable without schema tampering"
        ))),
    }
}

fn apply_attempt_creation(
    writer: &Writer<'_>,
    creation: &AttemptCreation<'_>,
) -> Result<AttemptCreated, StoreError> {
    // 1. The Run must exist and be Active at the revision the controller
    //    observed. Finalizing or terminal Runs create no new lineages.
    let run: Option<i64> = writer.query_optional(
        "SELECT s.revision FROM runs r JOIN run_status s ON s.run_id = r.id
         WHERE r.id = ?1 AND s.phase = 'Active'",
        &[Value::from(creation.run_id)],
        |row| row.get(0),
    )?;
    let Some(current_status_revision) = run else {
        return writer.fail(StoreError::AttemptNotLaunchReady {
            run_id: creation.run_id.to_string(),
            detail: "the Run does not exist or is not Active".to_string(),
        });
    };
    if Revision::new(current_status_revision) != creation.expected_run_status_revision {
        return writer.fail(StoreError::RevisionConflict {
            table: "run_status",
            id: creation.run_id.to_string(),
            expected: creation.expected_run_status_revision.get(),
            actual: Some(current_status_revision),
        });
    }

    // 2. LaunchReady includes the frozen context: T4/T8 require exactly one
    //    valid attachment for the parent Run.
    let attached: Option<Vec<u8>> = writer.query_optional(
        "SELECT context_plan_digest FROM run_context_plans WHERE run_id = ?1",
        &[Value::from(creation.run_id)],
        |row| row.get(0),
    )?;
    if attached.is_none() {
        return writer.fail(StoreError::AttemptNotLaunchReady {
            run_id: creation.run_id.to_string(),
            detail: "no ContextPlan is attached; the Run is not LaunchReady".to_string(),
        });
    }

    // 3. V1 permits at most one nonterminal Attempt per Run. The controller
    //    check gives the typed outcome; the partial unique index below is the
    //    race-proof backstop.
    let live: Option<String> = writer.query_optional(
        "SELECT attempt_id FROM attempt_status WHERE run_id = ?1 AND terminal = 0",
        &[Value::from(creation.run_id)],
        |row| row.get(0),
    )?;
    if let Some(live) = live {
        return writer.fail(StoreError::AttemptNotLaunchReady {
            run_id: creation.run_id.to_string(),
            detail: format!("nonterminal attempt {live} already owns this Run"),
        });
    }

    // 4. Ordinal: strictly Run-local history position, assigned inside the
    //    transaction so concurrent creations serialize deterministically.
    let ordinal: i64 = writer
        .query_optional(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM attempts WHERE run_id = ?1",
            &[Value::from(creation.run_id)],
            |row| row.get(0),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated("ordinal aggregation returned no row".to_string())
        })?;

    // 5. The session binds the RestoreGeneration of the committing
    //    transaction — read here, never supplied by the caller.
    let restore_generation: String = writer
        .query_optional(
            "SELECT restore_generation FROM system_state WHERE id = 1",
            &[],
            |row| row.get(0),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated(
                "system_state has no installation identity row".to_string(),
            )
        })?;

    let now = crate::scheduling::now(writer)?;
    writer.execute(
        "INSERT INTO attempts (id, run_id, ordinal, launch_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            Value::from(creation.attempt_id),
            Value::from(creation.run_id),
            Value::Integer(ordinal),
            Value::from(creation.launch_key),
            Value::Integer(now),
        ],
    )?;
    writer.execute(
        "INSERT INTO attempt_status
             (attempt_id, run_id, observed_execution, terminal, revision,
              launch_contact_state, launch_contact_initiated_at,
              launch_contact_epoch, started_at, finished_at, updated_at)
         VALUES (?1, ?2, 'ABSENT', 0, 1, 'NOT_CONTACTED', NULL, NULL, NULL, NULL, ?3)",
        &[
            Value::from(creation.attempt_id),
            Value::from(creation.run_id),
            Value::Integer(now),
        ],
    )?;
    writer.execute(
        "INSERT INTO agent_control_sessions
             (id, attempt_id, restore_generation, credential_revision,
              credential_hash, credential_rekeyed_at, state, created_at,
              revoked_at, revocation_reason)
         VALUES (?1, ?2, ?3, 1, ?4, NULL, 'ACTIVE', ?5, NULL, NULL)",
        &[
            Value::from(creation.session_id),
            Value::from(creation.attempt_id),
            Value::from(restore_generation.as_str()),
            Value::Blob(creation.credential_verifier.as_slice().to_vec()),
            Value::Integer(now),
        ],
    )?;

    // 6. Establish the Run/current-Attempt relation in the same transaction.
    //    The composite FK proves the pointer names an Attempt of this Run.
    let _advanced = writer.update_revisioned_by(
        "run_status",
        "run_id",
        creation.run_id,
        creation.expected_run_status_revision,
        &[("current_attempt_id", Value::from(creation.attempt_id))],
    )?;

    Ok(AttemptCreated {
        attempt_id: creation.attempt_id.to_string(),
        run_id: creation.run_id.to_string(),
        ordinal,
        launch_key: creation.launch_key.to_string(),
        session_id: creation.session_id.to_string(),
        restore_generation,
    })
}

fn apply_rekey(
    writer: &Writer<'_>,
    attempt_id: &str,
    new_verifier: &[u8; 32],
    expected_credential_revision: i64,
) -> Result<i64, StoreError> {
    forbid_rekey_unless_precontact(writer, attempt_id)?;

    let affected = writer.execute(
        "UPDATE agent_control_sessions
         SET credential_revision = credential_revision + 1,
             credential_hash = ?2,
             credential_rekeyed_at = ?3
         WHERE attempt_id = ?1 AND credential_revision = ?4 AND state = 'ACTIVE'",
        &[
            Value::from(attempt_id),
            Value::Blob(new_verifier.as_slice().to_vec()),
            Value::Integer(crate::scheduling::now(writer)?),
            Value::Integer(expected_credential_revision),
        ],
    )?;
    if affected != 1 {
        let actual: Option<i64> = writer.query_optional(
            "SELECT credential_revision FROM agent_control_sessions WHERE attempt_id = ?1",
            &[Value::from(attempt_id)],
            |row| row.get(0),
        )?;
        return writer.fail(StoreError::RevisionConflict {
            table: SESSION_TABLE,
            id: attempt_id.to_string(),
            expected: expected_credential_revision,
            actual,
        });
    }

    crate::command::append_internal_event(writer, "agent-control.session.rekeyed")?;
    Ok(expected_credential_revision + 1)
}

/// Refuses T4a unless the whole cross-row precondition set holds right now.
fn forbid_rekey_unless_precontact(writer: &Writer<'_>, attempt_id: &str) -> Result<(), StoreError> {
    let facts: Option<(String, i64)> = writer.query_optional(
        "SELECT st.launch_contact_state, st.terminal
         FROM attempt_status st WHERE st.attempt_id = ?1",
        &[Value::from(attempt_id)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some((contact_text, terminal)) = facts else {
        return writer.fail(StoreError::AgentControlRekeyForbidden {
            attempt_id: attempt_id.to_string(),
            detail: "the Attempt does not exist".to_string(),
        });
    };
    if terminal != 0 {
        return writer.fail(StoreError::AgentControlRekeyForbidden {
            attempt_id: attempt_id.to_string(),
            detail: "the Attempt is terminal".to_string(),
        });
    }
    if contact_text != LaunchContactState::NotContacted.as_str() {
        return writer.fail(StoreError::AgentControlRekeyForbidden {
            attempt_id: attempt_id.to_string(),
            detail: format!("launch contact is {contact_text}; the credential revision is frozen"),
        });
    }

    // Current-generation fence: an old-generation session (disaster restore)
    // can never promote itself by rekeying.
    let generation: Option<String> = writer.query_optional(
        "SELECT restore_generation FROM system_state WHERE id = 1",
        &[],
        |row| row.get(0),
    )?;
    let Some(current_generation) = generation else {
        return writer.fail(StoreError::InvariantViolated(
            "system_state has no installation identity row".to_string(),
        ));
    };
    let session: Option<String> = writer.query_optional(
        "SELECT restore_generation || '|' || state FROM agent_control_sessions
         WHERE attempt_id = ?1",
        &[Value::from(attempt_id)],
        |row| row.get(0),
    )?;
    let Some(bound) = session else {
        return writer.fail(StoreError::AgentControlRekeyForbidden {
            attempt_id: attempt_id.to_string(),
            detail: "the Attempt owns no AgentControlSession".to_string(),
        });
    };
    let (bound_generation, state) = bound.split_once('|').ok_or_else(|| {
        StoreError::InvariantViolated("session generation/state unreadable".to_string())
    })?;
    if state != "ACTIVE" {
        return writer.fail(StoreError::AgentControlRekeyForbidden {
            attempt_id: attempt_id.to_string(),
            detail: format!("the session is {state}, not ACTIVE"),
        });
    }
    if bound_generation != current_generation {
        return writer.fail(StoreError::AgentControlRekeyForbidden {
            attempt_id: attempt_id.to_string(),
            detail: "the session belongs to an older RestoreGeneration".to_string(),
        });
    }

    // Current hard authority: the Attempt must still be the Run's current
    // one, under an Active Run.
    let current: Option<String> = writer.query_optional(
        "SELECT s.current_attempt_id FROM run_status s
         WHERE s.run_id = (SELECT run_id FROM attempts WHERE id = ?1)
           AND s.phase = 'Active'",
        &[Value::from(attempt_id)],
        |row| row.get(0),
    )?;
    if current.as_deref() != Some(attempt_id) {
        return writer.fail(StoreError::AgentControlRekeyForbidden {
            attempt_id: attempt_id.to_string(),
            detail: "the Attempt is no longer the Run's current one under an Active Run"
                .to_string(),
        });
    }
    Ok(())
}

fn apply_launch_contact(
    writer: &Writer<'_>,
    run_id: &str,
    attempt_id: &str,
    controller_epoch: &str,
    expected_status_revision: Revision,
    expected_credential_revision: i64,
) -> Result<(), StoreError> {
    let status: Option<(String, i64)> = writer.query_optional(
        "SELECT launch_contact_state, revision FROM attempt_status
         WHERE attempt_id = ?1 AND run_id = ?2 AND terminal = 0",
        &[Value::from(attempt_id), Value::from(run_id)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some((contact_text, _status_revision)) = status else {
        return writer.fail(StoreError::LaunchContactStaleAuthority {
            attempt_id: attempt_id.to_string(),
            detail: "the Attempt is absent, foreign to the Run, or terminal".to_string(),
        });
    };

    // Lost-response reconciliation: the boundary already committed.
    if contact_text == LaunchContactState::ContactMayHaveOccurred.as_str() {
        return Ok(());
    }

    // Current hard authority: this Attempt must still own the Run, and the
    // Run must still be Active.
    let current: Option<String> = writer.query_optional(
        "SELECT current_attempt_id FROM run_status
         WHERE run_id = ?1 AND phase = 'Active'",
        &[Value::from(run_id)],
        |row| row.get(0),
    )?;
    if current.as_deref() != Some(attempt_id) {
        return writer.fail(StoreError::LaunchContactStaleAuthority {
            attempt_id: attempt_id.to_string(),
            detail: "the Attempt is no longer the Run's current one under an Active Run"
                .to_string(),
        });
    }

    // The exact credential revision about to be delivered must still be the
    // current one: T4b binds the launch package to the verifier it carries.
    let session_revision: Option<i64> = writer.query_optional(
        "SELECT credential_revision FROM agent_control_sessions
         WHERE attempt_id = ?1 AND state = 'ACTIVE'
           AND restore_generation = (SELECT restore_generation
                                     FROM system_state WHERE id = 1)",
        &[Value::from(attempt_id)],
        |row| row.get(0),
    )?;
    if session_revision.is_none() {
        return writer.fail(StoreError::LaunchContactStaleAuthority {
            attempt_id: attempt_id.to_string(),
            detail: "no current-generation ACTIVE session exists".to_string(),
        });
    }
    if session_revision != Some(expected_credential_revision) {
        return writer.fail(StoreError::LaunchContactStaleAuthority {
            attempt_id: attempt_id.to_string(),
            detail: format!(
                "session credential revision is {:?}, not the expected \
                 {expected_credential_revision}",
                session_revision
            ),
        });
    }

    // The monotonic boundary itself, under CAS.
    let now = crate::scheduling::now(writer)?;
    let _advanced = writer.update_revisioned_by(
        ATTEMPT_STATUS_TABLE,
        "attempt_id",
        attempt_id,
        expected_status_revision,
        &[
            (
                "launch_contact_state",
                Value::from(LaunchContactState::ContactMayHaveOccurred.as_str()),
            ),
            ("launch_contact_initiated_at", Value::Integer(now)),
            ("launch_contact_epoch", Value::from(controller_epoch)),
            ("updated_at", Value::Integer(now)),
        ],
    )?;

    crate::command::append_internal_event(writer, "run.attempt.contact-initiated")?;
    Ok(())
}

/// What the controller asks [`Store::record_execution_observation`] to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationUpdate {
    /// Record a factual observation; the Attempt stays nonterminal.
    Observe(Observation),
    /// Definitively end the lineage (`EXITED` or proven-absent) and revoke
    /// its session. Requires the durable contact marker.
    Terminal(Observation),
}

fn apply_observation(
    writer: &Writer<'_>,
    attempt_id: &str,
    expected_status_revision: Revision,
    update: ObservationUpdate,
) -> Result<Revision, StoreError> {
    let now = crate::scheduling::now(writer)?;
    match update {
        ObservationUpdate::Observe(observation) => {
            if matches!(observation, Observation::Exited) {
                return writer.fail(StoreError::InvariantViolated(
                    "EXITED is definitive; use the terminal update".to_string(),
                ));
            }
            let new_revision = writer.update_revisioned_by(
                ATTEMPT_STATUS_TABLE,
                "attempt_id",
                attempt_id,
                expected_status_revision,
                &[
                    ("observed_execution", Value::from(observation.as_str())),
                    ("updated_at", Value::Integer(now)),
                ],
            )?;
            // Stamp first-liveness exactly once. The CAS above owns
            // concurrency; this statement only fills a still-unset fact and
            // never overwrites one.
            if matches!(observation, Observation::Starting | Observation::Running) {
                writer.execute(
                    "UPDATE attempt_status SET started_at = COALESCE(started_at, ?2)
                     WHERE attempt_id = ?1",
                    &[Value::from(attempt_id), Value::Integer(now)],
                )?;
            }
            Ok(new_revision)
        }
        ObservationUpdate::Terminal(observation) => {
            let contact: Option<String> = writer.query_optional(
                "SELECT launch_contact_state FROM attempt_status WHERE attempt_id = ?1",
                &[Value::from(attempt_id)],
                |row| row.get(0),
            )?;
            if contact.as_deref() != Some(LaunchContactState::ContactMayHaveOccurred.as_str()) {
                let detail = contact.unwrap_or_else(|| "absent".to_string());
                return writer.fail(StoreError::InvariantViolated(format!(
                    "attempt {attempt_id} cannot terminalize while launch contact is \
                     not proven ({detail})"
                )));
            }
            let new_revision = writer.update_revisioned_by(
                ATTEMPT_STATUS_TABLE,
                "attempt_id",
                attempt_id,
                expected_status_revision,
                &[
                    ("observed_execution", Value::from(observation.as_str())),
                    ("terminal", Value::Integer(1)),
                    ("finished_at", Value::Integer(now)),
                    ("updated_at", Value::Integer(now)),
                ],
            )?;
            writer.execute(
                "UPDATE agent_control_sessions
                 SET state = 'REVOKED', revoked_at = ?2,
                     revocation_reason = COALESCE(revocation_reason, 'attempt-terminal')
                 WHERE attempt_id = ?1 AND state = 'ACTIVE'",
                &[Value::from(attempt_id), Value::Integer(now)],
            )?;
            crate::command::append_internal_event(writer, "run.attempt.terminal")?;
            Ok(new_revision)
        }
    }
}

fn apply_conclude_run(
    writer: &Writer<'_>,
    run_id: &str,
    terminal_target: &'static str,
    expected_run_status_revision: Revision,
) -> Result<(), StoreError> {
    // Only conclusions that carry no Candidate are reachable today; Completed
    // requires Candidate submission machinery (#33) and is deliberately
    // absent here.
    if !matches!(terminal_target, "Failed" | "Cancelled") {
        return writer.fail(StoreError::InvariantViolated(format!(
            "terminal target {terminal_target} cannot conclude a Run directly"
        )));
    }
    let now = crate::scheduling::now(writer)?;
    let _advanced = writer.update_revisioned_by(
        "run_status",
        "run_id",
        run_id,
        expected_run_status_revision,
        &[
            ("phase", Value::from(terminal_target)),
            ("terminal_target", Value::from(terminal_target)),
            ("active_slot", Value::Null),
            ("updated_at", Value::Integer(now)),
        ],
    )?;
    crate::command::append_internal_event(writer, "run.concluded")?;
    Ok(())
}

#[cfg(test)]
mod tests;
