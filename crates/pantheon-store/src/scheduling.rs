//! Durable scheduler state and the T3 Run-intent commit boundary.
//!
//! `docs/architecture/scheduling/scheduler-dispatch-and-run-intent-reconciliation.md`
//! is canonical for what T3 is: the single authoritative transaction that
//! turns one selected route into durable execution responsibility. This module
//! owns how that state is stored; `pantheon-engine` decides when to attempt
//! it.
//!
//! # What makes the slot exclusion real
//!
//! The v0.1.0 envelope has exactly one global active-execution slot. It is
//! enforced twice, deliberately:
//!
//! 1. Controller checks inside the authoritative transaction produce typed
//!    errors ([`StoreError::DispatchSlotUnavailable`],
//!    [`StoreError::RevisionConflict`]) for the expected concurrent outcomes;
//! 2. the `one_active_execution_slot` partial unique index on `run_status`
//!    rejects a second nonterminal Run at the database layer, so two callers
//!    racing under different command identities cannot both commit even if a
//!    future edit removes a check.
//!
//! Every T3 carries the observed `scheduler_state.revision`, and every
//! committed T3 advances it (with the fairness charge), so of two callers who
//! observed the same world, exactly one can commit.

use pantheon_core::config::Digest;
use pantheon_core::planning::{GoalPhase, TaskPhase};
use pantheon_core::scheduling::{
    ContextSourceSnapshot, DispatchMode, ExecutionBinding, GoalFairness,
};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

const SCHEDULER_TABLE: &str = "scheduler_state";
const TASK_SCHEDULING_TABLE: &str = "task_scheduling_state";
const GOAL_SCHEDULING_TABLE: &str = "goal_scheduling_state";

/// The stored component digests of one ConfigurationRevision, as read inside
/// a T3 revalidation.
type StoredRevisionDigests = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
);

/// The Task facts a T3 commit revalidates, read on the transaction's own
/// snapshot.
type TaskFacts = (String, String, i64, Option<String>, Vec<u8>);

/// The singleton durable scheduler state row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerStateRecord {
    pub dispatch_mode: DispatchMode,
    pub next_service_sequence: i64,
    pub revision: Revision,
}

/// One Goal's durable fairness position, with the row revision a T3 charge
/// must CAS against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSchedulingRow {
    pub goal_id: String,
    pub last_served_sequence: Option<i64>,
    pub revision: Revision,
}

impl GoalSchedulingRow {
    /// The pure ordering input this row contributes.
    #[must_use]
    pub fn fairness(&self) -> GoalFairness {
        GoalFairness {
            goal_id: self.goal_id.clone(),
            last_served_sequence: self.last_served_sequence,
        }
    }
}

/// One Task that is currently dispatchable, with every revision a T3 commit
/// must revalidate, read as one consistent snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchCandidate {
    /// The stable identity selected by the pure ordering decision.
    pub task_id: String,
    pub goal_id: String,
    pub spec_digest: Digest,
    pub eligible_since: i64,
    pub task_revision: Revision,
    pub goal_current_revision: i64,
    pub goal_row_revision: Revision,
    pub graph_revision: i64,
    pub workspace_id: String,
    pub workspace_resolved_base: String,
    pub workspace_revision: Revision,
    /// The `task_scheduling_state` row revision, for backoff CAS.
    pub scheduling_revision: Revision,
}

impl DispatchCandidate {
    /// The pure ordering input this candidate contributes.
    #[must_use]
    pub fn schedulable(&self) -> pantheon_core::scheduling::SchedulableTask {
        pantheon_core::scheduling::SchedulableTask {
            task_id: self.task_id.clone(),
            goal_id: self.goal_id.clone(),
            eligible_since: self.eligible_since,
        }
    }
}

/// Everything one scheduling cycle reads from authority, in one snapshot.
///
/// The three queries behind this run inside one explicit read transaction, so
/// ordering inputs agree with each other: a fairness row, a candidate list and
/// the scheduler singleton are never observed from different moments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingSnapshot {
    pub state: SchedulerStateRecord,
    /// One fairness position per Goal that has ever been charged. Absent rows
    /// mean "never served" and are handled by the pure selection rule.
    pub goals: Vec<GoalSchedulingRow>,
    pub candidates: Vec<DispatchCandidate>,
    /// The instant this snapshot considers current, used for backoff math.
    pub now: i64,
}

