//! Evidence for the Agent Control gateway: the worker-visible vertical slice
//! from `session.describe` through `artifact.seal` to `task.submit_result`,
//! the crash windows sealing spans, restart reconciliation, credential
//! hygiene, and the rule that payload can never select authority.
//!
//! The sealing ports are controllable doubles (as in `sealing::tests`) so a
//! crash window can be established at an exact step; real filesystem
//! confinement, Git behavior and CAS durability are proven in their own
//! crates and composed end-to-end in `pantheond`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pantheon_core::artifact::{EntryKind, RepositoryPath};
use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value;
use pantheon_core::context::{CONTEXT_BUILDER_VERSION, guidance_digest};
use pantheon_core::execution::LogicalAgentVersion;
use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::scheduling::{ContextSourceSnapshot, ExecutionBinding};
use pantheon_core::workspace::{RequestedBase, ResolvedBase};
use pantheon_store::{Command, RunIntent, SealAuthority as StoreSealAuthority, Store};

use crate::agent_control::{
    AgentControlError, AgentControlGateway, AgentSealOutcome, SubmitResultRequest,
    WorkerCredential, canonical_request_hash, internal_command_id,
};
use crate::configuration::{ConfigurationAuthority, SourceSet};
use crate::run::{Bearer, RandomBytes};
use crate::sealing::{
    BaseObject, CapturedEntry, ChangesetSealer, ContentObjectStore, ExternalFault, ObjectRef,
    SealCommand, SealRequest, TrustedBaseReader, WorkspaceTreeCapture,
};
use crate::workspace::{
    MaterializationTarget, MaterializerError, RepositoryMaterializer, WorkspaceCommand,
    WorkspaceController, WorkspaceRequest,
};

const BASE: &str = "dc6fcd729d1c3b0426712ab6985f28c19be95d55";
const APP_OID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ATTEMPT: &str = "attempt-1";

#[derive(Debug)]
struct FixedRandom(AtomicU64);

impl RandomBytes for FixedRandom {
    fn fill(&self, dest: &mut [u8]) -> Result<(), crate::run::RandomFailure> {
        let value = self.0.fetch_add(1, Ordering::SeqCst);
        for (index, byte) in dest.iter_mut().enumerate() {
            *byte = (value.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> ((index % 8) * 8)) as u8;
        }
        Ok(())
    }
}

/// A deterministic stand-in Git object name, exactly as in `sealing::tests`.
fn fixture_identity(preimage: &[u8], contents: &[u8]) -> String {
    if contents == preimage {
        return APP_OID.to_string();
    }
    Digest::of(contents).to_string()[7..47].to_string()
}

struct MemoryBase;

impl TrustedBaseReader for MemoryBase {
    fn base_tree(
        &self,
        _source: &Path,
        base: &ResolvedBase,
    ) -> Result<BTreeMap<Vec<u8>, BaseObject>, ExternalFault> {
        let mut tree = BTreeMap::new();
        tree.insert(
            b"app.txt".to_vec(),
            BaseObject {
                kind: EntryKind::Regular,
                oid: APP_OID.to_string(),
                size: b"original".len() as u64,
            },
        );
        let _ = base;
        Ok(tree)
    }

    fn blob_object_names(
        &self,
        _source: &Path,
        contents: &[&[u8]],
    ) -> Result<Vec<String>, ExternalFault> {
        Ok(contents
            .iter()
            .map(|content| fixture_identity(b"original", content))
            .collect())
    }

    fn blob_bytes(&self, _source: &Path, oid: &str) -> Result<Vec<u8>, ExternalFault> {
        if oid == APP_OID {
            return Ok(b"original".to_vec());
        }
        Err(ExternalFault {
            code: "base.missing-object".to_string(),
            detail: "unknown fixture object".to_string(),
        })
    }
}

struct MemoryCas {
    objects: RefCell<BTreeMap<Digest, Vec<u8>>>,
    publishes: Cell<usize>,
}

impl MemoryCas {
    fn new() -> Self {
        Self {
            objects: RefCell::new(BTreeMap::new()),
            publishes: Cell::new(0),
        }
    }
}

