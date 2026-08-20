//! The graph patch that turns a validated proposal into authoritative state.

use pantheon_core::planning::validate::Materializable;
use pantheon_core::planning::{GoalPhase, TaskPhase};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::planning::{PlanningState, TaskRecord};
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

/// What one successful materialization established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedPlan {
    pub task: TaskRecord,
    /// The graph revision this patch produced.
    pub graph_revision: i64,
    /// The Goal phase after the patch.
    pub goal_phase: GoalPhase,
}

impl Store {
    /// Materializes a validated plan into the Goal's TaskGraph.
    ///
    /// Every precondition the PlanningOperation froze is re-read inside this
    /// transaction and compared against current authoritative state: the Goal
    /// revision, the graph revision and the active ConfigurationRevision. A
    /// PlanningOperation whose world has moved is refused; it never mutates
    /// current state, and its PlanningRecord survives as historical
    /// provenance.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionConflict`] when the Goal revision, graph
    /// revision or active configuration has moved since planning, or when the
    /// operation has already been materialized;
    /// [`StoreError::InvariantViolated`] when durable state is not
    /// interpretable; plus the command envelope's stale-epoch and conflict
    /// failures. In every case the graph is unchanged.
    pub fn materialize_plan(
        &self,
        command: &Command<'_>,
        operation_id: &str,
        task_id: &str,
        plan: &Materializable,
    ) -> Result<Committed<MaterializedPlan>, StoreError> {
        self.execute_command(command, |writer| apply(writer, operation_id, task_id, plan))
    }
}

