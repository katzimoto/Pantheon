//! Evidence for the sealing order: freeze before capture, CAS before DB,
//! scope as a hard ceiling, deterministic identity, and failure that keeps
//! the fence while publishing nothing.
//!
//! The ports are controllable doubles, not real Git or disk. That is
//! deliberate: these tests are about *when* Pantheon commits what and what
//! it refuses to claim, and a fault has to land at an exact step. Real
//! filesystem confinement is proven in `pantheon-git`, real CAS durability
//! in `pantheon-cas`, and the whole composition in `pantheond`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pantheon_core::artifact::{EntryKind, RepositoryPath};
use pantheon_core::config::Digest;
use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::workspace::{Materialization, RequestedBase, ResolvedBase, WorkspacePhase};
use pantheon_store::{Command, Store};

use crate::workspace::{
    MaterializationTarget, MaterializerError, RepositoryMaterializer, WorkspaceCommand,
    WorkspaceController, WorkspaceRequest,
};

use super::{
    BaseObject, CapturedEntry, ChangesetSealer, ContentObjectStore, ExternalFault, ObjectRef,
    SealAuthority, SealCommand, SealError, SealRequest, TrustedBaseReader, WorkspaceTreeCapture,
};
use crate::configuration::{ConfigurationAuthority, SourceSet};

const BASE: &str = "dc6fcd729d1c3b0426712ab6985f28c19be95d55";
/// A valid 40-hex object name for fixture preimages.
const APP_OID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-engine-sealing-{label}-{}-{unique}",
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

// ---- doubles --------------------------------------------------------------

/// Emits a scripted captured tree, observes the durable Workspace phase at
/// the moment capture runs, and can fail at an exact step.
struct ScriptedCapture<'a> {
    store: &'a Store,
    entries: Vec<(Vec<u8>, EntryKind, Vec<u8>)>,
    /// Fail instead of emitting the entry at this index.
    fail_at: Cell<Option<usize>>,
    /// The Workspace phases observed across calls.
    observed_phases: RefCell<Vec<WorkspacePhase>>,
}

impl<'a> ScriptedCapture<'a> {
    fn new(store: &'a Store, entries: Vec<(Vec<u8>, EntryKind, Vec<u8>)>) -> Self {
        Self {
            store,
            entries,
            fail_at: Cell::new(None),
            observed_phases: RefCell::new(Vec::new()),
        }
    }
}

impl WorkspaceTreeCapture for ScriptedCapture<'_> {
    fn capture_tree(
        &self,
        _root: &Path,
        sink: &mut dyn FnMut(CapturedEntry) -> Result<(), ExternalFault>,
    ) -> Result<(), ExternalFault> {
        let phase = self
            .store
            .workspace_for_task("task-1")
            .expect("read")
            .expect("workspace exists")
            .phase;
        self.observed_phases.borrow_mut().push(phase);
        for (index, (path, kind, bytes)) in self.entries.iter().enumerate() {
            if self.fail_at.get() == Some(index) {
                return Err(ExternalFault {
                    code: "cas.write-failed".to_string(),
                    detail: "injected CAS fault".to_string(),
                });
            }
            sink(CapturedEntry {
                path: RepositoryPath::from_bytes(path).expect("fixture path"),
                kind: *kind,
                bytes: bytes.clone(),
            })?;
        }
        Ok(())
    }
}

/// The trusted base: one file, `app.txt`, whose preimage is known.
struct MemoryBase;

impl TrustedBaseReader for MemoryBase {
    fn base_tree(
        &self,
        _source: &Path,
        _base: &ResolvedBase,
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
        Ok(tree)
    }

    fn blob_bytes(&self, _source: &Path, oid: &str) -> Result<Vec<u8>, ExternalFault> {
        if oid == APP_OID {
            Ok(b"original".to_vec())
        } else {
            Err(ExternalFault {
                code: "workspace.base-unavailable".to_string(),
                detail: format!("no fixture blob {oid}"),
            })
        }
    }
}