impl ContentObjectStore for MemoryCas {
    fn publish(&self, bytes: &[u8]) -> Result<ObjectRef, ExternalFault> {
        self.publishes.set(self.publishes.get() + 1);
        let digest = Digest::of(bytes);
        self.objects.borrow_mut().insert(digest, bytes.to_vec());
        Ok(ObjectRef {
            digest,
            size: bytes.len() as u64,
        })
    }

    fn verify(&self, reference: &ObjectRef) -> Result<(), ExternalFault> {
        match self.objects.borrow().get(&reference.digest) {
            Some(bytes) if bytes.len() as u64 == reference.size => Ok(()),
            Some(_) => Err(ExternalFault {
                code: "cas.corrupt-object".to_string(),
                detail: "size mismatch".to_string(),
            }),
            None => Err(ExternalFault {
                code: "cas.object-unavailable".to_string(),
                detail: "missing".to_string(),
            }),
        }
    }

    fn read(&self, reference: &ObjectRef) -> Result<Vec<u8>, ExternalFault> {
        self.verify(reference)?;
        Ok(self.objects.borrow()[&reference.digest].clone())
    }
}

/// The changed tree every seal captures: one modified file plus one addition.
struct StaticCapture {
    entries: Vec<(Vec<u8>, EntryKind, &'static [u8])>,
}

impl WorkspaceTreeCapture for StaticCapture {
    fn capture_tree(
        &self,
        _root: &Path,
        sink: &mut dyn FnMut(CapturedEntry) -> Result<(), ExternalFault>,
    ) -> Result<(), ExternalFault> {
        for (path, kind, bytes) in &self.entries {
            sink(CapturedEntry {
                path: RepositoryPath::from_bytes(path).expect("fixture path"),
                kind: *kind,
                bytes: bytes.to_vec(),
            })?;
        }
        Ok(())
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

fn configuration_source() -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"enabled":true,"current":true,
    "accepts":["code.change"],"competencies":["code.analysis"],"routePolicy":"default",
    "executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"],"soul":"Careful coding agent identity.","behavior":"Plan first; keep changes minimal."}}],
  "routing": {{"policies":[{{"name":"default","priority":0,"ordering":["contextCapacity"],
    "tieBreak":"backendId","requiresKeyedLaunch":false}}]}},
  "execution": {{"profiles":[{{"name":"strict","isolationClass":"CONTAINER",
    "guarantees":["isolation.control-plane"],"networkMode":"NONE",
    "environmentIdentity":"sha256:image"}}],"backends":[
    {{"backendId":"fake-local","enabled":true,"selector":"fake"}}
  ]}},
  "evaluators": {{"versions":[{{"id":"unit-tests-v1","kind":"check",
    "argv":["/bin/check"],"timeoutMs":1000,"sandboxProfile":"strict",
    "resultProtocol":"p-v1"}}],"refs":[{{"ref":"{evaluator_ref}",
    "currentVersion":"unit-tests-v1"}}]}},
  "context": {{"schemaVersion":1,"mandatorySections":["task-contract","goal-contract","agent-soul","agent-behavior"],
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

fn goal_spec(resources: &[&str]) -> GoalSpec {
    GoalSpec {
        objective: "make one bounded change in the repository".to_string(),
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
            permitted_resources: resources.iter().map(|r| (*r).to_string()).collect(),
        },
    }
}

#[derive(Debug)]
struct DirMakerMaterializer;

impl RepositoryMaterializer for DirMakerMaterializer {
    fn resolve_base(
        &self,
        _source: &Path,
        _requested: &RequestedBase,
    ) -> Result<ResolvedBase, MaterializerError> {
        Ok(ResolvedBase::parse(BASE).expect("fixture base"))
    }

    fn materialize(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<ResolvedBase, MaterializerError> {
        std::fs::create_dir_all(target.destination).expect("destination");
        Ok(ResolvedBase::parse(BASE).expect("fixture base"))
    }

    fn observe(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<pantheon_core::workspace::Materialization, MaterializerError> {
        if target.destination.exists() {
            Ok(pantheon_core::workspace::Materialization::Present)
        } else {
            Ok(pantheon_core::workspace::Materialization::Absent)
        }
    }

    fn discard(&self, target: &MaterializationTarget<'_>) -> Result<(), MaterializerError> {
        let _ = std::fs::remove_dir_all(target.destination);
        Ok(())
    }
}

struct World {
    db_path: PathBuf,
    store: Store,
    workspace_root: PathBuf,
    bearer: Bearer,
}

fn unique_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "pantheon-engine-agent-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create test directory");
    path
}

/// One dispatched coding Task whose current Attempt carries a known bearer.
fn world(label: &str, seed: u64) -> World {
    let dir = unique_dir(label);
    let store = Store::open(dir.join("pantheon.db")).expect("open store");
    let epoch = store.restore_generation().expect("generation");

    ConfigurationAuthority::new(&store)
        .activate(
            &command(
                epoch.as_str(),
                "cfg-1",
                &[1u8; 32],
                "configuration.activated",
            ),
            &SourceSet::single("pantheon.json", configuration_source()),
        )
        .expect("configuration activates");

    let planning = crate::planning::PlanningController::new(&store);
    let spec = goal_spec(&["workspace://**"]);
    planning
        .create_goal(
            &command(epoch.as_str(), "goal-1", &[2u8; 32], "goal.created"),
            "goal-1",
            &spec,
        )
        .expect("create Goal");
    planning
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
    let config = store
        .configuration_pointer()
        .expect("read configuration")
        .active
        .expect("active configuration");
    let proposal = direct::plan(&PlanningInput {
        goal_id: "goal-1",
        goal_revision: 1,
        goal: &spec,
        expected_graph_revision: 0,
        configuration_activation_sequence: config.activation_sequence,
        trigger: Trigger::Initial,
    });
    planning
        .materialize(
            &command(epoch.as_str(), "graph-1", &[4u8; 32], "graph.patched"),
            "planning-1",
            "task-1",
            "goal-1",
            &proposal,
        )
        .expect("materialize the Task");

    let workspace_root = dir.join("workspaces");
    let source = dir.join("source");
    std::fs::create_dir_all(&source).expect("source dir");
    let requested = RequestedBase::parse("refs/heads/main").expect("ref");
    WorkspaceController::new(&store, &DirMakerMaterializer, &workspace_root)
        .ensure(
            &WorkspaceCommand {
                epoch: epoch.as_str(),
                id: "ws-1",
                request_hash: &[5u8; 32],
            },
            "workspace-1",
            "task-1",
            &WorkspaceRequest {
                source: &source,
                requested_base: &requested,
            },
        )
        .expect("the workspace becomes Ready");

    // T3 with the same frozen facts the scheduler commits.
    let active = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active");
    let snap = store.scheduling_snapshot().expect("snapshot");
    let candidate = snap.candidates.first().expect("dispatchable").clone();
    let agent = LogicalAgentVersion {
        name: "builder".to_string(),
        version: 1,
    };
    let binding = ExecutionBinding {
        task_id: "task-1".to_string(),
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
        task_spec_digest: candidate.spec_digest,
        goal_id: candidate.goal_id.clone(),
        goal_revision: candidate.goal_current_revision,
        graph_revision: candidate.graph_revision,
        agent,
        configuration_activation_sequence: active.activation_sequence,
        context_policy_digest: active.components.context_policy,
        agent_soul_digest: guidance_digest("Careful coding agent identity."),
        agent_behavior_digest: guidance_digest("Plan first; keep changes minimal."),
        workspace_id: "workspace-1".to_string(),
        workspace_resolved_base: BASE.to_string(),
    };
    let binding_digest = binding.digest();
    let snapshot_digest = snapshot.digest();
    let intent = RunIntent {
        run_id: "run-1",
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
        binding: &binding,
        snapshot_digest: &snapshot_digest,
        snapshot: &snapshot,
    };
    store
        .commit_run_intent(
            &command(epoch.as_str(), "cmd-t3", &[9u8; 32], "run.committed"),
            &intent,
        )
        .expect("the Run intent commits");

    // T3a: the one-time ContextPlan attachment T4 requires.
    let plan_canonical = Value::object([
        ("builder", Value::string(CONTEXT_BUILDER_VERSION)),
        ("fixture", Value::string(label)),
    ])
    .to_canonical_bytes();
    let plan_digest = Digest::of(&plan_canonical);
    let plan_json = String::from_utf8(plan_canonical).expect("utf-8");
    store
        .attach_run_context_plan(
            &command(
                epoch.as_str(),
                "cmd-t3a",
                &[21u8; 32],
                "run.context.attached",
            ),
            &pantheon_store::ContextPlanAttachment {
                run_id: "run-1",
                source_snapshot_digest: &snapshot_digest,
                plan_digest: &plan_digest,
                builder_version: CONTEXT_BUILDER_VERSION,
                plan_canonical_json: &plan_json,
            },
        )
        .expect("attachment commits");

    // T4: the Attempt-bound session this worker authenticates through.
    let random = FixedRandom(AtomicU64::new(seed));
    let bearer = Bearer::generate(&random).expect("entropy");
    let view = store.run_execution_view("run-1").expect("view").unwrap();
    store
        .create_attempt(
            &command(
                epoch.as_str(),
                &format!("cmd-t4-{label}"),
                &[31u8; 32],
                "run.attempt.created",
            ),
            &pantheon_store::AttemptCreation {
                run_id: "run-1",
                attempt_id: ATTEMPT,
                launch_key: &format!("{seed:02x}").repeat(32),
                session_id: "acs-1",
                credential_verifier: &bearer.verifier(),
                expected_run_status_revision: view.revision,
            },
        )
        .expect("T4 commits");

    World {
        db_path: dir.join("pantheon.db"),
        store,
        workspace_root,
        bearer,
    }
}

fn worker<'a>(world: &'a World) -> WorkerCredential<'a> {
    WorkerCredential {
        attempt_id: ATTEMPT,
        bearer: &world.bearer,
    }
}