pub(crate) fn apply(
    writer: &Writer<'_>,
    operation_id: &str,
    task_id: &str,
    plan: &Materializable,
) -> Result<MaterializedPlan, StoreError> {
    // 1. The frozen decision.
    let (goal_id, frozen_goal_revision, expected_graph_revision, frozen_config, state, op_revision) =
        writer
            .query_optional(
                "SELECT goal_id, goal_revision, expected_graph_revision,
                        configuration_activation_sequence, state, revision
                 FROM planning_operations WHERE id = ?1",
                &[Value::from(operation_id)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )?
            .ok_or_else(|| {
                StoreError::InvariantViolated(format!(
                    "planning operation {operation_id} does not exist"
                ))
            })?;

    // An operation materializes at most once. Without this, a replayed
    // materialization under a *different* command identity would patch the
    // graph twice.
    let state = PlanningState::parse(&state).ok_or_else(|| {
        StoreError::InvariantViolated(format!(
            "planning operation {operation_id} has unknown state"
        ))
    })?;
    if state != PlanningState::Planned {
        return writer.fail(StoreError::RevisionConflict {
            table: "planning_operations",
            id: operation_id.to_string(),
            expected: op_revision,
            actual: Some(op_revision),
        });
    }

    // 2. Re-read the Goal, inside this transaction.
    let (goal_phase, current_goal_revision, goal_row_revision) = writer
        .query_optional(
            "SELECT phase, current_revision, revision FROM goals WHERE id = ?1",
            &[Value::from(goal_id.as_str())],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?
        .ok_or_else(|| StoreError::InvariantViolated(format!("goal {goal_id} does not exist")))?;

    // The Goal revision the planner observed must still be current. A newer
    // revision means the plan describes a Goal that no longer exists.
    if current_goal_revision != frozen_goal_revision {
        return writer.fail(StoreError::RevisionConflict {
            table: "goal_revisions",
            id: goal_id,
            expected: frozen_goal_revision,
            actual: Some(current_goal_revision),
        });
    }
    // And the plan itself must have been validated against that revision.
    if plan.goal_revision() != frozen_goal_revision {
        return writer.fail(StoreError::InvariantViolated(format!(
            "plan was validated against goal revision {}, but the operation froze {frozen_goal_revision}",
            plan.goal_revision()
        )));
    }

    // 3. Re-read the graph revision.
    let current_graph_revision = writer
        .query_optional(
            "SELECT revision FROM task_graphs WHERE id = ?1",
            &[Value::from(goal_id.as_str())],
            |row| row.get::<_, i64>(0),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated(format!("goal {goal_id} has no task graph"))
        })?;
    if current_graph_revision != expected_graph_revision {
        return writer.fail(StoreError::RevisionConflict {
            table: "task_graphs",
            id: goal_id,
            expected: expected_graph_revision,
            actual: Some(current_graph_revision),
        });
    }

    // 4. Re-read the active configuration. The contract requires rechecking
    //    current *policy*, not only Goal and graph. Pantheon fails closed on
    //    any drift: the plan pinned evaluator versions from a configuration
    //    that is no longer active.
    let active_config = writer
        .query_optional(
            "SELECT activation_sequence FROM active_configuration WHERE id = 'singleton'",
            &[],
            |row| row.get::<_, Option<i64>>(0),
        )?
        .flatten()
        .ok_or_else(|| {
            StoreError::InvariantViolated(
                "no active configuration; cannot materialize a plan".to_string(),
            )
        })?;
    if active_config != frozen_config {
        return writer.fail(StoreError::RevisionConflict {
            table: "active_configuration",
            id: "singleton".to_string(),
            expected: frozen_config,
            actual: Some(active_config),
        });
    }

    // 5. Preconditions hold. Write the immutable spec and the Task.
    let spec = plan.spec();
    let spec_digest = spec.digest();
    let canonical = String::from_utf8(spec.to_value().to_canonical_bytes()).unwrap_or_default();
    writer.execute(
        "INSERT INTO task_specs (
             digest, goal_id, goal_revision, canonical_json, acceptance_digest,
             evaluator_registry_digest, configuration_activation_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (digest) DO NOTHING",
        &[
            Value::Blob(spec_digest.as_bytes().to_vec()),
            Value::from(goal_id.as_str()),
            Value::Integer(spec.goal_revision),
            Value::from(canonical),
            Value::Blob(spec.acceptance_digest().as_bytes().to_vec()),
            Value::Blob(
                spec.acceptance
                    .evaluator_registry_digest
                    .as_bytes()
                    .to_vec(),
            ),
            Value::Integer(spec.acceptance.configuration_activation_sequence),
        ],
    )?;

    let new_graph_revision = current_graph_revision + 1;

    // The Task's phase comes from the canonical readiness predicate, not from
    // an assumption that a first Task is always Ready.
    let phase = if prerequisites_satisfied(writer, task_id, new_graph_revision)? {
        TaskPhase::Ready
    } else {
        TaskPhase::Pending
    };

    writer.execute(
        "INSERT INTO tasks (
             id, goal_id, created_graph_revision, phase, revision,
             terminal_target, terminal_reason_json, active_run_id, spec_digest)
         VALUES (?1, ?2, ?3, ?4, 1, NULL, NULL, NULL, ?5)",
        &[
            Value::from(task_id),
            Value::from(goal_id.as_str()),
            Value::Integer(new_graph_revision),
            Value::from(phase.as_str()),
            Value::Blob(spec_digest.as_bytes().to_vec()),
        ],
    )?;

    // 6. The graph patch itself: an ordinary revisioned CAS against the
    //    revision this transaction just re-read.
    let patched_graph = writer.update_revisioned(
        "task_graphs",
        goal_id.as_str(),
        Revision::new(current_graph_revision),
        &[],
    )?;
    debug_assert_eq!(patched_graph, Revision::new(new_graph_revision));

    // 7. The Goal now has a coherent graph, so it leaves Planning. Task
    //    creation is not Goal success.
    let goal_phase = GoalPhase::parse(&goal_phase).ok_or_else(|| {
        StoreError::InvariantViolated(format!("goal {goal_id} has unknown phase"))
    })?;
    if goal_phase == GoalPhase::Planning {
        let _ = writer.update_revisioned(
            "goals",
            goal_id.as_str(),
            Revision::new(goal_row_revision),
            &[("phase", Value::from(GoalPhase::Active.as_str()))],
        )?;
    }

    // 8. The decision is spent.
    let _ = writer.update_revisioned(
        "planning_operations",
        operation_id,
        Revision::new(op_revision),
        &[("state", Value::from(PlanningState::Materialized.as_str()))],
    )?;

    Ok(MaterializedPlan {
        task: TaskRecord {
            id: task_id.to_string(),
            goal_id,
            phase,
            created_graph_revision: new_graph_revision,
            spec_digest,
            revision: Revision::new(1),
            active_run_id: None,
        },
        graph_revision: new_graph_revision,
        goal_phase: GoalPhase::Active,
    })
}

/// Whether every active incoming dependency of `task_id` is satisfied.
///
/// v1 has one prerequisite kind, `requires_success`: a downstream Task cannot
/// become logically runnable until the upstream Task is terminally
/// `Succeeded`. Expressed as a query over active edges rather than
/// special-cased to "a new Task has no edges", so it stays correct the moment
/// edges exist — and so a terminally *Failed* upstream is never silently
/// treated as satisfied.
pub(crate) fn prerequisites_satisfied(
    writer: &Writer<'_>,
    task_id: &str,
    graph_revision: i64,
) -> Result<bool, StoreError> {
    let unsatisfied = writer
        .query_optional(
            "SELECT COUNT(*)
             FROM task_graph_edges edge
             JOIN tasks upstream ON upstream.id = edge.upstream_task_id
             WHERE edge.downstream_task_id = ?1
               AND edge.kind = 'requires_success'
               AND edge.created_graph_revision <= ?2
               AND (edge.removed_graph_revision IS NULL
                    OR edge.removed_graph_revision > ?2)
               AND upstream.phase != 'Succeeded'",
            &[Value::from(task_id), Value::Integer(graph_revision)],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(0);
    Ok(unsatisfied == 0)
}
