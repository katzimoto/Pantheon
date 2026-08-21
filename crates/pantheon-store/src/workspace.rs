//! Durable Task-owned Workspace authority.
//!
//! `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`
//! is canonical for what a Workspace is; this module owns only how one is
//! stored. Four authoritative mutations, each a single call to
//! [`Store::execute_command`], so each inherits the one authoritative
//! transaction, the durable command outcome and the Event append from Issue
//! #18 rather than opening a second write path.
//!
//! # Why the lifecycle is split this finely
//!
//! The canonical recovery contract makes durable Pantheon state — not the
//! filesystem, not Git worktree metadata, not a lock file — the authority for
//! Workspace ownership. That only holds if the durable row can answer the
//! question recovery actually asks: *may external state exist?*
//!
//! ```text
//! Requested     durable intent, no side effect attempted   → Absent
//! Materializing side effects authorized and may have run   → Unknown
//! Ready         verified materialization                   → Present
//! Error         a controller operation failed              → whatever was observed
//! ```
//!
//! [`Store::open_workspace`] commits `Requested` before the engine touches a
//! filesystem, and [`Store::begin_workspace_materialization`] is the durable
//! marker that gives up the conclusion "nothing exists yet" — the same shape
//! the persistence contract uses for external-contact markers (T4b, T15,
//! T16). Without that second transition, a crash between the row insert and
//! the first `git init` would be indistinguishable from a crash before it.
//!
//! # What the database refuses, rather than this code
//!
//! Two invariants are `CREATE TABLE`/`CREATE INDEX` constraints in migration
//! 8, because they are properties of the stored state rather than of any one
//! code path:
//!
//! - `phase = 'Ready'` requires `materialization = 'Present'`, so a partially
//!   materialized Workspace cannot be recorded Ready by any statement;
//! - `workspaces_one_current_per_task` is a partial unique index, so a Task
//!   cannot acquire a second non-`Released` Workspace even if two callers
//!   race with different command identities.
//!
//! The checks below still exist so the *expected* outcomes are typed errors
//! rather than opaque constraint violations. The database is the backstop,
//! not the message.

use pantheon_core::planning::TaskPhase;
use pantheon_core::workspace::{Materialization, RequestedBase, ResolvedBase, WorkspacePhase};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::seal::{SealAuthority, validate_seal_authority};
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

/// The durable table this module owns, named once so the CAS calls and the
/// typed conflicts cannot disagree about it.
const TABLE: &str = "workspaces";

/// One durable Workspace row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub id: String,
    /// The Task that owns this Workspace. Ownership is Task-scoped and
    /// survives Run turnover.
    pub task_id: String,
    /// The opaque repository reference the owning Task declared.
    pub repository: String,
    /// The controller-trusted local repository root the base was resolved
    /// against.
    pub source_path: String,
    /// What was asked for. May move afterwards; this record does not follow
    /// it.
    pub requested_base: RequestedBase,
    /// What it resolved to, once, before any mutable state existed.
    pub resolved_base: ResolvedBase,
    pub phase: WorkspacePhase,
    pub materialization: Materialization,
    pub revision: Revision,
}

/// What a Workspace is durably bound to at creation.
///
/// The resolved base is required here rather than filled in later: the
/// canonical idempotency identity for a Workspace is "Workspace ID +
/// deterministic desired path/base", so a row that does not yet know its base
/// could not be reconciled against external state at all.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceBinding<'a> {
    pub task_id: &'a str,
    pub repository: &'a str,
    pub source_path: &'a str,
    pub requested_base: &'a RequestedBase,
    pub resolved_base: &'a ResolvedBase,
}

