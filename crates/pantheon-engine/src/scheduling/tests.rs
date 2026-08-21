//! Evidence for the engine-level scheduling path: eligibility, ordering,
//! admission and the T3 commit composed with real routing.
//!
//! The daemon restart properties are proven over a real socket in
//! `pantheond`; the transaction internals are proven in `pantheon-store`.
//! What this module establishes is the composition: that a scheduling cycle
//! drives the four stages in order, charges fairness exactly when T3 commits,
//! and never crosses an execution boundary.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use pantheon_core::execution::{
    BackendDescriptor, ControllerSafetyFacts, ExecutionOffer, ExecutionRequest, LaunchSemantics,
};
use pantheon_core::planning::direct;
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::scheduling::{DispatchMode, Suppression};
use pantheon_core::workspace::{RequestedBase, ResolvedBase};
use pantheon_store::{Command, Store};

use super::{ScheduleOutcome, SchedulingController};
use crate::configuration::{ConfigurationAuthority, SourceSet};
use crate::planning::PlanningController;
use crate::routing::{ExecutorBackend, ExecutorBackendPort};

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-scheduling-test-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.0.join("pantheon.db")
    }
}

fn command<'a>(
    epoch: &'a str,
    id: &'a str,
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

fn configuration_source() -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"enabled":true,"current":true,
    "accepts":["code.change"],"competencies":["code.analysis","code.editing","test.execution"],"routePolicy":"default",
    "executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"]}}],
  "routing": {{"policies":[{{"name":"default","priority":0,"ordering":["contextCapacity"],
    "tieBreak":"backendId","requiresKeyedLaunch":true}}]}},
  "execution": {{"profiles":[{{"name":"strict","isolationClass":"CONTAINER",
    "guarantees":["isolation.control-plane"],"networkMode":"NONE",
    "environmentIdentity":"sha256:image"}}],"backends":[
    {{"backendId":"fake-local","enabled":true,"selector":"fake"}}
  ]}},
  "evaluators": {{"versions":[{{"id":"unit-tests-v1","kind":"check",
    "argv":["/bin/check"],"timeoutMs":1000,"sandboxProfile":"strict",
    "resultProtocol":"p-v1"}}],"refs":[{{"ref":"{evaluator_ref}",
    "currentVersion":"unit-tests-v1"}}]}},
  "context": {{"schemaVersion":1,"mandatorySections":["task"],
    "preloadPriority":["task"],"memoryLimitTokens":4000,
    "workspaceOrientationLimitTokens":2000,"safetyMarginTokens":512,
    "optionalDropOrder":["memory"]}},
  "authorization": {{"schemaVersion":1,"rules":[
    {{"action":"filesystem.read","effect":"permit"}}
  ]}}
}}"#,
        evaluator_ref = direct::MVP_EVALUATOR_REF,
    )
}

/// Offers for exactly one Task and refuses everything else, so a fixture can
/// make one Goal admissible while another is not.
struct FakeBackend {
    descriptor: BackendDescriptor,
    /// The Task id this backend produces offers for.
    serves_task: &'static str,
    offer_calls: Cell<usize>,
}

impl FakeBackend {
    fn new(serves_task: &'static str) -> Self {
        Self {
            descriptor: BackendDescriptor {
                backend_id: "fake-local".to_string(),
                revision: 3,
                available_for_offers: true,
                placement: vec![],
                supported_execution_features: vec!["exec.shell".to_string()],
                context_capacity_tokens: 32_000,
                isolation_facts: vec!["isolation.control-plane".to_string()],
                resources: vec![],
                launch_semantics: LaunchSemantics::KeyedIdempotent,
            },
            serves_task,
            offer_calls: Cell::new(0),
        }
    }

    fn port(&self) -> ExecutorBackendPort<'_> {
        ExecutorBackendPort::new(
            self,
            ControllerSafetyFacts {
                isolation_guarantees: vec!["isolation.control-plane".to_string()],
                observational_launch_safe: false,
            },
        )
    }
}

