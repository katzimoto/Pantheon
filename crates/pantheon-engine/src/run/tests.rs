//! Evidence for Issue #31's control-flow claims: preparation gates, the
//! T4/T4a/T4b sequence, crash windows around them, lost acknowledgements,
//! UNKNOWN fencing, deliberate retry and frozen semantics.
//!
//! Crash windows are simulated exactly and deterministically: dropping a
//! [`RunController`] destroys its process-local bearer memory and nothing
//! else, which is precisely what a daemon crash destroys. The fake backend's
//! external world lives behind an `Arc` that survives the "crash", so the
//! restarted controller faces the same external reality the crashed one did.
//! No test depends on timing or sleeps.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pantheon_core::attempt::Observation;
use pantheon_core::config::Digest;
use pantheon_core::execution::{
    BackendDescriptor, ControllerSafetyFacts, ExecutionOffer, ExecutionRequest, LaunchSemantics,
};
use pantheon_core::planning::direct;
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::workspace::{RequestedBase, ResolvedBase};
use pantheon_store::{Command, Revision, Store};

use super::{
    ExecutionLauncher, LaunchPackage, LauncherFailure, MinRecoveryPolicy, RandomBytes,
    RandomFailure, RunController, RunOutcome, SandboxCheck, SandboxReadiness,
};
use crate::configuration::{ConfigurationAuthority, SourceSet};
use crate::routing::{ExecutorBackend, ExecutorBackendPort};
use crate::scheduling::{ScheduleOutcome, SchedulingController};

const SOUL_V1: &str = "Careful coding agent identity.";
const BEHAVIOR_V1: &str = "Plan first; keep changes minimal.";
const SOUL_V2: &str = "A different later identity.";

// ---------------------------------------------------------------------------
// Fixture world
// ---------------------------------------------------------------------------

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-run-test-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.0.join("pantheon.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

fn configuration_source(soul: &str, behavior: &str) -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"enabled":true,"current":true,
    "accepts":["code.change"],"competencies":["code.analysis","code.editing","test.execution"],"routePolicy":"default",
    "executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"],
    "soul":"{soul}","behavior":"{behavior}"}}],
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
  "context": {{"schemaVersion":1,
    "mandatorySections":["task-contract","goal-contract","agent-soul","agent-behavior"],
    "preloadPriority":["workspace-orientation"],"memoryLimitTokens":4000,
    "workspaceOrientationLimitTokens":2000,"safetyMarginTokens":512,
    "optionalDropOrder":["workspace-orientation"]}},
  "authorization": {{"schemaVersion":1,"rules":[
    {{"action":"filesystem.read","effect":"permit"}}
  ]}}
}}"#,
        evaluator_ref = direct::MVP_EVALUATOR_REF,
    )
}

/// Offers for exactly one Task so the fixture commits exactly one Run.
struct RoutingBackend;