impl Store {
    /// Commits durable Workspace identity and intention for a Task, before
    /// any filesystem or Git side effect.
    ///
    /// The Task is re-read inside this transaction: a Workspace is created
    /// for a Task that is logically eligible to own one, and eligibility is
    /// decided against durable state rather than against what the caller
    /// observed earlier.
    ///
    /// # Errors
    ///
    /// [`StoreError::WorkspaceHolderIneligible`] when the Task does not exist
    /// or is not in a phase that may own a new Workspace;
    /// [`StoreError::WorkspaceAlreadyCurrent`] when it already owns one; plus
    /// the command envelope's stale-epoch and conflict failures. Nothing is
    /// written in any of those cases.
    pub fn open_workspace(
        &self,
        command: &Command<'_>,
        workspace_id: &str,
        binding: &WorkspaceBinding<'_>,
    ) -> Result<Committed<WorkspaceRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let phase = task_phase(writer, binding.task_id)?;
            // `Ready` is the only phase from which this mission creates a
            // Workspace. It is deliberately narrow rather than "any
            // nonterminal phase": nothing yet moves a Task past Ready, so a
            // wider rule would be a claim no behaviour supports.
            if phase != TaskPhase::Ready {
                return writer.fail(StoreError::WorkspaceHolderIneligible {
                    task_id: binding.task_id.to_string(),
                    phase: phase.as_str(),
                });
            }

            if let Some(existing) = current_workspace_id(writer, binding.task_id)? {
                return writer.fail(StoreError::WorkspaceAlreadyCurrent {
                    task_id: binding.task_id.to_string(),
                    workspace_id: existing,
                });
            }

