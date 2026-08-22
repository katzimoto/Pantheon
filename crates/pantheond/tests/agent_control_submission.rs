//! End-to-end evidence for Issue #33, composed exactly as the daemon
//! composes it: real durable authority, the real confined capture boundary,
//! the real sterile base reader, the real local CAS — driven entirely
//! through the Attempt-bound Agent Control gateway.
//!
//! The demonstrated product proof:
//!
//! ```text
//! current Attempt A / AgentControlSession S
//!   -> worker requests sealing of current Workspace   (artifact.seal)
//!   -> immutable code.changeset Artifact C exists,
//!      carrying THIS lineage's ProductionRecord
//!   -> task.submit_result(expected Task revision, outputs={changeset:C})
//!   -> authoritative T6 rechecks A/Run/Task authority
//!   -> Candidate created exactly once
//!   -> Run -> Finalizing(target=Completed), slot retained
//!   -> Task -> Evaluating
//! ```
//!
//! Deep row-level facts, migration behavior and refusal matrices are proven
//! in `pantheon-store`; this test proves the composition works against the
//! concrete backends and that the worker surface alone can drive it.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use pantheon_cas::LocalFsCas;
use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value;
use pantheon_core::context::{CONTEXT_BUILDER_VERSION, guidance_digest};
use pantheon_core::execution::LogicalAgentVersion;
use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::scheduling::{ContextSourceSnapshot, ExecutionBinding};
use pantheon_engine::agent_control::{
    AgentControlGateway, AgentSealOutcome, SubmitResultRequest, WorkerCredential,
};
use pantheon_engine::configuration::{ConfigurationAuthority, SourceSet};
use pantheon_engine::planning::PlanningController;
use pantheon_engine::run::{Bearer, OsRandom};
use pantheon_engine::workspace::{WorkspaceCommand, WorkspaceController, WorkspaceRequest};
use pantheon_git::{ConfinedCapture, GitBaseReader, GitMaterializer};
use pantheon_store::{AttemptCreation, Command, Revision, RunIntent, Store};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheond-agent-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
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
    "accepts":["code.change"],"competencies":["code.analysis","code.editing","test.execution"],"routePolicy":"default",
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
            reference: "repo://whiskyshop".to_string(),
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

/// Drives planning to the one Ready coding Task, then T3 to its Active Run,
/// then T3a/T4 so an authenticated Attempt exists — returning everything the
/// gateway and the worker need.
#[allow(clippy::too_many_lines)]
fn ready_lineage(store: &Store, dir: &TempDir, seed: u8) -> (String /* attempt id */, Bearer) {
    let epoch = store.restore_generation().expect("generation");
    ConfigurationAuthority::new(store)
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

    let planning = PlanningController::new(store);
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

    // The worker's own Workspace, materialized through the real Git
    // materializer from the sterile control repo.
    let control = dir.path().join("control");
    let workspace_root = dir.path().join("workspaces");
    let requested = pantheon_core::workspace::RequestedBase::parse("refs/heads/main").expect("ref");
    WorkspaceController::new(
        store,
        &GitMaterializer::new(&control).expect("materializer"),
        &workspace_root,
    )
    .ensure(
        &WorkspaceCommand {
            epoch: epoch.as_str(),
            id: "ws-1",
            request_hash: &[5u8; 32],
        },
        "workspace-1",
        "task-1",
        &WorkspaceRequest {
            source: &dir.path().join("source"),
            requested_base: &requested,
        },
    )
    .expect("the workspace becomes Ready");

    // T3 with the frozen facts the scheduler would commit.
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
        task_id: candidate.task_id.clone(),
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
    let workspace = store
        .workspace_for_task(&candidate.task_id)
        .expect("read")
        .expect("the Task owns a Workspace");
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
        workspace_id: workspace.id.clone(),
        workspace_resolved_base: workspace.resolved_base.as_str().to_string(),
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
        ("fixture", Value::string("agent-control-e2e")),
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

    // T4: the Attempt-bound session whose bearer the worker will present.
    let bearer = Bearer::generate(&OsRandom).expect("entropy");
    let view = store.run_execution_view("run-1").expect("view").unwrap();
    store
        .create_attempt(
            &command(epoch.as_str(), "cmd-t4", &[31u8; 32], "run.attempt.created"),
            &AttemptCreation {
                run_id: "run-1",
                attempt_id: "attempt-1",
                launch_key: &format!("{seed:02x}").repeat(32),
                session_id: "acs-1",
                credential_verifier: &bearer.verifier(),
                expected_run_status_revision: view.revision,
            },
        )
        .expect("T4 commits");

    ("attempt-1".to_string(), bearer)
}

