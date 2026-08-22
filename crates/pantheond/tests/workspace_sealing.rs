//! End-to-end evidence for Issues #32 and #76, composed the way the daemon
//! composes it: real durable authority, the real confined capture boundary,
//! the real sterile base reader and the real local CAS.
//!
//! The sequence is the one the missions ask to be demonstrated:
//!
//! ```text
//! Ready coding Task T at immutable base B
//!   → T3 commits: T becomes Active under its current Run R
//!   → worker edits/commits/stages arbitrary local state
//!   → quiesce W (freeze) under R's authority; pin the trusted capture root
//!   → derive before-state from trusted B through sterile Git
//!   → derive after-state from confined no-follow reads of W
//!   → write required bytes to CAS
//!   → commit immutable WorkspaceRevision + code.changeset Artifact,
//!     revalidating R inside the publication transaction
//!   → remove the Workspace and its worker-local Git history entirely
//!   → the Artifact remains self-contained for every changed path
//! ```
//!
//! Hostile fixtures prove the trust boundary holds while doing it: sentinel
//! executables wired into every Git execution surface a worker can write,
//! and an outward symlink whose target content must never enter CAS.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use pantheon_cas::LocalFsCas;
use pantheon_core::config::Digest;
use pantheon_core::execution::LogicalAgentVersion;
use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::scheduling::{ContextSourceSnapshot, ExecutionBinding};
use pantheon_engine::configuration::{ConfigurationAuthority, SourceSet};
use pantheon_engine::planning::PlanningController;
use pantheon_engine::sealing::{
    ChangesetSealer, ContentObjectStore, ExternalFault, SealCommand, SealRequest,
    TrustedBaseReader, WorkspaceTreeCapture,
};
use pantheon_engine::workspace::{WorkspaceCommand, WorkspaceController, WorkspaceRequest};
use pantheon_git::{ConfinedCapture, GitBaseReader, GitMaterializer};
use pantheon_store::{Command, Revision, RunIntent, SealAuthority, Store};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheond-sealing-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
            permitted_resources: resources.iter().map(|r| r.to_string()).collect(),
        },
    }
}

/// Drives #23/#24/#27 to produce the one Ready coding Task this mission's
/// sealing belongs to.
fn ready_coding_task(store: &Store, resources: &[&str]) {
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
    let spec = goal_spec(resources);
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
}

fn ensure_workspace(
    store: &Store,
    materializer: &GitMaterializer,
    root: &Path,
    source: &Path,
    request_id: &str,
) -> pantheon_store::WorkspaceRecord {
    ensure_workspace_with(store, materializer, root, source, request_id)
}

fn ensure_workspace_with(
    store: &Store,
    materializer: &impl pantheon_engine::workspace::RepositoryMaterializer,
    root: &Path,
    source: &Path,
    request_id: &str,
) -> pantheon_store::WorkspaceRecord {
    let epoch = store.restore_generation().expect("generation");
    let requested = pantheon_core::workspace::RequestedBase::parse("refs/heads/main").expect("ref");
    WorkspaceController::new(store, materializer, root)
        .ensure(
            &WorkspaceCommand {
                epoch: epoch.as_str(),
                id: request_id,
                request_hash: &[5u8; 32],
            },
            "workspace-1",
            "task-1",
            &WorkspaceRequest {
                source,
                requested_base: &requested,
            },
        )
        .expect("the workspace becomes Ready")
}

fn sealer<'a>(
    store: &'a Store,
    capture: &'a dyn WorkspaceTreeCapture,
    base: &'a dyn TrustedBaseReader,
    cas: &'a LocalFsCas,
    workspace_root: &'a Path,
) -> ChangesetSealer<'a> {
    ChangesetSealer::new(store, capture, base, cas, workspace_root)
}

