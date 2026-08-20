//! Reading durable Goal, graph, Task and planning state.

use pantheon_core::config::Digest;
use pantheon_core::planning::GoalPhase;

use crate::error::StoreError;
use crate::planning::{
    GoalRecord, GraphRecord, PlanningOperationRecord, PlanningState, digest_from,
};
use crate::store::Store;
use crate::transaction::Revision;

impl Store {
    /// The current Goal row.
    pub fn goal(&self, goal_id: &str) -> Result<Option<GoalRecord>, StoreError> {
        self.read(|conn| {
            optional(conn.query_row(
                "SELECT phase, current_revision, revision FROM goals WHERE id = ?1",
                rusqlite::params![goal_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            ))?
            .map(|(phase, current_revision, revision)| {
                let phase = GoalPhase::parse(&phase).ok_or_else(|| {
                    StoreError::InvariantViolated(format!("goal {goal_id} has unknown phase"))
                })?;
                Ok(GoalRecord {
                    id: goal_id.to_string(),
                    phase,
                    current_revision,
                    revision: Revision::new(revision),
                })
            })
            .transpose()
        })
    }

    /// The immutable content of one Goal revision, as canonical JSON.
    pub fn goal_revision_json(
        &self,
        goal_id: &str,
        revision: i64,
    ) -> Result<Option<String>, StoreError> {
        self.read(|conn| {
            optional(conn.query_row(
                "SELECT canonical_json FROM goal_revisions WHERE goal_id = ?1 AND revision = ?2",
                rusqlite::params![goal_id, revision],
                |row| row.get::<_, String>(0),
            ))
        })
    }

    /// The Goal-owned TaskGraph.
    pub fn task_graph(&self, goal_id: &str) -> Result<Option<GraphRecord>, StoreError> {
        self.read(|conn| {
            Ok(optional(conn.query_row(
                "SELECT revision FROM task_graphs WHERE id = ?1",
                rusqlite::params![goal_id],
                |row| row.get::<_, i64>(0),
            ))?
            .map(|revision| GraphRecord {
                goal_id: goal_id.to_string(),
                revision: Revision::new(revision),
            }))
        })
    }

    /// Reads one current Task row by its durable identity.
    pub fn task(&self, task_id: &str) -> Result<Option<TaskRecord>, StoreError> {
        self.read(|conn| {
            optional(conn.query_row(
                "SELECT goal_id, phase, created_graph_revision, spec_digest, revision, active_run_id
                 FROM tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            ))?
            .map(|(goal_id, phase, created_graph_revision, spec_digest, revision, active_run_id)| {
                let phase = TaskPhase::parse(&phase).ok_or_else(|| {
                    StoreError::InvariantViolated(format!("task {task_id} has unknown phase"))
                })?;
                Ok(TaskRecord {
                    id: task_id.to_string(),
                    goal_id,
                    phase,
                    created_graph_revision,
                    spec_digest: digest_from(&spec_digest, "spec_digest")?,
                    revision: Revision::new(revision),
                    active_run_id,
                })
            })
            .transpose()
        })
    }

    /// The immutable Task specification, as canonical JSON.
    pub fn task_spec_json(&self, digest: Digest) -> Result<Option<String>, StoreError> {
        self.read(|conn| {
            optional(conn.query_row(
                "SELECT canonical_json FROM task_specs WHERE digest = ?1",
                rusqlite::params![digest.as_bytes().to_vec()],
                |row| row.get::<_, String>(0),
            ))
        })
    }

    /// One durable planning decision.
    pub fn planning_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<PlanningOperationRecord>, StoreError> {
        self.read(|conn| {
            optional(conn.query_row(
                "SELECT goal_id, goal_revision, expected_graph_revision,
                        configuration_activation_sequence, planning_input_digest, state, revision
                 FROM planning_operations WHERE id = ?1",
                rusqlite::params![operation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            ))?
            .map(
                |(goal_id, goal_revision, expected, config, digest, state, revision)| {
                    let state = PlanningState::parse(&state).ok_or_else(|| {
                        StoreError::InvariantViolated(format!(
                            "planning operation {operation_id} has unknown state"
                        ))
                    })?;
                    Ok(PlanningOperationRecord {
                        id: operation_id.to_string(),
                        goal_id,
                        goal_revision,
                        expected_graph_revision: expected,
                        configuration_activation_sequence: config,
                        planning_input_digest: digest_from(&digest, "planning_input_digest")?,
                        state,
                        revision: Revision::new(revision),
                    })
                },
            )
            .transpose()
        })
    }

    /// The immutable proposal recorded for a planning operation.
    pub fn planning_record_proposal(
        &self,
        operation_id: &str,
    ) -> Result<Option<(Digest, String)>, StoreError> {
        self.read(|conn| {
            optional(conn.query_row(
                "SELECT proposal_digest, canonical_proposal FROM planning_records
                 WHERE planning_operation_id = ?1",
                rusqlite::params![operation_id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
            ))?
            .map(|(digest, canonical)| Ok((digest_from(&digest, "proposal_digest")?, canonical)))
            .transpose()
        })
    }

    /// The evaluators component of the active ConfigurationRevision, as
    /// canonical JSON.
    ///
    /// This is the text Task materialization resolves evaluator refs from, so
    /// the pin and the stored component are the same bytes rather than two
    /// copies that could drift.
    pub fn active_evaluator_component_json(&self) -> Result<Option<String>, StoreError> {
        self.read(|conn| {
            optional(conn.query_row(
                "SELECT component.canonical_json
                 FROM active_configuration active
                 JOIN configuration_revisions revision
                   ON revision.activation_sequence = active.activation_sequence
                 JOIN configuration_components component
                   ON component.digest = revision.evaluator_registry_digest
                 WHERE active.id = 'singleton'",
                [],
                |row| row.get::<_, String>(0),
            ))
        })
    }
}

fn optional<T>(result: rusqlite::Result<T>) -> Result<Option<T>, StoreError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(StoreError::Sqlite(err)),
    }
}