            let now = now(writer)?;
            writer.execute(
                "INSERT INTO workspaces (
                     id, task_id, repository, source_path, requested_base, resolved_base,
                     phase, materialization, revision, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)",
                &[
                    Value::from(workspace_id),
                    Value::from(binding.task_id),
                    Value::from(binding.repository),
                    Value::from(binding.source_path),
                    Value::from(binding.requested_base.as_str()),
                    Value::from(binding.resolved_base.as_str()),
                    Value::from(WorkspacePhase::Requested.as_str()),
                    Value::from(Materialization::Absent.as_str()),
                    Value::Integer(now),
                ],
            )?;

            Ok(WorkspaceRecord {
                id: workspace_id.to_string(),
                task_id: binding.task_id.to_string(),
                repository: binding.repository.to_string(),
                source_path: binding.source_path.to_string(),
                requested_base: binding.requested_base.clone(),
                resolved_base: binding.resolved_base.clone(),
                phase: WorkspacePhase::Requested,
                materialization: Materialization::Absent,
                revision: Revision::new(1),
            })
        })
    }

    /// Records that materialization is authorized and external state may now
    /// exist.
    ///
    /// This must commit *before* the first filesystem or Git side effect. It
    /// is also the retry entry point: a Workspace that has never been `Ready`
    /// has never been mutable to any worker, so whatever exists at its path
    /// is controller-owned scratch that may be discarded and rebuilt at the
    /// same identity and base. A Workspace that *has* been `Ready` may hold
    /// unsealed work, and this refuses it.
    ///
    /// # Errors
    ///
    /// [`StoreError::WorkspaceNotRematerializable`] when the Workspace has
    /// already been mutable; [`StoreError::RevisionConflict`] when it does not
    /// exist or has moved; plus the command envelope's failures.
    pub fn begin_workspace_materialization(
        &self,
        command: &Command<'_>,
        workspace_id: &str,
        expected: Revision,
    ) -> Result<Committed<WorkspaceRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let phase = workspace_phase(writer, workspace_id, expected)?;
            if phase.has_been_mutable() {
                return writer.fail(StoreError::WorkspaceNotRematerializable {
                    workspace_id: workspace_id.to_string(),
                    phase: phase.as_str(),
                });
            }
            transition(
                writer,
                workspace_id,
                expected,
                WorkspacePhase::Materializing,
                Materialization::Unknown,
            )
        })
    }

    /// Records verified materialization: the Workspace becomes `Ready`.
    ///
    /// `verified_base` is the identity the materializer actually established
    /// on disk, and it is compared against the durable binding inside this
    /// transaction. Without that comparison, a materializer that checked out
    /// the wrong commit would still produce a `Ready` Workspace whose durable
    /// base is a lie.
    ///
    /// # Errors
    ///
    /// [`StoreError::WorkspaceBaseMismatch`] when the verified identity is not
    /// the bound one; [`StoreError::InvariantViolated`] when the Workspace is
    /// not `Materializing`; [`StoreError::RevisionConflict`] when it has
    /// moved; plus the command envelope's failures.
    pub fn complete_workspace_materialization(
        &self,
        command: &Command<'_>,
        workspace_id: &str,
        expected: Revision,
        verified_base: &ResolvedBase,
    ) -> Result<Committed<WorkspaceRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let (phase, bound_base) = workspace_phase_and_base(writer, workspace_id, expected)?;
            if phase != WorkspacePhase::Materializing {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "workspace {workspace_id} is {phase} and cannot become Ready"
                )));
            }
            if &bound_base != verified_base {
                return writer.fail(StoreError::WorkspaceBaseMismatch {
                    workspace_id: workspace_id.to_string(),
                    bound: bound_base.as_str().to_string(),
                    verified: verified_base.as_str().to_string(),
                });
            }
            transition(
                writer,
                workspace_id,
                expected,
                WorkspacePhase::Ready,
                Materialization::Present,
            )
        })
    }

    /// Records a failed materialization together with what is actually known
    /// about external state.
    ///
    /// `observed` is a factual observation, and the caller supplies
    /// [`Materialization::Unknown`] unless it established otherwise. An error
    /// is never by itself evidence that a partially created Workspace
    /// directory is absent.
    ///
    /// Refuses a Workspace that has already been mutable, for a reason that is
    /// not obvious from this transition alone. The rematerialization fence in
    /// [`Store::begin_workspace_materialization`] answers "has this Workspace
    /// ever been `Ready`?" from the row's *current* phase, and `Error` reads as
    /// never-mutable. So a `Ready` row moved to `Error` here would lose the only
    /// durable evidence that protects it, after which rebuilding it would pass
    /// every remaining check and discard worker-writable state. The evidence has
    /// to be non-erasable for the fence that reads it to mean anything.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when `observed` is `Present`, which
    /// would claim a verified materialization this path never performed;
    /// [`StoreError::WorkspaceNotRematerializable`] when the Workspace has
    /// already been mutable; [`StoreError::RevisionConflict`] when the
    /// Workspace has moved; plus the command envelope's failures.
    pub fn fail_workspace_materialization(
        &self,
        command: &Command<'_>,
        workspace_id: &str,
        expected: Revision,
        observed: Materialization,
    ) -> Result<Committed<WorkspaceRecord>, StoreError> {
        self.execute_command(command, |writer| {
            if observed == Materialization::Present {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "workspace {workspace_id} cannot record Present materialization \
                     on a failure path"
                )));
            }
            let phase = workspace_phase(writer, workspace_id, expected)?;
            if phase.has_been_mutable() {
                return writer.fail(StoreError::WorkspaceNotRematerializable {
                    workspace_id: workspace_id.to_string(),
                    phase: phase.as_str(),
                });
            }
            transition(
                writer,
                workspace_id,
                expected,
                WorkspacePhase::Error,
                observed,
            )
        })
    }

    /// Freezes a Ready Workspace: mutation authority is suspended while
    /// authoritative capture runs.
    ///
    /// This is the durable half of capture quiescence (see the sealing
    /// controller for the whole story): it CASes `Ready -> Frozen` under the
    /// normal command envelope, and inside the same transaction it re-reads
    /// and requires the seal's execution authority — the Run named by
    /// [`SealAuthority`] must be the Task's current, nonterminal,
    /// revision-current responsible Run, bound to exactly this Workspace at
    /// its immutable base, under a specification whose requested output slot
    /// permits a `code.changeset`. A committed freeze therefore means both
    /// that every Pantheon-visible mutation path is closed behind the
    /// serialized writer *and* that a live Run relation authorized this
    /// exact capture.
    ///
    /// Materialization stays exactly what it was (`Present` for a frozen
    /// Ready Workspace). A freeze is a control-plane fence, not an
    /// observation; nothing here claims or retracts external state.
    ///
    /// # Errors
    ///
    /// [`StoreError::WorkspaceNotFreezable`] when the Workspace is not in the
    /// verified-mutable state; [`StoreError::SealAuthorityInvalid`] when the
    /// claimed Run authority does not hold (see the crate-private
    /// `validate_seal_authority`);
    /// [`StoreError::RevisionConflict`] when it has moved or does not exist;
    /// plus the command envelope's failures. Nothing is written in any of
    /// those cases.
    pub fn freeze_workspace(
        &self,
        command: &Command<'_>,
        authority: &SealAuthority,
        task_id: &str,
        output_slot: &str,
        workspace_id: &str,
        expected: Revision,
    ) -> Result<Committed<WorkspaceRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let row: Option<(String, String, String)> = writer.query_optional(
                "SELECT task_id, phase, materialization FROM workspaces WHERE id = ?1",
                &[Value::from(workspace_id)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let Some((owner, phase_text, materialization_text)) = row else {
                return writer.fail(StoreError::RevisionConflict {
                    table: TABLE,
                    id: workspace_id.to_string(),
                    expected: expected.get(),
                    actual: None,
                });
            };
            if owner != task_id {
                return writer.fail(StoreError::SealAuthorityInvalid {
                    workspace_id: workspace_id.to_string(),
                    detail: format!("the workspace is owned by {owner}, not {task_id}"),
                });
            }
            let phase = WorkspacePhase::parse(&phase_text).ok_or_else(|| {
                StoreError::InvariantViolated(format!(
                    "workspace {workspace_id} has unknown phase {phase_text}"
                ))
            })?;
            let materialization =
                Materialization::parse(&materialization_text).ok_or_else(|| {
                    StoreError::InvariantViolated(format!(
                        "workspace {workspace_id} has unknown materialization \
                         {materialization_text}"
                    ))
                })?;
            if phase != WorkspacePhase::Ready || materialization != Materialization::Present {
                return writer.fail(StoreError::WorkspaceNotFreezable {
                    workspace_id: workspace_id.to_string(),
                    phase: phase.as_str(),
                    materialization: materialization.as_str(),
                });
            }
            // The freeze is the authorization act: prove the Run relation
            // before committing it.
            validate_seal_authority(writer, authority, task_id, output_slot, workspace_id)?;
            transition(
                writer,
                workspace_id,
                expected,
                WorkspacePhase::Frozen,
                Materialization::Present,
            )
        })
    }

    /// Revalidates the seal's execution authority against current durable
    /// state, writing nothing.
    ///
    /// This is the boundary an already-frozen retry runs before capture: a
    /// Workspace whose fence was established by an earlier attempt must not
    /// bypass current Run authorization merely because that fence exists.
    /// It takes the authoritative transaction through the normal command
    /// envelope — so the check is serialized against every other writer and
    /// its outcome is durably recorded as an Event — but performs no
    /// lifecycle mutation on purpose.
    ///
    /// # Errors
    ///
    /// [`StoreError::SealAuthorityInvalid`] when the claimed Run authority
    /// does not hold or the Workspace no longer matches the claim;
    /// [`StoreError::RevisionConflict`] when the Workspace has moved or does
    /// not exist; plus the command envelope's failures.
    pub fn validate_seal_authority_command(
        &self,
        command: &Command<'_>,
        authority: &SealAuthority,
        task_id: &str,
        output_slot: &str,
        workspace_id: &str,
        expected: Revision,
    ) -> Result<Committed<WorkspaceRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let record = read_in_transaction(writer, workspace_id)?;
            if record.revision != expected {
                return writer.fail(StoreError::RevisionConflict {
                    table: TABLE,
                    id: workspace_id.to_string(),
                    expected: expected.get(),
                    actual: Some(record.revision.get()),
                });
            }
            if record.task_id != task_id {
                return writer.fail(StoreError::SealAuthorityInvalid {
                    workspace_id: workspace_id.to_string(),
                    detail: format!(
                        "the workspace is owned by {}, not {task_id}",
                        record.task_id
                    ),
                });
            }
            if record.phase != WorkspacePhase::Frozen
                || record.materialization != Materialization::Present
            {
                return writer.fail(StoreError::WorkspaceNotFreezable {
                    workspace_id: workspace_id.to_string(),
                    phase: record.phase.as_str(),
                    materialization: record.materialization.as_str(),
                });
            }
            validate_seal_authority(writer, authority, task_id, output_slot, workspace_id)?;
            Ok(record)
        })
    }

    /// Records typed evidence that a capture attempt failed, without
    /// changing any lifecycle fact.
    ///
    /// The Workspace stays `Frozen`: thawing after an unexplained failure
    /// would hand mutation authority back while nobody knows what the
    /// failure did, and rematerializing would destroy worker-writable state
    /// that may be unsealed work. Recovery decides separately, from real
    /// evidence, what happens next. The command envelope appends the Event;
    /// this mutation writes nothing else on purpose.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionConflict`] when the Workspace has moved or does
    /// not exist; [`StoreError::InvariantViolated`] when it is not frozen —
    /// recording failure against a mutable Workspace would claim a fence
    /// that does not hold; plus the command envelope's failures.
    pub fn record_capture_failure(
        &self,
        command: &Command<'_>,
        workspace_id: &str,
        expected: Revision,
    ) -> Result<Committed<WorkspaceRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let row = read_in_transaction(writer, workspace_id)?;
            if row.revision != expected {
                return writer.fail(StoreError::RevisionConflict {
                    table: TABLE,
                    id: workspace_id.to_string(),
                    expected: expected.get(),
                    actual: Some(row.revision.get()),
                });
            }
            if row.phase != WorkspacePhase::Frozen {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "capture failure may only be recorded against a frozen \
                     workspace, and {workspace_id} is {}",
                    row.phase.as_str()
                )));
            }
            Ok(row)
        })
    }

    /// The Task's current Workspace, if it owns one.
    ///
    /// "Current" means any phase other than `Released`, matching the partial
    /// unique index that makes at most one such row possible.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when a stored row cannot be
    /// interpreted; [`StoreError::Sqlite`] on a storage failure.
    pub fn workspace_for_task(&self, task_id: &str) -> Result<Option<WorkspaceRecord>, StoreError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, repository, source_path, requested_base, resolved_base,
                            phase, materialization, revision
                     FROM workspaces WHERE task_id = ?1 AND phase != ?2",
                    rusqlite::params![task_id, WorkspacePhase::Released.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, i64>(7)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;

            row.map(
                |(
                    id,
                    repository,
                    source_path,
                    requested_base,
                    resolved_base,
                    phase,
                    materialization,
                    revision,
                )| {
                    Ok(WorkspaceRecord {
                        requested_base: parse_requested(&requested_base, &id)?,
                        resolved_base: parse_resolved(&resolved_base, &id)?,
                        phase: WorkspacePhase::parse(&phase).ok_or_else(|| {
                            StoreError::InvariantViolated(format!(
                                "workspace {id} has unknown phase {phase}"
                            ))
                        })?,
                        materialization: Materialization::parse(&materialization).ok_or_else(
                            || {
                                StoreError::InvariantViolated(format!(
                                    "workspace {id} has unknown materialization {materialization}"
                                ))
                            },
                        )?,
                        id,
                        task_id: task_id.to_string(),
                        repository,
                        source_path,
                        revision: Revision::new(revision),
                    })
                },
            )
            .transpose()
        })
    }
}

