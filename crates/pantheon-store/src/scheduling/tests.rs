//! Evidence for the durable half of Issue #29: the single-slot scheduler and
//! the atomic T3 Run-intent boundary.
//!
//! These tests drive the store directly. The engine-level orchestration is
//! proven where it belongs — `pantheon-engine`'s scheduling tests compose this
//! transaction with routing — and the daemon restart properties are proven
//! over a real socket in `pantheond`. What only a test at this altitude can
//! establish is what one authoritative transaction does to stored state under
//! success, staleness, pause, contention and injected failure.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Barrier;

use pantheon_core::config::Digest;
use pantheon_core::execution::LogicalAgentVersion;
use pantheon_core::planning::TaskPhase;
use pantheon_core::scheduling::{ContextSourceSnapshot, DispatchMode, ExecutionBinding};
use pantheon_core::workspace::{Materialization, RequestedBase, ResolvedBase};

use crate::command::{Command, Committed};
use crate::configuration::ActiveConfiguration;
use crate::error::StoreError;
use crate::planning::tests as fixture;
use crate::scheduling::{DispatchCandidate, RunIntent, RunIntentCommit};
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::{Revision, Value};
use crate::workspace::WorkspaceBinding;

const TASK: &str = "task-1";
const GOAL: &str = "goal-1";
const WORKSPACE: &str = "ws-1";

fn base() -> ResolvedBase {
    ResolvedBase::parse(&"a".repeat(40)).expect("fixture base")
}

/// A Goal whose first Task is Ready and owns a verified Workspace.
struct World {
    _dir: TempDir,
    db_path: PathBuf,
    store: Option<Store>,
    config_sequence: i64,
    active: ActiveConfiguration,
    resolved_base: String,
}

impl World {
    /// The open store handle.
    fn s(&self) -> &Store {
        self.store.as_ref().expect("the store is open")
    }

    /// A second handle for fixture work that mutates configuration.
    fn store_dummy(&self) -> &Store {
        self.s()
    }

    /// The activation sequence a pre-second-activation freeze observed.
    fn stale_configuration_sequence(&self) -> i64 {
        self.config_sequence
    }

    /// Closes the store so a later [`World::reopen`] can reconstruct it,
    /// exactly as a daemon restart would.
    fn close_store(&mut self) {
        if let Some(store) = self.store.take() {
            store.close().expect("close store");
        }
    }

    fn reopen(&mut self) {
        self.close_store();
        self.store = Some(Store::open(&self.db_path).expect("reopen store"));
    }
}

fn command<'a>(
    epoch: &'a str,
    id: &'static str,
    hash: &'a [u8; 32],
    event: &'static str,
) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: hash,
        event_type: event,
    }
}

fn world(label: &str) -> World {
    let (dir, store, sequence) = fixture::store_with_configuration(label);
    let db_path = dir.path().join("pantheon.db");
    let epoch = store.restore_generation().expect("generation");
    fixture::create_goal(&store, GOAL, "cmd-goal");
    let op = fixture::plan_and_record(&store, GOAL, sequence, "op-1");
    let registry_digest = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .as_ref()
        .expect("active")
        .components
        .evaluator_registry;
    let plan = fixture::validated(sequence, registry_digest, "unit-v1");
    fixture::materialize(&store, &op, TASK, &plan, "cmd-materialize").expect("materializes");

    materialize_workspace(&store, epoch.as_str(), WORKSPACE, TASK);

    let active_pointer = store.configuration_pointer().expect("pointer");
    let active = active_pointer.active.clone().expect("active");
    let resolved_base = base().as_str().to_string();
    World {
        _dir: dir,
        db_path,
        store: Some(store),
        config_sequence: sequence,
        active,
        resolved_base,
    }
}

/// Adds a second independent Goal/Task/Workspace so a second dispatchable
/// Task exists.
fn add_second_task(world: &World, goal_id: &'static str, task_id: &str, ws_id: &str) {
    let epoch = world
        .s()
        .restore_generation()
        .expect("generation")
        .to_string();
    fixture::create_goal(world.s(), goal_id, "cmd-goal-2");
    let op = fixture::plan_and_record(world.s(), goal_id, world.config_sequence, "op-2");
    let plan = fixture::validated_for(
        goal_id,
        world.config_sequence,
        world.active.components.evaluator_registry,
        "unit-v1",
    );
    fixture::materialize(world.s(), &op, task_id, &plan, "cmd-materialize-2")
        .expect("second task materializes");
    materialize_workspace(world.s(), &epoch, ws_id, task_id);
}