/// Dispatches the Ready coding Task through T3, the way the daemon's
/// scheduler does, so sealing runs under a real durable Run relation
/// (`run-1`, status revision 1) rather than a bare Task.
fn dispatch(store: &Store) {
    let pointer = store.configuration_pointer().expect("pointer");
    let active = pointer.active.as_ref().expect("active configuration");
    let snap = store.scheduling_snapshot().expect("snapshot");
    let candidate = snap
        .candidates
        .first()
        .expect("a dispatchable Task")
        .clone();

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
        // The guidance digests the fixture configuration's agents component
        // actually carries, so T3's frozen-guidance validation accepts it.
        agent_soul_digest: pantheon_core::context::guidance_digest(
            "Careful coding agent identity.",
        ),
        agent_behavior_digest: pantheon_core::context::guidance_digest(
            "Plan first; keep changes minimal.",
        ),
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
    let epoch = store.restore_generation().expect("generation");
    store
        .commit_run_intent(
            &command(epoch.as_str(), "cmd-t3", &[9u8; 32], "run.committed"),
            &intent,
        )
        .expect("the Run intent commits");
}

fn seal(
    store: &Store,
    capture: &dyn WorkspaceTreeCapture,
    base: &dyn TrustedBaseReader,
    cas: &LocalFsCas,
    workspace_root: &Path,
    command_id: &str,
) -> Result<pantheon_engine::sealing::SealedArtifact, pantheon_engine::sealing::SealError> {
    let epoch = store.restore_generation().expect("generation");
    sealer(store, capture, base, cas, workspace_root).seal(
        &SealCommand {
            epoch: epoch.as_str(),
            id: command_id,
            request_hash: &[6u8; 32],
        },
        &SealRequest {
            task_id: "task-1",
            output_slot: "changeset",
            authority: SealAuthority {
                run_id: "run-1".to_string(),
                expected_run_revision: Revision::new(1),
            },
        },
    )
}

/// One reconstructed changed path: manifest spelling, before bytes, after
/// bytes — `None` where the manifest says absent.
type ChangedPath = (String, Option<Vec<u8>>, Option<Vec<u8>>);

/// Reads every object in the CAS back through the store's Artifact record
/// plus the manifest's digests, reconstructing each changed path's
/// before/after semantics.
fn reconstruct(store: &Store, cas: &LocalFsCas, artifact_digest: Digest) -> Vec<ChangedPath> {
    let artifact = store
        .artifact(artifact_digest)
        .expect("read")
        .expect("sealed");
    let value = pantheon_core::config::parse::parse(&artifact.canonical_json)
        .expect("canonical manifest parses");
    let entries = match value.get("entries") {
        Some(pantheon_core::config::canonical::Value::Array(entries)) => entries.clone(),
        other => panic!("manifest entries missing: {other:?}"),
    };
    entries
        .into_iter()
        .map(|entry| {
            let path = match entry.get("path") {
                Some(pantheon_core::config::canonical::Value::String(text)) => text.clone(),
                other => panic!("entry path malformed: {other:?}"),
            };
            let side = |name: &str| match entry.get(name) {
                Some(side) => match side.get("state") {
                    Some(pantheon_core::config::canonical::Value::String(state))
                        if state == "present" =>
                    {
                        let blob = match side.get("blob") {
                            Some(pantheon_core::config::canonical::Value::String(digest)) => {
                                Digest::from_display(digest).expect("digest")
                            }
                            other => panic!("blob malformed: {other:?}"),
                        };
                        Some(
                            cas.read(&pantheon_engine::sealing::ObjectRef {
                                digest: blob,
                                size: match side.get("size") {
                                    Some(pantheon_core::config::canonical::Value::Integer(n)) => {
                                        *n as u64
                                    }
                                    other => panic!("size malformed: {other:?}"),
                                },
                            })
                            .expect("CAS-complete payload"),
                        )
                    }
                    _ => None,
                },
                None => None,
            };
            (path, side("before"), side("after"))
        })
        .collect()
}