#[test]
fn a_worker_drives_seal_and_submission_through_agent_control_alone() {
    let dir = TempDir::new("e2e");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    git(&source, &["init", "--quiet", "-b", "main"]);
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);

    let cas_root = dir.path().join("cas");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    let (attempt_id, bearer) = ready_lineage(&store, &dir, 7);

    // The worker edits its Workspace: modify one tracked file, add another.
    let repo = dir
        .path()
        .join("workspaces")
        .join("workspace-1")
        .join("repo");
    std::fs::write(repo.join("app.txt"), b"fixed\n").expect("modify");
    std::fs::create_dir_all(repo.join("src")).expect("src");
    std::fs::write(repo.join("src/new.rs"), b"brand new\n").expect("add");

    // The worker-facing gateway over exactly the concrete ports the daemon
    // wires. No operator surface is reachable from here by construction.
    let capture = ConfinedCapture::new();
    let base_reader =
        GitBaseReader::new(&dir.path().join("workspaces/workspace-1/repo")).expect("base reader");
    let cas = LocalFsCas::open(&cas_root).expect("cas");
    let gateway = AgentControlGateway::new(
        &store,
        &capture,
        &base_reader,
        &cas,
        dir.path().join("workspaces"),
    );
    let credential = WorkerCredential {
        attempt_id: &attempt_id,
        bearer: &bearer,
    };

    // 1. The only description the worker may obtain.
    let description = gateway.describe(&credential).expect("describe");
    assert_eq!(description.run_id, "run-1");
    assert_eq!(description.outputs.len(), 1);

    // 2. artifact.seal of the declared output slot.
    let outcome = gateway
        .seal_artifact(&credential, "req-seal", "changeset")
        .expect("seal executes");
    let AgentSealOutcome::Executed(sealed) = outcome else {
        panic!("expected execution, got {outcome:?}")
    };
    let artifact = store
        .artifact(sealed.artifact_digest)
        .expect("read artifact")
        .expect("the sealed Artifact is durable");
    assert_eq!(artifact.kind, "code.changeset");
    assert!(artifact.canonical_json.contains(r#""operation":"modify""#));
    assert!(artifact.canonical_json.contains("app.txt"));

    // 3. task.submit_result at the observed Task revision.
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

    // 4. The post-commit world, seen only through worker-visible effects:
    // the Run left Active, so the same payload under a new identity loses
    // deterministically, while the exact replay still reconciles.
    let second = gateway.submit_result(
        &credential,
        &SubmitResultRequest {
            request_id: "req-second",
            expected_task_revision: description.task_revision,
            outputs: vec![("changeset".to_string(), sealed.artifact_digest)],
        },
    );
    assert!(
        matches!(
            &second,
            Err(pantheon_engine::agent_control::AgentControlError::Store(
                pantheon_store::StoreError::SubmissionStaleAuthority { .. }
            ))
        ),
        "{second:?}"
    );

    // 5. Response-loss recovery for the submission itself: exact replay.
    let replay = gateway
        .submit_result(
            &credential,
            &SubmitResultRequest {
                request_id: "req-submit",
                expected_task_revision: description.task_revision,
                outputs: vec![("changeset".to_string(), sealed.artifact_digest)],
            },
        )
        .expect("replay reconciles");
    assert!(replay.reconciled);
    assert_eq!(replay.candidate_digest, submitted.candidate_digest);

    // 6. And the sealed Artifact remains self-contained content in CAS.
    let member_count = artifact.canonical_json.matches("\"blob\"").count();
    assert!(member_count >= 3, "modify+add changeset carries payloads");
}