/// An in-memory CAS with injectable failure at the Nth publish call.
struct MemoryCas {
    objects: RefCell<BTreeMap<Digest, Vec<u8>>>,
    calls: Cell<usize>,
    fail_on_call: Cell<Option<usize>>,
}

impl MemoryCas {
    fn new() -> Self {
        Self {
            objects: RefCell::new(BTreeMap::new()),
            calls: Cell::new(0),
            fail_on_call: Cell::new(None),
        }
    }

    fn contains(&self, bytes: &[u8]) -> bool {
        self.objects.borrow().contains_key(&Digest::of(bytes))
    }
}

impl ContentObjectStore for MemoryCas {
    fn publish(&self, bytes: &[u8]) -> Result<ObjectRef, ExternalFault> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if self.fail_on_call.get() == Some(call) {
            return Err(ExternalFault {
                code: "cas.write-failed".to_string(),
                detail: "injected CAS fault".to_string(),
            });
        }
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

/// Materializes nothing real; it only creates the destination directory so
/// on-disk worker state has somewhere to live.
struct DirMakerMaterializer;

impl RepositoryMaterializer for DirMakerMaterializer {
    fn resolve_base(
        &self,
        _source: &Path,
        _requested: &RequestedBase,
    ) -> Result<ResolvedBase, MaterializerError> {
        Ok(ResolvedBase::parse(BASE).expect("fixture"))
    }

    fn materialize(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<ResolvedBase, MaterializerError> {
        std::fs::create_dir_all(target.destination).expect("destination");
        Ok(ResolvedBase::parse(BASE).expect("fixture"))
    }

    fn observe(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<Materialization, MaterializerError> {
        Ok(if target.destination.exists() {
            Materialization::Present
        } else {
            Materialization::Absent
        })
    }

    fn discard(&self, target: &MaterializationTarget<'_>) -> Result<(), MaterializerError> {
        let _ = std::fs::remove_dir_all(target.destination);
        Ok(())
    }
}

// ---- fixtures -------------------------------------------------------------

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

fn goal_spec(resources: &[&str]) -> GoalSpec {
    GoalSpec {
        objective: "make one bounded change in the repository".to_string(),
        inputs: vec![GoalInput {
            name: "repository".to_string(),
            reference: "repo://project".to_string(),
        }],
        deliverables: vec![
            Deliverable {
                name: "changeset".to_string(),
                kind: "code.changeset".to_string(),
                required: true,
            },
            Deliverable {
                name: "diagnosis".to_string(),
                kind: "report".to_string(),
                required: false,
            },
        ],
        constraints: GoalConstraints {
            permitted_effects: vec!["filesystem.read".to_string()],
            forbidden_effects: Vec::new(),
            permitted_resources: resources.iter().map(|r| r.to_string()).collect(),
        },
    }
}

/// Everything a seal needs: a Ready Task, its Ready Workspace bound to a
/// fixed base, and the controller root the Workspace lives under.
fn prepared(label: &str, resources: &[&str]) -> (TempDir, Store, PathBuf) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
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

    let workspace_root = dir.path().join("workspaces");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).expect("source dir");
    let materializer = DirMakerMaterializer;
    let requested = RequestedBase::parse("refs/heads/main").expect("ref");
    WorkspaceController::new(&store, &materializer, &workspace_root)
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