fn capture() -> StaticCapture {
    StaticCapture {
        entries: vec![
            (
                b"app.txt".to_vec(),
                EntryKind::Regular,
                b"fixed" as &'static [u8],
            ),
            (
                b"src/new.txt".to_vec(),
                EntryKind::Regular,
                b"brand new" as &'static [u8],
            ),
        ],
    }
}

fn gateway<'a>(
    world: &'a World,
    capture: &'a StaticCapture,
    cas: &'a MemoryCas,
) -> AgentControlGateway<'a> {
    AgentControlGateway::new(
        &world.store,
        capture,
        &MemoryBase,
        cas,
        world.workspace_root.clone(),
    )
}

#[test]
fn describe_seal_and_submit_drive_the_whole_vertical_slice() {
    let world = world("vertical", 11);
    let capture = capture();
    let cas = MemoryCas::new();
    let gateway = gateway(&world, &capture, &cas);
    let credential = worker(&world);

    let description = gateway.describe(&credential).expect("describe");
    assert_eq!(description.task_phase, "Active");
    assert_eq!(description.outputs.len(), 1);
    assert_eq!(description.outputs[0].name, "changeset");

    let sealed = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("seal executes");
    let AgentSealOutcome::Executed(sealed) = sealed else {
        panic!("a fresh seal executes, got {sealed:?}");
    };

    // Provenance is proven behaviorally: submission of exactly this digest
    // succeeds only because T6 resolves it through THIS lineage's
    // ProductionRecord (the store-level suite queries that row directly).
    let submitted = gateway
        .submit_result(
            &credential,
            &SubmitResultRequest {
                request_id: "req-submit",
                expected_task_revision: description.task_revision,
                outputs: vec![("changeset".to_string(), sealed.artifact_digest)],
            },
        )
        .expect("submission commits");
    assert!(!submitted.reconciled);

    // The lifecycle moved atomically with the Candidate: a second, different
    // request now loses deterministically on current authority...
    let conflict = gateway.submit_result(
        &credential,
        &SubmitResultRequest {
            request_id: "req-second",
            expected_task_revision: description.task_revision,
            outputs: vec![("changeset".to_string(), sealed.artifact_digest)],
        },
    );
    assert!(
        matches!(
            &conflict,
            Err(AgentControlError::Store(
                pantheon_store::StoreError::SubmissionStaleAuthority { .. }
            ))
        ),
        "{conflict:?}"
    );
    // ...and the authenticated session no longer carries Run/Task authority.
    let fenced = gateway.describe(&credential);
    assert!(
        matches!(fenced, Err(AgentControlError::Store(_))),
        "{fenced:?}"
    );
}

