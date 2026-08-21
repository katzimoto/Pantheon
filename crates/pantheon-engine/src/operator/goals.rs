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
use pantheon_store::{Cursor, GoalDetail, GoalRecord, Store, TaskRecord};

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

        // Re-derive the proposal the planning transaction recorded. DIRECT
        // planning is deterministic, so the same authoritative state yields
        // the same proposal — but this call does not *rely* on that. The
        // store compares the plan's proposal digest against the recorded
        // PlanningRecord inside the materializing transaction, so a break in
        // determinism is refused there rather than trusted here. Checking it
        // again in this layer would be a second, weaker copy of a fence that
        // already exists in the only place it can be authoritative.
        let proposal = planning.proposal(&goal_id)?;

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
    /// The phase, the revision the ETag is derived from, and the Tasks all
    /// come from one durable read, so the representation a client caches
    /// under that validator is a state that actually existed.
    ///
    /// # Errors
    ///
    /// [`OperatorError::NotFound`] when no such Goal exists.
    pub fn goal(&self, goal_id: &str) -> Result<GoalView, OperatorError> {
        let detail = self
            .store
            .goal_detail(goal_id)?
            .ok_or_else(|| OperatorError::NotFound {
                resource: "goal",
                id: goal_id.to_string(),
            })?;
        self.view(detail)
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
    /// that controller does not exist yet. A Goal that was already finalizing
    /// toward `Succeeded` or `Failed` is retargeted in place, per the Goal
    /// lifecycle contract; one already targeting `Cancelled` changes nothing.
    ///
    /// # Errors
    ///
    /// [`OperatorError::NotFound`] when no such Goal exists, or
    /// [`OperatorError::Conflict`] when the Goal is terminal — terminal
    /// history never reopens.
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

    fn view(&self, detail: GoalDetail) -> Result<GoalView, OperatorError> {
        let spec = GoalSpec::from_canonical_json(&detail.revision_json)
            .map_err(|err| OperatorError::Internal(err.to_string()))?;
        Ok(GoalView {
            id: detail.goal.id,
            phase: detail.goal.phase,
            goal_revision: detail.goal.current_revision,
            revision: detail.goal.revision.get(),
            spec,
            tasks: detail.tasks.into_iter().map(task_view).collect(),
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