#[test]
fn a_sealed_changeset_is_cas_complete_and_survives_deletion_of_the_workspace() {
    let dir = TempDir::new("complete");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    git(&source, &["init", "--quiet", "-b", "main"]);
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    std::fs::write(source.join("gone.txt"), b"doomed\n").expect("write");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);
    let base_oid = git(&source, &["rev-parse", "HEAD"]);

    let control = dir.path().join("control");
    let workspace_root = dir.path().join("workspaces");
    let cas_root = dir.path().join("cas");
    let materializer = GitMaterializer::new(&control).expect("materializer");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    ready_coding_task(&store, &["workspace://**"]);

    ensure_workspace(&store, &materializer, &workspace_root, &source, "ws-req-1");
    dispatch(&store);

    // The worker does ordinary, messy coding work: edit, add, delete, link,
    // chmod — then stages half of it and commits on a branch for good
    // measure. None of that metadata may influence what gets sealed.
    let repo = workspace_root.join("workspace-1").join("repo");
    std::fs::write(repo.join("app.txt"), b"fixed\n").expect("modify");
    std::fs::create_dir_all(repo.join("src")).expect("src");
    std::fs::write(repo.join("src/new.rs"), b"brand new\n").expect("add");
    std::fs::remove_file(repo.join("gone.txt")).expect("delete");
    #[cfg(unix)]
    std::os::unix::fs::symlink("../app.txt", repo.join("src/latest")).expect("symlink");
    std::fs::set_permissions(
        repo.join("src/new.rs"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    git(&repo, &["add", "app.txt"]); // staged...
    std::fs::write(repo.join("unstaged.txt"), b"unstaged work\n").expect("unstaged");
    git(&repo, &["checkout", "--quiet", "-b", "worker/topic"]);
    git(&repo, &["commit", "--quiet", "-m", "worker checkpoint"]); // committed...

    let cas = LocalFsCas::open(&cas_root).expect("cas");
    let base_reader = GitBaseReader::new(&control).expect("base reader");
    let capture = ConfinedCapture::new();

    let sealed = seal(
        &store,
        &capture,
        &base_reader,
        &cas,
        &workspace_root,
        "seal-1",
    )
    .expect("seals");

    // The manifest binds the immutable base the Workspace was opened at.
    let artifact = store
        .artifact(sealed.artifact_digest)
        .expect("read")
        .expect("exists");
    assert!(artifact.canonical_json.contains(base_oid.as_str()));

    // Now destroy everything the worker touched: the whole Workspace,
    // including its local history and object database.
    std::fs::remove_dir_all(workspace_root.join("workspace-1")).expect("destroy workspace");

    // Every changed path still reconstructs exactly, before and after.
    let reconstructed = reconstruct(&store, &cas, sealed.artifact_digest);
    let find = |path: &str| {
        reconstructed
            .iter()
            .find(|(name, _, _)| name == path)
            .unwrap_or_else(|| panic!("{path} missing from reconstruction"))
    };

    let (_, app_before, app_after) = find("app.txt");
    assert_eq!(app_before.as_deref(), Some(b"original\n".as_slice()));
    assert_eq!(app_after.as_deref(), Some(b"fixed\n".as_slice()));

    let (_, new_before, new_after) = find("src/new.rs");
    assert!(new_before.is_none(), "an added path has no preimage");
    assert_eq!(new_after.as_deref(), Some(b"brand new\n".as_slice()));

    let (_, gone_before, gone_after) = find("gone.txt");
    assert_eq!(gone_before.as_deref(), Some(b"doomed\n".as_slice()));
    assert!(gone_after.is_none(), "a deleted path has no result");

    let (_, latest_before, latest_after) = find("src/latest");
    assert!(latest_before.is_none());
    // Link-target bytes, not dereferenced content.
    assert_eq!(
        latest_after.as_deref(),
        Some(b"../app.txt".as_slice()),
        "a symlink seals as its own bytes"
    );

    // The unstaged file was part of the permitted tree too (it changed), so
    // it is sealed as an add — staging state is irrelevant, tree state is
    // authoritative.
    let (_, _, unstaged_after) = find("unstaged.txt");
    assert_eq!(
        unstaged_after.as_deref(),
        Some(b"unstaged work\n".as_slice())
    );
}

#[test]
fn identity_ignores_worker_commits_branches_and_staging() {
    let dir = TempDir::new("identity");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    git(&source, &["init", "--quiet", "-b", "main"]);
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);

    let control = dir.path().join("control");
    let workspace_root = dir.path().join("workspaces");
    let cas_root = dir.path().join("cas");
    let materializer = GitMaterializer::new(&control).expect("materializer");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    ready_coding_task(&store, &["workspace://**"]);
    ensure_workspace(&store, &materializer, &workspace_root, &source, "ws-req-1");
    dispatch(&store);

    let repo = workspace_root.join("workspace-1").join("repo");
    std::fs::write(repo.join("app.txt"), b"fixed\n").expect("modify");

    let cas = LocalFsCas::open(&cas_root).expect("cas");
    let base_reader = GitBaseReader::new(&control).expect("base reader");
    let capture = ConfinedCapture::new();

    let first = seal(
        &store,
        &capture,
        &base_reader,
        &cas,
        &workspace_root,
        "seal-A",
    )
    .expect("first seal");

    // Move every piece of Git metadata a worker can move — new branch,
    // local commit, staged-and-unstaged churn — without changing one byte
    // of working-tree content.
    git(&repo, &["checkout", "--quiet", "-b", "another-branch"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "worker checkpoint"]);
    assert_ne!(
        git(&repo, &["rev-parse", "HEAD"]),
        base_oid_of(&source),
        "fixture moved HEAD"
    );

    let second = seal(
        &store,
        &capture,
        &base_reader,
        &cas,
        &workspace_root,
        "seal-B",
    )
    .expect("second seal");

    assert_eq!(
        first.artifact_digest, second.artifact_digest,
        "worker commits, branch names and staging state are not identity"
    );
}