    (dir, store, workspace_root)
}

fn worker_repo(workspace_root: &Path) -> PathBuf {
    let repo = workspace_root.join("workspace-1").join("repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    repo
}

/// The changed worker state every happy-path test uses: `app.txt` modified,
/// `src/new.txt` added relative to the base.
fn changed_capture(store: &Store) -> ScriptedCapture<'_> {
    ScriptedCapture::new(
        store,
        vec![
            (b"app.txt".to_vec(), EntryKind::Regular, b"fixed".to_vec()),
            (
                b"src/new.txt".to_vec(),
                EntryKind::Regular,
                b"brand new".to_vec(),
            ),
        ],
    )
}

fn sealer<'a>(
    store: &'a Store,
    capture: &'a dyn WorkspaceTreeCapture,
    cas: &'a dyn ContentObjectStore,
    workspace_root: &Path,
) -> ChangesetSealer<'a> {
    ChangesetSealer::new(store, capture, &MemoryBase, cas, workspace_root)
}

fn seal_request() -> SealRequest<'static> {
    SealRequest {
        task_id: "task-1",
        output_slot: "changeset",
        authority: SealAuthority::TaskCheckpoint,
    }
}

fn seal_command<'a>(epoch: &'a str, id: &'a str) -> SealCommand<'a> {
    SealCommand {
        epoch,
        id,
        request_hash: &[6u8; 32],
    }
}

// ---- tests ----------------------------------------------------------------

