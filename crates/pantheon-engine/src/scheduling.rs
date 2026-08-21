//! The Scheduler→Run-Controller control path: eligibility, ordering,
//! admission, and the T3 Run-intent commit.
//!
//! `docs/architecture/scheduling/scheduler-dispatch-and-run-intent-reconciliation.md`
//! is canonical for the boundary this module drives. The four scheduler
//! stages are kept deliberately distinct:
//!
//! ```text
//! 1. eligibility   Store::scheduling_snapshot — durable logical gates only
//! 2. ordering      pantheon_core::scheduling::select_service — pure
//! 3. admission     dispatch gates + single-slot check + side-effect-free routing
//! 4. commitment    Store::commit_run_intent — one authoritative transaction
//! ```
//!
//! # What this controller never does
//!
//! It never launches an executor, provisions a Sandbox, builds a ContextPlan,
//! retrieves Memory, renders a prompt or touches a repository. Routing is the
//! existing side-effect-free [`RoutingController`] path; commitment is the
//! store's T3 transaction. A failure before T3 charges no fairness and holds
//! no slot; a committed T3 creates durable responsibility, not execution.

use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value;
use pantheon_core::scheduling::{
    ContextSourceSnapshot, DispatchMode, ExecutionBinding, GoalFairness, SchedulableTask,
    Suppression, service_order,
};
use std::borrow::Borrow;

use pantheon_store::{Command, Committed, DispatchCandidate, RunIntent, Store, StoreError};

use crate::configuration::{ConfigurationAuthority, ConfigurationError};
use crate::routing::{ExecutorBackendPort, RoutingController, RoutingError};

/// How long a Task that failed before T3 waits before reconsideration.
///
/// Backoff durations are configuration, not architecture; this constant is
/// the v0.1.0 default for the daemon's tick-driven cycle.
const BACKOFF_SECONDS: i64 = 60;

/// A failure along the scheduling control path.
#[derive(Debug)]
pub enum SchedulingError {
    /// Durable state rejected or could not perform the mutation.
    Store(StoreError),
    /// Configuration required for scheduling is missing or uninterpretable.
    Configuration(ConfigurationError),
}

impl std::fmt::Display for SchedulingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(err) => write!(f, "scheduling store failure: {err}"),
            Self::Configuration(err) => write!(f, "scheduling configuration failure: {err}"),
        }
    }
}

impl std::error::Error for SchedulingError {}

impl From<StoreError> for SchedulingError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

impl From<ConfigurationError> for SchedulingError {
    fn from(err: ConfigurationError) -> Self {
        Self::Configuration(err)
    }
}

/// What one scheduling cycle decided. Every variant is a normal outcome:
/// suppression and deferral are the scheduler working, not failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleOutcome {
    /// No Task is currently eligible.
    Idle,
    /// Selection is suppressed by a temporary gate. The Tasks' semantic
    /// eligibility and waiting ages are untouched.
    Suppressed(Suppression),
    /// The selected Task could not be routed or admitted before T3; durable
    /// backoff was recorded and no fairness was charged.
    Deferred {
        task_id: String,
        reason: &'static str,
    },
    /// T3 refused the commit because the world moved between observation and
    /// commit (pause, fairness advance, slot loss). Nothing was written; the
    /// next cycle re-derives everything from authority.
    Contention { task_id: String },
    /// One Run intent committed atomically. The Task is now Active because a
    /// durable Run owns responsibility — no execution exists yet.
    Committed { run_id: String, task_id: String },
}

/// Drives one scheduling cycle against current authority.
///
/// Generic over how the store is held, for the same reason
/// [`RoutingController`] is: the daemon shares one owner, a test borrows.
#[derive(Debug)]
pub struct SchedulingController<'store, 'authority, S: Borrow<Store>> {
    store: &'store Store,
    configuration: &'authority ConfigurationAuthority<S>,
}

