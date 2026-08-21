//! End-to-end evidence for Issue #27, composed the way the daemon composes
//! it: real durable authority, the real control path and the real Git
//! materializer.
//!
//! `pantheon-engine` proves the ordering between durable state and external
//! effect against a controllable double, and `pantheon-git` proves the Git
//! properties against real repositories. Neither proves that the two halves
//! fit together, which is what this file is for — and the composition root is
//! the only crate allowed to name both.
//!
//! The sequence below is the one the mission asks to be demonstrated:
//!
//! ```text
//! Ready coding Task T with repository input R@main
//!   → resolve main to immutable base B
//!   → create durable Task Workspace W bound to T + B
//!   → materialize isolated writable Task-local repository state
//!   → modify/commit locally inside W
//!   → source repository authoritative refs remain unchanged
//!   → daemon restart
//!   → reconcile/reopen the same W and base B
//! ```

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::workspace::{Materialization, RequestedBase, WorkspacePhase};
use pantheon_engine::configuration::{ConfigurationAuthority, SourceSet};
use pantheon_engine::planning::PlanningController;
use pantheon_engine::workspace::RepositoryMaterializer;
use pantheon_engine::workspace::{
    WorkspaceCommand, WorkspaceController, WorkspaceError, WorkspaceRequest,
};
use pantheon_git::GitMaterializer;
use pantheon_store::{Command, Store, WorkspaceBinding, WorkspaceRecord};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheond-workspace-test-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Process::new("git")
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
        .expect("run git")
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let output = git(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The operator's repository: one commit on `main`.
fn source_repository(root: &Path) -> PathBuf {
    let source = root.join("source");
    std::fs::create_dir_all(&source).expect("create source");
    ok(&source, &["init", "--quiet", "-b", "main"]);
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    ok(&source, &["add", "-A"]);
    ok(&source, &["commit", "--quiet", "-m", "base"]);
    source
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
    "actions":["filesystem.read"]}}],
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
            permitted_resources: vec!["workspace://src/**".to_string()],
        },
    }
}