fn base_oid_of(source: &Path) -> String {
    git(source, &["rev-parse", "HEAD"])
}

#[test]
fn hostile_git_control_state_is_inert_data_and_source_refs_stay_untouched() {
    let dir = TempDir::new("hostile");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    git(&source, &["init", "--quiet", "-b", "main"]);
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);

    let control = dir.path().join("control");
    let workspace_root = dir.path().join("workspaces");
    let cas_root = dir.path().join("cas");
    let materializer = GitMaterializer::new(&control).expect("materializer");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    ready_coding_task(&store, &["workspace://**"]);
    ensure_workspace(&store, &materializer, &workspace_root, &source, "ws-req-1");
    dispatch(&store);

    let source_refs_before = git(&source, &["show-ref"]);
    let repo = workspace_root.join("workspace-1").join("repo");

    // The worker wires every repository-configurable execution surface Git
    // offers to a sentinel executable. If any of them ran with Pantheon's
    // authority during capture, the marker file appears and this test
    // fails; if any of them was *consulted* as configuration, sealing would
    // behave differently or fail for reasons of the worker's choosing.
    let sentinels = dir.path().join("sentinels");
    std::fs::create_dir_all(sentinels.join("hooks")).expect("sentinel dirs");
    let marker = dir.path().join("HOSTILE-EXEC-MARKER");
    let sentinel = sentinels.join("hook-sentinel.sh");
    std::fs::write(
        &sentinel,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .expect("sentinel script");
    std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    // hooks + configured hooks path
    std::fs::write(sentinels.join("hooks/pre-commit"), b"#!/bin/sh\nexit 1\n").expect("hook");
    std::fs::write(
        sentinels.join("hooks/post-checkout"),
        b"#!/bin/sh\nexit 1\n",
    )
    .expect("hook");

    // recursively included configuration, credential helper, URL rewrite,
    // fsmonitor, diff/textconv, merge driver, filter
    let deeper = sentinels.join("deeper.gitconfig");
    std::fs::write(
        &deeper,
        "[credential] helper = !sh -c 'touch HOSTILE'\n[url \"evil://\"] insteadOf = https://real/\n",
    )
    .expect("deep config");
    std::fs::write(
        repo.join(".git/config"),
        format!(
            "[core]\n\trepositoryformatversion = 0\n\thooksPath = {}\n\
             \tfsmonitor = {} --touch\n[diff]\n\texternal = {}\n\
             [include]\n\tpath = {}\n",
            sentinels.join("hooks").display(),
            sentinel.display(),
            sentinel.display(),
            deeper.display(),
        ),
    )
    .expect("hostile config");
    std::fs::create_dir_all(repo.join(".git/info")).expect("info dir");
    std::fs::write(repo.join(".git/info/attributes"), b"* filter=evil\n").expect("attributes");
    std::fs::write(
        repo.join(".git/config"),
        std::fs::read_to_string(repo.join(".git/config")).expect("reread")
            + &format!(
                "[filter \"evil\"]\n\tsmudge = {}\n\tclean = {}\n",
                sentinel.display(),
                sentinel.display()
            ),
    )
    .expect("filters");

    // alternates pointing outward at an unrelated location
    std::fs::create_dir_all(repo.join(".git/objects/info")).expect("objects info");
    std::fs::write(
        repo.join(".git/objects/info/alternates"),
        "/nonexistent/objects\n",
    )
    .expect("alternates");

    // a corrupt index and corrupt refs must not matter either
    std::fs::write(repo.join(".git/index"), b"\x00corrupt index bytes\x00").expect("index");
    std::fs::write(repo.join(".git/refs/heads/broken"), b"not-an-object\n").expect("ref");

    // Worker content changes, including one file whose *content* is itself
    // hostile-looking Git config — data, never behavior.
    std::fs::write(repo.join("app.txt"), b"fixed\n").expect("modify");
    std::fs::write(
        repo.join("looks-hostile.gitconfig"),
        "[alias] boom = !rm -rf /\n",
    )
    .expect("hostile-looking content");

    let cas = LocalFsCas::open(&cas_root).expect("cas");
    let base_reader = GitBaseReader::new(&control).expect("base reader");
    let capture = ConfinedCapture::new();

    let sealed = seal(
        &store,
        &capture,
        &base_reader,
        &cas,
        &workspace_root,
        "seal-hostile",
    )
    .expect(
        "capture succeeds from the sterile projection where hostile \
                 worker Git metadata is irrelevant",
    );

    // No sentinel ever executed.
    assert!(
        !marker.exists(),
        "a worker-wired Git execution surface ran with controller authority"
    );

    // The authoritative source repository is byte-for-byte unchanged.
    assert_eq!(git(&source, &["show-ref"]), source_refs_before);

    // And the sealed changeset is still CAS-complete for its changed paths.
    let reconstructed = reconstruct(&store, &cas, sealed.artifact_digest);
    let (_, _, app_after) = reconstructed
        .iter()
        .find(|(name, _, _)| name == "app.txt")
        .expect("app.txt changed");
    assert_eq!(app_after.as_deref(), Some(b"fixed\n".as_slice()));
}

