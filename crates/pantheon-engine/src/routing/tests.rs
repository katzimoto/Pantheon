//! End-to-end evidence for the pre-Run routing path.

use std::cell::{Cell, RefCell};

use pantheon_core::execution::{
    BackendDescriptor, ControllerSafetyFacts, ExecutionOffer, ExecutionRequest, LaunchSemantics,
};
use pantheon_core::planning::TaskPhase;
use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_store::{Command, Store};

use super::{ExecutorBackend, ExecutorBackendPort, RoutingController, RoutingError};
use crate::configuration::{ConfigurationAuthority, SourceSet};

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-routing-test-{label}-{}-{unique}",
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

fn command<'a>(epoch: &'a str, id: &'a str, hash: &'a [u8; 32], event: &'a str) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: hash,
        event_type: event,
    }
}

fn configuration_source(backend_enabled: bool, requires_keyed_launch: bool) -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"enabled":true,"current":true,
    "accepts":["code.change"],"competencies":["code.analysis","code.editing","test.execution"],"routePolicy":"default",
    "executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"]}}],
  "routing": {{"policies":[{{"name":"default","priority":0,"ordering":["contextCapacity"],
    "tieBreak":"backendId","requiresKeyedLaunch":{requires_keyed_launch}}}]}},
  "execution": {{"profiles":[{{"name":"strict","isolationClass":"CONTAINER",
    "guarantees":["isolation.control-plane"],"networkMode":"NONE",
    "environmentIdentity":"sha256:image"}}],"backends":[
    {{"backendId":"fake-local","enabled":{backend_enabled},"selector":"fake"}},
    {{"backendId":"fake-secondary","enabled":true,"selector":"fake"}}
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

fn prepared(label: &str, backend_enabled: bool, requires_keyed_launch: bool) -> (TempDir, Store) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("read generation");
    authority
        .activate(
            &command(
                epoch.as_str(),
                "cfg-1",
                &[1u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single(
                "pantheon.json",
                configuration_source(backend_enabled, requires_keyed_launch),
            ),
        )
        .expect("configuration activates");
    (dir, store)
}

fn loaded(
    store: &Store,
    backend_enabled: bool,
    requires_keyed_launch: bool,
) -> ConfigurationAuthority<&Store> {
    let authority = ConfigurationAuthority::new(store);
    authority
        .load(&SourceSet::single(
            "pantheon.json",
            configuration_source(backend_enabled, requires_keyed_launch),
        ))
        .expect("load active configuration");
    authority
}

fn materialize_ready_task(store: &Store) {
    let controller = crate::planning::PlanningController::new(store);
    let spec = goal_spec();
    let epoch = store.restore_generation().expect("read generation");
    controller
        .create_goal(
            &command(epoch.as_str(), "goal-1", &[2u8; 32], "goal.created"),
            "goal-1",
            &spec,
        )
        .expect("create Goal");
    controller
        .plan(
            &command(
                epoch.as_str(),
                "planning-1",
                &[3u8; 32],
                "planning.recorded",
            ),
            "planning-1",
            "goal-1",
        )
        .expect("record plan");
    let goal = store
        .goal("goal-1")
        .expect("read Goal")
        .expect("Goal exists");
    let graph = store
        .task_graph("goal-1")
        .expect("read graph")
        .expect("graph exists");
    let config = store
        .configuration_pointer()
        .expect("read configuration")
        .active
        .expect("configuration active");
    let proposal = direct::plan(&PlanningInput {
        goal_id: "goal-1",
        goal_revision: goal.current_revision,
        goal: &spec,
        expected_graph_revision: graph.revision.get(),
        configuration_activation_sequence: config.activation_sequence,
        trigger: Trigger::Initial,
    });
    controller
        .materialize(
            &command(epoch.as_str(), "graph-1", &[4u8; 32], "graph.patched"),
            "planning-1",
            "task-1",
            "goal-1",
            &proposal,
        )
        .expect("materialize Ready Task");
}

#[derive(Debug, Clone, Copy)]
enum OfferMode {
    Compatible,
    NoOffers,
    MissingFeature,
}

#[derive(Debug)]
struct FakeBackend {
    descriptor: RefCell<BackendDescriptor>,
    mode: OfferMode,
    offer_calls: Cell<usize>,
}

impl FakeBackend {
    fn new(backend_id: &str, mode: OfferMode, launch_semantics: LaunchSemantics) -> Self {
        Self {
            descriptor: RefCell::new(BackendDescriptor {
                backend_id: backend_id.to_string(),
                revision: 1,
                available_for_offers: true,
                placement: vec!["local".to_string()],
                supported_execution_features: vec!["exec.shell".to_string()],
                context_capacity_tokens: 16_000,
                isolation_facts: vec!["isolation.control-plane".to_string()],
                resources: Vec::new(),
                launch_semantics,
            }),
            mode,
            offer_calls: Cell::new(0),
        }
    }

    fn set_revision(&self, revision: u64) {
        self.descriptor.borrow_mut().revision = revision;
    }
}

impl ExecutorBackend for FakeBackend {
    fn describe(&self) -> BackendDescriptor {
        self.descriptor.borrow().clone()
    }

    fn offer(
        &self,
        request: &ExecutionRequest,
    ) -> Result<Vec<ExecutionOffer>, super::BackendError> {
        self.offer_calls.set(self.offer_calls.get() + 1);
        if matches!(self.mode, OfferMode::NoOffers) {
            return Ok(Vec::new());
        }
        let descriptor = self.descriptor.borrow();
        let supported_execution_features = if matches!(self.mode, OfferMode::MissingFeature) {
            Vec::new()
        } else {
            descriptor.supported_execution_features.clone()
        };
        Ok(vec![ExecutionOffer {
            request_digest: request.digest(),
            backend_id: descriptor.backend_id.clone(),
            descriptor_revision: descriptor.revision,
            descriptor_digest: descriptor.digest(),
            supported_execution_features,
            context_capacity_tokens: descriptor.context_capacity_tokens,
            placement: descriptor.placement.clone(),
            isolation_facts: descriptor.isolation_facts.clone(),
            resources: descriptor.resources.clone(),
            launch_semantics: descriptor.launch_semantics,
            offer_reference: format!("offer-{}-{}", descriptor.backend_id, descriptor.revision),
        }])
    }
}

struct ReconfiguringBackend<'store, 'authority> {
    fake: FakeBackend,
    store: &'store Store,
    authority: &'authority ConfigurationAuthority<&'store Store>,
    changed: Cell<bool>,
}

impl<'store, 'authority> ExecutorBackend for ReconfiguringBackend<'store, 'authority> {
    fn describe(&self) -> BackendDescriptor {
        if !self.changed.replace(true) {
            let epoch = self.store.restore_generation().expect("read generation");
            self.authority
                .activate(
                    &command(
                        epoch.as_str(),
                        "cfg-during-routing",
                        &[8u8; 32],
                        "configuration.activated",
                    ),
                    &SourceSet::single("pantheon.json", configuration_source(true, false)),
                )
                .expect("activate concurrent configuration");
        }
        self.fake.describe()
    }

    fn offer(
        &self,
        request: &ExecutionRequest,
    ) -> Result<Vec<ExecutionOffer>, super::BackendError> {
        self.fake.offer(request)
    }
}

fn port<'a>(
    backend: &'a dyn ExecutorBackend,
    observational_launch_safe: bool,
) -> ExecutorBackendPort<'a> {
    ExecutorBackendPort::new(
        backend,
        ControllerSafetyFacts {
            isolation_guarantees: vec!["isolation.control-plane".to_string()],
            observational_launch_safe,
        },
    )
}

fn route(
    store: &Store,
    authority: &ConfigurationAuthority<&Store>,
    task_id: &str,
    backends: &[ExecutorBackendPort<'_>],
) -> Result<pantheon_core::execution::RoutingResult, RoutingError> {
    RoutingController::new(store, authority).route_ready_task(task_id, backends)
}

#[test]
fn ready_task_routes_to_one_agent_and_offer_without_execution_side_effects() {
    let (_dir, store) = prepared("success", true, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, true);
    let fake = FakeBackend::new(
        "fake-local",
        OfferMode::Compatible,
        LaunchSemantics::KeyedIdempotent,
    );

    let result =
        route(&store, &authority, "task-1", &[port(&fake, false)]).expect("route succeeds");
    let task = store
        .task("task-1")
        .expect("read task")
        .expect("task exists");

    assert_eq!(result.task_id, "task-1");
    assert_eq!(result.task_revision, 1);
    assert_eq!(result.candidate.agent.name, "builder");
    assert_eq!(result.candidate.offer.backend_id, "fake-local");
    assert_eq!(result.configuration.activation_sequence, 1);
    assert_eq!(task.phase, TaskPhase::Ready);
    assert_eq!(task.active_run_id, None);
    assert_eq!(fake.offer_calls.get(), 1);
}

#[test]
fn incompatible_or_disabled_offers_leave_the_task_ready() {
    let (_dir, store) = prepared("incompatible", true, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, true);
    let fake = FakeBackend::new(
        "fake-local",
        OfferMode::MissingFeature,
        LaunchSemantics::KeyedIdempotent,
    );
    let error =
        route(&store, &authority, "task-1", &[port(&fake, false)]).expect_err("offer is rejected");
    assert!(
        matches!(error, RoutingError::NoCompatibleOffers { rejections, .. } if !rejections.is_empty())
    );
    assert_eq!(
        store
            .task("task-1")
            .expect("read task")
            .expect("task")
            .phase,
        TaskPhase::Ready
    );

    let (_dir, store) = prepared("disabled", false, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, false, true);
    let fake = FakeBackend::new(
        "fake-local",
        OfferMode::Compatible,
        LaunchSemantics::KeyedIdempotent,
    );
    let error = route(&store, &authority, "task-1", &[port(&fake, false)])
        .expect_err("disabled backend is skipped");
    assert!(matches!(error, RoutingError::NoCompatibleOffers { .. }));
    assert_eq!(fake.offer_calls.get(), 0);
}

#[test]
fn observational_launch_is_filtered_or_allowed_by_the_captured_route_policy() {
    let (_dir, store) = prepared("unsafe-observational", true, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, true);
    let fake = FakeBackend::new(
        "fake-local",
        OfferMode::Compatible,
        LaunchSemantics::Observational,
    );
    assert!(matches!(
        route(&store, &authority, "task-1", &[port(&fake, false)]),
        Err(RoutingError::NoCompatibleOffers { .. })
    ));

    let (_dir, store) = prepared("allowed-observational", true, false);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, false);
    let fake = FakeBackend::new(
        "fake-local",
        OfferMode::Compatible,
        LaunchSemantics::Observational,
    );
    let result =
        route(&store, &authority, "task-1", &[port(&fake, true)]).expect("policy permits it");
    assert_eq!(
        result.candidate.offer.launch_semantics,
        LaunchSemantics::Observational
    );
}

#[test]
fn selection_is_stable_when_backend_inputs_are_permuted() {
    let (_dir, store) = prepared("tie-break", true, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, true);
    let primary = FakeBackend::new(
        "fake-local",
        OfferMode::Compatible,
        LaunchSemantics::KeyedIdempotent,
    );
    let secondary = FakeBackend::new(
        "fake-secondary",
        OfferMode::Compatible,
        LaunchSemantics::KeyedIdempotent,
    );

    let first = route(
        &store,
        &authority,
        "task-1",
        &[port(&secondary, false), port(&primary, false)],
    )
    .expect("first route");
    let second = route(
        &store,
        &authority,
        "task-1",
        &[port(&primary, false), port(&secondary, false)],
    )
    .expect("second route");
    assert_eq!(first, second);
    assert_eq!(first.candidate.offer.backend_id, "fake-local");
}

#[test]
fn descriptor_revision_and_configuration_revision_are_bound_not_reinterpreted() {
    let (_dir, store) = prepared("provenance", true, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, true);
    let fake = FakeBackend::new(
        "fake-local",
        OfferMode::Compatible,
        LaunchSemantics::KeyedIdempotent,
    );
    let first = route(&store, &authority, "task-1", &[port(&fake, false)]).expect("first route");

    fake.set_revision(2);
    let second = route(&store, &authority, "task-1", &[port(&fake, false)]).expect("second route");
    assert_eq!(first.candidate.offer.descriptor_revision, 1);
    assert_eq!(second.candidate.offer.descriptor_revision, 2);
    assert!(first.candidate.offer.is_stale_against(&fake.describe()));

    let epoch = store.restore_generation().expect("read generation");
    authority
        .activate(
            &command(
                epoch.as_str(),
                "cfg-2",
                &[9u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single("pantheon.json", configuration_source(true, false)),
        )
        .expect("activate a later configuration");
    let current = authority.snapshot().expect("current snapshot");
    assert!(
        first.is_stale_against(pantheon_core::execution::ConfigurationBinding::new(
            current.active().activation_sequence,
            current.active().content_digest,
            current.active().components,
        ))
    );
}

#[test]
fn configuration_change_during_offer_collection_returns_a_stale_failure() {
    let (_dir, store) = prepared("stale-during-routing", true, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, true);
    let backend = ReconfiguringBackend {
        fake: FakeBackend::new(
            "fake-local",
            OfferMode::Compatible,
            LaunchSemantics::KeyedIdempotent,
        ),
        store: &store,
        authority: &authority,
        changed: Cell::new(false),
    };

    assert!(matches!(
        route(&store, &authority, "task-1", &[port(&backend, false)]),
        Err(RoutingError::StaleConfiguration)
    ));
    assert_eq!(backend.fake.offer_calls.get(), 1);
}

#[test]
fn routing_does_not_create_a_run_or_other_execution_authority() {
    let (_dir, store) = prepared("no-authority", true, true);
    materialize_ready_task(&store);
    let authority = loaded(&store, true, true);
    let fake = FakeBackend::new(
        "fake-local",
        OfferMode::NoOffers,
        LaunchSemantics::KeyedIdempotent,
    );
    let _ = route(&store, &authority, "task-1", &[port(&fake, false)]);

    let task = store
        .task("task-1")
        .expect("read task")
        .expect("task exists");
    assert_eq!(task.phase, TaskPhase::Ready);
    assert!(task.active_run_id.is_none());
    assert!(
        store
            .tasks_for_goal("goal-1")
            .expect("read tasks")
            .iter()
            .all(|task| { task.phase == TaskPhase::Ready && task.active_run_id.is_none() })
    );
    assert_eq!(fake.offer_calls.get(), 1);
}