/// Applies one revisioned phase/materialization transition and returns the
/// row it produced.
fn transition(
    writer: &Writer<'_>,
    workspace_id: &str,
    expected: Revision,
    phase: WorkspacePhase,
    materialization: Materialization,
) -> Result<WorkspaceRecord, StoreError> {
    let revision = writer.update_revisioned(
        TABLE,
        workspace_id,
        expected,
        &[
            ("phase", Value::from(phase.as_str())),
            ("materialization", Value::from(materialization.as_str())),
        ],
    )?;
    let mut record = read_in_transaction(writer, workspace_id)?;
    record.revision = revision;
    Ok(record)
}

/// The owning Task's phase, read inside the authoritative transaction.
fn task_phase(writer: &Writer<'_>, task_id: &str) -> Result<TaskPhase, StoreError> {
    let phase: Option<String> = writer.query_optional(
        "SELECT phase FROM tasks WHERE id = ?1",
        &[Value::from(task_id)],
        |row| row.get(0),
    )?;
    let Some(phase) = phase else {
        return writer.fail(StoreError::WorkspaceHolderIneligible {
            task_id: task_id.to_string(),
            phase: "Absent",
        });
    };
    TaskPhase::parse(&phase).ok_or_else(|| {
        StoreError::InvariantViolated(format!("task {task_id} has unknown phase {phase}"))
    })
}