// ---- Issue #75: base-blob read accounting ----------------------------------

use std::cell::Cell;
use std::collections::BTreeMap;

use pantheon_core::workspace::ResolvedBase;
use pantheon_engine::sealing::BaseObject;

/// Wraps the real sterile base reader and counts what sealing asks of it,
/// distinguishing base-blob payload reads (the per-file fetch) from
/// identity computations and the one-shot tree listing. This is the
/// operation counter the mission's performance property is stated against.
struct CountingBase<'a> {
    inner: &'a GitBaseReader,
    blob_reads: Cell<usize>,
    identity_calls: Cell<usize>,
}

impl TrustedBaseReader for CountingBase<'_> {
    fn base_tree(
        &self,
        source: &Path,
        base: &ResolvedBase,
    ) -> Result<BTreeMap<Vec<u8>, BaseObject>, ExternalFault> {
        self.inner.base_tree(source, base)
    }

    fn blob_object_names(
        &self,
        source: &Path,
        contents: &[&[u8]],
    ) -> Result<Vec<String>, ExternalFault> {
        self.identity_calls.set(self.identity_calls.get() + 1);
        self.inner.blob_object_names(source, contents)
    }

    fn blob_bytes(&self, source: &Path, oid: &str) -> Result<Vec<u8>, ExternalFault> {
        self.blob_reads.set(self.blob_reads.get() + 1);
        self.inner.blob_bytes(source, oid)
    }
}

/// Deterministic 4096-byte payload, distinct per index: no two bulk files
/// share content with each other or with anything else in the fixture.
fn bulk_content(index: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; BULK_BYTES];
    let mut state = (index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for byte in &mut bytes {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_896_407);
        *byte = (state >> 33) as u8;
    }
    bytes
}

const BULK_FILES: usize = 300;
const BULK_BYTES: usize = 4096;

