//! Goal reads and the two Goal mutations Operator Control exposes.
//!
//! # Why creating a Goal is three commands
//!
//! Issue #26's end state is `create Goal -> DIRECT planning -> one Ready
//! Task`. Those are three authoritative transactions in the #18 kernel —
//! creating the Goal, recording the planning decision, and materializing the
//! plan — and collapsing them into one would mean inventing an alternate
//! write path, which the mission forbids.
//!
//! What makes that safe under retry is that all three command identities and
//! both resource identities are *derived* from the operator's own
//! `(commandEpoch, commandId)`. A retry re-derives the same identities, so
//! each step that already committed replays and each step that did not runs
//! once. A crash between steps leaves a Goal that the same request completes
//! rather than a Goal nobody can reach.

use std::borrow::Borrow;

use pantheon_core::planning::goal::GoalSpec;
use pantheon_core::planning::{GoalPhase, TaskPhase};
use pantheon_store::{Cursor, GoalRecord, Store, TaskRecord};

use crate::operator::{
    CommandIdentity, OperatorError, OperatorService, derive_command_id, derive_goal_id,
};
use crate::planning::{PlanningController, PlanningError};

/// A Goal as an operator sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalView {
    pub id: String,
    pub phase: GoalPhase,
    /// The semantic GoalRevision the Goal is currently pursuing.
    pub goal_revision: i64,
    /// The authoritative row revision. This is the concurrency token an ETag
    /// is derived from — it advances on *every* authoritative mutation,
    /// including a lifecycle transition that leaves `goal_revision` alone.
    pub revision: i64,
    pub spec: GoalSpec,
    /// The Goal's Tasks. Embedded because #26 exposes no Task resource, and
    /// an operator that cannot see the Ready Task cannot see the outcome the
    /// mission is about.
    pub tasks: Vec<TaskView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskView {
    pub id: String,
    pub phase: TaskPhase,
    pub created_graph_revision: i64,
    /// The immutable specification identity. Not the specification itself:
    /// #26 exposes no Task resource, and a digest is enough to tell two
    /// Tasks' contracts apart.
    pub spec_digest: String,
}

/// A Goal in a list, without its full specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSummary {
    pub id: String,
    pub phase: GoalPhase,
    pub goal_revision: i64,
    pub revision: i64,
}

/// A Goal list, with the journal position it corresponds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalsPage {
    pub goals: Vec<GoalSummary>,
    /// Read from the same durable snapshot as `goals`. A client that starts
    /// watching strictly after this cursor cannot miss an Event that changed
    /// what the list showed.
    pub snapshot_cursor: Cursor,
}