fn materialize_workspace(store: &Store, epoch: &str, workspace_id: &str, task_id: &str) {
    // Command identities are derived per Workspace so a second fixture in
    // the same epoch is a new command rather than a replay of the first.
    let open_id = format!("cmd-ws-open-{workspace_id}");
    let begin_id = format!("cmd-ws-begin-{workspace_id}");
    let complete_id = format!("cmd-ws-complete-{workspace_id}");
    let binding = WorkspaceBinding {
        task_id,
        repository: "repo://whiskyshop",
        source_path: "/tmp/pantheon-test-source",
        requested_base: &requested(),
        resolved_base: &base(),
    };
    store
        .open_workspace(
            &Command {
                epoch,
                id: open_id.as_str(),
                request_hash: &[7u8; 32],
                event_type: "workspace.opened",
            },
            workspace_id,
            &binding,
        )
        .expect("workspace opens");
    store
        .begin_workspace_materialization(
            &Command {
                epoch,
                id: begin_id.as_str(),
                request_hash: &[8u8; 32],
                event_type: "workspace.materializing",
            },
            workspace_id,
            Revision::new(1),
        )
        .expect("materialization begins");
    let record = store
        .complete_workspace_materialization(
            &Command {
                epoch,
                id: complete_id.as_str(),
                request_hash: &[9u8; 32],
                event_type: "workspace.ready",
            },
            workspace_id,
            Revision::new(2),
            &base(),
        )
        .expect("materialization completes");
    let Committed::Executed { value: record, .. } = record else {
        panic!("a fresh workspace completion executes");
    };
    assert_eq!(
        record.phase,
        pantheon_core::workspace::WorkspacePhase::Ready
    );
    assert_eq!(record.materialization, Materialization::Present);
}

fn requested() -> RequestedBase {
    RequestedBase::parse("main").expect("fixture ref")
}

/// The approved guidance of one Agent version under an activated revision,
/// extracted from the stored immutable agents component.
fn stored_guidance(
    store: &Store,
    active: &ActiveConfiguration,
    agent: &LogicalAgentVersion,
) -> pantheon_core::context::AgentGuidance {
    let agents_json = store
        .revision_agents_component_json(active.activation_sequence)
        .expect("read agents component")
        .expect("agents component stored");
    let value = pantheon_core::config::parse::parse(&agents_json).expect("fixture component");
    pantheon_core::context::frozen_agent_guidance(&value, agent).expect("fixture guidance")
}

/// The frozen strategy and context-source authority for `TASK`.
#[derive(Clone)]
struct Frozen {
    binding: ExecutionBinding,
    snapshot: ContextSourceSnapshot,
    binding_digest: Digest,
    snapshot_digest: Digest,
}

impl Frozen {
    /// Builds a T3 intent for `candidate` from this frozen authority.
    ///
    /// Every expectation comes from the candidate the caller read, so a test
    /// that wants a stale commit mutates exactly one field afterwards. The
    /// scheduler/fairness expectations are left at neutral values because
    /// they depend on how many commits ran before; tests set them explicitly.
    fn intent<'a>(&'a self, candidate: &'a DispatchCandidate, run_id: &'a str) -> RunIntent<'a> {
        RunIntent {
            run_id,
            task_id: candidate.task_id.as_str(),
            goal_id: candidate.goal_id.as_str(),
            expected_task_revision: candidate.task_revision,
            expected_goal_row_revision: candidate.goal_row_revision,
            expected_goal_current_revision: candidate.goal_current_revision,
            expected_graph_revision: candidate.graph_revision,
            expected_workspace_revision: candidate.workspace_revision,
            expected_scheduler_revision: Revision::new(0),
            expected_goal_fairness_revision: None,
            expected_task_scheduling_revision: candidate.scheduling_revision,
            configuration_activation_sequence: self.binding.configuration_activation_sequence,
            binding_digest: &self.binding_digest,
            binding: &self.binding,
            snapshot_digest: &self.snapshot_digest,
            snapshot: &self.snapshot,
        }
    }
}

fn frozen_for(
    store: &Store,
    active: &ActiveConfiguration,
    resolved_base: &str,
    goal_id: &'static str,
    task_id: &'static str,
    ws_id: &str,
) -> Frozen {
    let agent = LogicalAgentVersion {
        name: "builder".to_string(),
        version: 1,
    };
    // The guidance digests the active revision actually carries, read back
    // through the same extraction rule T3 validates with — so a fixture can
    // freeze honest identities without duplicating fixture text.
    let guidance = stored_guidance(store, active, &agent);
    let binding = ExecutionBinding {
        task_id: task_id.to_string(),
        agent: agent.clone(),
        request_digest: Digest::of(b"request"),
        offer_digest: Digest::of(b"offer"),
        backend_id: "fake-local".to_string(),
        descriptor_revision: 3,
        descriptor_digest: Digest::of(b"descriptor"),
        execution_profile_digest: active.components.execution_profile,
        sandbox_profile_digest: Digest::of(b"sandbox-profile"),
        route_policy_digest: active.components.routing,
        configuration_activation_sequence: active.activation_sequence,
        configuration_content_digest: active.content_digest,
        component_digests: active.components,
    };
    let snapshot = ContextSourceSnapshot {
        task_spec_digest: fixture::tasks_of(store, goal_id)[0].spec_digest,
        goal_id: goal_id.to_string(),
        goal_revision: 1,
        graph_revision: 1,
        agent: agent.clone(),
        configuration_activation_sequence: active.activation_sequence,
        context_policy_digest: active.components.context_policy,
        agent_soul_digest: pantheon_core::context::guidance_digest(&guidance.soul),
        agent_behavior_digest: pantheon_core::context::guidance_digest(&guidance.behavior),
        workspace_id: ws_id.to_string(),
        workspace_resolved_base: resolved_base.to_string(),
    };
    let binding_digest = binding.digest();
    let snapshot_digest = snapshot.digest();
    Frozen {
        binding,
        snapshot,
        binding_digest,
        snapshot_digest,
    }
}