#[test]
fn response_loss_after_a_successful_seal_returns_the_recorded_result_without_recapture() {
    let world = world("response-loss", 12);
    let capture = capture();
    let cas = MemoryCas::new();
    let gateway = gateway(&world, &capture, &cas);
    let credential = worker(&world);

    let first = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("first seal executes");
    let AgentSealOutcome::Executed(first) = first else {
        panic!("expected execution, got {first:?}")
    };
    let publishes_after_first = cas.publishes.get();

    let second = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("retry reconciles");
    assert_eq!(second, AgentSealOutcome::Reconciled(first.artifact_digest));
    assert_eq!(
        cas.publishes.get(),
        publishes_after_first,
        "a recorded outcome short-circuits before any external work"
    );
}

#[test]
fn a_crash_after_the_freeze_converges_on_the_same_artifact_and_one_record() {
    let world = world("crash-frozen", 13);
    let capture = capture();
    let cas = MemoryCas::new();
    let credential = worker(&world);

    // Simulate the crash window by hand: the durable request row exists and
    // the Workspace is already fenced, but capture never ran.
    let verifier = credential.verifier();
    let request_hash = canonical_request_hash(
        "artifact.seal",
        ATTEMPT,
        "req-seal",
        &[("outputSlot", "changeset".into())],
    );
    let view = pantheon_store::AgentCredential {
        attempt_id: ATTEMPT,
        verifier: &verifier,
    };
    world
        .store
        .open_agent_request(
            view,
            pantheon_store::AgentOperation::SealArtifact,
            "req-seal",
            &request_hash,
        )
        .expect("row opens");
    let ws = world
        .store
        .workspace_for_task("task-1")
        .expect("workspace readable")
        .expect("workspace exists");
    let run_view = world
        .store
        .run_execution_view("run-1")
        .expect("view")
        .unwrap();
    let authority = StoreSealAuthority {
        run_id: "run-1".to_string(),
        expected_run_revision: run_view.revision,
    };
    let epoch = world.store.restore_generation().expect("generation");
    world
        .store
        .freeze_workspace(
            &command(
                epoch.as_str(),
                "cmd-freeze",
                &[44u8; 32],
                "workspace.frozen",
            ),
            &authority,
            "task-1",
            "changeset",
            &ws.id,
            ws.revision,
        )
        .expect("freeze commits");

    // Restart-equivalent: a fresh gateway drives the same request id.
    let gateway = gateway(&world, &capture, &cas);
    let outcome = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("recovery converges");
    let AgentSealOutcome::Executed(sealed) = outcome else {
        panic!("expected execution after recovery, got {outcome:?}")
    };

    // Convergence evidence: the recovered request's outcome reconciles on
    // the same identity.
    let retry = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("recorded outcome");
    assert_eq!(retry, AgentSealOutcome::Reconciled(sealed.artifact_digest));
}