impl<'store, 'authority, S: Borrow<Store>> SchedulingController<'store, 'authority, S> {
    #[must_use]
    pub const fn new(
        store: &'store Store,
        configuration: &'authority ConfigurationAuthority<S>,
    ) -> Self {
        Self {
            store,
            configuration,
        }
    }

    /// Runs one full cycle: select, route, admit, commit.
    ///
    /// # Errors
    ///
    /// [`SchedulingError`] when durable state cannot be read or the store
    /// refuses a mutation in a way that is not ordinary contention.
    pub fn schedule_once(
        &self,
        backends: &[ExecutorBackendPort<'_>],
    ) -> Result<ScheduleOutcome, SchedulingError> {
        // Stage gate: configuration readiness suppresses selection without
        // making anything ineligible.
        let snapshot = self.configuration.snapshot()?;
        if snapshot.compiled().is_none() {
            return Ok(ScheduleOutcome::Suppressed(
                Suppression::ConfigurationUnavailable,
            ));
        }
        let binding = ConfigurationBindingOf(&snapshot).binding();

        // Stage 1+2: durable eligibility, then the pure ordering decision.
        let snap = self.store.scheduling_snapshot()?;
        if snap.state.dispatch_mode == DispatchMode::Paused {
            return Ok(ScheduleOutcome::Suppressed(Suppression::OperatorPause));
        }

        // Stage 3a: the single global slot. A held slot is transient scarcity,
        // not ineligibility, so it suppresses rather than defers.
        if self.store.slot_holder()?.is_some() {
            return Ok(ScheduleOutcome::Suppressed(Suppression::SlotHeld));
        }

        // Stage 2: the full deterministic order. Walking it (rather than only
        // its head) is what makes a temporarily unavailable older Task unable
        // to block the queue: each deferral records backoff and the walk
        // continues to the next Goal's work.
        let fairness: Vec<GoalFairness> = snap.goals.iter().map(|row| row.fairness()).collect();
        let tasks: Vec<SchedulableTask> = snap
            .candidates
            .iter()
            .map(|candidate| candidate.schedulable())
            .collect();
        let order = service_order(&fairness, &tasks);
        if order.is_empty() {
            return Ok(ScheduleOutcome::Idle);
        }
        let router = RoutingController::new(self.store, self.configuration);

        let mut last_deferred: Option<(String, &'static str)> = None;
        for (goal_id, task_id) in order {
            let candidate = snap
                .candidates
                .iter()
                .find(|candidate| candidate.task_id == task_id)
                .expect("the ordered Task came from the same snapshot");

            // Stage 3b: side-effect-free routing under the captured revision.
            let routed = match router.route_ready_task(&task_id, backends) {
                Ok(routed) => routed,
                Err(error) => {
                    eprintln!("DEBUG route fail {task_id}: {error:?}");
                    if let ScheduleOutcome::Deferred { task_id, reason } =
                        self.defer(candidate, &error, snap.now)?
                    {
                        last_deferred = Some((task_id, reason));
                    }
                    continue;
                }
            };

            return self.commit(&routed, candidate, &goal_id, &task_id, &snap, binding);
        }

        match last_deferred {
            Some((task_id, reason)) => Ok(ScheduleOutcome::Deferred { task_id, reason }),
            None => Ok(ScheduleOutcome::Idle),
        }
    }

    /// Stage 4 for one routed Task: freeze authority and commit T3.
    fn commit(
        &self,
        routed: &pantheon_core::execution::RoutingResult,
        candidate: &DispatchCandidate,
        goal_id: &str,
        task_id: &str,
        snap: &pantheon_store::SchedulingSnapshot,
        binding: pantheon_core::execution::ConfigurationBinding,
    ) -> Result<ScheduleOutcome, SchedulingError> {
        // Fresh pre-commit observations of the facts the source snapshot must
        // freeze. T3 revalidates every one of them inside its transaction.
        let goal = self.store.goal(goal_id)?.ok_or_else(|| {
            StoreError::InvariantViolated(format!("selected goal {goal_id} disappeared"))
        })?;
        let graph = self.store.task_graph(goal_id)?.ok_or_else(|| {
            StoreError::InvariantViolated(format!("selected goal {goal_id} has no graph"))
        })?;
        let workspace = self.store.workspace_for_task(task_id)?.ok_or_else(|| {
            StoreError::InvariantViolated(format!("selected task {task_id} has no Workspace"))
        })?;

        let binding_frozen = ExecutionBinding {
            task_id: task_id.to_string(),
            agent: routed.candidate.agent.clone(),
            request_digest: routed.request.digest(),
            offer_digest: routed.candidate.offer.digest(),
            backend_id: routed.candidate.offer.backend_id.clone(),
            descriptor_revision: routed.candidate.offer.descriptor_revision,
            descriptor_digest: routed.candidate.offer.descriptor_digest,
            execution_profile_digest: routed.candidate.execution_profile_digest,
            sandbox_profile_digest: routed.request.sandbox_profile_digest,
            route_policy_digest: routed.request.route_policy_digest,
            configuration_activation_sequence: binding.activation_sequence,
            configuration_content_digest: binding.content_digest,
            component_digests: binding.component_digests,
        };
        let snapshot_frozen = ContextSourceSnapshot {
            task_spec_digest: routed.task_spec_digest,
            goal_id: goal_id.to_string(),
            goal_revision: goal.current_revision,
            graph_revision: graph.revision.get(),
            agent: routed.candidate.agent.clone(),
            configuration_activation_sequence: binding.activation_sequence,
            context_policy_digest: binding.component_digests.context_policy,
            workspace_id: workspace.id.clone(),
            workspace_resolved_base: workspace.resolved_base.as_str().to_string(),
        };
        let binding_digest = binding_frozen.digest();
        let snapshot_digest = snapshot_frozen.digest();

        // Durable identities derived from content: the same frozen world is
        // the same command (so a lost response replays), and a different
        // attempt is a different command by construction.
        let request_hash = request_hash(
            task_id,
            &binding_digest,
            &snapshot_digest,
            snap.state.revision.get(),
            candidate.task_revision.get(),
        );
        let command_id = format!("t3-{}", short_hex(&request_hash));
        let run_identity = Digest::of(
            &run_identity_value(
                task_id,
                &binding_digest,
                &snapshot_digest,
                snap.state.next_service_sequence,
            )
            .to_canonical_bytes(),
        );
        let run_id = format!("run-{}", short_hex(run_identity.as_bytes()));

        let intent = RunIntent {
            run_id: &run_id,
            task_id,
            goal_id,
            expected_task_revision: candidate.task_revision,
            expected_goal_row_revision: goal.revision,
            expected_goal_current_revision: goal.current_revision,
            expected_graph_revision: graph.revision.get(),
            expected_workspace_revision: workspace.revision,
            expected_scheduler_revision: snap.state.revision,
            expected_goal_fairness_revision: snap
                .goals
                .iter()
                .find(|row| row.goal_id == goal_id)
                .map(|row| row.revision),
            expected_task_scheduling_revision: candidate.scheduling_revision,
            configuration_activation_sequence: binding.activation_sequence,
            binding_digest: &binding_digest,
            binding: &binding_frozen,
            snapshot_digest: &snapshot_digest,
            snapshot: &snapshot_frozen,
        };

        let epoch = self.store.restore_generation()?;
        let command = Command {
            epoch: epoch.as_str(),
            id: &command_id,
            request_hash: &request_hash,
            event_type: "run.committed",
        };

        // The atomic boundary. Contention here is ordinary: pause, fairness
        // advance or slot loss between observation and commit rolls everything
        // back and the next cycle re-derives from authority.
        match self.store.commit_run_intent(&command, &intent) {
            Ok(Committed::Executed { value, .. }) => Ok(ScheduleOutcome::Committed {
                run_id: value.run_id,
                task_id: task_id.to_string(),
            }),
            // A replay means this exact frozen world already committed; the
            // durable outcome stands and there is nothing left to do.
            Ok(Committed::Replayed { .. }) => Ok(ScheduleOutcome::Committed {
                run_id,
                task_id: task_id.to_string(),
            }),
            Err(StoreError::DispatchPaused)
            | Err(StoreError::DispatchSlotUnavailable { .. })
            | Err(StoreError::RevisionConflict { .. }) => Ok(ScheduleOutcome::Contention {
                task_id: task_id.to_string(),
            }),
            Err(err) => Err(err.into()),
        }
    }

    /// Records durable backoff for a routing/admission failure before T3.
    ///
    /// Fairness is never charged here, and the Task's waiting age is never
    /// touched: only the temporary `next_attempt_at` suppression point moves.
    fn defer(
        &self,
        candidate: &DispatchCandidate,
        error: &RoutingError,
        now: i64,
    ) -> Result<ScheduleOutcome, SchedulingError> {
        let Some(code) = backoff_code(error) else {
            // State that self-corrects by leaving the candidate set (the Task
            // moved on) needs no durable suppression.
            return Ok(ScheduleOutcome::Deferred {
                task_id: candidate.task_id.clone(),
                reason: "routing-state-moved",
            });
        };
        let _new_revision = self.store.record_scheduling_backoff(
            &candidate.task_id,
            candidate.scheduling_revision,
            code,
            "{}",
            now + BACKOFF_SECONDS,
        )?;
        Ok(ScheduleOutcome::Deferred {
            task_id: candidate.task_id.clone(),
            reason: code,
        })
    }
}

/// The captured ConfigurationRevision of one published snapshot.
struct ConfigurationBindingOf<'a>(&'a crate::configuration::Snapshot);

impl ConfigurationBindingOf<'_> {
    fn binding(&self) -> pantheon_core::execution::ConfigurationBinding {
        pantheon_core::execution::ConfigurationBinding::new(
            self.0.active().activation_sequence,
            self.0.active().content_digest,
            self.0.active().components,
        )
    }
}