impl ExecutorBackend for FakeBackend {
    fn describe(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn offer(
        &self,
        request: &ExecutionRequest,
    ) -> Result<Vec<ExecutionOffer>, crate::routing::BackendError> {
        self.offer_calls.set(self.offer_calls.get() + 1);
        if request.task_id != self.serves_task {
            return Ok(Vec::new());
        }
        Ok(vec![ExecutionOffer {
            request_digest: request.digest(),
            backend_id: self.descriptor.backend_id.clone(),
            descriptor_revision: self.descriptor.revision,
            descriptor_digest: self.descriptor.digest(),
            supported_execution_features: self.descriptor.supported_execution_features.clone(),
            context_capacity_tokens: self.descriptor.context_capacity_tokens,
            placement: self.descriptor.placement.clone(),
            isolation_facts: self.descriptor.isolation_facts.clone(),
            resources: self.descriptor.resources.clone(),
            launch_semantics: self.descriptor.launch_semantics,
            offer_reference: "fixture-offer".to_string(),
        }])
    }
}

fn goal_spec() -> GoalSpec {
    GoalSpec {
        objective: "perform a bounded coding task".to_string(),
        inputs: vec![GoalInput {
            name: "repository".to_string(),
            reference: "repo://project".to_string(),
        }],
        deliverables: vec![Deliverable {
            name: "changeset".to_string(),
            kind: "code.changeset".to_string(),
            required: true,
        }],
        constraints: GoalConstraints {
            permitted_effects: vec!["filesystem.read".to_string()],
            forbidden_effects: Vec::new(),
            permitted_resources: vec!["workspace://src/**".to_string()],
        },
    }
}

/// Activates and publishes the fixture configuration.
fn load_authority(store: &Store) -> ConfigurationAuthority<&'_ Store> {
    let authority = ConfigurationAuthority::new(store);
    let epoch = store.restore_generation().expect("read generation");
    authority
        .activate(
            &command(
                epoch.as_str(),
                "cfg-1",
                &[1u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single("configuration.json", configuration_source()),
        )
        .expect("activate configuration");
    authority
}

/// Drives the Goal-to-Ready-Task path and then materializes the Task's
/// Workspace at the durable layer, exactly as #27's controller would.
fn ready_task_with_workspace(store: &Store, goal_id: &str, task_id: &str, workspace_id: &str) {
    let planning = PlanningController::new(store);
    let spec = goal_spec();
    let epoch = store.restore_generation().expect("read generation");
    planning
        .create_goal(
            &command(epoch.as_str(), goal_id, &[2u8; 32], "goal.created"),
            goal_id,
            &spec,
        )
        .expect("create Goal");
    let plan_id = format!("plan-{goal_id}");
    let graph_id = format!("graph-{goal_id}");
    planning
        .plan(
            &command(epoch.as_str(), &plan_id, &[3u8; 32], "planning.recorded"),
            &format!("planning-{goal_id}"),
            goal_id,
        )
        .expect("record plan");
    let proposal = planning.proposal(goal_id).expect("re-derive proposal");
    planning
        .materialize(
            &command(epoch.as_str(), &graph_id, &[4u8; 32], "graph.patched"),
            &format!("planning-{goal_id}"),
            task_id,
            goal_id,
            &proposal,
        )
        .expect("materialize Ready Task");

    let requested = RequestedBase::parse("main").expect("fixture ref");
    let resolved = ResolvedBase::parse(&"a".repeat(40)).expect("fixture base");
    let binding = pantheon_store::WorkspaceBinding {
        task_id,
        repository: "repo://project",
        source_path: "/tmp/pantheon-scheduling-test-source",
        requested_base: &requested,
        resolved_base: &resolved,
    };
    // Command identities are derived per Workspace so a second fixture in
    // the same epoch is a new command rather than a replay of the first.
    let open_id = format!("ws-open-{workspace_id}");
    let begin_id = format!("ws-begin-{workspace_id}");
    let complete_id = format!("ws-complete-{workspace_id}");
    store
        .open_workspace(
            &command(epoch.as_str(), &open_id, &[7u8; 32], "workspace.opened"),
            workspace_id,
            &binding,
        )
        .expect("open workspace");
    store
        .begin_workspace_materialization(
            &command(
                epoch.as_str(),
                &begin_id,
                &[8u8; 32],
                "workspace.materializing",
            ),
            workspace_id,
            pantheon_store::Revision::new(1),
        )
        .expect("begin materialization");
    store
        .complete_workspace_materialization(
            &command(epoch.as_str(), &complete_id, &[9u8; 32], "workspace.ready"),
            workspace_id,
            pantheon_store::Revision::new(2),
            &resolved,
        )
        .expect("complete materialization");
}

#[test]
fn a_ready_task_with_a_feasible_route_commits_exactly_one_run_intent() {
    let dir = TempDir::new("sched-commit");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = load_authority(&store);
    ready_task_with_workspace(&store, "goal-1", "task-1", "ws-1");
    let backend = FakeBackend::new("task-1");

    let outcome = SchedulingController::new(&store, &authority)
        .schedule_once(&[backend.port()])
        .expect("the cycle runs");
    let ScheduleOutcome::Committed { run_id, task_id } = outcome else {
        panic!("expected a committed Run intent, got {outcome:?}");
    };
    assert_eq!(task_id, "task-1");
    assert!(run_id.starts_with("run-"));

    // The Task is Active because a durable Run owns responsibility.
    let tasks = store
        .goal_detail("goal-1")
        .expect("read")
        .expect("goal")
        .tasks;
    assert_eq!(tasks[0].phase, pantheon_core::planning::TaskPhase::Active);
    assert_eq!(tasks[0].active_run_id.as_deref(), Some(run_id.as_str()));

    // The slot is durably held; the next cycle is suppressed by it.
    assert_eq!(
        store.slot_holder().expect("slot"),
        Some((run_id.clone(), "task-1".to_string()))
    );
    let next = SchedulingController::new(&store, &authority)
        .schedule_once(&[backend.port()])
        .expect("the second cycle runs");
    assert_eq!(
        next,
        ScheduleOutcome::Suppressed(Suppression::SlotHeld),
        "the single slot suppresses further selection"
    );

    // Fairness was charged exactly once, atomically with the commit.
    let state = store.scheduler_state().expect("state");
    assert_eq!(state.next_service_sequence, 2);
}

#[test]
fn t3_makes_no_backend_contact_beyond_side_effect_free_offers() {
    // The structural half of this property is the port itself: ExecutorBackend
    // exposes describe/offer and nothing else, so a scheduling cycle cannot
    // launch even by mistake. The behavioural half is that offer generation
    // happened exactly once — during routing — and never again for the
    // commit or the suppressed follow-up cycle.
    let dir = TempDir::new("sched-no-contact");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = load_authority(&store);
    ready_task_with_workspace(&store, "goal-1", "task-1", "ws-1");
    let backend = FakeBackend::new("task-1");
    let controller = SchedulingController::new(&store, &authority);

    let outcome = controller
        .schedule_once(&[backend.port()])
        .expect("cycle runs");
    assert!(matches!(outcome, ScheduleOutcome::Committed { .. }));
    assert_eq!(
        backend.offer_calls.get(),
        1,
        "offers are requested once per routing attempt"
    );

    // The suppressed follow-up performs no backend work at all.
    let _ = controller.schedule_once(&[backend.port()]);
    assert_eq!(backend.offer_calls.get(), 1);

    // No Attempt/LaunchKey surface exists at all in this schema generation:
    // the store's schema guard pins that, and the Run's status row is Active
    // with nothing to launch.
    assert!(store.slot_holder().expect("slot").is_some());
}

#[test]
fn a_routing_failure_defers_without_charging_fairness_or_holding_the_slot() {
    let dir = TempDir::new("sched-defer");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = load_authority(&store);
    ready_task_with_workspace(&store, "goal-1", "task-1", "ws-1");
    // The only backend refuses this Task, so routing finds no candidate.
    let backend = FakeBackend::new("some-other-task");

    let before = store.scheduler_state().expect("state");
    let outcome = SchedulingController::new(&store, &authority)
        .schedule_once(&[backend.port()])
        .expect("the cycle runs");
    let ScheduleOutcome::Deferred { task_id, reason } = outcome else {
        panic!("expected a deferral, got {outcome:?}");
    };
    assert_eq!(task_id, "task-1");
    assert_eq!(reason, "no-compatible-offer");

    // No fairness charge, no scheduler advance, no slot, no Run.
    let after = store.scheduler_state().expect("state");
    assert_eq!(after.next_service_sequence, before.next_service_sequence);
    assert_eq!(after.revision, before.revision);
    assert!(store.slot_holder().expect("slot").is_none());
    let tasks = store
        .goal_detail("goal-1")
        .expect("read")
        .expect("goal")
        .tasks;
    assert_eq!(tasks[0].phase, pantheon_core::planning::TaskPhase::Ready);

    // Durable backoff suppresses the next cycle's selection without making
    // the Task ineligible. The exact stored fields are pinned by the
    // pantheon-store scheduling tests; here the observable outcome is what
    // matters.
    let snap = store.scheduling_snapshot().expect("snapshot");
    assert!(snap.candidates.is_empty(), "backoff suppresses selection");
}

#[test]
fn fairness_is_charged_only_when_a_run_commits() {
    // Two Goals, two Tasks. The first Goal's Task cannot be routed; the
    // second's can. Ordering picks goal-1 first (stable id), it fails, and
    // goal-2 must still be served in the same cycle — with goal-1 charged
    // nothing.
    let dir = TempDir::new("sched-fairness");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = load_authority(&store);
    ready_task_with_workspace(&store, "goal-a", "task-a", "ws-a");
    ready_task_with_workspace(&store, "goal-b", "task-b", "ws-b");
    let backend = FakeBackend::new("task-b");

    let outcome = SchedulingController::new(&store, &authority)
        .schedule_once(&[backend.port()])
        .expect("the cycle runs");
    let ScheduleOutcome::Committed { task_id, .. } = outcome else {
        panic!("expected goal-b's Task to be served, got {outcome:?}");
    };
    assert_eq!(
        task_id, "task-b",
        "a deferred older Task never blocks the queue"
    );

    let state = store.scheduler_state().expect("state");
    assert_eq!(state.next_service_sequence, 2, "exactly one service charge");
    let snap = store.scheduling_snapshot().expect("snapshot");
    let charged: Vec<_> = snap
        .goals
        .iter()
        .filter(|row| row.last_served_sequence.is_some())
        .collect();
    assert_eq!(charged.len(), 1);
    assert_eq!(charged[0].goal_id, "goal-b");
}

#[test]
fn operator_pause_suppresses_selection_and_preserves_waiting_age() {
    let dir = TempDir::new("sched-pause");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = load_authority(&store);
    ready_task_with_workspace(&store, "goal-1", "task-1", "ws-1");
    let backend = FakeBackend::new("task-1");
    let controller = SchedulingController::new(&store, &authority);

    let eligible_since =
        store.scheduling_snapshot().expect("snapshot").candidates[0].eligible_since;

    // Pause through the same command path the Operator surface uses.
    let state = store.scheduler_state().expect("state");
    let epoch = store.restore_generation().expect("generation");
    store
        .set_dispatch_mode(
            &command(epoch.as_str(), "pause-1", &[31u8; 32], "dispatch.paused"),
            DispatchMode::Paused,
            state.revision,
        )
        .expect("pause commits");

    let outcome = controller
        .schedule_once(&[backend.port()])
        .expect("cycle runs");
    assert_eq!(
        outcome,
        ScheduleOutcome::Suppressed(Suppression::OperatorPause)
    );
    assert_eq!(
        backend.offer_calls.get(),
        0,
        "a paused scheduler routes nothing"
    );

    // Suppression is not ineligibility: the interval survives untouched.
    let snap = store.scheduling_snapshot().expect("snapshot");
    assert_eq!(snap.candidates.len(), 1);
    assert_eq!(snap.candidates[0].eligible_since, eligible_since);

    // Resume re-opens dispatch and the very next cycle commits.
    let state = store.scheduler_state().expect("state");
    store
        .set_dispatch_mode(
            &command(epoch.as_str(), "resume-1", &[32u8; 32], "dispatch.resumed"),
            DispatchMode::Running,
            state.revision,
        )
        .expect("resume commits");
    let outcome = controller
        .schedule_once(&[backend.port()])
        .expect("cycle runs");
    assert!(matches!(outcome, ScheduleOutcome::Committed { .. }));
}