#[test]
fn base_blob_reads_track_changes_not_the_base_tree() {
    let dir = TempDir::new("read-accounting");
    let source = dir.path().join("source");
    std::fs::create_dir_all(source.join("bulk")).expect("source");
    git(&source, &["init", "--quiet", "-b", "main"]);
    for index in 0..BULK_FILES {
        std::fs::write(
            source.join(format!("bulk/file-{index:03}.bin")),
            bulk_content(index),
        )
        .expect("bulk file");
    }
    // Both same-size collision candidates: `collision.txt` (64 bytes) and
    // `modified.txt` — same size as their preimages, different bytes.
    std::fs::write(source.join("bulk/collision.txt"), b"A".repeat(64)).expect("collision base");
    std::fs::write(source.join("modified.txt"), b"before-version").expect("modified base");
    std::fs::write(source.join("deleted.txt"), b"doomed\n").expect("deleted base");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);

    let control = dir.path().join("control");
    let workspace_root = dir.path().join("workspaces");
    let cas_root = dir.path().join("cas");
    let materializer = GitMaterializer::new(&control).expect("materializer");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    ready_coding_task(&store, &["workspace://**"]);
    ensure_workspace(&store, &materializer, &workspace_root, &source, "ws-req-1");
    dispatch(&store);

    // The worker touches almost nothing: every bulk file keeps its exact
    // bytes; the two same-size rewrites are the only content collisions; one
    // path is deleted and one added.
    let repo = workspace_root.join("workspace-1").join("repo");
    std::fs::write(repo.join("bulk/collision.txt"), b"B".repeat(64)).expect("collision after");
    std::fs::write(repo.join("modified.txt"), b"after-version!").expect("modified after");
    std::fs::remove_file(repo.join("deleted.txt")).expect("delete");
    std::fs::write(repo.join("added.txt"), b"brand new\n").expect("add");

    let cas = LocalFsCas::open(&cas_root).expect("cas");
    let real_reader = GitBaseReader::new(&control).expect("base reader");
    let counting = CountingBase {
        inner: &real_reader,
        blob_reads: Cell::new(0),
        identity_calls: Cell::new(0),
    };
    let capture = ConfinedCapture::new();

    let sealed = seal(
        &store,
        &capture,
        &counting,
        &cas,
        &workspace_root,
        "seal-accounting",
    )
    .expect("seals");

    // Every unchanged bulk path stays out of the changeset; the four real
    // changes carry exactly the right operations.
    let reconstructed = reconstruct(&store, &cas, sealed.artifact_digest);
    let changed_paths: Vec<&str> = reconstructed
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect();
    assert_eq!(
        changed_paths,
        vec![
            "added.txt",
            "bulk/collision.txt",
            "deleted.txt",
            "modified.txt"
        ],
        "exactly the changed paths, canonically ordered"
    );

    let (_, modified_before, modified_after) = reconstructed
        .iter()
        .find(|(name, _, _)| name == "modified.txt")
        .expect("modified entry");
    assert_eq!(
        modified_before.as_deref(),
        Some(b"before-version".as_slice())
    );
    assert_eq!(
        modified_after.as_deref(),
        Some(b"after-version!".as_slice())
    );

    let (_, collision_before, collision_after) = reconstructed
        .iter()
        .find(|(name, _, _)| name == "bulk/collision.txt")
        .expect("collision entry");
    assert_eq!(
        collision_before.as_deref(),
        Some(b"A".repeat(64).as_slice()),
        "the authoritative preimage of the same-size collision"
    );
    assert_eq!(collision_after.as_deref(), Some(b"B".repeat(64).as_slice()));

    let (_, deleted_before, deleted_after) = reconstructed
        .iter()
        .find(|(name, _, _)| name == "deleted.txt")
        .expect("deleted entry");
    assert_eq!(deleted_before.as_deref(), Some(b"doomed\n".as_slice()));
    assert!(deleted_after.is_none());

    assert_eq!(
        counting.blob_reads.get(),
        3,
        "base-blob payload reads track the changeset: one preimage each for \
         the two same-size modifies and the delete. The 300 unchanged \
         same-size files cost zero reads (pre-change baseline: 305 — one \
         per unchanged candidate, two per same-size modify, one for the \
         delete)"
    );
    assert_eq!(
        counting.identity_calls.get(),
        1,
        "all comparison identities derive from one batched computation over \
         captured bytes"
    );
}

