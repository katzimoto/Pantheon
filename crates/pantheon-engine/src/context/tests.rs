//! Evidence for the composed Issue #30 path: a committed Run's context is
//! prepared deterministically from its frozen source snapshot, attached
//! exactly once, and never re-derived from newer active state — across
//! restarts, configuration changes and Workspace lifecycle movement.
//!
//! The transaction internals are proven in `pantheon-store`; the pure
//! selection rules in `pantheon-core`. What this module establishes is the
//! composition: that preparation reads only frozen identities, reconciles
//! idempotently, and fails closed when a frozen source cannot be honored.

use std::sync::atomic::{AtomicU64, Ordering};

use pantheon_core::config::Digest;
use pantheon_core::context::{CONTEXT_BUILDER_VERSION, guidance_digest};
use pantheon_core::execution::{
    BackendDescriptor, ControllerSafetyFacts, ExecutionOffer, ExecutionRequest, LaunchSemantics,
};
use pantheon_core::planning::direct;
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::workspace::{RequestedBase, ResolvedBase};
use pantheon_store::{Command, RunContextPlanRecord, Store};

use super::{ContextPreparationController, ContextPreparationError};
use crate::configuration::{ConfigurationAuthority, SourceSet};
use crate::routing::{ExecutorBackend, ExecutorBackendPort};
use crate::scheduling::{ScheduleOutcome, SchedulingController};

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-context-test-{label}-{}-{unique}",
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

const SOUL_V1: &str = "Careful coding agent identity.";
const BEHAVIOR_V1: &str = "Plan first; keep changes minimal.";

fn configuration_source(soul: &str, behavior: &str, memory_limit: i64) -> String {
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
    "preloadPriority":["workspace-orientation"],"memoryLimitTokens":{memory_limit},
    "workspaceOrientationLimitTokens":2000,"safetyMarginTokens":512,
    "optionalDropOrder":["workspace-orientation"]}},
  "authorization": {{"schemaVersion":1,"rules":[
    {{"action":"filesystem.read","effect":"permit"}}
  ]}}
}}"#,
        evaluator_ref = direct::MVP_EVALUATOR_REF,
        soul = soul,
        behavior = behavior,
        memory_limit = memory_limit,
    )
}

/// Offers for exactly one Task so the fixture commits exactly one Run.
struct FakeBackend {
    descriptor: BackendDescriptor,
    serves_task: &'static str,
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
            &SourceSet::single(
                "configuration.json",
                configuration_source(SOUL_V1, BEHAVIOR_V1, 4000),
            ),
        )
        .expect("activate configuration");
    authority
}

/// Drives the Goal-to-Ready-Task path and materializes the Task's Workspace.
fn ready_task_with_workspace(store: &Store, goal_id: &str, task_id: &str, workspace_id: &str) {
    let planning = crate::planning::PlanningController::new(store);
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
        source_path: "/tmp/pantheon-context-test-source",
        requested_base: &requested,
        resolved_base: &resolved,
    };
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

/// A world with one committed Run, ready for preparation.
struct World {
    dir: TempDir,
    store: Option<Store>,
    run_id: String,
}

impl World {
    fn s(&self) -> &Store {
        self.store.as_ref().expect("store open")
    }