/// Commits T3 for the world's first Task with neutral scheduler expectations
/// filled from current state.
fn commit_first(
    world: &World,
    frozen: &Frozen,
    run_id: &'static str,
    command_id: &'static str,
    hash: &'static [u8; 32],
) -> Result<Committed<RunIntentCommit>, StoreError> {
    let snap = world.s().scheduling_snapshot().expect("snapshot");
    let candidate = snap.candidates.first().expect("a dispatchable Task");
    let mut intent = frozen.intent(candidate, run_id);
    intent.expected_scheduler_revision = snap.state.revision;
    let epoch = world.s().restore_generation().expect("generation");
    world.s().commit_run_intent(
        &command(epoch.as_str(), command_id, hash, "run.committed"),
        &intent,
    )
}

#[test]
fn a_fresh_installation_dispatches_by_default() {
    let dir = TempDir::new("sched-bootstrap");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    let state = store.scheduler_state().expect("state");
    assert_eq!(state.dispatch_mode, DispatchMode::Running);
    assert_eq!(state.next_service_sequence, 1);
    assert_eq!(state.revision, Revision::new(1));
}

#[test]
fn t3_commits_one_run_and_activates_the_task_atomically() {
    let world = world("t3-happy");
    let frozen = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    let before = world.s().scheduling_snapshot().expect("snapshot");

    let committed = commit_first(&world, &frozen, "run-1", "cmd-t3", &[11u8; 32])
        .expect("the Run intent commits");
    let Committed::Executed { value, .. } = committed else {
        panic!("a fresh command must execute, not replay");
    };

    assert_eq!(value.run_id, "run-1");
    assert_eq!(
        value.charged_service_sequence,
        before.state.next_service_sequence
    );

    // The Task is Active because a durable Run owns responsibility.
    let tasks = fixture::tasks_of(world.s(), GOAL);
    assert_eq!(tasks[0].phase, TaskPhase::Active);
    assert_eq!(tasks[0].active_run_id.as_deref(), Some("run-1"));
    assert_eq!(
        tasks[0].revision.get(),
        before.candidates[0].task_revision.get() + 1
    );

    // The slot is durably held by exactly this Run.
    assert_eq!(
        world.s().slot_holder().expect("slot"),
        Some(("run-1".to_string(), TASK.to_string()))
    );

    // Fairness charged atomically, sequence advanced, eligibility closed.
    let after = world.s().scheduling_snapshot().expect("snapshot");
    assert_eq!(
        after.state.next_service_sequence,
        before.state.next_service_sequence + 1
    );
    assert_eq!(after.goals.len(), 1);
    assert_eq!(after.goals[0].goal_id, GOAL);
    assert_eq!(
        after.goals[0].last_served_sequence,
        Some(value.charged_service_sequence)
    );
    assert!(
        after.candidates.is_empty(),
        "an Active Task is not schedulable"
    );
    let row = world.s().task_scheduling_row_for_test(TASK).expect("row");
    assert_eq!(row.eligible_since, None, "leaving Ready ends the interval");
    assert_eq!(row.next_attempt_at, None, "backoff normalized on success");
    assert_eq!(row.last_failure_code, None);

    // Exactly one of each frozen authority row exists.
    assert_eq!(world.s().run_rows_for_test(), 1);
    assert_eq!(world.s().binding_rows_for_test(), 1);
    assert_eq!(world.s().snapshot_rows_for_test(), 1);
}

#[test]
fn a_replay_of_the_same_command_commits_nothing_new() {
    let world = world("t3-replay");
    let frozen = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    let snap = world.s().scheduling_snapshot().expect("snapshot");
    let mut intent = frozen.intent(&snap.candidates[0], "run-1");
    intent.expected_scheduler_revision = snap.state.revision;
    let epoch = world
        .s()
        .restore_generation()
        .expect("generation")
        .to_string();

    // The replay classification happens before the mutation is ever named,
    // so the same intent object is correct for both calls even though the
    // world moved between them.
    let first = world.s().commit_run_intent(
        &command(&epoch, "cmd-t3", &[11u8; 32], "run.committed"),
        &intent,
    );
    assert!(matches!(first, Ok(Committed::Executed { .. })));

    let replay = world.s().commit_run_intent(
        &command(&epoch, "cmd-t3", &[11u8; 32], "run.committed"),
        &intent,
    );
    assert!(matches!(replay, Ok(Committed::Replayed { .. })));

    assert_eq!(world.s().run_rows_for_test(), 1, "no second Run appeared");
    let state = world.s().scheduler_state().expect("state");
    assert_eq!(
        state.next_service_sequence, 2,
        "fairness was not charged twice"
    );
}