impl ExecutorBackend for RoutingBackend {
    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            backend_id: "fake-local".to_string(),
            revision: 3,
            available_for_offers: true,
            placement: vec![],
            supported_execution_features: vec!["exec.shell".to_string()],
            context_capacity_tokens: 32_000,
            isolation_facts: vec!["isolation.control-plane".to_string()],
            resources: vec![],
            launch_semantics: LaunchSemantics::KeyedIdempotent,
        }
    }

    fn offer(
        &self,
        request: &ExecutionRequest,
    ) -> Result<Vec<ExecutionOffer>, crate::routing::BackendError> {
        if request.task_id != "task-1" {
            return Ok(Vec::new());
        }
        Ok(vec![ExecutionOffer {
            request_digest: request.digest(),
            backend_id: "fake-local".to_string(),
            descriptor_revision: 3,
            descriptor_digest: self.describe().digest(),
            supported_execution_features: vec!["exec.shell".to_string()],
            context_capacity_tokens: 32_000,
            placement: vec![],
            isolation_facts: vec!["isolation.control-plane".to_string()],
            resources: vec![],
            launch_semantics: LaunchSemantics::KeyedIdempotent,
            offer_reference: "fake-offer".to_string(),
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

/// Drives the Goal-to-Ready-Task path and materializes the Workspace.
fn ready_task_with_workspace(store: &Store) {
    let planning = crate::planning::PlanningController::new(store);
    let spec = goal_spec();
    let epoch = store.restore_generation().expect("generation");
    planning
        .create_goal(
            &command(epoch.as_str(), "goal-cmd", &[2u8; 32], "goal.created"),
            "goal-1",
            &spec,
        )
        .expect("create Goal");
    planning
        .plan(
            &command(epoch.as_str(), "plan-cmd", &[3u8; 32], "planning.recorded"),
            "planning-goal-1",
            "goal-1",
        )
        .expect("record plan");
    let proposal = planning.proposal("goal-1").expect("re-derive proposal");
    planning
        .materialize(
            &command(epoch.as_str(), "graph-cmd", &[4u8; 32], "graph.patched"),
            "planning-goal-1",
            "task-1",
            "goal-1",
            &proposal,
        )
        .expect("materialize Ready Task");

    let requested = RequestedBase::parse("main").expect("fixture ref");
    let resolved = ResolvedBase::parse(&"a".repeat(40)).expect("fixture base");
    let binding = pantheon_store::WorkspaceBinding {
        task_id: "task-1",
        repository: "repo://project",
        source_path: "/tmp/pantheon-run-test-source",
        requested_base: &requested,
        resolved_base: &resolved,
    };
    store
        .open_workspace(
            &command(epoch.as_str(), "ws-open", &[7u8; 32], "workspace.opened"),
            "ws-1",
            &binding,
        )
        .expect("open workspace");
    store
        .begin_workspace_materialization(
            &command(
                epoch.as_str(),
                "ws-begin",
                &[8u8; 32],
                "workspace.materializing",
            ),
            "ws-1",
            Revision::new(1),
        )
        .expect("begin materialization");
    store
        .complete_workspace_materialization(
            &command(epoch.as_str(), "ws-done", &[9u8; 32], "workspace.ready"),
            "ws-1",
            Revision::new(2),
            &resolved,
        )
        .expect("complete materialization");
}

/// One installation: durable state plus the Run the scheduler committed.
struct World {
    dir: TempDir,
    store: Option<Store>,
    run_id: String,
}

impl World {
    fn s(&self) -> &Store {
        self.store.as_ref().expect("store open")
    }

    fn close_store(&mut self) {
        if let Some(store) = self.store.take() {
            store.close().expect("close store");
        }
    }

    /// Reopens the store exactly as a daemon restart would.
    fn reopen(&mut self) {
        self.close_store();
        self.store = Some(Store::open(self.dir.db_path()).expect("reopen store"));
    }

    /// Reads the whole database file's bytes, for secrecy scans.
    fn db_bytes(&self) -> Vec<u8> {
        std::fs::read(self.dir.db_path()).expect("read database file")
    }
}

/// An installation whose configuration is activated and whose single coding
/// Task is Ready owning a verified Workspace, with `commit` deciding how the
/// Run intent comes into being and which Run identity it produced.
///
/// The same activation already happened through one
/// [`ConfigurationAuthority`] instance; a fresh authority carries no
/// process-local snapshot, which is exactly the "nothing published" state a
/// second instance would wrongly report.
fn world_with_run(
    label: &str,
    commit: impl FnOnce(&ConfigurationAuthority<&Store>, &Store) -> String,
) -> World {
    let dir = TempDir::new(label);
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    authority
        .activate(
            &command(
                epoch.as_str(),
                "cfg-1",
                &[1u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single(
                "configuration.json",
                configuration_source(SOUL_V1, BEHAVIOR_V1),
            ),
        )
        .expect("activate configuration");
    ready_task_with_workspace(&store);
    let run_id = commit(&authority, &store);
    World {
        dir,
        store: Some(store),
        run_id,
    }
}

fn committed_world(label: &str) -> World {
    world_with_run(label, |authority, store| {
        let outcome = SchedulingController::new(store, authority)
            .schedule_once(&[ExecutorBackendPort::new(
                &RoutingBackend,
                ControllerSafetyFacts {
                    isolation_guarantees: vec!["isolation.control-plane".to_string()],
                    observational_launch_safe: false,
                },
            )])
            .expect("the cycle runs");
        let ScheduleOutcome::Committed { run_id, .. } = outcome else {
            panic!("expected a committed Run intent, got {outcome:?}");
        };
        run_id
    })
}

// ---------------------------------------------------------------------------
// Deterministic entropy
// ---------------------------------------------------------------------------

/// Counter-based deterministic entropy. Distinct output per draw, stable per
/// seed, nothing like unpredictable — which is why production composes
/// [`super::OsRandom`] instead.
#[derive(Debug)]
struct FixedRandom(AtomicU64);

impl FixedRandom {
    fn new(seed: u64) -> Self {
        Self(AtomicU64::new(seed))
    }
}

impl RandomBytes for FixedRandom {
    fn fill(&self, dest: &mut [u8]) -> Result<(), RandomFailure> {
        let value = self.0.fetch_add(1, Ordering::SeqCst);
        for (index, byte) in dest.iter_mut().enumerate() {
            *byte = (value.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> ((index % 8) * 8)) as u8;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The fake external execution world (survives controller crashes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineageState {
    Starting,
    Running,
    Exited,
}

#[derive(Debug, Default)]
struct ExternalWorld {
    /// Keyed-idempotent truth: one logical lineage per LaunchKey.
    lineages: BTreeMap<String, LineageState>,
    /// Every ensureExecution contact, with its delivered credential.
    contacts: Vec<(String, String)>,
    /// When set, ensure creates the lineage but Pantheon loses the ack.
    lose_acknowledgements: bool,
    /// When set, ensure fails before the backend receives anything at all.
    drop_calls: bool,
    /// When set, inspection cannot establish existence.
    unobservable: bool,
}

impl ExternalWorld {
    fn advance(&self, launch_key: &str) -> LineageState {
        let state = self.lineages.get(launch_key).copied();
        match state {
            Some(LineageState::Starting) => LineageState::Running,
            Some(LineageState::Running) => LineageState::Exited,
            other => other.unwrap_or(LineageState::Exited),
        }
    }
}

#[derive(Debug)]
struct FakeLauncher {
    world: Arc<Mutex<ExternalWorld>>,
}

impl FakeLauncher {
    fn new(world: &Arc<Mutex<ExternalWorld>>) -> Self {
        Self {
            world: Arc::clone(world),
        }
    }

    fn set_observation(&self, launch_key: &str, observation: Observation) {
        let state = match observation {
            Observation::Starting => LineageState::Starting,
            Observation::Running => LineageState::Running,
            Observation::Exited => LineageState::Exited,
            Observation::Absent => {
                self.world
                    .lock()
                    .expect("world")
                    .lineages
                    .remove(launch_key);
                return;
            }
            Observation::Unknown => unreachable!("UNKNOWN is not a settable lineage state"),
        };
        self.world
            .lock()
            .expect("world")
            .lineages
            .insert(launch_key.to_string(), state);
    }

    fn observation_of(&self, launch_key: &str) -> Option<LineageState> {
        self.world
            .lock()
            .expect("world")
            .lineages
            .get(launch_key)
            .copied()
    }

    fn contacts(&self) -> Vec<(String, String)> {
        self.world.lock().expect("world").contacts.clone()
    }
}

impl ExecutionLauncher for FakeLauncher {
    fn backend_id(&self) -> &str {
        "fake-local"
    }

    fn launch_semantics(&self) -> LaunchSemantics {
        LaunchSemantics::KeyedIdempotent
    }

    fn ensure_execution(
        &self,
        package: &LaunchPackage<'_>,
    ) -> Result<Observation, LauncherFailure> {
        let mut world = self.world.lock().expect("world");
        if world.drop_calls {
            // The call never reached the backend.
            return Err(injected_failure());
        }
        world
            .lineages
            .entry(package.launch_key.to_string())
            .or_insert(LineageState::Starting);
        world.contacts.push((
            package.launch_key.to_string(),
            package.credential.expose().to_string(),
        ));
        if world.lose_acknowledgements {
            // The lineage exists externally; Pantheon just never hears it.
            return Err(injected_failure());
        }
        Ok(Observation::Starting)
    }

    fn inspect_execution(&self, launch_key: &str) -> Result<Observation, LauncherFailure> {
        let mut world = self.world.lock().expect("world");
        if world.unobservable {
            return Err(injected_failure());
        }
        let next = world.advance(launch_key);
        if let Some(state) = world.lineages.get_mut(launch_key) {
            *state = next;
        }
        Ok(match next {
            LineageState::Starting => Observation::Starting,
            LineageState::Running => Observation::Running,
            LineageState::Exited => Observation::Exited,
        })
    }
}

fn injected_failure() -> LauncherFailure {
    LauncherFailure {
        detail: "injected fault".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Fake Sandbox (test infrastructure only)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FakeSandbox {
    refuses: bool,
}

impl SandboxReadiness for FakeSandbox {
    fn verify_ready(&self, _check: SandboxCheck<'_>) -> Result<(), String> {
        if self.refuses {
            Err("injected sandbox failure".to_string())
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Controller assembly helpers
// ---------------------------------------------------------------------------

struct Harness {
    launcher: FakeLauncher,
    sandbox: FakeSandbox,
    policy: MinRecoveryPolicy,
}

impl Harness {
    fn healthy(world: &Arc<Mutex<ExternalWorld>>) -> Self {
        Self {
            launcher: FakeLauncher::new(world),
            sandbox: FakeSandbox { refuses: false },
            policy: MinRecoveryPolicy::default(),
        }
    }

    fn deps(&self) -> super::ReconciliationDeps<'_> {
        super::ReconciliationDeps {
            launcher: &self.launcher,
            sandbox: &self.sandbox,
            policy: &self.policy,
        }
    }
}

fn controller<'s>(store: &'s Store, seed: u64) -> RunController<'s, FixedRandom> {
    RunController::new(store, FixedRandom::new(seed), "test-incarnation")
}

#[test]
fn scheduling_alone_creates_zero_attempts_and_zero_contact() {
    let world = committed_world("boundary-scheduler");
    let external = Arc::new(Mutex::new(ExternalWorld::default()));

    // committed_world already ran schedule_once: T3 is durable.
    let view = world
        .s()
        .run_execution_view(&world.run_id)
        .expect("read")
        .expect("the Run exists");
    assert_eq!(view.phase, "Active");
    assert!(
        view.attempt.is_none(),
        "no Attempt exists before the Run Controller"
    );
    assert!(
        external.lock().expect("world").contacts.is_empty(),
        "the Scheduler never contacts a backend"
    );
}

#[test]
fn a_preparation_failure_concludes_the_run_with_zero_attempts() {
    let world = committed_world("prep-failure");
    let external = Arc::new(Mutex::new(ExternalWorld::default()));
    let harness = Harness {
        sandbox: FakeSandbox { refuses: true },
        ..Harness::healthy(&external)
    };

    let outcome = controller(world.s(), 1)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("reconcile");
    assert!(
        matches!(outcome, RunOutcome::ConcludedInPreparation { .. }),
        "got {outcome:?}"
    );

    // Zero lineage rows anywhere, zero backend contact, slot released.
    let view = world
        .s()
        .run_execution_view(&world.run_id)
        .expect("read")
        .expect("the Run exists");
    assert_eq!(view.phase, "Failed");
    assert_eq!(view.current_attempt_id, None);
    assert_eq!(view.attempt, None);
    assert!(
        world
            .s()
            .nonterminal_run_inventory()
            .expect("inventory")
            .is_empty(),
        "a concluded Run leaves the inventory and releases the slot"
    );
    assert!(external.lock().expect("world").contacts.is_empty());
    assert_eq!(
        world
            .s()
            .attempt_history_count(&world.run_id)
            .expect("count"),
        0
    );
}

/// Pass 1 establishes the lineage; a later pass crosses the contact boundary.
fn established(world: &World, external: &Arc<Mutex<ExternalWorld>>) -> RunOutcome {
    let harness = Harness::healthy(external);
    controller(world.s(), 1)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("establish")
}

fn live_lineage(world: &World) -> pantheon_store::AttemptLineageView {
    world
        .s()
        .run_execution_view(&world.run_id)
        .expect("read")
        .expect("Run exists")
        .attempt
        .expect("live lineage")
}

#[test]
fn t4_establishes_without_contact_and_launch_crosses_the_boundary_in_order() {
    let world = committed_world("t4-t4b-order");
    let external = Arc::new(Mutex::new(ExternalWorld::default()));

    let outcome = established(&world, &external);
    let RunOutcome::AttemptEstablished {
        attempt_id,
        launch_key,
    } = &outcome
    else {
        panic!("pass one establishes, got {outcome:?}");
    };
    assert!(
        external.lock().expect("world").contacts.is_empty(),
        "T4 alone never contacts the backend"
    );
    let live = live_lineage(&world);
    assert_eq!(&live.attempt.id, attempt_id);
    assert_eq!(&live.attempt.launch_key, launch_key);
    assert_eq!(
        live.launch_contact_state,
        super::LaunchContactState::NotContacted
    );
    assert_eq!(live.session.credential_revision, 1);

    // Pass two: T4b commits the boundary, then ensure contacts the backend.
    let harness = Harness::healthy(&external);
    let outcome = controller(world.s(), 2)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("launch");
    let RunOutcome::Launched { observation, .. } = &outcome else {
        panic!("pass two launches, got {outcome:?}");
    };
    assert_eq!(*observation, Observation::Starting);

    let launcher = FakeLauncher::new(&external);
    let contacts = launcher.contacts();
    assert_eq!(contacts.len(), 1, "exactly one keyed contact");
    assert_eq!(contacts[0].0, *launch_key);

    // The durable boundary was crossed before that single contact.
    let live = live_lineage(&world);
    assert_eq!(
        live.launch_contact_state,
        super::LaunchContactState::ContactMayHaveOccurred
    );
}

#[test]
fn the_delivered_credential_is_exactly_the_current_revision() {
    // FixedRandom(9) draws its first 32 bytes for the rekey bearer; this
    // replica predicts exactly what incarnation B will deliver.
    let mut expected = [0u8; 32];
    FixedRandom::new(9).fill(&mut expected).expect("draw");
    let expected_hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();

    let mut world = committed_world("credential-link");
    let external = Arc::new(Mutex::new(ExternalWorld::default()));
    let _ = established(&world, &external);
    world.reopen(); // bearer memory lost

    let harness = Harness::healthy(&external);
    controller(world.s(), 9)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("restart reconcile");

    let contacts = FakeLauncher::new(&external).contacts();
    assert_eq!(contacts.len(), 1);
    assert_eq!(
        contacts[0].1, expected_hex,
        "delivery carried the rekeyed bearer, not stale material"
    );
}

#[test]
fn a_crash_after_t4_before_contact_rekeys_the_same_session_and_continues() {
    let mut world = committed_world("crash-precontact");
    let external = Arc::new(Mutex::new(ExternalWorld::default()));

    // Incarnation A establishes the lineage and dies with its memory.
    let outcome = established(&world, &external);
    let RunOutcome::AttemptEstablished { launch_key, .. } = &outcome else {
        panic!("got {outcome:?}");
    };
    let original_key = launch_key.clone();
    let original_session = live_lineage(&world).session.id;

    // Incarnation B reconstructs over the same durable store and fake
    // external world. Bearer memory is gone by construction.
    world.reopen();
    let harness = Harness::healthy(&external);
    let outcome = controller(world.s(), 9)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("restart reconcile");
    let RunOutcome::Launched { attempt_id, .. } = outcome else {
        panic!("the restarted controller continues, got {outcome:?}");
    };

    // Same Attempt identity, same LaunchKey, same session — rotated only in
    // credential revision through T4a, never a second lineage.
    let live = live_lineage(&world);
    assert_eq!(live.attempt.id, attempt_id);
    assert_eq!(live.attempt.launch_key, original_key);
    assert_eq!(live.session.id, original_session);
    assert_eq!(live.session.credential_revision, 2, "T4a rotated once");
    assert_eq!(
        world
            .s()
            .attempt_history_count(&world.run_id)
            .expect("count"),
        1,
        "no second creation"
    );
}

#[test]
fn losing_the_bearer_after_contact_never_rekeys() {
    let world = committed_world("postcontact-no-rekey");
    let external = Arc::new(Mutex::new(ExternalWorld {
        unobservable: true,
        ..ExternalWorld::default()
    }));

    // Establish and cross the boundary with incarnation A, which then dies.
    {
        let harness = Harness::healthy(&external);
        let mut a = controller(world.s(), 1);
        a.reconcile_run(&world.run_id, &harness.deps())
            .expect("establish");
        let launched = a
            .reconcile_run(&world.run_id, &harness.deps())
            .expect("launch");
        assert!(matches!(launched, RunOutcome::Launched { .. }));
    }

    // Incarnation B has no bearer memory and an unobservable backend:
    // reconciliation must fence, never rotate the frozen credential.
    let harness = Harness::healthy(&external);
    let outcome = controller(world.s(), 8)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("reconcile");
    assert!(
        matches!(outcome, RunOutcome::UnknownFenced { .. }),
        "unobservable post-contact state fences, got {outcome:?}"
    );

    let live = live_lineage(&world);
    assert_eq!(live.session.credential_revision, 1, "frozen after contact");
    assert_eq!(live.observed_execution, Observation::Unknown);
    assert!(!live.terminal, "UNKNOWN keeps the Attempt nonterminal");
    assert_eq!(
        world
            .s()
            .attempt_history_count(&world.run_id)
            .expect("count"),
        1,
        "no replacement lineage"
    );
}

#[test]
fn unknown_fences_repeatedly_without_release_or_replacement() {
    let world = committed_world("unknown-loop");
    let external = Arc::new(Mutex::new(ExternalWorld {
        unobservable: true,
        ..ExternalWorld::default()
    }));

    {
        let harness = Harness::healthy(&external);
        let mut a = controller(world.s(), 1);
        a.reconcile_run(&world.run_id, &harness.deps())
            .expect("establish");
        a.reconcile_run(&world.run_id, &harness.deps())
            .expect("launch");
    }

    // Repeated reconciliations while UNKNOWN: every pass fences identically.
    let harness = Harness::healthy(&external);
    let mut b = controller(world.s(), 3);
    for _ in 0..3 {
        let outcome = b
            .reconcile_run(&world.run_id, &harness.deps())
            .expect("fence");
        assert!(matches!(outcome, RunOutcome::UnknownFenced { .. }));
    }

    assert_eq!(FakeLauncher::new(&external).contacts().len(), 1);
    assert_eq!(
        world
            .s()
            .attempt_history_count(&world.run_id)
            .expect("count"),
        1,
        "UNKNOWN authorizes no replacement Attempt"
    );
    assert_eq!(
        world.s().slot_holder().expect("slot").map(|(run, _)| run),
        Some(world.run_id.clone()),
        "the single execution slot stays retained/fenced"
    );
}

#[test]
fn a_lost_acknowledgement_continues_one_external_lineage_after_restart() {
    let world = committed_world("lost-ack");
    let external = Arc::new(Mutex::new(ExternalWorld {
        lose_acknowledgements: true,
        ..ExternalWorld::default()
    }));

    {
        let harness = Harness::healthy(&external);
        let mut a = controller(world.s(), 1);
        a.reconcile_run(&world.run_id, &harness.deps())
            .expect("establish");
        let launched = a
            .reconcile_run(&world.run_id, &harness.deps())
            .expect("launch");
        let RunOutcome::Launched { observation, .. } = launched else {
            panic!("got {launched:?}");
        };
        assert_eq!(
            observation,
            Observation::Unknown,
            "lost ack is conservative ambiguity, not failure"
        );
    }

    // The backend DID receive the call and holds exactly one lineage.
    let key = {
        let launcher = FakeLauncher::new(&external);
        assert_eq!(launcher.contacts().len(), 1);
        launcher.contacts()[0].0.clone()
    };

    // Restart: inspection proves the lineage alive; the SAME lineage continues.
    let harness = Harness::healthy(&external);
    let outcome = controller(world.s(), 5)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("reconcile");
    let RunOutcome::Reconciled { observation, .. } = outcome else {
        panic!("got {outcome:?}");
    };
    assert_eq!(observation, Observation::Running);

    let launcher = FakeLauncher::new(&external);
    assert_eq!(
        launcher.contacts().len(),
        1,
        "no duplicate ensureExecution for the same key"
    );
    assert_eq!(launcher.observation_of(&key), Some(LineageState::Running));
    assert_eq!(
        world
            .s()
            .attempt_history_count(&world.run_id)
            .expect("count"),
        1
    );
}

#[test]
fn an_ambiguous_pre_delivery_crash_never_infers_safe_replacement() {
    let world = committed_world("pre-delivery");
    let external = Arc::new(Mutex::new(ExternalWorld {
        drop_calls: true,
        ..ExternalWorld::default()
    }));

    {
        let harness = Harness::healthy(&external);
        let mut a = controller(world.s(), 1);
        a.reconcile_run(&world.run_id, &harness.deps())
            .expect("establish");
        let launched = a
            .reconcile_run(&world.run_id, &harness.deps())
            .expect("launch");
        assert!(matches!(
            launched,
            RunOutcome::Launched {
                observation: Observation::Unknown,
                ..
            }
        ));
    }

    // T4b is durable even though nothing was ever delivered...
    let live = live_lineage(&world);
    assert_eq!(
        live.launch_contact_state,
        super::LaunchContactState::ContactMayHaveOccurred
    );

    // ...and only a definitive backend proof of absence may end the lineage.
    // The default policy permits no retry, so the Run concludes Failed rather
    // than quietly minting replacement execution authority.
    let harness = Harness::healthy(&external);
    let outcome = controller(world.s(), 6)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("reconcile");
    assert!(
        matches!(outcome, RunOutcome::ConcludedAfterFailure { .. }),
        "absence proven and retries exhausted, got {outcome:?}"
    );

    assert!(
        FakeLauncher::new(&external).contacts().is_empty(),
        "the dropped call never reached the backend"
    );
    assert_eq!(
        world
            .s()
            .attempt_history_count(&world.run_id)
            .expect("count"),
        1,
        "exactly one lineage existed, ever"
    );
    let view = world
        .s()
        .run_execution_view(&world.run_id)
        .expect("read")
        .expect("Run exists");
    assert_eq!(view.phase, "Failed");
}

#[test]
fn an_intentional_retry_gets_new_identity_under_the_unchanged_run() {
    let mut world = committed_world("retry-deliberate");
    let external = Arc::new(Mutex::new(ExternalWorld::default()));
    let harness = Harness {
        policy: MinRecoveryPolicy {
            max_attempts_per_run: 2,
        },
        ..Harness::healthy(&external)
    };

    let binding_before = world
        .s()
        .run_execution_view(&world.run_id)
        .expect("read")
        .unwrap()
        .binding_digest;
    let plan_before = loop {
        let harness_deps = harness.deps();
        let outcome = controller(world.s(), 1)
            .reconcile_run(&world.run_id, &harness_deps)
            .expect("establish");
        if let RunOutcome::AttemptEstablished { attempt_id, .. } = &outcome {
            break (
                attempt_id.clone(),
                world
                    .s()
                    .run_execution_view(&world.run_id)
                    .expect("read")
                    .unwrap()
                    .context_plan_digest,
            );
        }
    };
    let (first_attempt, plan_digest) = plan_before;
    assert!(plan_digest.is_some(), "ContextReady is a T4 precondition");

    // Launch, then prove the lineage definitively ended.
    controller(world.s(), 2)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("launch");
    FakeLauncher::new(&external).set_observation(
        &live_lineage(&world).attempt.launch_key,
        Observation::Exited,
    );
    let outcome = controller(world.s(), 3)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("decide");
    let RunOutcome::RetryArmed {
        attempt_id: ended,
        next_ordinal,
    } = outcome
    else {
        panic!("policy arms one deliberate retry, got {outcome:?}");
    };
    assert_eq!(ended, first_attempt);
    assert_eq!(next_ordinal, 2);

    // The deliberate retry creates a genuinely new lineage under the same
    // immutable Binding and attached ContextPlan.
    world.reopen(); // the retrying incarnation starts fresh
    let harness = Harness {
        policy: MinRecoveryPolicy {
            max_attempts_per_run: 2,
        },
        ..Harness::healthy(&external)
    };
    let outcome = controller(world.s(), 4)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("retry");
    let RunOutcome::AttemptEstablished {
        attempt_id,
        launch_key,
    } = outcome
    else {
        panic!("got {outcome:?}");
    };
    assert_ne!(attempt_id, first_attempt);
    assert_eq!(
        world
            .s()
            .attempt_history_count(&world.run_id)
            .expect("count"),
        2
    );

    let view = world
        .s()
        .run_execution_view(&world.run_id)
        .expect("read")
        .unwrap();
    assert_eq!(view.binding_digest, binding_before, "Binding never changes");
    assert_eq!(
        view.context_plan_digest, plan_digest,
        "the attached plan never changes"
    );
    let live = view.attempt.expect("new lineage current");
    assert_eq!(&live.attempt.id, &attempt_id);
    assert_eq!(&live.attempt.launch_key, &launch_key);
    assert_eq!(live.attempt.ordinal, 2);
}

#[test]
fn frozen_semantics_survive_a_newer_active_configuration() {
    // Reference world: everything under configuration v1, end to end.
    let reference = committed_world("frozen-reference");
    let external_ref = Arc::new(Mutex::new(ExternalWorld::default()));
    established(&reference, &external_ref);
    let reference_plan = reference
        .s()
        .run_execution_view(&reference.run_id)
        .expect("read")
        .unwrap()
        .context_plan_digest;

    // Drifted world: T3 commits under v1, then v2 activates BEFORE any
    // preparation. The Run's semantics must stay exactly what T3 froze.
    let drifted = committed_world("frozen-drift");
    {
        let store = drifted.s();
        let authority = ConfigurationAuthority::new(store);
        let epoch = store.restore_generation().expect("generation");
        authority
            .activate(
                &command(
                    epoch.as_str(),
                    "cfg-2",
                    &[2u8; 32],
                    "configuration.activated",
                ),
                &SourceSet::single(
                    "configuration.json",
                    configuration_source(SOUL_V2, BEHAVIOR_V1),
                ),
            )
            .expect("activate v2");
    }

    let external_drift = Arc::new(Mutex::new(ExternalWorld::default()));
    let outcome = established(&drifted, &external_drift);
    assert!(
        matches!(outcome, RunOutcome::AttemptEstablished { .. }),
        "preparation still reaches LaunchReady from frozen sources"
    );

    let view = drifted
        .s()
        .run_execution_view(&drifted.run_id)
        .expect("read")
        .unwrap();
    assert_eq!(
        view.context_plan_digest, reference_plan,
        "the newer active revision cannot substitute different guidance into \
         the existing Run"
    );
    assert_ne!(
        view.binding_digest,
        Digest::of(b"never"),
        "sanity: digests are real values"
    );
}

#[test]
fn bearer_material_never_reaches_disk_or_debug_output() {
    let mut world = committed_world("secrecy");
    let external = Arc::new(Mutex::new(ExternalWorld::default()));

    // Establish with a known deterministic stream: draw order is launch key
    // then bearer, so this replica predicts the bearer in memory.
    let mut bearer_bytes = [0u8; 32];
    {
        let replica = FixedRandom::new(1);
        let mut sink = [0u8; 32];
        replica.fill(&mut sink).expect("draw key");
        replica.fill(&mut bearer_bytes).expect("draw bearer");
    }
    let bearer_hex: String = bearer_bytes.iter().map(|b| format!("{b:02x}")).collect();

    let mut holder = controller(world.s(), 1);
    let outcome = holder
        .reconcile_run(&world.run_id, &Harness::healthy(&external).deps())
        .expect("establish");
    assert!(matches!(outcome, RunOutcome::AttemptEstablished { .. }));

    // Debug output of the state that holds live credentials redacts.
    let debug = format!("{holder:?}");
    assert!(
        !debug.contains(&bearer_hex),
        "bearer material must never appear in debug output"
    );
    assert!(debug.contains("[REDACTED]"), "redaction is explicit");

    // The durable file never contains it either — before or after contact.
    drop(holder);
    let harness = Harness::healthy(&external);
    controller(world.s(), 2)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("launch");
    let bytes = world.db_bytes();
    let needle = bearer_hex.as_bytes();
    assert!(
        !bytes.windows(needle.len()).any(|window| window == needle),
        "the database file must never contain raw bearer material"
    );

    // And a rekeyed bearer is equally absent after restart recovery.
    let mut second_bearer = [0u8; 32];
    FixedRandom::new(9)
        .fill(&mut second_bearer)
        .expect("draw rekey bearer");
    let second_hex: String = second_bearer.iter().map(|b| format!("{b:02x}")).collect();
    world.reopen();
    controller(world.s(), 9)
        .reconcile_run(&world.run_id, &harness.deps())
        .expect("recovered launch");
    let bytes = world.db_bytes();
    for secret in [needle, second_hex.as_bytes()] {
        assert!(
            !bytes.windows(secret.len()).any(|window| window == secret),
            "no raw credential revision ever reaches durable bytes"
        );
    }
}

#[test]
fn policy_readiness_refuses_a_binding_whose_profile_identity_vanished() {
    let mut world = world_with_run("policy-refusal", |_authority, store| {
        let active = store
            .configuration_pointer()
            .expect("pointer")
            .active
            .expect("active");
        let agent = pantheon_core::execution::LogicalAgentVersion {
            name: "builder".to_string(),
            version: 1,
        };
        let agents_json = store
            .revision_agents_component_json(active.activation_sequence)
            .expect("read agents component")
            .expect("agents component stored");
        let value = pantheon_core::config::parse::parse(&agents_json).expect("fixture component");
        let guidance =
            pantheon_core::context::frozen_agent_guidance(&value, &agent).expect("guidance");

        // A T3 commit whose Binding names a sandbox identity no frozen profile
        // carries. The Scheduler would never produce this; a direct caller
        // can. T3 itself does not validate the sandbox digest (feasibility is
        // a pre-T3 routing fact), so PolicyReady is the gate that must refuse.
        let snap = store.scheduling_snapshot().expect("snapshot");
        let candidate = snap.candidates.first().expect("dispatchable Task").clone();
        let binding_frozen = pantheon_core::scheduling::ExecutionBinding {
            task_id: candidate.task_id.clone(),
            agent: agent.clone(),
            request_digest: Digest::of(b"request"),
            offer_digest: Digest::of(b"offer"),
            backend_id: "fake-local".to_string(),
            descriptor_revision: 3,
            descriptor_digest: Digest::of(b"descriptor"),
            execution_profile_digest: active.components.execution_profile,
            sandbox_profile_digest: Digest::of(b"never-a-configured-profile"),
            route_policy_digest: active.components.routing,
            configuration_activation_sequence: active.activation_sequence,
            configuration_content_digest: active.content_digest,
            component_digests: active.components,
        };
        let snapshot_frozen = pantheon_core::scheduling::ContextSourceSnapshot {
            task_spec_digest: candidate.spec_digest,
            goal_id: candidate.goal_id.clone(),
            goal_revision: candidate.goal_current_revision,
            graph_revision: candidate.graph_revision,
            agent,
            configuration_activation_sequence: active.activation_sequence,
            context_policy_digest: active.components.context_policy,
            agent_soul_digest: pantheon_core::context::guidance_digest(&guidance.soul),
            agent_behavior_digest: pantheon_core::context::guidance_digest(&guidance.behavior),
            workspace_id: "ws-1".to_string(),
            workspace_resolved_base: "a".repeat(40),
        };
        let binding_digest = binding_frozen.digest();
        let snapshot_digest = snapshot_frozen.digest();
        let intent = pantheon_store::RunIntent {
            run_id: "run-policy",
            task_id: &candidate.task_id,
            goal_id: &candidate.goal_id,
            expected_task_revision: candidate.task_revision,
            expected_goal_row_revision: candidate.goal_row_revision,
            expected_goal_current_revision: candidate.goal_current_revision,
            expected_graph_revision: candidate.graph_revision,
            expected_workspace_revision: candidate.workspace_revision,
            expected_scheduler_revision: snap.state.revision,
            expected_goal_fairness_revision: None,
            expected_task_scheduling_revision: candidate.scheduling_revision,
            configuration_activation_sequence: active.activation_sequence,
            binding_digest: &binding_digest,
            binding: &binding_frozen,
            snapshot_digest: &snapshot_digest,
            snapshot: &snapshot_frozen,
        };
        match store
            .commit_run_intent(
                &command(
                    store.restore_generation().expect("generation").as_str(),
                    "cmd-t3-bogus",
                    &[12u8; 32],
                    "run.committed",
                ),
                &intent,
            )
            .expect("T3 accepts the frozen facts it validates")
        {
            pantheon_store::Committed::Executed { value, .. } => value.run_id,
            other => panic!("got {other:?}"),
        }
    });

    // Preparation reaches the PolicyReady gate and fails closed loudly: a
    // Binding whose strategy cannot be re-derived from the frozen revision is
    // corruption-shaped evidence, not an ordinary preparation failure.
    let external = Arc::new(Mutex::new(ExternalWorld::default()));
    let error = controller(world.s(), 1)
        .reconcile_run(&world.run_id, &Harness::healthy(&external).deps())
        .expect_err("an unverifiable sandbox identity must not reach LaunchReady");
    let detail = match error {
        super::RunControllerError::Store(pantheon_store::StoreError::InvariantViolated(detail)) => {
            detail
        }
        other => panic!("expected the typed policy refusal, got {other:?}"),
    };
    assert!(
        detail.contains("sandbox identity"),
        "the refusal names the failing gate: {detail}"
    );
    assert!(external.lock().expect("world").contacts.is_empty());
}