/// What one successful T3 established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunIntentCommit {
    pub run_id: String,
    /// The Task after activation: phase `Active`, pointing at the Run.
    pub task_revision: Revision,
    /// The scheduler singleton revision after the fairness charge advanced.
    pub scheduler_revision: Revision,
    /// The service sequence charged to the Goal, atomically with the Run.
    pub charged_service_sequence: i64,
}
/// Everything the engine froze before asking the store to commit T3.
///
/// Digests are computed by the caller; this crate persists them and verifies
/// them against *stored* authority, but it does not hash anything itself.
#[derive(Debug, Clone)]
pub struct RunIntent<'a> {
    pub run_id: &'a str,
    pub task_id: &'a str,
    pub goal_id: &'a str,
    pub expected_task_revision: Revision,
    pub expected_goal_row_revision: Revision,
    pub expected_goal_current_revision: i64,
    pub expected_graph_revision: i64,
    pub expected_workspace_revision: Revision,
    pub expected_scheduler_revision: Revision,
    /// `None` when no fairness row existed for the Goal at observation time.
    pub expected_goal_fairness_revision: Option<Revision>,
    pub expected_task_scheduling_revision: Revision,
    pub configuration_activation_sequence: i64,
    pub binding_digest: &'a Digest,
    pub binding: &'a ExecutionBinding,
    pub snapshot_digest: &'a Digest,
    pub snapshot: &'a ContextSourceSnapshot,
}