#[test]
fn a_second_task_cannot_take_the_held_slot_while_a_run_is_responsible() {
    let world = world("t3-slot");
    add_second_task(&world, "goal-2", "task-2", "ws-2");
    let frozen_first = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    commit_first(&world, &frozen_first, "run-1", "cmd-t3a", &[11u8; 32])
        .expect("the first Run commits");

    // The second Task is fully eligible; only the slot stands in its way.
    let after = world.s().scheduling_snapshot().expect("snapshot");
    assert_eq!(after.candidates.len(), 1);
    assert_eq!(after.candidates[0].task_id, "task-2");

    let frozen_second = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        "goal-2",
        "task-2",
        "ws-2",
    );
    let mut intent = frozen_second.intent(&after.candidates[0], "run-2");
    intent.expected_scheduler_revision = after.state.revision;
    let epoch = world
        .s()
        .restore_generation()
        .expect("generation")
        .to_string();
    let refused = world.s().commit_run_intent(
        &command(&epoch, "cmd-t3b", &[12u8; 32], "run.committed"),
        &intent,
    );

    match refused {
        Err(StoreError::DispatchSlotUnavailable {
            held_by_run,
            held_for_task,
        }) => {
            assert_eq!(held_by_run, "run-1");
            assert_eq!(held_for_task, TASK);
        }
        other => panic!("expected a typed slot refusal, got {other:?}"),
    }

    // The refusal wrote nothing: no fairness charge for goal-2, no Run, and
    // the Task remains Ready with its waiting age intact.
    assert_eq!(world.s().run_rows_for_test(), 1);
    let state = world.s().scheduler_state().expect("state");
    assert_eq!(state.next_service_sequence, 2);
    let tasks = fixture::tasks_of(world.s(), "goal-2");
    assert_eq!(tasks[0].phase, TaskPhase::Ready);
    let row = world
        .s()
        .task_scheduling_row_for_test("task-2")
        .expect("row");
    assert_eq!(
        row.eligible_since,
        Some(after.candidates[0].eligible_since),
        "a slot refusal never resets the waiting age"
    );
}

#[test]
fn two_racing_commits_resolve_to_exactly_one_run() {
    let world = Arc::new(world("t3-race"));
    add_second_task(&world, "goal-2", "task-2", "ws-2");

    // Both callers observe the same world before the race.
    let pre = world.s().scheduling_snapshot().expect("snapshot");
    assert_eq!(pre.candidates.len(), 2);
    let observed_revision = pre.state.revision;
    let candidate_for = |task_id: &str| {
        pre.candidates
            .iter()
            .find(|candidate| candidate.task_id == task_id)
            .expect("candidate")
            .clone()
    };

    struct Racer {
        world: Arc<World>,
        frozen: Frozen,
        candidate: DispatchCandidate,
        observed_revision: Revision,
        run_id: &'static str,
        command_id: &'static str,
        hash: [u8; 32],
        barrier: Arc<Barrier>,
        epoch: String,
    }

    impl Racer {
        fn commit(self) -> Result<Committed<RunIntentCommit>, StoreError> {
            let mut intent = self.frozen.intent(&self.candidate, self.run_id);
            intent.expected_scheduler_revision = self.observed_revision;
            self.barrier.wait();
            self.world.s().commit_run_intent(
                &command(&self.epoch, self.command_id, &self.hash, "run.committed"),
                &intent,
            )
        }
    }

    let barrier = Arc::new(Barrier::new(2));
    let epoch = world
        .s()
        .restore_generation()
        .expect("generation")
        .to_string();
    let racers = vec![
        Racer {
            world: Arc::clone(&world),
            frozen: frozen_for(
                world.s(),
                &world.active,
                &world.resolved_base,
                GOAL,
                TASK,
                WORKSPACE,
            ),
            candidate: candidate_for(TASK),
            observed_revision,
            run_id: "run-a",
            command_id: "cmd-a",
            hash: [21u8; 32],
            barrier: Arc::clone(&barrier),
            epoch: epoch.clone(),
        },
        Racer {
            world: Arc::clone(&world),
            frozen: frozen_for(
                world.s(),
                &world.active,
                &world.resolved_base,
                "goal-2",
                "task-2",
                "ws-2",
            ),
            candidate: candidate_for("task-2"),
            observed_revision,
            run_id: "run-b",
            command_id: "cmd-b",
            hash: [22u8; 32],
            barrier: Arc::clone(&barrier),
            epoch,
        },
    ];

    let mut handles = Vec::new();
    for racer in racers {
        handles.push(std::thread::spawn(move || racer.commit()));
    }
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.join().expect("no panic"));
    }

    // Exactly one committed T3; the loser received a clean typed refusal.
    let winners: Vec<&RunIntentCommit> = results
        .iter()
        .filter_map(|result| match result {
            Ok(Committed::Executed { value, .. }) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "exactly one racing caller commits: {results:?}"
    );
    let winner_run = winners[0].run_id.clone();

    for result in &results {
        if let Err(err) = result {
            match err {
                StoreError::RevisionConflict { table, .. } => {
                    assert_eq!(
                        *table, "scheduler_state",
                        "the loser lost the fairness fence"
                    )
                }
                StoreError::DispatchSlotUnavailable { .. } => {}
                other => panic!("unexpected refusal: {other:?}"),
            }
        }
    }

    // One durable Run holds the slot; the loser's Task is untouched.
    assert_eq!(world.s().run_rows_for_test(), 1);
    let (holder_run, holder_task) = world.s().slot_holder().expect("slot").expect("held");
    assert_eq!(holder_run, winner_run);
    let loser_task = if holder_task == TASK { "task-2" } else { TASK };
    let tasks = fixture::tasks_of(world.s(), if loser_task == TASK { GOAL } else { "goal-2" });
    assert_eq!(tasks[0].phase, TaskPhase::Ready);

    let state = world.s().scheduler_state().expect("state");
    assert_eq!(state.next_service_sequence, observed_revision.get() + 1);
}