impl<S: Borrow<Store>> OperatorService<'_, S> {
    /// Creates a Goal, plans it, and materializes its one Ready Task.
    ///
    /// # Errors
    ///
    /// [`OperatorError::NotReady`] when there is no usable active
    /// configuration to plan against, [`OperatorError::Invalid`] when the
    /// proposal cannot become authority, or the mapped store failure.
    pub fn create_goal(
        &self,
        command: &CommandIdentity,
        spec: &GoalSpec,
    ) -> Result<GoalView, OperatorError> {
        let goal_id = derive_goal_id(&command.epoch, &command.id);
        let operation_id = derive_command_id(&command.epoch, &command.id, "plan");
        let task_id = derive_command_id(&command.epoch, &command.id, "task");
        let planning = PlanningController::new(self.store);

        let create = command.step("goal-create");
        planning.create_goal(&create.command("goal.created"), &goal_id, spec)?;

        let record = command.step("goal-plan");
        planning.plan(
            &record.command("planning.recorded"),
            &operation_id,
            &goal_id,
        )?;

        // Re-derive the proposal and check it against the one the planning
        // transaction durably recorded. The recorded digest is authority
        // provenance, and provenance nothing ever reads is not a fence at
        // all: without this, a change that broke DIRECT determinism would
        // materialize a Task from a proposal no PlanningRecord describes.
        let proposal = planning.proposal(&goal_id)?;
        let (recorded, _) = self
            .store
            .planning_record_proposal(&operation_id)?
            .ok_or_else(|| {
                OperatorError::Internal(format!(
                    "planning operation {operation_id} recorded no proposal"
                ))
            })?;
        if recorded != proposal.digest() {
            return Err(OperatorError::Internal(format!(
                "the proposal for {goal_id} no longer reproduces its recorded digest {recorded}"
            )));
        }

        let materialize = command.step("goal-materialize");
        planning.materialize(
            &materialize.command("task.materialized"),
            &operation_id,
            &task_id,
            &goal_id,
            &proposal,
        )?;

        self.goal(&goal_id)
    }

    /// One Goal, with its Tasks.
    ///
    /// # Errors
    ///
    /// [`OperatorError::NotFound`] when no such Goal exists.
    pub fn goal(&self, goal_id: &str) -> Result<GoalView, OperatorError> {
        let record = self
            .store
            .goal(goal_id)?
            .ok_or_else(|| OperatorError::NotFound {
                resource: "goal",
                id: goal_id.to_string(),
            })?;
        self.view(record)
    }

    /// Every Goal, with the journal cursor the list corresponds to.
    ///
    /// # Errors
    ///
    /// [`OperatorError::Internal`] when durable state cannot be read.
    pub fn goals(&self) -> Result<GoalsPage, OperatorError> {
        let snapshot = self.store.goal_snapshot()?;
        Ok(GoalsPage {
            goals: snapshot.goals.into_iter().map(summary).collect(),
            snapshot_cursor: snapshot.cursor,
        })
    }

    /// Drives a Goal toward `Cancelled`.
    ///
    /// The result is the Goal in `Finalizing` with terminal target
    /// `Cancelled`, never a `Cancelled` Goal: reaching terminal requires the
    /// Goal Completion Controller to confirm obligations are finalized, and
    /// that controller does not exist yet.
    ///
    /// # Errors
    ///
    /// [`OperatorError::NotFound`] when no such Goal exists, or
    /// [`OperatorError::Conflict`] when the Goal is terminal or already
    /// finalizing toward another outcome.
    pub fn cancel_goal(
        &self,
        command: &CommandIdentity,
        goal_id: &str,
    ) -> Result<GoalView, OperatorError> {
        // Read first so a cancel against an unknown Goal is a 404 rather than
        // a durable-invariant failure. Goals are never deleted, so nothing
        // can remove the row between this read and the transaction.
        if self.store.goal(goal_id)?.is_none() {
            return Err(OperatorError::NotFound {
                resource: "goal",
                id: goal_id.to_string(),
            });
        }
        self.store
            .cancel_goal(&command.command("goal.cancel.requested"), goal_id)?;
        self.goal(goal_id)
    }

    fn view(&self, record: GoalRecord) -> Result<GoalView, OperatorError> {
        let canonical = self
            .store
            .goal_revision_json(&record.id, record.current_revision)?
            .ok_or_else(|| {
                OperatorError::Internal(format!(
                    "goal {} revision {} is not stored",
                    record.id, record.current_revision
                ))
            })?;
        let spec = GoalSpec::from_canonical_json(&canonical)
            .map_err(|err| OperatorError::Internal(err.to_string()))?;
        let tasks = self
            .store
            .tasks_for_goal(&record.id)?
            .into_iter()
            .map(task_view)
            .collect();
        Ok(GoalView {
            id: record.id,
            phase: record.phase,
            goal_revision: record.current_revision,
            revision: record.revision.get(),
            spec,
            tasks,
        })
    }
}

fn summary(record: GoalRecord) -> GoalSummary {
    GoalSummary {
        id: record.id,
        phase: record.phase,
        goal_revision: record.current_revision,
        revision: record.revision.get(),
    }
}

fn task_view(record: TaskRecord) -> TaskView {
    TaskView {
        id: record.id,
        phase: record.phase,
        created_graph_revision: record.created_graph_revision,
        spec_digest: record.spec_digest.to_hex(),
    }
}

impl From<PlanningError> for OperatorError {
    fn from(err: PlanningError) -> Self {
        match err {
            PlanningError::Store(store) => store.into(),
            PlanningError::Invalid(detail) => Self::Invalid(detail.to_string()),
            // Planning cannot proceed without usable configuration. That is a
            // readiness fact about the daemon, not a defect in the request,
            // so it must not be reported as the caller's fault.
            PlanningError::Configuration(detail) => Self::NotReady(detail),
        }
    }
}