impl Store {
    /// The durable scheduler singleton.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when the row exists but cannot be
    /// interpreted; [`StoreError::Sqlite`] on storage failure.
    pub fn scheduler_state(&self) -> Result<SchedulerStateRecord, StoreError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT dispatch_mode, next_service_sequence, revision
                     FROM scheduler_state WHERE id = 'singleton'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            row.map(parse_state).transpose()?.ok_or_else(|| {
                StoreError::InvariantViolated("scheduler_state has no singleton row".to_string())
            })
        })
    }

    /// The Run currently holding the global execution slot, if any.
    ///
    /// Admission consults this before spending a routing cycle; restart
    /// reconstruction proves the same property from the same row.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when more than one row holds the
    /// slot, which the partial unique index makes unreachable but which would
    /// mean the schema has been tampered with.
    pub fn slot_holder(&self) -> Result<Option<(String, String)>, StoreError> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id FROM run_status WHERE active_slot IS NOT NULL ORDER BY run_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut holders = Vec::new();
            for row in rows {
                holders.push(row?);
            }
            match holders.len() {
                0 => Ok(None),
                1 => Ok(holders.pop()),
                n => Err(StoreError::InvariantViolated(format!(
                    "{n} runs hold the single execution slot"
                ))),
            }
        })
    }

    /// Reads everything one scheduling decision needs, as one snapshot.
    ///
    /// A Task appears in `candidates` only when it satisfies the full logical
    /// eligibility predicate: Ready under a dispatchable Goal, prerequisites
    /// satisfied, zero nonterminal responsible Runs, owning a verified
    /// Workspace, currently eligible with any backoff elapsed. Temporary
    /// suppression (pause, configuration readiness, slot occupancy) is *not*
    /// applied here; the controller applies those gates so an operator pause
    /// never erases a waiting age or pretends a Task became ineligible.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read or interpreted.
    pub fn scheduling_snapshot(&self) -> Result<SchedulingSnapshot, StoreError> {
        self.read_snapshot(|conn| {
            let state = parse_state(
                conn.query_row(
                    "SELECT dispatch_mode, next_service_sequence, revision
                     FROM scheduler_state WHERE id = 'singleton'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .map_err(|err| {
                    StoreError::InvariantViolated(format!(
                        "scheduler_state singleton unreadable: {err}"
                    ))
                })?,
            )?;

            let now: i64 = conn.query_row("SELECT unixepoch()", [], |row| row.get(0))?;

            let mut stmt = conn.prepare(
                "SELECT goal_id, last_served_sequence, revision
                 FROM goal_scheduling_state ORDER BY goal_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(GoalSchedulingRow {
                    goal_id: row.get::<_, String>(0)?,
                    last_served_sequence: row.get::<_, Option<i64>>(1)?,
                    revision: Revision::new(row.get::<_, i64>(2)?),
                })
            })?;
            let mut goals = Vec::new();
            for row in rows {
                goals.push(row?);
            }

            let mut stmt = conn.prepare(
                "SELECT t.id, t.goal_id, t.spec_digest, t.revision,
                        g.current_revision, g.revision AS goal_row_revision,
                        gr.revision AS graph_revision,
                        w.id AS workspace_id, w.resolved_base, w.revision AS workspace_revision,
                        s.eligible_since, s.revision AS scheduling_revision
                 FROM tasks t
                 JOIN goals g ON g.id = t.goal_id
                 JOIN task_graphs gr ON gr.id = t.goal_id
                 JOIN workspaces w
                   ON w.task_id = t.id AND w.phase = 'Ready' AND w.materialization = 'Present'
                 JOIN task_scheduling_state s ON s.task_id = t.id
                 WHERE t.phase = 'Ready'
                   AND g.phase IN ('Planning', 'Active', 'Evaluating')
                   AND s.eligible_since IS NOT NULL
                   AND (s.next_attempt_at IS NULL OR s.next_attempt_at <= ?1)
                   AND NOT EXISTS (
                       SELECT 1 FROM run_status rs
                       WHERE rs.task_id = t.id
                         AND rs.phase IN ('Active', 'Finalizing'))
                   AND NOT EXISTS (
                       SELECT 1 FROM task_graph_edges e
                       JOIN tasks up ON up.id = e.upstream_task_id
                       WHERE e.downstream_task_id = t.id
                         AND e.kind = 'requires_success'
                         AND e.created_graph_revision <= gr.revision
                         AND (e.removed_graph_revision IS NULL
                              OR e.removed_graph_revision > gr.revision)
                         AND up.phase != 'Succeeded')
                 ORDER BY t.id",
            )?;
            let rows = stmt.query_map(rusqlite::params![now], |row| {
                let spec_digest: Vec<u8> = row.get(2)?;
                Ok(DispatchCandidate {
                    task_id: row.get(0)?,
                    goal_id: row.get(1)?,
                    spec_digest: crate::planning::digest_from(&spec_digest, "spec_digest")
                        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?,
                    eligible_since: row.get(10)?,
                    task_revision: Revision::new(row.get(3)?),
                    goal_current_revision: row.get(4)?,
                    goal_row_revision: Revision::new(row.get(5)?),
                    graph_revision: row.get(6)?,
                    workspace_id: row.get(7)?,
                    workspace_resolved_base: row.get(8)?,
                    workspace_revision: Revision::new(row.get(9)?),
                    scheduling_revision: Revision::new(row.get(11)?),
                })
            })?;
            let mut candidates = Vec::new();
            for row in rows {
                candidates.push(row?);
            }

            Ok(SchedulingSnapshot {
                state,
                goals,
                candidates,
                now,
            })
        })
    }

    /// Commits the operator's desired dispatch mode through the command
    /// envelope.
    ///
    /// The CAS on `scheduler_state.revision` is the optimistic-concurrency
    /// path the Operator contract requires (`If-Match`). Pausing an already
    /// paused dispatcher is accepted and audited: it re-asserts durable intent
    /// rather than pretending nothing happened.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionConflict`] when the singleton moved since the
    /// caller observed it; plus the command envelope's failures.
    pub fn set_dispatch_mode(
        &self,
        command: &Command<'_>,
        mode: DispatchMode,
        expected: Revision,
    ) -> Result<Committed<SchedulerStateRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let now = now(writer)?;
            let _new_revision = writer.update_revisioned(
                SCHEDULER_TABLE,
                "singleton",
                expected,
                &[
                    ("dispatch_mode", Value::from(mode.as_str())),
                    ("updated_at", Value::Integer(now)),
                ],
            )?;
            read_state(writer)
        })
    }

    /// Records durable scheduling-attempt backoff for one Task.
    ///
    /// This is controller bookkeeping, not an operator command: it goes
    /// through the serialized authoritative writer without a command identity,
    /// exactly like every internal eligibility transition. It never touches
    /// Task lifecycle and never resets `eligible_since`.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionConflict`] when the row moved or does not exist.
    pub fn record_scheduling_backoff(
        &self,
        task_id: &str,
        expected: Revision,
        code: &str,
        detail_json: &str,
        next_attempt_at: i64,
    ) -> Result<Revision, StoreError> {
        self.write(|writer| {
            writer.update_revisioned_by(
                TASK_SCHEDULING_TABLE,
                "task_id",
                task_id,
                expected,
                &[
                    ("next_attempt_at", Value::Integer(next_attempt_at)),
                    ("last_failure_code", Value::from(code)),
                    ("last_failure_detail_json", Value::from(detail_json)),
                    ("updated_at", Value::Integer(now(writer)?)),
                ],
            )
        })
    }

    /// Commits one T3 Run-intent: the atomic boundary between scheduling and
    /// execution responsibility.
    ///
    /// Every mutable precondition the caller observed is re-read *inside* this
    /// transaction and compared against what the caller froze; any drift rolls
    /// the whole commit back. On success the same transaction commits the
    /// immutable Binding, the immutable source-snapshot identity, the Run, its
    /// `Active` status holding the global slot, Task `Ready -> Active` with the
    /// responsible-Run pointer, the Goal fairness service charge, the
    /// scheduler-sequence advance, the Task's eligibility-interval close-out,
    /// and the Event. Nothing outside this database is touched: no process,
    /// backend, network, repository, retrieval or rendering happens here.
    ///
    /// # Errors
    ///
    /// - [`StoreError::DispatchPaused`] when durable dispatch mode is PAUSED;
    /// - [`StoreError::DispatchSlotUnavailable`] when another nonterminal Run
    ///   holds the single execution slot;
    /// - [`StoreError::TaskNotDispatchable`] when the Task is absent, not
    ///   Ready, or already names a responsible Run;
    /// - [`StoreError::GoalNotDispatchable`] when the Goal's lifecycle fences
    ///   new Runs;
    /// - [`StoreError::RevisionConflict`] when any observed authority moved;
    /// - [`StoreError::InvariantViolated`] when frozen identities disagree
    ///   with stored ones, which means the caller tried to bind records from
    ///   different worlds;
    ///
    /// plus the command envelope's failures. In every failure case nothing is
    /// written.
    #[allow(clippy::too_many_lines)]
    pub fn commit_run_intent(
        &self,
        command: &Command<'_>,
        intent: &RunIntent<'_>,
    ) -> Result<Committed<RunIntentCommit>, StoreError> {
        self.execute_command(command, |writer| apply_run_intent(writer, intent))
    }
}