    fn run_id(&self) -> &str {
        &self.run_id
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
}

fn committed_world(label: &str) -> World {
    let dir = TempDir::new(label);
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = load_authority(&store);
    ready_task_with_workspace(&store, "goal-1", "task-1", "ws-1");
    let backend = FakeBackend::new("task-1");
    let outcome = SchedulingController::new(&store, &authority)
        .schedule_once(&[backend.port()])
        .expect("the cycle runs");
    let ScheduleOutcome::Committed { run_id, .. } = outcome else {
        panic!("expected a committed Run intent, got {outcome:?}");
    };
    World {
        dir,
        store: Some(store),
        run_id,
    }
}

#[test]
fn preparation_builds_and_attaches_exactly_one_deterministic_plan() {
    // The composed invariant: T3's frozen universe → deterministic plan →
    // one-time attachment, with provenance traceable to frozen identities and
    // no execution surface created.
    let mut world = committed_world("ctx-attach");
    let store = world.s();
    let prepared = ContextPreparationController::new(store)
        .prepare_run_context(world.run_id())
        .expect("preparation succeeds");

    assert_eq!(prepared.run_id, world.run_id());
    assert_eq!(
        prepared.plan.source_snapshot_digest,
        prepared.source_snapshot_digest
    );
    // Provenance comes from the frozen identities.
    assert_eq!(prepared.plan.goal_id, "goal-1");
    assert_eq!(prepared.plan.goal_revision, 1);
    assert_eq!(prepared.plan.workspace_id, "ws-1");
    assert_eq!(prepared.plan.agent_soul_digest, guidance_digest(SOUL_V1));
    // The selected guidance bodies are the frozen ones, verbatim.
    let soul_section = prepared
        .plan
        .sections
        .iter()
        .find(|s| s.kind == "agent-soul")
        .expect("soul section present");
    assert_eq!(soul_section.instruction.as_deref(), Some(SOUL_V1));

    // Exactly one durable attachment naming exactly these identities.
    let record = store
        .run_context_plan(world.run_id())
        .expect("read")
        .expect("attached");
    assert_eq!(
        record,
        RunContextPlanRecord {
            run_id: world.run_id().to_string(),
            context_source_snapshot_digest: prepared.source_snapshot_digest,
            context_plan_digest: prepared.context_plan_digest,
        }
    );
    // The stored canonical bytes are the builder version's own claim.
    let (builder_version,) = store
        .configuration_component_json(prepared.plan.context_policy_digest)
        .expect("read")
        .map(|(domain, _)| (domain,))
        .expect("policy component exists");
    assert_eq!(
        builder_version, "context",
        "the frozen policy resolves by digest"
    );
    assert_eq!(CONTEXT_BUILDER_VERSION, "context-builder-v1");

    // No execution surface appeared: the Run stays Active holding the slot,
    // with nothing to launch.
    assert_eq!(
        store.slot_holder().expect("slot").map(|(run, _)| run),
        Some(world.run_id().to_string())
    );
    world.close_store();
}

#[test]
fn repeated_preparation_reconciles_without_replacing_anything() {
    // Idempotence invariant: same Run + same sources + same plan always
    // reconciles; the durable attachment never moves.
    let mut world = committed_world("ctx-idempotent");
    let first = ContextPreparationController::new(world.s())
        .prepare_run_context(world.run_id())
        .expect("first preparation");
    let second = ContextPreparationController::new(world.s())
        .prepare_run_context(world.run_id())
        .expect("second preparation");
    assert_eq!(first.context_plan_digest, second.context_plan_digest);
    assert_eq!(first.source_snapshot_digest, second.source_snapshot_digest);
    world.close_store();
}

#[test]
fn a_crash_before_attachment_rebuilds_the_identical_plan_after_restart() {
    // Restart-before-attachment invariant: closing the process between T3 and
    // attachment changes nothing. A fresh controller over reopened durable
    // state reconstructs byte-for-byte the same plan another process built.
    let mut reference = committed_world("ctx-crash-ref");
    let reference_plan = ContextPreparationController::new(reference.s())
        .prepare_run_context(reference.run_id())
        .expect("reference preparation");
    reference.close_store();

    let mut restarted = committed_world("ctx-crash-restart");
    restarted.reopen();
    let rebuilt = ContextPreparationController::new(restarted.s())
        .prepare_run_context(restarted.run_id())
        .expect("reconstruction after restart");
    assert_eq!(
        rebuilt.context_plan_digest, reference_plan.context_plan_digest,
        "the same frozen world produces the same plan across processes"
    );
    assert_eq!(
        rebuilt.run_id, reference.run_id,
        "Run identity is content-derived, not process-local"
    );
    restarted.close_store();
}

#[test]
fn a_crash_after_attachment_observes_the_same_attachment_and_keeps_it() {
    // Restart-after-attachment invariant: the attachment is durable history;
    // retry observes it and never replaces it.
    let mut world = committed_world("ctx-after");
    let before = ContextPreparationController::new(world.s())
        .prepare_run_context(world.run_id())
        .expect("preparation");
    world.reopen();

    let record = world
        .s()
        .run_context_plan(world.run_id())
        .expect("read")
        .expect("attachment survived restart");
    assert_eq!(record.context_plan_digest, before.context_plan_digest);
    assert_eq!(
        record.context_source_snapshot_digest,
        before.source_snapshot_digest
    );

    let again = ContextPreparationController::new(world.s())
        .prepare_run_context(world.run_id())
        .expect("retry reconciles");
    assert_eq!(again.context_plan_digest, before.context_plan_digest);
    let record = world
        .s()
        .run_context_plan(world.run_id())
        .expect("read")
        .expect("still attached");
    assert_eq!(record.context_plan_digest, before.context_plan_digest);
    world.close_store();
}

#[test]
fn activating_a_newer_configuration_revision_never_changes_an_existing_runs_plan() {
    // Frozen-source enforcement: after T3, neither a new active revision nor
    // changed Agent guidance can reach into an existing Run — whether the Run
    // was already prepared or prepares for the first time after the change.
    let mut world = committed_world("ctx-newer-config");
    let store = world.s();

    // First preparation under the original revision.
    let original = ContextPreparationController::new(store)
        .prepare_run_context(world.run_id())
        .expect("preparation under the frozen revision");

    // Activate a materially different revision: different guidance text and
    // different policy numbers.
    let epoch = store.restore_generation().expect("generation");
    let authority = ConfigurationAuthority::new(store);
    authority
        .activate(
            &command(
                epoch.as_str(),
                "cfg-2",
                &[5u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single(
                "configuration.json",
                configuration_source("Rewritten identity.", "Rewritten behavior.", 9999),
            ),
        )
        .expect("activate the newer revision");

    // A Run that already prepared keeps its exact plan.
    world.reopen();
    let after = ContextPreparationController::new(world.s())
        .prepare_run_context(world.run_id())
        .expect("retry after activation reconciles");
    assert_eq!(after.context_plan_digest, original.context_plan_digest);
    let record = world
        .s()
        .run_context_plan(world.run_id())
        .expect("read")
        .expect("attached");
    assert_eq!(record.context_plan_digest, original.context_plan_digest);

    // And a Run that had NOT prepared yet still builds from the frozen
    // generation, not from what is now active. Prove it on a second world
    // whose preparation happens only after the change.
    let mut late = committed_world("ctx-late-prep");
    let store_late = late.s();
    let epoch = store_late.restore_generation().expect("generation");
    let authority = ConfigurationAuthority::new(store_late);
    authority
        .activate(
            &command(
                epoch.as_str(),
                "cfg-2",
                &[6u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single(
                "configuration.json",
                configuration_source("Rewritten identity.", "Rewritten behavior.", 9999),
            ),
        )
        .expect("activate the newer revision");
    let prepared = ContextPreparationController::new(store_late)
        .prepare_run_context(late.run_id())
        .expect("preparation uses the frozen revision");
    let soul_section = prepared
        .plan
        .sections
        .iter()
        .find(|s| s.kind == "agent-soul")
        .expect("soul section present");
    assert_eq!(
        soul_section.instruction.as_deref(),
        Some(SOUL_V1),
        "guidance comes from the frozen Agent version, not the active one"
    );
    assert_eq!(
        prepared.plan.agent_soul_digest,
        guidance_digest(SOUL_V1),
        "the frozen digest governs"
    );
    // Byte-for-byte identical to the same world prepared before any change.
    assert_eq!(
        prepared.context_plan_digest, original.context_plan_digest,
        "activation order cannot influence plan identity"
    );
    late.close_store();
    world.close_store();
}

#[test]
fn mutable_workspace_lifecycle_after_t3_does_not_mutate_the_initial_plan() {
    // Mutable-state boundary: the Workspace may legitimately advance after T3
    // (here: frozen by sealing); preparation still proves only the frozen
    // ownership/base relation and produces the identical plan.
    let mut world = committed_world("ctx-ws-frozen");
    let store = world.s();

    let expected = ContextPreparationController::new(store)
        .prepare_run_context(world.run_id())
        .expect("preparation while Ready");

    // Freeze the Workspace through #32's authoritative transition. The
    // materialization completion left the row at revision 3.
    let epoch = store.restore_generation().expect("generation");
    store
        .freeze_workspace(
            &command(
                epoch.as_str(),
                "cmd-freeze",
                &[15u8; 32],
                "workspace.frozen",
            ),
            "ws-1",
            pantheon_store::Revision::new(3),
        )
        .expect("workspace freezes");

    world.reopen();
    let after = ContextPreparationController::new(world.s())
        .prepare_run_context(world.run_id())
        .expect("preparation ignores mutable lifecycle");
    assert_eq!(after.context_plan_digest, expected.context_plan_digest);
    let orientation = after
        .plan
        .sections
        .iter()
        .find(|s| s.kind == "workspace-orientation")
        .expect("orientation present");
    assert_eq!(orientation.key, "");
    world.close_store();
}

/// A frozen snapshot fixture for the pure verifier tests.
fn honest_snapshot() -> pantheon_core::scheduling::ContextSourceSnapshot {
    pantheon_core::scheduling::ContextSourceSnapshot {
        task_spec_digest: Digest::of(b"spec"),
        goal_id: "goal-1".to_string(),
        goal_revision: 1,
        graph_revision: 47,
        agent: pantheon_core::execution::LogicalAgentVersion {
            name: "builder".to_string(),
            version: 1,
        },
        configuration_activation_sequence: 43,
        context_policy_digest: Digest::of(b"context-policy"),
        agent_soul_digest: guidance_digest(SOUL_V1),
        agent_behavior_digest: guidance_digest(BEHAVIOR_V1),
        workspace_id: "ws-1".to_string(),
        workspace_resolved_base: "a".repeat(40),
    }
}

#[test]
fn a_tampered_or_missing_frozen_snapshot_fails_its_own_identity_check() {
    // Corruption fence: stored bytes that no longer produce the Run's frozen
    // digest are refused before any decoding happens; an absent row fails
    // closed as unavailable rather than being skipped.
    let snapshot = honest_snapshot();
    let canonical = String::from_utf8(snapshot.to_value().to_canonical_bytes()).expect("utf-8");

    let err =
        super::decode_frozen_snapshot(Some(r#"{"tampered":true}"#.to_string()), snapshot.digest())
            .expect_err("tampered bytes must fail");
    assert!(
        matches!(err, ContextPreparationError::SourceDigestMismatch { source, .. } if source == "context-source-snapshot")
    );

    let err = super::decode_frozen_snapshot(None, snapshot.digest())
        .expect_err("an absent snapshot must fail closed");
    assert!(matches!(
        err,
        ContextPreparationError::FrozenSnapshotUnavailable { .. }
    ));

    let decoded =
        super::decode_frozen_snapshot(Some(canonical.clone()), snapshot.digest()).expect("decodes");
    assert_eq!(decoded, snapshot);
}

#[test]
fn a_tampered_task_specification_fails_closed_instead_of_preparing() {
    let snapshot = honest_snapshot();
    // Well-formed spec bytes whose content differs from the frozen digest:
    // they parse, so the failure must come from the identity check.
    let mut tampered = verifier_task_spec("goal-1", 1);
    tampered.objective = "tampered objective".to_string();
    let tampered_json = String::from_utf8(tampered.to_value().to_canonical_bytes()).expect("utf-8");
    let err = super::decode_task_spec(Some(tampered_json), snapshot.task_spec_digest)
        .expect_err("bytes that do not reproduce the spec digest fail closed");
    assert!(matches!(
        err,
        ContextPreparationError::SourceDigestMismatch { source, .. } if source == "task-specification"
    ));
    // Unparsable persisted bytes are a malformed source, not a substitute.
    let err = super::decode_task_spec(
        Some(r#"{"tampered":true}"#.to_string()),
        snapshot.task_spec_digest,
    )
    .expect_err("malformed spec bytes fail closed");
    assert!(matches!(
        err,
        ContextPreparationError::MalformedSource { .. }
    ));
    let err = super::decode_task_spec(None, snapshot.task_spec_digest)
        .expect_err("a missing spec fails closed");
    assert!(matches!(
        err,
        ContextPreparationError::RequiredSourceUnavailable { detail }
            if detail.contains("task specification")
    ));
}

/// A minimal TaskSpec fixture for the pure verifier tests.
fn verifier_task_spec(goal_id: &str, revision: i64) -> pantheon_core::planning::TaskSpec {
    use pantheon_core::planning::task::{AcceptanceContract, TaskOutput, TaskScope};
    pantheon_core::planning::TaskSpec {
        task_type: "code.change".to_string(),
        objective: "objective".to_string(),
        inputs: vec![],
        outputs: vec![TaskOutput {
            name: "changeset".to_string(),
            kind: "code.changeset".to_string(),
            required: true,
        }],
        competencies: vec![],
        scope: TaskScope {
            resources: vec![],
            permitted_effects: vec![],
            forbidden_effects: vec![],
        },
        acceptance: AcceptanceContract {
            criteria: vec![],
            evaluator_registry_digest: Digest::of(b"registry"),
            configuration_activation_sequence: 43,
        },
        goal_id: goal_id.to_string(),
        goal_revision: revision,
    }
}

#[test]
fn a_spec_for_another_goal_is_a_wrong_source_relation() {
    // Relation fence: the spec carries its owner; only comparing closes the
    // swap a bare digest lookup would accept.
    let snapshot = honest_snapshot();
    let foreign = verifier_task_spec("goal-other", 1);
    let err = super::verify_task_relation(&foreign, &snapshot)
        .expect_err("a foreign Goal relation must fail closed");
    assert!(matches!(
        err,
        ContextPreparationError::WrongSourceRelation { .. }
    ));

    let right_goal = verifier_task_spec("goal-1", 2);
    let err = super::verify_task_relation(&right_goal, &snapshot)
        .expect_err("a foreign revision relation must fail closed");
    assert!(matches!(
        err,
        ContextPreparationError::WrongSourceRelation { .. }
    ));

    let honest = verifier_task_spec("goal-1", 1);
    super::verify_task_relation(&honest, &snapshot).expect("the frozen relation verifies");
}

#[test]
fn guidance_that_does_not_reproduce_the_frozen_digests_fails_closed() {
    // Defense-in-depth fence: even though T3 validated the pairing at commit
    // time, preparation independently refuses guidance whose bytes no longer
    // reproduce the digests frozen at T3.
    let mut snapshot = honest_snapshot();
    let component = format!(
        r#"{{"agents":[{{"name":"builder","version":1,"soul":"{}","behavior":"{}"}}]}}"#,
        SOUL_V1, BEHAVIOR_V1
    );
    super::decode_agent_guidance(&component, &snapshot).expect("honest guidance verifies");

    snapshot.agent_soul_digest = Digest::of(b"somewhere-else");
    let err = super::decode_agent_guidance(&component, &snapshot)
        .expect_err("divergent guidance must fail closed");
    assert!(matches!(
        err,
        ContextPreparationError::SourceDigestMismatch { source, .. } if source == "agent-guidance"
    ));

    // An absent version in the frozen component is a missing frozen source.
    snapshot.agent = pantheon_core::execution::LogicalAgentVersion {
        name: "ghost".to_string(),
        version: 9,
    };
    let err = super::decode_agent_guidance(&component, &snapshot)
        .expect_err("an absent frozen Agent version fails closed");
    assert!(matches!(
        err,
        ContextPreparationError::RequiredSourceUnavailable { detail } if detail.contains("ghost@9")
    ));
}

#[test]
fn a_workspace_outside_the_frozen_relation_fails_closed() {
    // Ownership and base are part of the frozen contract; phase is not.
    use pantheon_core::workspace::{Materialization, ResolvedBase, WorkspacePhase};
    let snapshot = honest_snapshot();

    let err = super::verify_workspace_relation(None, "task-1", &snapshot)
        .expect_err("a missing Workspace fails closed");
    assert!(matches!(
        err,
        ContextPreparationError::RequiredSourceUnavailable { detail } if detail.contains("ws-1")
    ));

    let record = |task_id: &str, base: &str| pantheon_store::WorkspaceRecord {
        id: "ws-1".to_string(),
        task_id: task_id.to_string(),
        repository: "repo://project".to_string(),
        source_path: "/tmp/irrelevant".to_string(),
        requested_base: RequestedBase::parse("main").unwrap(),
        resolved_base: ResolvedBase::parse(base).unwrap(),
        phase: WorkspacePhase::Frozen,
        materialization: Materialization::Present,
        revision: pantheon_store::Revision::new(3),
    };

    let err = super::verify_workspace_relation(
        Some(record("task-2", &"a".repeat(40))),
        "task-1",
        &snapshot,
    )
    .expect_err("another Task's Workspace must fail closed");
    assert!(
        matches!(err, ContextPreparationError::WrongSourceRelation { ref detail } if detail.contains("belongs to task task-2"))
    );

    let err = super::verify_workspace_relation(
        Some(record("task-1", &"b".repeat(40))),
        "task-1",
        &snapshot,
    )
    .expect_err("a moved base must fail closed");
    assert!(
        matches!(err, ContextPreparationError::WrongSourceRelation { ref detail } if detail.contains("not the frozen base"))
    );

    // Phase deliberately does not participate: a Workspace that advanced to
    // Frozen after T3 still satisfies the frozen ownership/base relation.
    let verified = super::verify_workspace_relation(
        Some(record("task-1", &"a".repeat(40))),
        "task-1",
        &snapshot,
    )
    .expect("the frozen relation holds regardless of lifecycle");
    assert_eq!(verified.phase, WorkspacePhase::Frozen);
}