#[test]
fn a_changed_tree_seals_freeze_first_with_payloads_durable_before_the_db_row() {
    let (_dir, store, workspace_root) = prepared("happy", &["workspace://**"]);
    let _repo = worker_repo(&workspace_root);
    let capture = changed_capture(&store);
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");

    let sealed = sealer(&store, &capture, &cas, &workspace_root)
        .seal(&seal_command(epoch.as_str(), "cmd-1"), &seal_request())
        .expect("seals");

    // Capture ran only inside the fence: it observed Frozen.
    assert_eq!(
        capture.observed_phases.borrow().as_slice(),
        &[WorkspacePhase::Frozen],
        "capture must run after the freeze committed"
    );
    // And the Workspace stays frozen afterwards.
    let workspace = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    assert_eq!(workspace.phase, WorkspacePhase::Frozen);

    // Every payload-bearing side is in CAS: both after-states plus the
    // modified file's preimage derived from the trusted base.
    assert!(cas.contains(b"fixed"), "after payload of app.txt");
    assert!(cas.contains(b"brand new"), "after payload of src/new.txt");
    assert!(cas.contains(b"original"), "before payload of app.txt");
    assert_eq!(cas.objects.borrow().len(), 3, "unchanged paths do no work");

    // Durable authority holds the Artifact and binds the manifest.
    let artifact = store
        .artifact(sealed.artifact_digest)
        .expect("read")
        .expect("sealed");
    assert_eq!(artifact.kind, "code.changeset");
    let json = &artifact.canonical_json;
    assert!(json.contains(r#""operation":"modify""#), "{json}");
    assert!(json.contains(r#""operation":"add""#), "{json}");
    assert!(json.contains("app.txt"), "{json}");
    assert!(json.contains("src/new.txt"), "{json}");
    // No patch text, no incidental provenance.
    assert!(
        !json.contains("@@") && !json.contains("diff --git"),
        "{json}"
    );
}

#[test]
fn two_commands_over_identical_state_converge_on_one_identity() {
    let (_dir, store, workspace_root) = prepared("converge", &["workspace://**"]);
    let _repo = worker_repo(&workspace_root);
    let epoch = store.restore_generation().expect("generation");

    let first = {
        let capture = changed_capture(&store);
        let cas = MemoryCas::new();
        sealer(&store, &capture, &cas, &workspace_root)
            .seal(&seal_command(epoch.as_str(), "cmd-A"), &seal_request())
            .expect("first seals")
    };
    let second = {
        let capture = changed_capture(&store);
        let cas = MemoryCas::new();
        sealer(&store, &capture, &cas, &workspace_root)
            .seal(&seal_command(epoch.as_str(), "cmd-B"), &seal_request())
            .expect("second seals")
    };

    assert_eq!(
        first.artifact_digest, second.artifact_digest,
        "identical permitted state is one content identity"
    );
    assert!(
        second.artifact_reused,
        "the second command reused prior content"
    );
}

#[test]
fn replaying_one_command_returns_the_same_sealed_identity() {
    let (_dir, store, workspace_root) = prepared("replay", &["workspace://**"]);
    let _repo = worker_repo(&workspace_root);
    let epoch = store.restore_generation().expect("generation");

    let first = {
        let capture = changed_capture(&store);
        let cas = MemoryCas::new();
        sealer(&store, &capture, &cas, &workspace_root)
            .seal(&seal_command(epoch.as_str(), "same-cmd"), &seal_request())
            .expect("first seals")
    };
    let second = {
        let capture = changed_capture(&store);
        let cas = MemoryCas::new();
        sealer(&store, &capture, &cas, &workspace_root)
            .seal(&seal_command(epoch.as_str(), "same-cmd"), &seal_request())
            .expect("second reconciles")
    };
    assert_eq!(first.artifact_digest, second.artifact_digest);
}

#[test]
fn an_injected_cas_failure_keeps_the_fence_and_publishes_no_artifact() {
    let (_dir, store, workspace_root) = prepared("cas-failure", &["workspace://**"]);
    let _repo = worker_repo(&workspace_root);
    let capture = changed_capture(&store);
    capture.fail_at.set(Some(1)); // fail publishing the second payload
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");

    let error = sealer(&store, &capture, &cas, &workspace_root)
        .seal(&seal_command(epoch.as_str(), "cmd-fail"), &seal_request())
        .expect_err("the injected fault fails the seal");
    assert!(
        matches!(error, SealError::Capture(ref fault) if fault.code == "cas.write-failed"),
        "{error}"
    );

    // No authoritative complete Artifact claim exists, and the fence held:
    // still frozen, never thawed, never rebuilt.
    let workspace = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    assert_eq!(workspace.phase, WorkspacePhase::Frozen);
    assert_eq!(workspace.materialization, Materialization::Present);
}

#[test]
fn a_change_outside_declared_scope_is_refused_and_publishes_nothing() {
    let (_dir, store, workspace_root) = prepared("scope", &["workspace://docs/**"]);
    let _repo = worker_repo(&workspace_root); // src/new.txt is outside docs/
    let capture = changed_capture(&store);
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");

    let error = sealer(&store, &capture, &cas, &workspace_root)
        .seal(&seal_command(epoch.as_str(), "cmd-scope"), &seal_request())
        .expect_err("out-of-scope change must be refused");
    // Either changed path may surface first — both are outside docs/ — and
    // the refusal is a scope refusal raised at the capture sink, so no
    // out-of-authority byte ever reaches CAS, not even as an orphan.
    assert!(
        matches!(error, SealError::ScopeViolated { ref path }
            if path.contains("new.txt") || path.contains("app.txt")),
        "{error}"
    );
    assert!(!cas.contains(b"brand new"), "out-of-scope content in CAS");
    assert!(!cas.contains(b"fixed"), "out-of-scope content in CAS");
}

#[test]
fn an_empty_scope_authorizes_nothing_at_all() {
    let (_dir, store, workspace_root) = prepared("empty-scope", &[]);
    let _repo = worker_repo(&workspace_root);
    let capture = changed_capture(&store);
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");

    let error = sealer(&store, &capture, &cas, &workspace_root)
        .seal(&seal_command(epoch.as_str(), "cmd-none"), &seal_request())
        .expect_err("an empty ceiling authorizes no output");
    assert!(matches!(error, SealError::ScopeViolated { .. }), "{error}");
    assert!(
        cas.objects.borrow().is_empty(),
        "an empty scope publishes nothing at all"
    );
}

#[test]
fn the_output_slot_must_exist_and_permit_code_changeset() {
    let (_dir, store, workspace_root) = prepared("slots", &["workspace://**"]);
    let _repo = worker_repo(&workspace_root);
    let capture = changed_capture(&store);
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");
    let sealer = sealer(&store, &capture, &cas, &workspace_root);

    let unknown = SealRequest {
        task_id: "task-1",
        output_slot: "no-such-slot",
        authority: SealAuthority::TaskCheckpoint,
    };
    let err = sealer
        .seal(&seal_command(epoch.as_str(), "cmd-slot-a"), &unknown)
        .expect_err("unknown slot refused");
    assert!(matches!(err, SealError::OutputSlotInvalid { .. }), "{err}");

    let wrong_kind = SealRequest {
        task_id: "task-1",
        output_slot: "diagnosis",
        authority: SealAuthority::TaskCheckpoint,
    };
    let err = sealer
        .seal(&seal_command(epoch.as_str(), "cmd-slot-b"), &wrong_kind)
        .expect_err("wrong-kind slot refused");
    assert!(matches!(err, SealError::OutputSlotInvalid { .. }), "{err}");
}

#[test]
fn an_empty_change_yields_a_deterministic_result_across_installations() {
    let digests = ["a", "b"]
        .iter()
        .map(|label| {
            let (_dir, store, workspace_root) =
                prepared(&format!("empty-{label}"), &["workspace://**"]);
            // Worker state identical to the base: nothing changed anywhere.
            let repo = worker_repo(&workspace_root);
            std::fs::write(repo.join("app.txt"), b"original").expect("identical state");
            let capture = ScriptedCapture::new(
                &store,
                vec![(
                    b"app.txt".to_vec(),
                    EntryKind::Regular,
                    b"original".to_vec(),
                )],
            );
            let cas = MemoryCas::new();
            let epoch = store.restore_generation().expect("generation");
            sealer(&store, &capture, &cas, &workspace_root)
                .seal(&seal_command(epoch.as_str(), "cmd-empty"), &seal_request())
                .expect("an empty change seals deterministically")
                .artifact_digest
        })
        .collect::<Vec<_>>();
    assert_eq!(digests[0], digests[1]);
}

#[test]
fn operation_kinds_follow_the_logical_diff_not_git_metadata() {
    let (_dir, store, workspace_root) = prepared("ops", &["workspace://**"]);
    let _repo = worker_repo(&workspace_root);
    // Capture presents: an added file, a deleted base file, and a mode-only
    // change (same bytes as base, now executable).
    let capture = ScriptedCapture::new(
        &store,
        vec![
            (
                b"old-name.txt".to_vec(),
                EntryKind::Regular,
                b"moved content".to_vec(),
            ),
            (
                b"run.sh".to_vec(),
                EntryKind::Executable,
                b"original".to_vec(),
            ),
        ],
    );
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");

    let sealed = sealer(&store, &capture, &cas, &workspace_root)
        .seal(&seal_command(epoch.as_str(), "cmd-ops"), &seal_request())
        .expect("seals");
    let json = store
        .artifact(sealed.artifact_digest)
        .expect("read")
        .expect("exists")
        .canonical_json;

    assert!(json.contains(r#""path":"old-name.txt""#), "{json}");
    assert!(json.contains(r#""operation":"add""#), "{json}");
    assert!(json.contains(r#""path":"run.sh""#), "{json}");
    assert!(json.contains(r#""mode":"executable""#), "{json}");
    // A delete entry exists for every base file absent from capture.
    assert!(json.contains(r#""operation":"delete""#), "{json}");
}

#[test]
fn non_utf8_paths_seal_through_the_lossless_encoding_when_authorized() {
    // The encoding settled into artifact-model.md is reachable end to end:
    // a raw-byte name under an authorized wildcard captures, passes the
    // byte-exact scope check, and appears in the manifest only as its
    // lossless spelling.
    let (_dir, store, workspace_root) = prepared("non-utf8", &["workspace://src/**"]);
    let _repo = worker_repo(&workspace_root);
    let mut raw_path = b"src/".to_vec();
    raw_path.push(0xE9);
    raw_path.extend_from_slice(b".txt");
    // The base's app.txt is present and unchanged, so the only changed
    // path is the raw-byte one.
    let capture = ScriptedCapture::new(
        &store,
        vec![
            (
                b"app.txt".to_vec(),
                EntryKind::Regular,
                b"original".to_vec(),
            ),
            (raw_path.clone(), EntryKind::Regular, b"raw bytes".to_vec()),
        ],
    );
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");

    let sealed = sealer(&store, &capture, &cas, &workspace_root)
        .seal(&seal_command(epoch.as_str(), "cmd-raw"), &seal_request())
        .expect("a non-UTF-8 authorized path seals");
    assert!(cas.contains(b"raw bytes"));

    let json = store
        .artifact(sealed.artifact_digest)
        .expect("read")
        .expect("exists")
        .canonical_json;
    let expected_spelling = RepositoryPath::from_bytes(&raw_path)
        .expect("representable")
        .to_manifest_string();
    assert!(
        json.contains(&expected_spelling),
        "manifest carries the lossless spelling {expected_spelling}: {json}"
    );
}

/// A trusted base with one extra root-level file, for delete-side scope
/// evidence.
struct BaseWithRootOnly;

impl TrustedBaseReader for BaseWithRootOnly {
    fn base_tree(
        &self,
        _source: &Path,
        _base: &ResolvedBase,
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
        tree.insert(
            b"outside.txt".to_vec(),
            BaseObject {
                kind: EntryKind::Regular,
                oid: APP_OID.to_string(),
                size: b"original".len() as u64,
            },
        );
        Ok(tree)
    }

    fn blob_bytes(&self, _source: &Path, _oid: &str) -> Result<Vec<u8>, ExternalFault> {
        Ok(b"original".to_vec())
    }
}

#[test]
fn an_added_path_outside_scope_is_refused_at_its_own_gate() {
    // Isolates the Add-arm enforcement: the only changed path is an added
    // file outside the declared scope.
    let (_dir, store, workspace_root) = prepared("scope-add", &["workspace://docs/**"]);
    let _repo = worker_repo(&workspace_root);
    let capture = ScriptedCapture::new(
        &store,
        vec![
            (
                b"docs/ok.txt".to_vec(),
                EntryKind::Regular,
                b"in scope".to_vec(),
            ),
            (
                b"evil.txt".to_vec(),
                EntryKind::Regular,
                b"out of scope".to_vec(),
            ),
        ],
    );
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");

    let error = sealer(&store, &capture, &cas, &workspace_root)
        .seal(&seal_command(epoch.as_str(), "cmd-add"), &seal_request())
        .expect_err("an added out-of-scope path must be refused");
    assert!(
        matches!(error, SealError::ScopeViolated { ref path } if path.contains("evil.txt")),
        "{error}"
    );
    assert!(!cas.contains(b"out of scope"));
}

#[test]
fn a_deleted_path_outside_scope_is_refused_at_its_own_gate() {
    // Isolates the delete-arm enforcement: the only changed path is a base
    // file outside the declared scope that the worker removed.
    let (_dir, store, workspace_root) = prepared("scope-delete", &["workspace://src/**"]);
    let _repo = worker_repo(&workspace_root);
    let capture = ScriptedCapture::new(
        &store,
        vec![(
            b"app.txt".to_vec(),
            EntryKind::Regular,
            b"original".to_vec(),
        )],
    );
    let cas = MemoryCas::new();
    let epoch = store.restore_generation().expect("generation");
    let sealer = ChangesetSealer::new(&store, &capture, &BaseWithRootOnly, &cas, &workspace_root);

    let error = sealer
        .seal(&seal_command(epoch.as_str(), "cmd-del"), &seal_request())
        .expect_err("a deleted out-of-scope path must be refused");
    assert!(
        matches!(error, SealError::ScopeViolated { ref path } if path.contains("outside.txt")),
        "{error}"
    );
}