fn current_workspace_id(writer: &Writer<'_>, task_id: &str) -> Result<Option<String>, StoreError> {
    writer.query_optional(
        "SELECT id FROM workspaces WHERE task_id = ?1 AND phase != ?2",
        &[
            Value::from(task_id),
            Value::from(WorkspacePhase::Released.as_str()),
        ],
        |row| row.get(0),
    )
}

/// The Workspace's phase, refusing to proceed if its revision has already
/// moved.
///
/// Reading the phase and CASing on the revision are two statements, so this
/// checks the revision here as well. Both run inside one `BEGIN IMMEDIATE`
/// transaction on the single authoritative writer, so nothing can change the
/// row between them; the check exists so a caller that observed a stale
/// revision fails on the fact it got wrong rather than on a phase it was
/// never entitled to read.
fn workspace_phase(
    writer: &Writer<'_>,
    workspace_id: &str,
    expected: Revision,
) -> Result<WorkspacePhase, StoreError> {
    Ok(workspace_phase_and_base(writer, workspace_id, expected)?.0)
}

fn workspace_phase_and_base(
    writer: &Writer<'_>,
    workspace_id: &str,
    expected: Revision,
) -> Result<(WorkspacePhase, ResolvedBase), StoreError> {
    let row: Option<(String, String, i64)> = writer.query_optional(
        "SELECT phase, resolved_base, revision FROM workspaces WHERE id = ?1",
        &[Value::from(workspace_id)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;

    let Some((phase, base, revision)) = row else {
        return writer.fail(StoreError::RevisionConflict {
            table: TABLE,
            id: workspace_id.to_string(),
            expected: expected.get(),
            actual: None,
        });
    };
    if revision != expected.get() {
        return writer.fail(StoreError::RevisionConflict {
            table: TABLE,
            id: workspace_id.to_string(),
            expected: expected.get(),
            actual: Some(revision),
        });
    }

    let phase = WorkspacePhase::parse(&phase).ok_or_else(|| {
        StoreError::InvariantViolated(format!(
            "workspace {workspace_id} has unknown phase {phase}"
        ))
    })?;
    Ok((phase, parse_resolved(&base, workspace_id)?))
}

fn read_in_transaction(
    writer: &Writer<'_>,
    workspace_id: &str,
) -> Result<WorkspaceRecord, StoreError> {
    let row = writer
        .query_optional(
            "SELECT task_id, repository, source_path, requested_base, resolved_base,
                    phase, materialization, revision
             FROM workspaces WHERE id = ?1",
            &[Value::from(workspace_id)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated(format!(
                "workspace {workspace_id} disappeared inside its own transaction"
            ))
        })?;

    let (
        task_id,
        repository,
        source_path,
        requested_base,
        resolved_base,
        phase,
        materialization,
        revision,
    ) = row;
    Ok(WorkspaceRecord {
        id: workspace_id.to_string(),
        task_id,
        repository,
        source_path,
        requested_base: parse_requested(&requested_base, workspace_id)?,
        resolved_base: parse_resolved(&resolved_base, workspace_id)?,
        phase: WorkspacePhase::parse(&phase).ok_or_else(|| {
            StoreError::InvariantViolated(format!(
                "workspace {workspace_id} has unknown phase {phase}"
            ))
        })?,
        materialization: Materialization::parse(&materialization).ok_or_else(|| {
            StoreError::InvariantViolated(format!(
                "workspace {workspace_id} has unknown materialization {materialization}"
            ))
        })?,
        revision: Revision::new(revision),
    })
}

fn parse_requested(value: &str, workspace_id: &str) -> Result<RequestedBase, StoreError> {
    RequestedBase::parse(value).map_err(|err| {
        StoreError::InvariantViolated(format!(
            "workspace {workspace_id} has an unusable requested base: {err}"
        ))
    })
}

fn parse_resolved(value: &str, workspace_id: &str) -> Result<ResolvedBase, StoreError> {
    ResolvedBase::parse(value).map_err(|err| {
        StoreError::InvariantViolated(format!(
            "workspace {workspace_id} has an unusable resolved base: {err}"
        ))
    })
}

fn now(writer: &Writer<'_>) -> Result<i64, StoreError> {
    writer
        .query_optional("SELECT unixepoch()", &[], |row| row.get::<_, i64>(0))?
        .ok_or_else(|| StoreError::InvariantViolated("could not read the current time".to_string()))
}

#[cfg(test)]
mod tests;