fn apply_run_intent(
    writer: &Writer<'_>,
    intent: &RunIntent<'_>,
) -> Result<RunIntentCommit, StoreError> {
    // 1. Dispatch permission, first: PAUSED fences new T3 commits outright.
    let state = read_state(writer)?;
    if state.dispatch_mode != DispatchMode::Running {
        return writer.fail(StoreError::DispatchPaused);
    }
    // The fairness/scheduler fence: a caller who observed an older world must
    // not commit into the new one. Every committed T3 advances this revision,
    // which is what makes two racing callers resolve deterministically.
    if state.revision != intent.expected_scheduler_revision {
        return writer.fail(StoreError::RevisionConflict {
            table: SCHEDULER_TABLE,
            id: "singleton".to_string(),
            expected: intent.expected_scheduler_revision.get(),
            actual: Some(state.revision.get()),
        });
    }

    // 2. Configuration currency: the captured revision must still be active,
    //    and the Binding must belong to exactly that revision — all component
    //    digests, not a chosen few. A mixed-revision Binding is unrepresentable.
    let active: Option<i64> = writer
        .query_optional(
            "SELECT activation_sequence FROM active_configuration WHERE id = 'singleton'",
            &[],
            |row| row.get::<_, Option<i64>>(0),
        )?
        .flatten();
    if active != Some(intent.configuration_activation_sequence) {
        return writer.fail(StoreError::RevisionConflict {
            table: "active_configuration",
            id: "singleton".to_string(),
            expected: intent.configuration_activation_sequence,
            actual: active,
        });
    }
    let stored_revision: Option<StoredRevisionDigests> = writer.query_optional(
        "SELECT content_digest, agents_digest, routing_digest, execution_profile_digest,
                    evaluator_registry_digest, context_policy_digest, authorization_digest
             FROM configuration_revisions WHERE activation_sequence = ?1",
        &[Value::Integer(intent.configuration_activation_sequence)],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        },
    )?;
    let Some((
        content_digest,
        agents_digest,
        routing_digest,
        execution_profile_digest,
        evaluator_registry_digest,
        context_policy_digest,
        authorization_digest,
    )) = stored_revision
    else {
        return writer.fail(StoreError::InvariantViolated(format!(
            "active configuration names revision {} which is not stored",
            intent.configuration_activation_sequence
        )));
    };
    let components = &intent.binding.component_digests;
    let matches = content_digest.as_slice()
        == intent.binding.configuration_content_digest.as_bytes()
        && agents_digest.as_slice() == components.agents.as_bytes()
        && routing_digest.as_slice() == components.routing.as_bytes()
        && execution_profile_digest.as_slice() == components.execution_profile.as_bytes()
        && evaluator_registry_digest.as_slice() == components.evaluator_registry.as_bytes()
        && context_policy_digest.as_slice() == components.context_policy.as_bytes()
        && authorization_digest.as_slice() == components.authorization.as_bytes();
    if !matches || intent.snapshot.context_policy_digest != components.context_policy {
        return writer.fail(StoreError::InvariantViolated(
            "the frozen Binding or source snapshot does not belong to the active \
             ConfigurationRevision"
                .to_string(),
        ));
    }
    // The frozen records carry their own owners; those owners must be this
    // Run's Task and Goal. Both foreign keys would happily accept a record
    // frozen for someone else, so only this comparison closes the swap.
    if intent.binding.task_id != intent.task_id {
        return writer.fail(StoreError::InvariantViolated(format!(
            "the frozen Binding names task {} but this Run commits for {}",
            intent.binding.task_id, intent.task_id
        )));
    }
    if intent.snapshot.goal_id != intent.goal_id {
        return writer.fail(StoreError::InvariantViolated(format!(
            "the frozen source snapshot names goal {} but this Run commits under {}",
            intent.snapshot.goal_id, intent.goal_id
        )));
    }

    // 3. Task currency: identity, Ready phase, expected revision, zero
    //    responsible Runs.
    let task: Option<TaskFacts> = writer.query_optional(
        "SELECT goal_id, phase, revision, active_run_id, spec_digest
         FROM tasks WHERE id = ?1",
        &[Value::from(intent.task_id)],
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
    let Some((goal_id, phase_text, task_revision, active_run_id, spec_digest)) = task else {
        return writer.fail(StoreError::TaskNotDispatchable {
            task_id: intent.task_id.to_string(),
            phase: "Absent",
            active_run_id: None,
        });
    };
    let phase = TaskPhase::parse(&phase_text).ok_or_else(|| {
        StoreError::InvariantViolated(format!(
            "task {} has unknown phase {phase_text}",
            intent.task_id
        ))
    })?;
    if goal_id != intent.goal_id {
        return writer.fail(StoreError::TaskNotDispatchable {
            task_id: intent.task_id.to_string(),
            phase: phase.as_str(),
            active_run_id,
        });
    }
    if phase != TaskPhase::Ready || active_run_id.is_some() {
        return writer.fail(StoreError::TaskNotDispatchable {
            task_id: intent.task_id.to_string(),
            phase: phase.as_str(),
            active_run_id,
        });
    }
    if Revision::new(task_revision) != intent.expected_task_revision {
        return writer.fail(StoreError::RevisionConflict {
            table: "tasks",
            id: intent.task_id.to_string(),
            expected: intent.expected_task_revision.get(),
            actual: Some(task_revision),
        });
    }
    if spec_digest.as_slice() != intent.snapshot.task_spec_digest.as_bytes().as_slice() {
        return writer.fail(StoreError::InvariantViolated(format!(
            "frozen source snapshot names task spec {:?} but task {} stores a different one",
            intent.snapshot.task_spec_digest, intent.task_id
        )));
    }

    // 4. Goal and graph currency.
    let goal: Option<(String, i64, i64)> = writer.query_optional(
        "SELECT phase, current_revision, revision FROM goals WHERE id = ?1",
        &[Value::from(intent.goal_id)],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let Some((goal_phase_text, goal_current_revision, goal_row_revision)) = goal else {
        return writer.fail(StoreError::InvariantViolated(format!(
            "task {} names goal {} which does not exist",
            intent.task_id, intent.goal_id
        )));
    };
    let goal_phase = GoalPhase::parse(&goal_phase_text).ok_or_else(|| {
        StoreError::InvariantViolated(format!(
            "goal {} has unknown phase {goal_phase_text}",
            intent.goal_id
        ))
    })?;
    if !matches!(
        goal_phase,
        GoalPhase::Planning | GoalPhase::Active | GoalPhase::Evaluating
    ) {
        return writer.fail(StoreError::GoalNotDispatchable {
            goal_id: intent.goal_id.to_string(),
            phase: goal_phase.as_str(),
        });
    }
    if Revision::new(goal_row_revision) != intent.expected_goal_row_revision {
        return writer.fail(StoreError::RevisionConflict {
            table: "goals",
            id: intent.goal_id.to_string(),
            expected: intent.expected_goal_row_revision.get(),
            actual: Some(goal_row_revision),
        });
    }
    if goal_current_revision != intent.expected_goal_current_revision
        || goal_current_revision != intent.snapshot.goal_revision
    {
        return writer.fail(StoreError::RevisionConflict {
            table: "goal_revisions",
            id: intent.goal_id.to_string(),
            expected: intent.snapshot.goal_revision,
            actual: Some(goal_current_revision),
        });
    }

    let graph_revision = writer
        .query_optional(
            "SELECT revision FROM task_graphs WHERE id = ?1",
            &[Value::from(intent.goal_id)],
            |row| row.get::<_, i64>(0),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated(format!("goal {} has no task graph", intent.goal_id))
        })?;
    if graph_revision != intent.expected_graph_revision
        || graph_revision != intent.snapshot.graph_revision
    {
        return writer.fail(StoreError::RevisionConflict {
            table: "task_graphs",
            id: intent.goal_id.to_string(),
            expected: intent.expected_graph_revision,
            actual: Some(graph_revision),
        });
    }

    // 5. Workspace currency: ownership, verified materialization and the exact
    //    base the source snapshot froze. Reused, never re-created per Run.
    let workspace: Option<(String, String, String, i64, String)> = writer.query_optional(
        "SELECT task_id, phase, materialization, revision, resolved_base
         FROM workspaces WHERE id = ?1",
        &[Value::from(intent.snapshot.workspace_id.as_str())],
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
    let Some((ws_task_id, ws_phase, ws_materialization, ws_revision, ws_base)) = workspace else {
        return writer.fail(StoreError::RevisionConflict {
            table: "workspaces",
            id: intent.snapshot.workspace_id.clone(),
            expected: intent.expected_workspace_revision.get(),
            actual: None,
        });
    };
    if ws_task_id != intent.task_id
        || ws_phase != "Ready"
        || ws_materialization != "Present"
        || Revision::new(ws_revision) != intent.expected_workspace_revision
    {
        return writer.fail(StoreError::RevisionConflict {
            table: "workspaces",
            id: intent.snapshot.workspace_id.clone(),
            expected: intent.expected_workspace_revision.get(),
            actual: Some(ws_revision),
        });
    }
    if ws_base != intent.snapshot.workspace_resolved_base {
        return writer.fail(StoreError::InvariantViolated(format!(
            "frozen source snapshot names workspace base {:?} but the Workspace resolved to {ws_base:?}",
            intent.snapshot.workspace_resolved_base
        )));
    }

    // 6. Capacity: the single slot must be free. The unique index below is
    //    the race-proof backstop for exactly this invariant.
    if let Some((run_id, task_id)) = writer.query_optional(
        "SELECT run_id, task_id FROM run_status WHERE active_slot IS NOT NULL LIMIT 1",
        &[],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )? {
        return writer.fail(StoreError::DispatchSlotUnavailable {
            held_by_run: run_id,
            held_for_task: task_id,
        });
    }

    // 7. Preconditions hold. Freeze immutable authority, then move the world.
    let now = now(writer)?;
    writer.execute(
        "INSERT INTO execution_bindings
             (digest, task_id, configuration_activation_sequence,
              configuration_content_digest, canonical_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (digest) DO NOTHING",
        &[
            Value::Blob(intent.binding_digest.as_bytes().to_vec()),
            Value::from(intent.binding.task_id.as_str()),
            Value::Integer(intent.binding.configuration_activation_sequence),
            Value::Blob(
                intent
                    .binding
                    .configuration_content_digest
                    .as_bytes()
                    .to_vec(),
            ),
            Value::from(canonical_json(&intent.binding.to_value())),
            Value::Integer(now),
        ],
    )?;
    writer.execute(
        "INSERT INTO context_source_snapshots
             (digest, configuration_activation_sequence, context_policy_digest,
              task_spec_digest, goal_id, goal_revision, graph_revision,
              agent_name, agent_version, workspace_id, workspace_resolved_base,
              canonical_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (digest) DO NOTHING",
        &[
            Value::Blob(intent.snapshot_digest.as_bytes().to_vec()),
            Value::Integer(intent.snapshot.configuration_activation_sequence),
            Value::Blob(intent.snapshot.context_policy_digest.as_bytes().to_vec()),
            Value::Blob(intent.snapshot.task_spec_digest.as_bytes().to_vec()),
            Value::from(intent.snapshot.goal_id.as_str()),
            Value::Integer(intent.snapshot.goal_revision),
            Value::Integer(intent.snapshot.graph_revision),
            Value::from(intent.snapshot.agent.name.as_str()),
            Value::Integer(i64::from(intent.snapshot.agent.version)),
            Value::from(intent.snapshot.workspace_id.as_str()),
            Value::from(intent.snapshot.workspace_resolved_base.as_str()),
            Value::from(canonical_json(&intent.snapshot.to_value())),
            Value::Integer(now),
        ],
    )?;
    writer.execute(
        "INSERT INTO runs
             (id, task_id, binding_digest, context_source_snapshot_digest, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            Value::from(intent.run_id),
            Value::from(intent.task_id),
            Value::Blob(intent.binding_digest.as_bytes().to_vec()),
            Value::Blob(intent.snapshot_digest.as_bytes().to_vec()),
            Value::Integer(now),
        ],
    )?;
    writer.execute(
        "INSERT INTO run_status
             (run_id, task_id, phase, terminal_target, revision, active_slot, updated_at)
         VALUES (?1, ?2, 'Active', NULL, 1, 'global', ?3)",
        &[
            Value::from(intent.run_id),
            Value::from(intent.task_id),
            Value::Integer(now),
        ],
    )?;

    // 8. Task activation. The row-local CHECK constraints prove the pointer
    //    is coherent with the new phase; this CAS proves only the holder of
    //    the observed revision could perform it.
    let task_new_revision = writer.update_revisioned(
        "tasks",
        intent.task_id,
        intent.expected_task_revision,
        &[
            ("phase", Value::from(TaskPhase::Active.as_str())),
            ("active_run_id", Value::from(intent.run_id)),
        ],
    )?;

    // 9. Fairness charge: the Goal is served exactly when the Run commits.
    let charged = state.next_service_sequence;
    match intent.expected_goal_fairness_revision {
        None => {
            let affected = writer.execute(
                "INSERT INTO goal_scheduling_state
                     (goal_id, last_served_sequence, revision, created_at, updated_at)
                 VALUES (?1, ?2, 1, ?3, ?3)",
                &[
                    Value::from(intent.goal_id),
                    Value::Integer(charged),
                    Value::Integer(now),
                ],
            )?;
            if affected != 1 {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "fairness insert for goal {} affected {affected} rows",
                    intent.goal_id
                )));
            }
        }
        Some(expected_fairness) => {
            let updated = writer.update_revisioned_by(
                GOAL_SCHEDULING_TABLE,
                "goal_id",
                intent.goal_id,
                expected_fairness,
                &[
                    ("last_served_sequence", Value::Integer(charged)),
                    ("updated_at", Value::Integer(now)),
                ],
            );
            if matches!(
                updated,
                Err(StoreError::RevisionConflict { actual: None, .. })
            ) {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "goal {} had a fairness row at observation time and none at commit",
                    intent.goal_id
                )));
            }
            let _ = updated?;
        }
    }
    let scheduler_new_revision = writer.update_revisioned(
        SCHEDULER_TABLE,
        "singleton",
        state.revision,
        &[
            ("next_service_sequence", Value::Integer(charged + 1)),
            ("updated_at", Value::Integer(now)),
        ],
    )?;

    // 10. Close out the Task's eligibility interval: leaving Ready ends it, so
    //     the next interval (if any) starts fresh, and temporary failure
    //     state is normalized away.
    let _normalized = writer.update_revisioned_by(
        TASK_SCHEDULING_TABLE,
        "task_id",
        intent.task_id,
        intent.expected_task_scheduling_revision,
        &[
            ("eligible_since", Value::Null),
            ("next_attempt_at", Value::Null),
            ("last_failure_code", Value::Null),
            ("last_failure_detail_json", Value::Null),
            ("updated_at", Value::Integer(now)),
        ],
    )?;

    Ok(RunIntentCommit {
        run_id: intent.run_id.to_string(),
        task_revision: task_new_revision,
        scheduler_revision: scheduler_new_revision,
        charged_service_sequence: charged,
    })
}