#[test]
fn a_fresh_command_identity_cannot_overlap_a_responsible_run() {
    let world = world("t3-overlap");
    let frozen = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    commit_first(&world, &frozen, "run-1", "cmd-t3a", &[11u8; 32]).expect("the first Run commits");

    // A different command identity, a different run id, and expectations
    // rebuilt from current state: none of it may bypass one-Run-per-Task.
    let state = world.s().scheduler_state().expect("state");
    let tasks = fixture::tasks_of(world.s(), GOAL);
    let stale_candidate = DispatchCandidate {
        task_id: TASK.to_string(),
        goal_id: GOAL.to_string(),
        spec_digest: tasks[0].spec_digest,
        eligible_since: 0,
        task_revision: tasks[0].revision,
        goal_current_revision: 1,
        goal_row_revision: Revision::new(2),
        graph_revision: 1,
        workspace_id: WORKSPACE.to_string(),
        workspace_resolved_base: world.resolved_base.clone(),
        workspace_revision: Revision::new(3),
        scheduling_revision: Revision::new(2),
    };
    let mut intent = frozen.intent(&stale_candidate, "run-2");
    intent.expected_scheduler_revision = state.revision;
    let epoch = world
        .s()
        .restore_generation()
        .expect("generation")
        .to_string();
    let refused = world.s().commit_run_intent(
        &command(&epoch, "cmd-t3b", &[13u8; 32], "run.committed"),
        &intent,
    );

    match refused {
        Err(StoreError::TaskNotDispatchable {
            task_id,
            phase,
            active_run_id,
        }) => {
            assert_eq!(task_id, TASK);
            assert_eq!(phase, TaskPhase::Active.as_str());
            assert_eq!(active_run_id.as_deref(), Some("run-1"));
        }
        other => panic!("expected a typed dispatch refusal, got {other:?}"),
    }
    assert_eq!(world.s().run_rows_for_test(), 1);
}