#[test]
fn a_second_request_over_the_same_frozen_workspace_converges_on_one_identity() {
    let world = world("converge-two-requests", 20);
    let capture = capture();
    let cas = MemoryCas::new();
    let gateway = gateway(&world, &capture, &cas);
    let credential = worker(&world);

    let first = gateway
        .seal_artifact(&credential, "req-a", "changeset")
        .expect("first seal");
    let AgentSealOutcome::Executed(first) = first else {
        panic!("expected execution, got {first:?}")
    };

    // A different request id over the identical frozen state: content
    // identity converges, provenance reconciles instead of conflicting.
    let second = gateway
        .seal_artifact(&credential, "req-b", "changeset")
        .expect("second seal converges");
    let AgentSealOutcome::Executed(second) = second else {
        panic!("expected execution, got {second:?}")
    };
    assert_eq!(second.artifact_digest, first.artifact_digest);
    assert!(second.artifact_reused, "one content identity serves both");
}

#[test]
fn a_crash_after_publication_before_the_result_converges_without_republishing() {
    let world = world("crash-published", 14);
    let capture = capture();
    let cas = MemoryCas::new();
    let credential = worker(&world);

    // Crash window: request row recorded, freeze + capture + publication all
    // committed under the derived command identity — then the process died
    // before the result was recorded.
    let verifier = credential.verifier();
    let request_hash = canonical_request_hash(
        "artifact.seal",
        ATTEMPT,
        "req-seal",
        &[("outputSlot", "changeset".into())],
    );
    let view = pantheon_store::AgentCredential {
        attempt_id: ATTEMPT,
        verifier: &verifier,
    };
    world
        .store
        .open_agent_request(
            view,
            pantheon_store::AgentOperation::SealArtifact,
            "req-seal",
            &request_hash,
        )
        .expect("row opens");

    let description = {
        let gateway = gateway(&world, &capture, &cas);
        gateway.describe(&credential).expect("describe")
    };
    let run_view = world
        .store
        .run_execution_view("run-1")
        .expect("view")
        .unwrap();
    let command_id = internal_command_id(ATTEMPT, "req-seal", &request_hash);
    let epoch = world.store.restore_generation().expect("generation");
    let sealer = ChangesetSealer::new(
        &world.store,
        &capture,
        &MemoryBase,
        &cas,
        world.workspace_root.clone(),
    );
    sealer
        .seal(
            &SealCommand {
                epoch: epoch.as_str(),
                id: &command_id,
                request_hash: &request_hash,
            },
            &SealRequest {
                task_id: &description.task_id,
                output_slot: "changeset",
                authority: StoreSealAuthority {
                    run_id: description.run_id.clone(),
                    expected_run_revision: run_view.revision,
                },
                producer_attempt_id: Some(ATTEMPT),
            },
        )
        .expect("publication commits");
    let objects_after_crash = cas.objects.borrow().len();

    // Restart-equivalent retry: the ledger replays publication, the gateway
    // records the result it can recompute deterministically.
    let gateway = gateway(&world, &capture, &cas);
    let outcome = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("recovery converges");
    let AgentSealOutcome::Executed(recovered) = outcome else {
        panic!("expected execution after recovery, got {outcome:?}")
    };
    let sealed_digest = recovered.artifact_digest;
    assert_eq!(
        cas.objects.borrow().len(),
        objects_after_crash,
        "recovery converges on the same content identity: no new objects"
    );
    // And exactly one provenance row exists for this lineage and slot —
    // proven behaviorally: the reconciled digest still submits cleanly.
    let description = gateway.describe(&credential).expect("describe");
    let submitted = gateway
        .submit_result(
            &credential,
            &SubmitResultRequest {
                request_id: "req-submit",
                expected_task_revision: description.task_revision,
                outputs: vec![("changeset".to_string(), sealed_digest)],
            },
        )
        .expect("the recovered seal submits");
    assert!(!submitted.reconciled);
}

