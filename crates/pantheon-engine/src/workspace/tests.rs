//! Evidence for the Workspace control path: ordering between durable state
//! and external effect, restart reopening, and deterministic failure.
//!
//! The materializer here is a controllable double, not a real repository.
//! That is deliberate: these tests are about *when* Pantheon commits what,
//! and a fault has to be injected at an exact step rather than provoked and
//! hoped for. The real Git behaviour — isolation, credential sterility, the
//! immutable base — is proven against actual repositories in `pantheon-git`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::workspace::{Materialization, RequestedBase, ResolvedBase, WorkspacePhase};
use pantheon_store::{Command, Store};

use super::{
    MaterializationTarget, MaterializerError, RepositoryMaterializer, WorkspaceCommand,
    WorkspaceController, WorkspaceError, WorkspaceRequest,
};
use crate::configuration::{ConfigurationAuthority, SourceSet};

const BASE: &str = "dc6fcd729d1c3b0426712ab6985f28c19be95d55";
const MOVED: &str = "3ab5ae51b3728243d6d221857e865ec97189e6e1";

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-workspace-test-{label}-{}-{unique}",
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

/// What the double was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Resolve(String),
    Discard(String),
    Materialize(String),
    Observe(String),
}

/// A materializer whose every answer is set by the test.
struct Fake {
    /// What `resolve_base` returns. Changing it models the source ref moving.
    resolves_to: RefCell<String>,
    /// Fails the next `materialize` call when set.
    materialize_fails: RefCell<bool>,
    /// What `observe` reports for a destination the test has "created".
    present: RefCell<bool>,
    /// What `materialize` claims to have verified, when it succeeds.
    verifies: RefCell<Option<String>>,
    steps: RefCell<Vec<Step>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            resolves_to: RefCell::new(BASE.to_string()),
            materialize_fails: RefCell::new(false),
            present: RefCell::new(false),
            verifies: RefCell::new(None),
            steps: RefCell::new(Vec::new()),
        }
    }

    fn steps(&self) -> Vec<Step> {
        self.steps.borrow().clone()
    }
}

impl RepositoryMaterializer for Fake {
    fn resolve_base(
        &self,
        source: &Path,
        _requested: &RequestedBase,
    ) -> Result<ResolvedBase, MaterializerError> {
        self.steps
            .borrow_mut()
            .push(Step::Resolve(source.display().to_string()));
        Ok(ResolvedBase::parse(&self.resolves_to.borrow()).expect("fixture object name"))
    }