#[test]
fn paused_dispatch_fences_t3_survives_restart_and_never_cancels_committed_work() {
    let mut world = world("t3-pause");
    let frozen = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    commit_first(&world, &frozen, "run-1", "cmd-t3a", &[11u8; 32]).expect("the first Run commits");

    // Pause through the same command path the Operator surface uses.
    let state = world.s().scheduler_state().expect("state");
    let epoch = world
        .s()
        .restore_generation()
        .expect("generation")
        .to_string();
    world
        .s()
        .set_dispatch_mode(
            &command(&epoch, "cmd-pause", &[31u8; 32], "dispatch.paused"),
            DispatchMode::Paused,
            state.revision,
        )
        .expect("pause commits");

    // A committed Run is untouched by pausing.
    assert_eq!(
        world.s().slot_holder().expect("slot"),
        Some(("run-1".to_string(), TASK.to_string()))
    );

    // New T3 is fenced while paused — including after full reconstruction.
    world.reopen();
    let reopened = world.s();
    assert_eq!(
        reopened.scheduler_state().expect("state").dispatch_mode,
        DispatchMode::Paused,
        "an ordinary restart never silently resumes"
    );
    let snap = reopened.scheduling_snapshot().expect("snapshot");
    assert!(
        snap.candidates.is_empty(),
        "the Active Task is not schedulable anyway"
    );

    // Fixture work continues against the reopened store: durable rows are
    // identical, only process memory changed.
    let epoch2 = reopened
        .restore_generation()
        .expect("generation")
        .to_string();
    fixture::create_goal(reopened, "goal-2", "cmd-goal-2");
    let op = fixture::plan_and_record(reopened, "goal-2", world.config_sequence, "op-2");
    let plan = fixture::validated_for(
        "goal-2",
        world.config_sequence,
        world.active.components.evaluator_registry,
        "unit-v1",
    );
    fixture::materialize(reopened, &op, "task-2", &plan, "cmd-materialize-2")
        .expect("second task materializes");
    materialize_workspace(reopened, &epoch2, "ws-2", "task-2");

    let snap = reopened.scheduling_snapshot().expect("snapshot");
    assert_eq!(snap.candidates.len(), 1);
    let frozen_second = frozen_for(
        reopened,
        &world.active,
        &world.resolved_base,
        "goal-2",
        "task-2",
        "ws-2",
    );
    let mut intent = frozen_second.intent(&snap.candidates[0], "run-2");
    intent.expected_scheduler_revision = snap.state.revision;
    let refused = reopened.commit_run_intent(
        &command(&epoch, "cmd-t3b", &[12u8; 32], "run.committed"),
        &intent,
    );
    assert!(
        matches!(refused, Err(StoreError::DispatchPaused)),
        "PAUSED fences new T3: {refused:?}"
    );
    assert_eq!(reopened.run_rows_for_test(), 1, "nothing was written");

    // Resume re-opens dispatch. The identical request is no longer refused
    // for pausing; it is now refused only by the slot run-1 still holds,
    // which is the difference the fence exists to make observable.
    let state = reopened.scheduler_state().expect("state");
    reopened
        .set_dispatch_mode(
            &command(&epoch, "cmd-resume", &[32u8; 32], "dispatch.resumed"),
            DispatchMode::Running,
            state.revision,
        )
        .expect("resume commits");
    let snap = reopened.scheduling_snapshot().expect("snapshot");
    let mut intent = frozen_second.intent(&snap.candidates[0], "run-2");
    intent.expected_scheduler_revision = snap.state.revision;
    let second = reopened.commit_run_intent(
        &command(&epoch, "cmd-t3c", &[14u8; 32], "run.committed"),
        &intent,
    );
    match second {
        Err(StoreError::DispatchSlotUnavailable { held_by_run, .. }) => {
            assert_eq!(held_by_run, "run-1");
        }
        other => panic!("after resume only the slot may refuse: {other:?}"),
    }
    assert_eq!(reopened.run_rows_for_test(), 1);
}