/// Which routing failures deserve durable backoff, as a stable code.
fn backoff_code(error: &RoutingError) -> Option<&'static str> {
    match error {
        RoutingError::NoCompatibleOffers { .. } => Some("no-compatible-offer"),
        RoutingError::AgentResolution(_) => Some("no-eligible-agent"),
        RoutingError::UnregisteredBackend { .. } => Some("backend-unregistered"),
        RoutingError::BackendOffer { .. } => Some("backend-offer-failed"),
        RoutingError::Selection(_) => Some("no-compatible-offer"),
        RoutingError::StaleConfiguration => Some("stale-configuration"),
        // Everything else means durable state moved; the next snapshot simply
        // will not contain the same decision.
        _ => None,
    }
}

fn request_hash(
    task_id: &str,
    binding_digest: &Digest,
    snapshot_digest: &Digest,
    scheduler_revision: i64,
    task_revision: i64,
) -> [u8; 32] {
    *Digest::of(
        &Value::object([
            ("kind", Value::string("scheduler.run-intent")),
            ("taskId", Value::string(task_id)),
            ("bindingDigest", Value::string(binding_digest.to_string())),
            ("snapshotDigest", Value::string(snapshot_digest.to_string())),
            ("schedulerRevision", Value::Integer(scheduler_revision)),
            ("taskRevision", Value::Integer(task_revision)),
        ])
        .to_canonical_bytes(),
    )
    .as_bytes()
}

fn run_identity_value(
    task_id: &str,
    binding_digest: &Digest,
    snapshot_digest: &Digest,
    next_service_sequence: i64,
) -> Value {
    Value::object([
        ("kind", Value::string("run")),
        ("taskId", Value::string(task_id)),
        ("bindingDigest", Value::string(binding_digest.to_string())),
        ("snapshotDigest", Value::string(snapshot_digest.to_string())),
        ("nextServiceSequence", Value::Integer(next_service_sequence)),
    ])
}

fn short_hex(hash: &[u8; 32]) -> String {
    hash.iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
