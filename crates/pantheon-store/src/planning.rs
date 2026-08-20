//! Durable Goal, planning and TaskGraph authority.
//!
//! Three authoritative mutations, each one call to
//! [`Store::execute_command`], so each inherits the single authoritative
//! transaction, the durable command outcome and the Event append from Issue
//! #18 rather than opening another write path:
//!
//! 1. [`Store::create_goal`] — the Goal, its first immutable revision, and
//!    its empty graph at revision 0.
//! 2. [`Store::record_direct_planning`] — the durable PlanningOperation and
//!    its immutable normalized PlanningRecord.
//! 3. [`Store::materialize_plan`] — the graph patch that creates the Task.
//!
//! # Why materialization re-reads
//!
//! `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`:
//!
//! > A PlanningRecord is not lifecycle authority and is never proof that its
//! > proposal was materialized. Graph Controller separately rechecks
//! > GoalRevision/GraphRevision/current policy before GraphPatch commit.
//!
//! So [`Store::materialize_plan`] loads the Goal revision, the graph revision
//! and the active configuration *inside* the write transaction and compares
//! them against what the PlanningOperation froze. Nothing is trusted from the
//! caller's earlier read, and a proposal whose preconditions have moved is
//! refused with a typed conflict rather than applied to current state.

use pantheon_core::config::Digest;
use pantheon_core::planning::{GoalPhase, GoalSpec, TaskPhase};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

/// The mutable Goal row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRecord {
    pub id: String,
    pub phase: GoalPhase,
    /// The revision pointer naming the current immutable GoalRevision.
    pub current_revision: i64,
    /// The row revision, for lifecycle CAS.
    pub revision: Revision,
}

/// The Goal-owned TaskGraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRecord {
    pub goal_id: String,
    /// The graph revision. A patch CASes this.
    pub revision: Revision,
}

/// A materialized Task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: String,
    pub goal_id: String,
    pub phase: TaskPhase,
    pub created_graph_revision: i64,
    pub spec_digest: Digest,
    pub revision: Revision,
    /// Always `None` in this mission; present because the canonical phase
    /// invariants are expressed in terms of it.
    pub active_run_id: Option<String>,
}

/// The durable planning decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanningOperationRecord {
    pub id: String,
    pub goal_id: String,
    pub goal_revision: i64,
    pub expected_graph_revision: i64,
    pub configuration_activation_sequence: i64,
    pub planning_input_digest: Digest,
    pub state: PlanningState,
    pub revision: Revision,
}

/// Where a planning decision stands.
///
/// The persistence contract names a `state` column but does not enumerate its
/// domain; this is the minimum this mission's behaviour needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanningState {
    /// A proposal exists and has not been materialized.
    Planned,
    /// Its proposal became authoritative graph state.
    Materialized,
    /// It was refused and can never materialize.
    Rejected,
}

impl PlanningState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Materialized => "Materialized",
            Self::Rejected => "Rejected",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "Planned" => Self::Planned,
            "Materialized" => Self::Materialized,
            "Rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

/// What a DIRECT planning decision froze.
#[derive(Debug, Clone, Copy)]
pub struct PlanningDecision<'a> {
    pub operation_id: &'a str,
    pub goal_id: &'a str,
    pub goal_revision: i64,
    pub expected_graph_revision: i64,
    pub configuration_activation_sequence: i64,
    pub planning_input_digest: Digest,
    pub trigger_kind: &'a str,
    pub planner_implementation: &'a str,
    pub planner_version: &'a str,
}

/// The immutable normalized proposal produced by that decision.
#[derive(Debug, Clone, Copy)]
pub struct ProposalRecord<'a> {
    pub digest: Digest,
    pub canonical: &'a str,
    pub normalization_provenance: &'a str,
}

impl Store {
    /// Creates a Goal, its first immutable revision, and its empty graph.
    ///
    /// The Goal enters `Planning`: it exists but has no coherent TaskGraph
    /// yet. The graph row is created here at revision 0 so the first patch is
    /// an ordinary CAS rather than a create-if-absent special case.
    ///
    /// # Errors
    ///
    /// [`StoreError`] from the command envelope, or a storage failure. On any
    /// failure nothing is created.
    pub fn create_goal(
        &self,
        command: &Command<'_>,
        goal_id: &str,
        spec: &GoalSpec,
    ) -> Result<Committed<GoalRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let now = now(writer)?;
            let canonical = String::from_utf8(spec.to_value().to_canonical_bytes())
                .unwrap_or_default();