#[test]
fn stale_authority_fails_closed_without_writing_anything() {
    // Each scenario corrupts exactly one expectation of an otherwise valid
    // T3 and asserts the typed refusal plus a completely pristine world.
    enum Stale {
        Task,
        GoalRow,
        GoalRevision,
        Graph,
        Workspace,
        Scheduler,
        Configuration,
        BindingComponents,
        BindingForOtherTask,
        SnapshotBase,
        SnapshotSpec,
        SnapshotPolicy,
        SnapshotForOtherGoal,
        FairnessRow,
    }

    let scenarios = [
        (Stale::Task, "task"),
        (Stale::GoalRow, "goal-row"),
        (Stale::GoalRevision, "goal-rev"),
        (Stale::Graph, "graph"),
        (Stale::Workspace, "workspace"),
        (Stale::Scheduler, "scheduler"),
        (Stale::Configuration, "config"),
        (Stale::BindingComponents, "components"),
        (Stale::BindingForOtherTask, "binding-owner"),
        (Stale::SnapshotBase, "base"),
        (Stale::SnapshotSpec, "spec"),
        (Stale::SnapshotPolicy, "policy"),
        (Stale::SnapshotForOtherGoal, "snapshot-owner"),
        (Stale::FairnessRow, "fairness"),
    ];

    for (scenario, label) in scenarios {
        let world = world("t3-stale");
        let mut frozen = frozen_for(
            world.s(),
            &world.active,
            &world.resolved_base,
            GOAL,
            TASK,
            WORKSPACE,
        );

        // Mutations of the frozen authority happen before any borrow exists;
        // expectation mutations happen after the intent is built.
        match scenario {
            Stale::BindingComponents => {
                frozen.binding.component_digests.authorization = Digest::of(b"tampered");
                frozen.binding_digest = frozen.binding.digest();
            }
            Stale::SnapshotBase => {
                frozen.snapshot.workspace_resolved_base = "b".repeat(40);
                frozen.snapshot_digest = frozen.snapshot.digest();
            }
            Stale::SnapshotSpec => {
                frozen.snapshot.task_spec_digest = Digest::of(b"another-task");
                frozen.snapshot_digest = frozen.snapshot.digest();
            }
            // The snapshot names a context-policy generation the captured
            // revision does not contain: every other digest agrees, so only
            // the explicit cross-check between snapshot and revision can
            // refuse it.
            Stale::SnapshotPolicy => {
                frozen.snapshot.context_policy_digest = Digest::of(b"unrelated-policy");
                frozen.snapshot_digest = frozen.snapshot.digest();
            }
            // A record frozen for a different owner: every digest is
            // internally consistent, so only the owner comparison inside T3
            // can refuse the swap.
            Stale::BindingForOtherTask => {
                frozen.binding.task_id = "task-elsewhere".to_string();
                frozen.binding_digest = frozen.binding.digest();
            }
            Stale::SnapshotForOtherGoal => {
                frozen.snapshot.goal_id = "goal-elsewhere".to_string();
                frozen.snapshot_digest = frozen.snapshot.digest();
            }
            _ => {}
        }

        let snap = world.s().scheduling_snapshot().expect("snapshot");
        let mut intent = frozen.intent(&snap.candidates[0], "run-1");
        intent.expected_scheduler_revision = snap.state.revision;

        match scenario {
            Stale::Task => intent.expected_task_revision = Revision::new(99),
            Stale::GoalRow => intent.expected_goal_row_revision = Revision::new(99),
            Stale::GoalRevision => intent.expected_goal_current_revision = 99,
            Stale::Graph => intent.expected_graph_revision = 99,
            Stale::Workspace => intent.expected_workspace_revision = Revision::new(99),
            Stale::Scheduler => intent.expected_scheduler_revision = Revision::new(99),
            // A stale-but-existing revision: activate a newer configuration
            // so the sequence under test resolves to a stored row. This is
            // what makes the currency fence itself load-bearing rather than
            // shadowed by the missing-row invariant.
            Stale::Configuration => {
                fixture::activate_configuration(world.store_dummy(), "cfg-later", 9000);
                intent.configuration_activation_sequence = world.stale_configuration_sequence();
            }
            Stale::FairnessRow => intent.expected_goal_fairness_revision = Some(Revision::new(9)),
            _ => {}
        }

        let epoch = world
            .s()
            .restore_generation()
            .expect("generation")
            .to_string();
        let refused = world.s().commit_run_intent(
            &command(&epoch, "cmd-t3", &[15u8; 32], "run.committed"),
            &intent,
        );
        let Err(ref err) = refused else {
            panic!("scenario {label} must refuse");
        };
        assert!(
            matches!(
                err,
                StoreError::RevisionConflict { .. } | StoreError::InvariantViolated(_)
            ),
            "scenario {label} refused with an unexpected type: {err:?}"
        );

        // Nothing leaked: no Run, no frozen authority rows, no fairness
        // charge, no scheduler advance, and the Task is still dispatchable
        // with its waiting age intact.
        assert_eq!(world.s().run_rows_for_test(), 0, "{label}");
        assert_eq!(world.s().binding_rows_for_test(), 0, "{label}");
        assert_eq!(world.s().snapshot_rows_for_test(), 0, "{label}");
        let after = world.s().scheduling_snapshot().expect("snapshot");
        assert_eq!(after.candidates.len(), 1, "{label}");
        assert_eq!(
            after.state.next_service_sequence, snap.state.next_service_sequence,
            "{label}"
        );
        assert_eq!(after.state.revision, snap.state.revision, "{label}");
        assert!(after.goals.is_empty(), "{label}");
    }
}

#[test]
fn an_injected_late_failure_rolls_back_every_frozen_row() {
    let world = world("t3-rollback");
    let frozen = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    let snap = world.s().scheduling_snapshot().expect("snapshot");

    // The scheduling-state normalization runs last in the transaction, so a
    // wrong expectation there fails the commit *after* the Binding, snapshot,
    // Run, slot, activation and fairness writes have already been applied.
    // Rollback must erase all of them.
    let mut doomed = frozen.intent(&snap.candidates[0], "run-1");
    doomed.expected_scheduler_revision = snap.state.revision;
    doomed.expected_task_scheduling_revision = Revision::new(999);

    let epoch = world
        .s()
        .restore_generation()
        .expect("generation")
        .to_string();
    let refused = world.s().commit_run_intent(
        &command(&epoch, "cmd-t3", &[16u8; 32], "run.committed"),
        &doomed,
    );
    assert!(
        matches!(
            &refused,
            Err(StoreError::RevisionConflict { table, .. }) if *table == "task_scheduling_state"
        ),
        "the injected failure surfaces as the stale row it hit: {refused:?}"
    );

    assert_eq!(world.s().run_rows_for_test(), 0);
    assert_eq!(world.s().binding_rows_for_test(), 0);
    assert_eq!(world.s().snapshot_rows_for_test(), 0);
    assert!(world.s().slot_holder().expect("slot").is_none());
    let after = world.s().scheduling_snapshot().expect("snapshot");
    assert_eq!(
        after.state.next_service_sequence,
        snap.state.next_service_sequence
    );
    assert_eq!(after.goals.len(), 0);
    let tasks = fixture::tasks_of(world.s(), GOAL);
    assert_eq!(tasks[0].phase, TaskPhase::Ready);
    assert_eq!(tasks[0].active_run_id, None);

    // A correct retry under a fresh command identity then succeeds in full —
    // which also proves no content-addressed residue survived the rollback.
    let retry_snap = world.s().scheduling_snapshot().expect("snapshot");
    let mut retry = frozen.intent(&retry_snap.candidates[0], "run-1");
    retry.expected_scheduler_revision = retry_snap.state.revision;
    let committed = world.s().commit_run_intent(
        &command(&epoch, "cmd-t3-retry", &[17u8; 32], "run.committed"),
        &retry,
    );
    assert!(matches!(committed, Ok(Committed::Executed { .. })));
}