/// Drives #23's configuration activation and #24's planning path to produce
/// the one Ready coding Task this mission's Workspace belongs to.
fn ready_coding_task(store: &Store) {
    let epoch = store.restore_generation().expect("read generation");
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
    let spec = goal_spec();
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
        .expect("configuration active");
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

fn ensure(
    store: &Store,
    materializer: &GitMaterializer,
    root: &Path,
    source: &Path,
    request_id: &str,
) -> Result<WorkspaceRecord, WorkspaceError> {
    let epoch = store.restore_generation().expect("read generation");
    let requested = RequestedBase::parse("refs/heads/main").expect("valid ref name");
    WorkspaceController::new(store, materializer, root).ensure(
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
}

#[test]
fn a_task_owns_one_isolated_workspace_that_survives_a_daemon_restart() {
    let dir = TempDir::new("end-to-end");
    let source = source_repository(&dir.0);
    let root = dir.0.join("workspaces");
    let control = dir.0.join("control");
    std::fs::create_dir_all(&control).expect("create control root");
    let db = dir.0.join("pantheon.db");

    let store = Store::open(&db).expect("open store");
    ready_coding_task(&store);

    let materializer = GitMaterializer::new(&control).expect("create materializer");
    let workspace =
        ensure(&store, &materializer, &root, &source, "req-1").expect("the workspace materializes");

    // The Workspace binds to the repository the Task declares, to the ref the
    // operator asked for, and to the immutable commit that ref resolved to.
    let base = ok(&source, &["rev-parse", "refs/heads/main"]);
    assert_eq!(workspace.repository, "repo://whiskyshop");
    assert_eq!(workspace.requested_base.as_str(), "refs/heads/main");
    assert_eq!(workspace.resolved_base.as_str(), base);
    assert_eq!(workspace.phase, WorkspacePhase::Ready);
    assert_eq!(workspace.materialization, Materialization::Present);

    let repo = root.join(&workspace.id).join("repo");
    assert_eq!(ok(&repo, &["rev-parse", "HEAD"]), base);

    // A worker does ordinary coding work: edit, stage, commit, branch.
    let source_refs_before = ok(&source, &["show-ref"]);
    std::fs::write(repo.join("app.txt"), b"fixed\n").expect("write");
    std::fs::write(repo.join("fix.txt"), b"note\n").expect("write");
    ok(&repo, &["add", "-A"]);
    ok(&repo, &["commit", "--quiet", "-m", "worker change"]);
    ok(&repo, &["checkout", "--quiet", "-b", "worker/fix"]);
    let worker_commit = ok(&repo, &["rev-parse", "HEAD"]);

    // The authoritative source repository is untouched, in refs and objects.
    assert_eq!(ok(&source, &["show-ref"]), source_refs_before);
    assert!(
        !git(&source, &["cat-file", "-e", &worker_commit])
            .status
            .success(),
        "worker objects must not reach the source object database"
    );

    // The source's `main` moves on afterwards, as a shared branch does.
    std::fs::write(source.join("app.txt"), b"someone else\n").expect("write");
    ok(&source, &["add", "-A"]);
    ok(&source, &["commit", "--quiet", "-m", "unrelated"]);
    assert_ne!(ok(&source, &["rev-parse", "refs/heads/main"]), base);

    // Daemon restart: the store is closed and reopened from the same file,
    // and every controller is rebuilt. Nothing survives in process memory.
    store.close().expect("close store");
    let store = Store::open(&db).expect("reopen store");
    let materializer = GitMaterializer::new(&control).expect("recreate materializer");
    let reopened =
        ensure(&store, &materializer, &root, &source, "req-2").expect("the workspace reopens");

    assert_eq!(reopened.id, workspace.id, "the same Workspace");
    assert_eq!(reopened.task_id, "task-1", "owned by the same Task");
    assert_eq!(
        reopened.resolved_base.as_str(),
        base,
        "still bound to the base it resolved, not to where main moved"
    );
    assert_eq!(reopened.phase, WorkspacePhase::Ready);

    // And the worker's own state is still there: reopening reconciled, it did
    // not rebuild.
    assert_eq!(ok(&repo, &["rev-parse", "HEAD"]), worker_commit);
    assert_eq!(
        std::fs::read_to_string(repo.join("app.txt")).expect("worktree file"),
        "fixed\n"
    );
}

#[test]
fn a_failed_materialization_never_reports_ready_and_leaves_the_source_alone() {
    let dir = TempDir::new("failure");
    let source = source_repository(&dir.0);
    let root = dir.0.join("workspaces");
    let control = dir.0.join("control");
    std::fs::create_dir_all(&control).expect("create control root");
    let db = dir.0.join("pantheon.db");

    let store = Store::open(&db).expect("open store");
    ready_coding_task(&store);
    let materializer = GitMaterializer::new(&control).expect("create materializer");
    let source_refs_before = ok(&source, &["show-ref"]);

    // Stage the exact state a crash produces: durable Workspace identity and
    // an immutable base are committed, and nothing has been materialized yet.
    // Committing identity before any side effect is what makes this state
    // reachable at all, and reconciling from it is what the mission asks the
    // controller to do.
    let requested = RequestedBase::parse("refs/heads/main").expect("valid ref name");
    let base = materializer
        .resolve_base(&source, &requested)
        .expect("the requested base resolves");
    let epoch = store.restore_generation().expect("read generation");
    store
        .open_workspace(
            &command(
                epoch.as_str(),
                "crashed-request",
                &[6u8; 32],
                "workspace.requested",
            ),
            "workspace-1",
            &WorkspaceBinding {
                task_id: "task-1",
                repository: "repo://whiskyshop",
                source_path: source.to_str().expect("utf-8 path"),
                requested_base: &requested,
                resolved_base: &base,
            },
        )
        .expect("durable identity commits");

    // Now the fault: the source repository becomes unreadable. The Workspace
    // repository is still created, and the fetch that would fill it fails —
    // so real partial filesystem state exists when the failure is recorded.
    let saboteur = Saboteur::arm(&source);
    let error =
        ensure(&store, &materializer, &root, &source, "req-1").expect_err("materialization fails");
    assert!(
        matches!(error, WorkspaceError::Materialization { .. }),
        "unexpected error: {error}"
    );
    let repo = root.join("workspace-1").join("repo");
    assert!(
        repo.exists(),
        "the fault must leave partial filesystem state, or this proves nothing"
    );
    drop(saboteur);

    let record = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("durable identity survives the failure");
    assert_ne!(
        record.phase,
        WorkspacePhase::Ready,
        "a partial materialization must never be reported Ready"
    );
    assert_eq!(record.phase, WorkspacePhase::Error);
    assert_eq!(
        record.materialization,
        Materialization::Unknown,
        "an error is not proof that the partial filesystem state is gone"
    );
    assert_eq!(record.resolved_base.as_str(), base.as_str());
    assert_eq!(
        ok(&source, &["show-ref"]),
        source_refs_before,
        "a failed materialization must not touch the source repository"
    );

    // Retry: the same Workspace and the same base. The partial state is
    // discarded and rebuilt rather than a second Workspace being created,
    // because durable state proves this one was never mutable to a worker.
    let recovered =
        ensure(&store, &materializer, &root, &source, "req-2").expect("the retry materializes");
    assert_eq!(recovered.id, record.id);
    assert_eq!(recovered.resolved_base.as_str(), base.as_str());
    assert_eq!(recovered.phase, WorkspacePhase::Ready);
    assert_eq!(ok(&repo, &["rev-parse", "HEAD"]), base.as_str());
}

/// Makes a repository unreadable for as long as it is held, then restores it.
///
/// Deterministic fault injection: the failure lands on a known step rather
/// than being provoked and hoped for, and it is undone on drop so the same
/// test can then prove the retry works.
struct Saboteur {
    from: PathBuf,
    to: PathBuf,
}

impl Saboteur {
    fn arm(repository: &Path) -> Self {
        let from = repository.join(".git");
        let to = repository.join(".git-hidden");
        std::fs::rename(&from, &to).expect("hide the repository");
        Self { from, to }
    }
}

impl Drop for Saboteur {
    fn drop(&mut self) {
        let _ = std::fs::rename(&self.to, &self.from);
    }
}