#[test]
fn restart_preserves_session_identity_and_returns_existing_authority_exactly_once() {
    let label = "restart";
    let world = world(label, 15);
    let first_capture = capture();
    let first_cas = MemoryCas::new();
    {
        let credential = worker(&world);
        gateway(&world, &first_capture, &first_cas)
            .seal_artifact(&credential, "req-seal", "changeset")
            .expect("seals");
    }

    // Ordinary controller restart: close the store and reopen the same
    // installation; no startup-only repair path exists or is used.
    let db_path = world.db_path.clone();
    let workspace_root = world.workspace_root.clone();
    let bearer_hex = world.bearer.expose().to_string();
    drop(world.store);

    let reopened = Store::open(&db_path).expect("reopen");
    let reopened_world = ReopenedWorld {
        _dir_guard: (),
        store: reopened,
        workspace_root,
    };
    let second_capture = capture();
    let second_cas = MemoryCas::new();
    let gateway = AgentControlGateway::new(
        &reopened_world.store,
        &second_capture,
        &MemoryBase,
        &second_cas,
        reopened_world.workspace_root.clone(),
    );
    // The bearer is regenerated from the same deterministic entropy, standing
    // in for a worker that kept its launch material across the restart.
    let regenerated = Bearer::generate(&FixedRandom(AtomicU64::new(15))).expect("entropy");
    assert_eq!(regenerated.expose(), bearer_hex);
    let credential = WorkerCredential {
        attempt_id: ATTEMPT,
        bearer: &regenerated,
    };

    // The persisted current verifier still authenticates the same bearer.
    let description = gateway.describe(&credential).expect("same identity");
    assert_eq!(description.task_phase, "Active");

    // The recorded seal outcome is returned once, without recapture.
    let outcome = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("reconciles after restart");
    assert!(matches!(outcome, AgentSealOutcome::Reconciled(_)));
    assert_eq!(second_cas.publishes.get(), 0);

    // And submission proceeds on the reconciled lineage, exactly once.
    let digest = match outcome {
        AgentSealOutcome::Reconciled(digest) => digest,
        other => panic!("expected reconcile, got {other:?}"),
    };
    let outputs = vec![("changeset".to_string(), digest)];
    let submitted = gateway
        .submit_result(
            &credential,
            &SubmitResultRequest {
                request_id: "req-submit",
                expected_task_revision: description.task_revision,
                outputs: outputs.clone(),
            },
        )
        .expect("submits after restart");
    assert!(!submitted.reconciled);

    // Exactly-once authority across the same restarted process: an identical
    // replay reconciles from the ledger even though lifecycle has moved on.
    let replay = gateway
        .submit_result(
            &credential,
            &SubmitResultRequest {
                request_id: "req-submit",
                expected_task_revision: description.task_revision,
                outputs,
            },
        )
        .expect("ledger replay wins over lifecycle staleness");
    assert!(replay.reconciled);
    assert_eq!(replay.candidate_digest, submitted.candidate_digest);
}