#[test]
fn restart_reconstructs_the_same_slot_and_ordering_inputs() {
    let mut world = world("t3-restart");
    add_second_task(&world, "goal-2", "task-2", "ws-2");
    let frozen_first = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    commit_first(&world, &frozen_first, "run-1", "cmd-t3a", &[11u8; 32])
        .expect("the first Run commits");
    let before = world.s().scheduling_snapshot().expect("snapshot");

    world.reopen();
    let reopened = world.s();

    // The slot survives reconstruction: admission sees it held without any
    // in-memory state.
    assert_eq!(
        reopened.slot_holder().expect("slot"),
        Some(("run-1".to_string(), TASK.to_string()))
    );

    // Ordering inputs reconstruct identically: same service sequence, same
    // fairness charge, same remaining candidate.
    let after = reopened.scheduling_snapshot().expect("snapshot");
    assert_eq!(
        after.state.next_service_sequence,
        before.state.next_service_sequence
    );
    assert_eq!(after.goals, before.goals);
    assert_eq!(after.candidates, before.candidates);
    assert_eq!(
        reopened.scheduler_state().expect("state").dispatch_mode,
        DispatchMode::Running
    );
}

#[test]
fn backoff_suppresses_selection_without_touching_lifecycle_or_waiting_age() {
    let world = world("t3-backoff");
    let before = world.s().scheduling_snapshot().expect("snapshot");
    let candidate = before.candidates[0].clone();

    let _advanced = world
        .s()
        .record_scheduling_backoff(
            TASK,
            candidate.scheduling_revision,
            "temporarily-unavailable",
            r#"{"detail":"no compatible offer"}"#,
            before.now + 3_600,
        )
        .expect("backoff records");

    let suppressed = world.s().scheduling_snapshot().expect("snapshot");
    assert!(
        suppressed.candidates.is_empty(),
        "future backoff suppresses selection"
    );
    let tasks = fixture::tasks_of(world.s(), GOAL);
    assert_eq!(
        tasks[0].phase,
        TaskPhase::Ready,
        "backoff never mutates lifecycle"
    );
    let row = world.s().task_scheduling_row_for_test(TASK).expect("row");
    assert_eq!(
        row.eligible_since,
        Some(candidate.eligible_since),
        "backoff never resets the waiting age"
    );
    assert_eq!(
        row.last_failure_code.as_deref(),
        Some("temporarily-unavailable")
    );

    // Once the suppression point elapses, the same interval is reconsidered.
    let _elapsed = world
        .s()
        .record_scheduling_backoff(
            TASK,
            Revision::new(candidate.scheduling_revision.get() + 1),
            "temporarily-unavailable",
            r#"{"detail":"no compatible offer"}"#,
            before.now - 1,
        )
        .expect("elapsed backoff records");
    let reconsidered = world.s().scheduling_snapshot().expect("snapshot");
    assert_eq!(reconsidered.candidates.len(), 1);
    assert_eq!(
        reconsidered.candidates[0].eligible_since, candidate.eligible_since,
        "the eligibility interval continued across suppression"
    );
}

#[test]
fn the_database_rejects_a_second_nonterminal_run_even_bypassing_the_controller() {
    // This is the race-proof backstop, not the primary fence: with every
    // controller check removed, two nonterminal Runs still cannot exist. It
    // exists so a future edit that deletes a check cannot silently turn the
    // unique index into decoration.
    let world = world("t3-backstop");
    let frozen = frozen_for(
        world.s(),
        &world.active,
        &world.resolved_base,
        GOAL,
        TASK,
        WORKSPACE,
    );
    commit_first(&world, &frozen, "run-1", "cmd-t3a", &[11u8; 32]).expect("the first Run commits");

    let refused = world.s().write(|writer| {
        writer.execute(
            "INSERT INTO runs
                 (id, task_id, binding_digest, context_source_snapshot_digest, created_at)
             SELECT 'run-2', task_id, binding_digest, context_source_snapshot_digest, unixepoch()
             FROM runs WHERE id = 'run-1'",
            &[],
        )?;
        writer.execute(
            "INSERT INTO run_status
                 (run_id, task_id, phase, terminal_target, revision, active_slot, updated_at)
             VALUES ('run-2', ?1, 'Active', NULL, 1, 'global', unixepoch())",
            &[Value::from(TASK)],
        )
    });

    match refused {
        Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(code, _))) => {
            assert_eq!(
                code.extended_code,
                rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE,
                "the slot index is what refused the second nonterminal Run"
            );
        }
        other => panic!("expected the unique index to refuse the insert: {other:?}"),
    }
}
