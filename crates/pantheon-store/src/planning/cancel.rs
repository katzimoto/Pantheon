//! Goal cancellation as a desired-state transition.
//!
//! `docs/architecture/goals/goal-lifecycle-and-completion-controller.md`
//! ("Cancellation") makes cancellation a *target*, not an outcome:
//!
//! ```text
//! Planning/Active/Evaluating -> Finalizing / terminalTarget=Cancelled
//! ```
//!
//! and "Goal becomes Cancelled only when those obligations are safely
//! finalized". This module commits exactly that transition and the fence it
//! carries; it never terminalizes a Goal, because the Goal Completion
//! Controller that decides obligations are finalized does not exist yet and
//! inventing an MVP-only substitute is what the mission forbids.
//!
//! The same rule governs the Goal's Tasks. `docs/architecture/tasks/task-lifecycle.md`
//! ("Cancellation", invariant 9) says cancellation "use[s] Finalizing +
//! terminalTarget and never leave[s] a terminal Task with a live Run", so
//! every nonterminal Task is driven to the same target under the *same*
//! transaction and the same command. Propagating the fence in a second
//! transaction would leave a window in which the Goal is fenced and its Tasks
//! are still schedulable.

use pantheon_core::planning::{GoalPhase, TaskPhase};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::planning::GoalRecord;
use crate::store::Store;
use crate::transaction::{Revision, Value};

/// What one accepted cancellation committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancellation {
    /// The Goal as it now stands: `Finalizing`, targeting `Cancelled`.
    pub goal: GoalRecord,
    /// How many nonterminal Tasks were driven to the same target.
    pub tasks_fenced: usize,
    /// Whether the Goal was already targeting `Cancelled` when this command
    /// arrived, so this call changed no Goal phase.
    pub already_targeted: bool,
}

impl Store {
    /// Drives a Goal, and every nonterminal Task it owns, toward `Cancelled`.
    ///
    /// # Errors
    ///
    /// [`StoreError::GoalNotCancellable`] when the Goal is terminal or is
    /// already finalizing toward a different outcome — nothing is written and
    /// the command consumes no durable identity. Otherwise any storage
    /// failure, or a [`StoreError::RevisionConflict`] if another writer moved
    /// the Goal first.
    pub fn cancel_goal(
        &self,
        command: &Command<'_>,
        goal_id: &str,
    ) -> Result<Committed<Cancellation>, StoreError> {
        self.execute_command(command, |writer| {
            let (phase, current_revision, revision, terminal_target) = writer
                .query_optional(
                    "SELECT phase, current_revision, revision, terminal_target
                     FROM goals WHERE id = ?1",
                    &[Value::from(goal_id)],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )?
                .ok_or_else(|| {
                    // Goals are never deleted, so a caller that read the Goal
                    // and then cancelled it cannot lose the row underneath.
                    StoreError::InvariantViolated(format!("goal {goal_id} does not exist"))
                })?;

            let phase = GoalPhase::parse(&phase).ok_or_else(|| {
                StoreError::InvariantViolated(format!("goal {goal_id} has unknown phase"))
            })?;
            let target = terminal_target.as_deref();

            let already_targeted = match (phase, target) {
                // The ordinary case: fence a live Goal.
                (GoalPhase::Planning | GoalPhase::Active | GoalPhase::Evaluating, _) => false,
                // Cancelling an already-cancelling Goal is the retry the
                // contract calls state-independent: the target is already
                // what the caller asked for.
                (GoalPhase::Finalizing, Some("Cancelled")) => true,
                // Terminal Goals never reopen, and retargeting a Goal already
                // finalizing toward Succeeded or Failed is a transition the
                // Goal contract's table does not grant.
                _ => {
                    return writer.fail(StoreError::GoalNotCancellable {
                        goal_id: goal_id.to_string(),
                        phase: phase.as_str(),
                        terminal_target: terminal_target.clone(),
                    });
                }
            };

            let goal_revision = if already_targeted {
                Revision::new(revision)
            } else {
                writer.update_revisioned(
                    "goals",
                    goal_id,
                    Revision::new(revision),
                    &[
                        ("phase", Value::from(GoalPhase::Finalizing.as_str())),
                        (
                            "terminal_target",
                            Value::from(GoalPhase::Cancelled.as_str()),
                        ),
                    ],
                )?
            };

            let tasks_fenced = fence_tasks(writer, goal_id)?;

            Ok(Cancellation {
                goal: GoalRecord {
                    id: goal_id.to_string(),
                    phase: GoalPhase::Finalizing,
                    current_revision,
                    revision: goal_revision,
                },
                tasks_fenced,
                already_targeted,
            })
        })
    }
}

/// Drives every nonterminal Task of `goal_id` to `Finalizing` targeting
/// `Cancelled`, leaving Tasks already on that target untouched.
///
/// Each Task moves through its own revisioned CAS on the revision read in
/// this same transaction, so the fence is expressed the way every other
/// lifecycle write is rather than as a bulk `UPDATE ... WHERE phase IN (...)`
/// that would advance revisions without a compare.
fn fence_tasks(
    writer: &crate::transaction::Writer<'_>,
    goal_id: &str,
) -> Result<usize, StoreError> {
    let rows = writer.query_all(
        "SELECT id, phase, revision, terminal_target
         FROM tasks WHERE goal_id = ?1 ORDER BY id",
        &[Value::from(goal_id)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;

    let mut fenced = 0;
    for (task_id, phase, revision, terminal_target) in rows {
        let phase = TaskPhase::parse(&phase).ok_or_else(|| {
            StoreError::InvariantViolated(format!("task {task_id} has unknown phase"))
        })?;
        if phase.is_terminal() || terminal_target.as_deref() == Some("Cancelled") {
            continue;
        }
        // Invariant 8: cancellation never terminalizes a Task while a
        // responsible Run is still nonterminal. Setting the *target* is
        // always safe — it is the terminal phase that must wait — so a Task
        // with a live Run is fenced here regardless, and terminalized by
        // whatever later owns Run finalization. The Task's `active_run_id` is
        // therefore deliberately not consulted.
        let fenced_at = writer.update_revisioned(
            "tasks",
            &task_id,
            Revision::new(revision),
            &[
                ("phase", Value::from(TaskPhase::Finalizing.as_str())),
                (
                    "terminal_target",
                    Value::from(TaskPhase::Cancelled.as_str()),
                ),
                (
                    "terminal_reason_json",
                    Value::from(r#"{"code":"goal-cancelled"}"#),
                ),
            ],
        )?;
        if fenced_at.get() != revision + 1 {
            return writer.fail(StoreError::InvariantViolated(format!(
                "fencing task {task_id} left it at revision {}, not {}",
                fenced_at.get(),
                revision + 1
            )));
        }
        fenced += 1;
    }
    Ok(fenced)
}

#[cfg(test)]
mod tests;