/// Sets the scheduler singleton's dispatch mode, returning the row after the
/// change. Used by both the command wrapper above and tests.
fn read_state(writer: &Writer<'_>) -> Result<SchedulerStateRecord, StoreError> {
    let row: Option<(String, i64, i64)> = writer.query_optional(
        "SELECT dispatch_mode, next_service_sequence, revision
         FROM scheduler_state WHERE id = 'singleton'",
        &[],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    row.map(parse_state).transpose()?.ok_or_else(|| {
        StoreError::InvariantViolated("scheduler_state has no singleton row".to_string())
    })
}

fn parse_state(row: (String, i64, i64)) -> Result<SchedulerStateRecord, StoreError> {
    let (mode, sequence, revision) = row;
    let dispatch_mode = DispatchMode::parse(&mode).ok_or_else(|| {
        StoreError::InvariantViolated(format!(
            "scheduler_state contains an unparsable dispatch mode {mode}"
        ))
    })?;
    Ok(SchedulerStateRecord {
        dispatch_mode,
        next_service_sequence: sequence,
        revision: Revision::new(revision),
    })
}

fn canonical_json(value: &pantheon_core::config::canonical::Value) -> String {
    String::from_utf8(value.to_canonical_bytes()).unwrap_or_default()
}

fn now(writer: &Writer<'_>) -> Result<i64, StoreError> {
    writer
        .query_optional("SELECT unixepoch()", &[], |row| row.get::<_, i64>(0))?
        .ok_or_else(|| StoreError::InvariantViolated("could not read the current time".to_string()))
}

/// Direct row readers for evidence tests. These are deliberately test-only:
/// a public read path nothing but tests call is exactly what
/// `scripts/check-store-read-paths.sh` exists to refuse.
#[cfg(test)]
pub(crate) struct TaskSchedulingRowForTest {
    pub eligible_since: Option<i64>,
    pub next_attempt_at: Option<i64>,
    pub last_failure_code: Option<String>,
}

#[cfg(test)]
impl TaskSchedulingRowForTest {
    /// Reads one Task's scheduling row through the store's read connection.
    pub(crate) fn read(store: &Store, task_id: &str) -> Option<Self> {
        store.task_scheduling_row_for_test(task_id)
    }
}

#[cfg(test)]
impl Store {
    pub(crate) fn run_rows_for_test(&self) -> i64 {
        self.read(|conn| count(conn, "runs")).expect("count runs")
    }

    pub(crate) fn binding_rows_for_test(&self) -> i64 {
        self.read(|conn| count(conn, "execution_bindings"))
            .expect("count bindings")
    }

    pub(crate) fn snapshot_rows_for_test(&self) -> i64 {
        self.read(|conn| count(conn, "context_source_snapshots"))
            .expect("count snapshots")
    }

    pub(crate) fn task_scheduling_row_for_test(
        &self,
        task_id: &str,
    ) -> Option<TaskSchedulingRowForTest> {
        self.read(|conn| {
            conn.query_row(
                "SELECT eligible_since, next_attempt_at, last_failure_code
                 FROM task_scheduling_state WHERE task_id = ?1",
                rusqlite::params![task_id],
                |row| {
                    Ok(TaskSchedulingRowForTest {
                        eligible_since: row.get(0)?,
                        next_attempt_at: row.get(1)?,
                        last_failure_code: row.get(2)?,
                    })
                },
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Sqlite(other)),
            })
        })
        .expect("read scheduling row")
    }
}

#[cfg(test)]
fn count(conn: &rusqlite::Connection, table: &str) -> Result<i64, StoreError> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get(0))
        .map_err(StoreError::Sqlite)
}

#[cfg(test)]
pub(crate) mod tests;