struct ReopenedWorld {
    _dir_guard: (),
    store: Store,
    workspace_root: PathBuf,
}

#[test]
fn a_payload_cannot_reference_content_that_does_not_exist() {
    // The worker names Artifacts by digest alone; the type system carries no
    // Task/Run/Workspace fields it could select authority with, and T6 only
    // admits digests that exist AND carry this lineage's ProductionRecord
    // (proven with real rows in the store-level suite).
    let world = world("foreign-digest", 16);
    let my_capture = capture();
    let my_cas = MemoryCas::new();
    let gateway = gateway(&world, &my_capture, &my_cas);
    let credential = worker(&world);
    let description = gateway.describe(&credential).expect("describe");

    let forged = Digest::of(b"content-no-lineage-ever-sealed");
    let error = gateway
        .submit_result(
            &credential,
            &SubmitResultRequest {
                request_id: "req-forged",
                expected_task_revision: description.task_revision,
                outputs: vec![("changeset".to_string(), forged)],
            },
        )
        .expect_err("unsealed content cannot be submitted");
    assert!(
        matches!(
            &error,
            AgentControlError::Store(pantheon_store::StoreError::CandidateInvalid { .. })
        ),
        "{error:?}"
    );
    // The refusal wrote nothing and left the lineage authorized.
    let own = gateway.describe(&credential).expect("still authorized");
    assert_eq!(own.task_phase, "Active");
}

#[test]
fn raw_bearer_material_never_reaches_disk_or_debug_rendering() {
    let world = world("bearer-hygiene", 18);
    let capture = capture();
    let cas = MemoryCas::new();
    let gateway = gateway(&world, &capture, &cas);
    let credential = worker(&world);
    gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("seals");

    let hex = world.bearer.expose();
    assert_eq!(hex.len(), 64, "fixture sanity");

    // Debug rendering redacts.
    let rendered = format!("{credential:?}");
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
    assert!(!rendered.contains(hex), "{rendered}");

    // The database file never contains the raw bytes — scanned directly, not
    // through any view.
    let bytes = std::fs::read(&world.db_path).expect("db file");
    let needle = hex.as_bytes();
    assert!(
        !bytes.windows(needle.len()).any(|window| window == needle),
        "raw bearer material found persisted"
    );
}