            writer.execute(
                "INSERT INTO goals (id, phase, current_revision, revision, terminal_target, created_at)
                 VALUES (?1, ?2, 1, 1, NULL, ?3)",
                &[
                    Value::from(goal_id),
                    Value::from(GoalPhase::Planning.as_str()),
                    Value::Integer(now),
                ],
            )?;
            writer.execute(
                "INSERT INTO goal_revisions (goal_id, revision, content_digest, canonical_json, created_at)
                 VALUES (?1, 1, ?2, ?3, ?4)",
                &[
                    Value::from(goal_id),
                    Value::Blob(spec.digest().as_bytes().to_vec()),
                    Value::from(canonical),
                    Value::Integer(now),
                ],
            )?;
            writer.execute(
                "INSERT INTO task_graphs (id, revision) VALUES (?1, 0)",
                &[Value::from(goal_id)],
            )?;

            Ok(GoalRecord {
                id: goal_id.to_string(),
                phase: GoalPhase::Planning,
                current_revision: 1,
                revision: Revision::new(1),
            })
        })
    }

    /// Records one DIRECT planning decision and its immutable proposal.
    ///
    /// This commits *evidence*. It grants no authority to mutate the graph;
    /// [`Store::materialize_plan`] re-establishes every precondition before
    /// anything becomes authoritative.
    ///
    /// # Errors
    ///
    /// [`StoreError`] from the command envelope, or a storage failure.
    pub fn record_direct_planning(
        &self,
        command: &Command<'_>,
        decision: &PlanningDecision<'_>,
        proposal: &ProposalRecord<'_>,
    ) -> Result<Committed<PlanningOperationRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let now = now(writer)?;
            writer.execute(
                "INSERT INTO planning_operations (
                     id, goal_id, goal_revision, expected_graph_revision, trigger_kind,
                     planning_input_digest, planner_implementation, planner_version,
                     configuration_activation_sequence, state, revision, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11)",
                &[
                    Value::from(decision.operation_id),
                    Value::from(decision.goal_id),
                    Value::Integer(decision.goal_revision),
                    Value::Integer(decision.expected_graph_revision),
                    Value::from(decision.trigger_kind),
                    Value::Blob(decision.planning_input_digest.as_bytes().to_vec()),
                    Value::from(decision.planner_implementation),
                    Value::from(decision.planner_version),
                    Value::Integer(decision.configuration_activation_sequence),
                    Value::from(PlanningState::Planned.as_str()),
                    Value::Integer(now),
                ],
            )?;
            writer.execute(
                "INSERT INTO planning_records (
                     planning_operation_id, proposal_digest, canonical_proposal,
                     normalization_provenance, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                &[
                    Value::from(decision.operation_id),
                    Value::Blob(proposal.digest.as_bytes().to_vec()),
                    Value::from(proposal.canonical),
                    Value::from(proposal.normalization_provenance),
                    Value::Integer(now),
                ],
            )?;

            Ok(PlanningOperationRecord {
                id: decision.operation_id.to_string(),
                goal_id: decision.goal_id.to_string(),
                goal_revision: decision.goal_revision,
                expected_graph_revision: decision.expected_graph_revision,
                configuration_activation_sequence: decision.configuration_activation_sequence,
                planning_input_digest: decision.planning_input_digest,
                state: PlanningState::Planned,
                revision: Revision::new(1),
            })
        })
    }
}

fn now(writer: &Writer<'_>) -> Result<i64, StoreError> {
    writer
        .query_optional("SELECT unixepoch()", &[], |row| row.get::<_, i64>(0))?
        .ok_or_else(|| StoreError::InvariantViolated("could not read the current time".to_string()))
}

pub(crate) fn digest_from(bytes: &[u8], column: &str) -> Result<Digest, StoreError> {
    let array: [u8; 32] = bytes.try_into().map_err(|_| {
        StoreError::InvariantViolated(format!(
            "{column} is {} bytes, not a 32-byte digest",
            bytes.len()
        ))
    })?;
    Ok(Digest::from_bytes(array))
}

pub use materialize::MaterializedPlan;

pub(crate) mod materialize;
mod read;

#[cfg(test)]
mod fencing_tests;
#[cfg(test)]
pub(crate) mod tests;