#[test]
fn changed_path_derivation_works_through_a_sha256_source_repository() {
    // The comparison signal must follow the source repository's actual
    // object format, never a 40-hex assumption. Base listing, identity
    // computation, unchanged verdicts, capture, CAS and publication all
    // run here against a real SHA-256 repository.
    //
    // Only the Workspace materialization step is substituted, and only
    // because `GitMaterializer` cannot yet seed a Workspace from a 64-hex
    // base (`git init` defaults to SHA-1, so its fetch cannot name the
    // source commit) — a #32-era limitation outside #75's scope, reported
    // rather than fixed here. Capture does not need a worker-side Git
    // repository at all: it reads the settled tree through the confined
    // boundary.
    let dir = TempDir::new("sha256-seal");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    git(
        &source,
        &["init", "--quiet", "--object-format=sha256", "-b", "main"],
    );
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    std::fs::write(source.join("same.txt"), b"untouched\n").expect("write");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);
    let base_oid = git(&source, &["rev-parse", "HEAD"]);

    let control = dir.path().join("control");
    let workspace_root = dir.path().join("workspaces");
    let cas_root = dir.path().join("cas");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    ready_coding_task(&store, &["workspace://**"]);
    ensure_workspace_with(
        &store,
        &PlainDirMaterializer,
        &workspace_root,
        &source,
        "ws-req-1",
    );
    dispatch(&store);

    let repo = workspace_root.join("workspace-1").join("repo");
    std::fs::create_dir_all(&repo).expect("workspace tree");
    std::fs::write(repo.join("app.txt"), b"fixed\n").expect("modify");
    std::fs::write(repo.join("same.txt"), b"untouched\n").expect("unchanged");
    std::fs::write(repo.join("added.txt"), b"brand new\n").expect("add");

    let cas = LocalFsCas::open(&cas_root).expect("cas");
    let sealed = seal(
        &store,
        &ConfinedCapture::new(),
        &GitBaseReader::new(&control).expect("base reader"),
        &cas,
        &workspace_root,
        "seal-sha256",
    )
    .expect("seals");

    assert!(sealed.artifact_json.contains(base_oid.trim()));
    let reconstructed = reconstruct(&store, &cas, sealed.artifact_digest);
    let names: Vec<&str> = reconstructed
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["added.txt", "app.txt"],
        "exactly the changed paths; the unchanged same-size path stays out"
    );
    let (_, app_before, app_after) = reconstructed
        .iter()
        .find(|(name, _, _)| name == "app.txt")
        .expect("app.txt");
    assert_eq!(app_before.as_deref(), Some(b"original\n".as_slice()));
    assert_eq!(app_after.as_deref(), Some(b"fixed\n".as_slice()));
}

/// Materializes nothing but the destination directory, so a test can drive
/// the full real sealing stack against a Workspace whose seeding strategy
/// is not the subject under test.
struct PlainDirMaterializer;

impl pantheon_engine::workspace::RepositoryMaterializer for PlainDirMaterializer {
    fn resolve_base(
        &self,
        source: &Path,
        requested: &pantheon_core::workspace::RequestedBase,
    ) -> Result<pantheon_core::workspace::ResolvedBase, pantheon_engine::workspace::MaterializerError>
    {
        let revision = format!("{}^{{commit}}", requested.as_str());
        let resolved = git(source, &["rev-parse", "--verify", "--quiet", &revision]);
        pantheon_core::workspace::ResolvedBase::parse(resolved.trim()).map_err(|err| {
            pantheon_engine::workspace::MaterializerError {
                code: "workspace.base-unresolvable".to_string(),
                detail: format!("{requested} did not resolve to a commit: {err}"),
            }
        })
    }

    fn materialize(
        &self,
        target: &pantheon_engine::workspace::MaterializationTarget<'_>,
    ) -> Result<pantheon_core::workspace::ResolvedBase, pantheon_engine::workspace::MaterializerError>
    {
        std::fs::create_dir_all(target.destination).map_err(|err| {
            pantheon_engine::workspace::MaterializerError {
                code: "workspace.materialization-failed".to_string(),
                detail: format!("could not create the workspace directory: {err}"),
            }
        })?;
        Ok(target.base.clone())
    }

    fn observe(
        &self,
        target: &pantheon_engine::workspace::MaterializationTarget<'_>,
    ) -> Result<
        pantheon_core::workspace::Materialization,
        pantheon_engine::workspace::MaterializerError,
    > {
        Ok(if target.destination.exists() {
            pantheon_core::workspace::Materialization::Present
        } else {
            pantheon_core::workspace::Materialization::Absent
        })
    }

    fn discard(
        &self,
        target: &pantheon_engine::workspace::MaterializationTarget<'_>,
    ) -> Result<(), pantheon_engine::workspace::MaterializerError> {
        let _ = std::fs::remove_dir_all(target.destination);
        Ok(())
    }
}