    fn materialize(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<ResolvedBase, MaterializerError> {
        self.steps
            .borrow_mut()
            .push(Step::Materialize(target.workspace_id.to_string()));
        if *self.materialize_fails.borrow() {
            return Err(MaterializerError {
                code: "workspace.materialization-failed".to_string(),
                detail: "injected fault".to_string(),
            });
        }
        *self.present.borrow_mut() = true;
        let verified = self
            .verifies
            .borrow()
            .clone()
            .unwrap_or_else(|| target.base.as_str().to_string());
        Ok(ResolvedBase::parse(&verified).expect("fixture object name"))
    }

    fn observe(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<Materialization, MaterializerError> {
        self.steps
            .borrow_mut()
            .push(Step::Observe(target.workspace_id.to_string()));
        Ok(if *self.present.borrow() {
            Materialization::Present
        } else {
            Materialization::Absent
        })
    }

    fn discard(&self, target: &MaterializationTarget<'_>) -> Result<(), MaterializerError> {
        self.steps
            .borrow_mut()
            .push(Step::Discard(target.workspace_id.to_string()));
        *self.present.borrow_mut() = false;
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

fn goal_spec(with_repository: bool) -> GoalSpec {
    GoalSpec {
        objective: "perform a bounded coding task".to_string(),
        inputs: if with_repository {
            vec![GoalInput {
                name: "repository".to_string(),
                reference: "repo://project".to_string(),
            }]
        } else {
            Vec::new()
        },
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

/// A store holding one Ready Task, exactly as #24 produces one.
fn prepared(label: &str, with_repository: bool) -> (TempDir, Store) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.0.join("pantheon.db")).expect("open store");
    let epoch = store.restore_generation().expect("read generation");
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

    let controller = crate::planning::PlanningController::new(&store);
    let spec = goal_spec(with_repository);
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
    controller
        .materialize(
            &command(epoch.as_str(), "graph-1", &[4u8; 32], "graph.patched"),
            "planning-1",
            "task-1",
            "goal-1",
            &proposal,
        )
        .expect("materialize the Task");
    (dir, store)
}

fn request<'a>(source: &'a Path, base: &'a RequestedBase) -> WorkspaceRequest<'a> {
    WorkspaceRequest {
        source,
        requested_base: base,
    }
}

fn ensure(
    store: &Store,
    fake: &Fake,
    root: &Path,
    source: &Path,
    base: &RequestedBase,
    command_id: &str,
) -> Result<pantheon_store::WorkspaceRecord, WorkspaceError> {
    let epoch = store.restore_generation().expect("read generation");
    let controller = WorkspaceController::new(store, fake, root);
    controller.ensure(
        &WorkspaceCommand {
            epoch: epoch.as_str(),
            id: command_id,
            request_hash: &[5u8; 32],
        },
        "workspace-1",
        "task-1",
        &request(source, base),
    )
}

fn main_ref() -> RequestedBase {
    RequestedBase::parse("refs/heads/main").expect("valid ref name")
}

#[test]
fn a_ready_task_materializes_one_workspace_bound_to_the_resolved_base() {
    let (dir, store) = prepared("fresh", true);
    let fake = Fake::new();
    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");

    let workspace = ensure(&store, &fake, &root, &source, &main_ref(), "req-1")
        .expect("the workspace materializes");

    assert_eq!(workspace.task_id, "task-1");
    assert_eq!(workspace.phase, WorkspacePhase::Ready);
    assert_eq!(workspace.materialization, Materialization::Present);
    assert_eq!(workspace.requested_base.as_str(), "refs/heads/main");
    assert_eq!(workspace.resolved_base.as_str(), BASE);
    // Bound to the repository the Task declares, not to anything the caller
    // passed in.
    assert_eq!(workspace.repository, "repo://project");

    // Resolution precedes materialization: the immutable base exists before
    // any mutable state does.
    let steps = fake.steps();
    let resolve = steps
        .iter()
        .position(|step| matches!(step, Step::Resolve(_)))
        .expect("the base was resolved");
    let materialize = steps
        .iter()
        .position(|step| matches!(step, Step::Materialize(_)))
        .expect("the workspace was materialized");
    assert!(
        resolve < materialize,
        "the base must be resolved before mutable state exists: {steps:?}"
    );
}

#[test]
fn the_workspace_path_is_derived_from_durable_identity_alone() {
    let (dir, store) = prepared("path", true);
    let fake = Fake::new();
    let root = dir.0.join("workspaces");
    let controller = WorkspaceController::new(&store, &fake, &root);

    // The canonical Workspace idempotency identity is "Workspace ID +
    // deterministic desired path/base": two processes that have only read
    // durable state must compute the same path.
    assert_eq!(
        controller.path_of("workspace-1"),
        root.join("workspace-1").join("repo")
    );
    assert_eq!(
        WorkspaceController::new(&store, &fake, &root).path_of("workspace-1"),
        controller.path_of("workspace-1")
    );
}

#[test]
fn a_restart_reopens_the_same_workspace_and_base_without_creating_another() {
    let (dir, store) = prepared("restart", true);
    let fake = Fake::new();
    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");

    let first = ensure(&store, &fake, &root, &source, &main_ref(), "req-1")
        .expect("the workspace materializes");

    // The source ref moves after resolution. A reopen must not follow it.
    *fake.resolves_to.borrow_mut() = MOVED.to_string();

    // A restart: a new controller, a new operator request identity, nothing
    // carried in process memory.
    let reopened =
        ensure(&store, &fake, &root, &source, &main_ref(), "req-2").expect("the workspace reopens");

    assert_eq!(reopened.id, first.id, "the same Workspace identity");
    assert_eq!(
        reopened.resolved_base.as_str(),
        BASE,
        "still bound to the base it resolved, not to where the ref moved"
    );
    assert_eq!(reopened.phase, WorkspacePhase::Ready);
    // "The Task's one current Workspace" is the same row it was before, so
    // nothing created a second authority. That the database refuses a second
    // one at all is proven where it lives, in `pantheon-store`.
    assert_eq!(
        store
            .workspace_for_task("task-1")
            .expect("read")
            .expect("exists")
            .id,
        first.id
    );
    // And a reopen resolves nothing: the durable binding is the answer.
    assert_eq!(
        fake.steps()
            .iter()
            .filter(|step| matches!(step, Step::Resolve(_)))
            .count(),
        1,
        "the base is resolved exactly once, when the Workspace is created"
    );
}

#[test]
fn a_failed_materialization_never_becomes_ready_and_retries_at_the_same_base() {
    let (dir, store) = prepared("failure", true);
    let fake = Fake::new();
    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");
    *fake.materialize_fails.borrow_mut() = true;

    let err = ensure(&store, &fake, &root, &source, &main_ref(), "req-1")
        .expect_err("the injected fault fails the mission");
    assert!(
        matches!(err, WorkspaceError::Materialization { .. }),
        "unexpected error: {err}"
    );

    let after_failure = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("durable state survives the failure");
    assert_eq!(after_failure.phase, WorkspacePhase::Error);
    assert_ne!(
        after_failure.phase,
        WorkspacePhase::Ready,
        "a partial materialization must never be reported Ready"
    );
    assert_eq!(after_failure.resolved_base.as_str(), BASE);

    // Retry. The Workspace has never been mutable to a worker, so the same
    // identity and the same base are rebuilt rather than a second Workspace
    // being created.
    *fake.materialize_fails.borrow_mut() = false;
    let recovered = ensure(&store, &fake, &root, &source, &main_ref(), "req-2")
        .expect("the retry materializes");
    assert_eq!(recovered.id, after_failure.id);
    assert_eq!(recovered.resolved_base.as_str(), BASE);
    assert_eq!(recovered.phase, WorkspacePhase::Ready);
}

#[test]
fn a_failure_records_unknown_external_state_rather_than_assuming_absence() {
    let (dir, store) = prepared("failure-unknown", true);
    // This double reports Present even while failing, which is the shape a
    // real materializer that died after `git init` would have.
    struct HalfDone;
    impl RepositoryMaterializer for HalfDone {
        fn resolve_base(
            &self,
            _source: &Path,
            _requested: &RequestedBase,
        ) -> Result<ResolvedBase, MaterializerError> {
            Ok(ResolvedBase::parse(BASE).expect("fixture"))
        }
        fn materialize(
            &self,
            _target: &MaterializationTarget<'_>,
        ) -> Result<ResolvedBase, MaterializerError> {
            Err(MaterializerError {
                code: "workspace.materialization-failed".to_string(),
                detail: "died after creating the directory".to_string(),
            })
        }
        fn observe(
            &self,
            _target: &MaterializationTarget<'_>,
        ) -> Result<Materialization, MaterializerError> {
            Ok(Materialization::Present)
        }
        fn discard(&self, _target: &MaterializationTarget<'_>) -> Result<(), MaterializerError> {
            Ok(())
        }
    }

    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");
    let half = HalfDone;
    let epoch = store.restore_generation().expect("read generation");
    let controller = WorkspaceController::new(&store, &half, &root);
    let base = main_ref();
    controller
        .ensure(
            &WorkspaceCommand {
                epoch: epoch.as_str(),
                id: "req-1",
                request_hash: &[5u8; 32],
            },
            "workspace-1",
            "task-1",
            &request(&source, &base),
        )
        .expect_err("materialization fails");

    let record = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("exists");
    assert_eq!(record.phase, WorkspacePhase::Error);
    assert_eq!(
        record.materialization,
        Materialization::Unknown,
        "a failure path may never record verified materialization"
    );
}

#[test]
fn a_ready_workspace_whose_state_vanished_fails_closed_instead_of_being_rebuilt() {
    let (dir, store) = prepared("missing", true);
    let fake = Fake::new();
    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");
    ensure(&store, &fake, &root, &source, &main_ref(), "req-1").expect("materializes");

    // The Workspace directory is gone, and it may have held unsealed work.
    *fake.present.borrow_mut() = false;

    let err = ensure(&store, &fake, &root, &source, &main_ref(), "req-2")
        .expect_err("a missing Ready workspace is not silently recreated");
    assert!(
        matches!(
            err,
            WorkspaceError::Missing {
                observed: Materialization::Absent,
                ..
            }
        ),
        "unexpected error: {err}"
    );
    // Durable authority is untouched: still Ready, still one Workspace.
    let record = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("exists");
    assert_eq!(record.phase, WorkspacePhase::Ready);
    assert_eq!(record.id, "workspace-1");
}

#[test]
fn a_materializer_that_establishes_another_base_cannot_produce_a_ready_workspace() {
    let (dir, store) = prepared("wrong-base", true);
    let fake = Fake::new();
    *fake.verifies.borrow_mut() = Some(MOVED.to_string());
    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");

    let err = ensure(&store, &fake, &root, &source, &main_ref(), "req-1")
        .expect_err("a mismatched base is refused");
    assert!(
        matches!(
            err,
            WorkspaceError::Store(pantheon_store::StoreError::WorkspaceBaseMismatch { .. })
        ),
        "unexpected error: {err}"
    );
    let record = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("exists");
    assert_ne!(record.phase, WorkspacePhase::Ready);
    assert_eq!(record.resolved_base.as_str(), BASE);
}

#[test]
fn a_task_that_declares_no_repository_gets_no_workspace() {
    let (dir, store) = prepared("no-repository", false);
    let fake = Fake::new();
    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");

    let err = ensure(&store, &fake, &root, &source, &main_ref(), "req-1")
        .expect_err("a task with no repository input is refused");
    assert!(
        matches!(err, WorkspaceError::Ineligible { .. }),
        "unexpected error: {err}"
    );
    assert!(
        store.workspace_for_task("task-1").expect("read").is_none(),
        "nothing durable was created"
    );
    assert!(
        fake.steps().is_empty(),
        "eligibility is decided before the source repository is touched: {:?}",
        fake.steps()
    );
}

#[test]
fn a_request_for_a_different_base_does_not_quietly_reuse_the_existing_workspace() {
    let (dir, store) = prepared("conflict", true);
    let fake = Fake::new();
    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");
    ensure(&store, &fake, &root, &source, &main_ref(), "req-1").expect("materializes");

    let other = RequestedBase::parse("refs/heads/release").expect("valid ref name");
    let err = ensure(&store, &fake, &root, &source, &other, "req-2")
        .expect_err("a different requested base conflicts");
    assert!(
        matches!(err, WorkspaceError::Conflict { .. }),
        "unexpected error: {err}"
    );
    assert_eq!(
        store
            .workspace_for_task("task-1")
            .expect("read")
            .expect("exists")
            .requested_base
            .as_str(),
        "refs/heads/main"
    );
}

#[test]
fn nothing_durable_exists_when_the_base_cannot_be_resolved() {
    let (dir, store) = prepared("unresolvable", true);
    struct Unresolvable;
    impl RepositoryMaterializer for Unresolvable {
        fn resolve_base(
            &self,
            _source: &Path,
            _requested: &RequestedBase,
        ) -> Result<ResolvedBase, MaterializerError> {
            Err(MaterializerError {
                code: "workspace.base-unresolvable".to_string(),
                detail: "no such ref".to_string(),
            })
        }
        fn materialize(
            &self,
            _target: &MaterializationTarget<'_>,
        ) -> Result<ResolvedBase, MaterializerError> {
            panic!("materialize must not be reached")
        }
        fn observe(
            &self,
            _target: &MaterializationTarget<'_>,
        ) -> Result<Materialization, MaterializerError> {
            panic!("observe must not be reached")
        }
        fn discard(&self, _target: &MaterializationTarget<'_>) -> Result<(), MaterializerError> {
            panic!("discard must not be reached")
        }
    }

    let root = dir.0.join("workspaces");
    let source = dir.0.join("source");
    let epoch = store.restore_generation().expect("read generation");
    let base = main_ref();
    WorkspaceController::new(&store, &Unresolvable, &root)
        .ensure(
            &WorkspaceCommand {
                epoch: epoch.as_str(),
                id: "req-1",
                request_hash: &[5u8; 32],
            },
            "workspace-1",
            "task-1",
            &request(&source, &base),
        )
        .expect_err("resolution failure fails the mission");

    assert!(
        store.workspace_for_task("task-1").expect("read").is_none(),
        "a Workspace is never created bound to a base nobody could resolve"
    );
}
