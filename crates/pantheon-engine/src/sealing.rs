//! Sealing a settled Task Workspace into an immutable `code.changeset`.
//!
//! This controller owns the ordering that makes the sealed Artifact
//! trustworthy, and nothing else decides it:
//!
//! ```text
//! re-read Task + Run + Workspace authority      (durable truth, not caller claims)
//!   ↓ freeze the Workspace                      Ready -> Frozen, under the command envelope
//!   ↓ pin the capture root                      derived from durable identity + controller root
//!   ↓ list the trusted base tree                (port: platform crate)
//!   ↓ confined no-follow capture                (port: platform crate)
//!   ↓ publish every payload to CAS              (port: CAS backend) — bytes durable FIRST
//!   ↓ compare same-kind/same-size entries       through Git object names of the captured
//!                                                 bytes — no unchanged base blob is read
//!   ↓ derive changed/deleted before-states from the immutable base through
//!     trusted repository state                  (port: platform crate)
//!   ↓ diff logical states, scope-check every changed path
//!   ↓ build the canonical manifest              (pantheon-core, pure)
//!   ↓ ONE authoritative transaction             re-read fence + Run authority, commit rows
//! ```
//!
//! # Authority, stated truthfully
//!
//! Since #29 a coding Task does its work under a durable Run: T3 commits
//! the Task to `Active` with a single nonterminal responsible Run, freezing
//! the exact Binding, source snapshot, Workspace identity and immutable
//! base the Run operates against. Canonical sealing happens on
//! `task.submit_result`, under that same Run. The [`SealAuthority`] a
//! caller presents names that Run plus the `run_status` revision its proof
//! was read at, and it is a claim, never a fact: the freeze transaction,
//! the already-frozen revalidation, and the final publication each re-read
//! every fact inside their own authoritative transaction and refuse the
//! seal unless the named Run is still the Task's current responsible Run,
//! nonterminal, at the claimed revision, bound to exactly this Workspace
//! at its immutable base, and operating under a specification whose
//! requested output slot permits a `code.changeset`. There is deliberately
//! no zero-Run authority left: a Task with zero nonterminal Runs has no
//! execution owner, so nothing settled exists to seal.
//!
//! # Quiescence, stated truthfully
//!
//! Freezing is the durable half of quiescence: after it commits, every
//! Pantheon-visible mutation path is serialized behind the single
//! authoritative writer and the Workspace row says `Frozen`. The other half
//! — that no execution owner is running — is *proved*, not assumed: the
//! freeze transaction requires the owning Task to be `Active` under the
//! claimed current Run, so the fence and the responsibility relation are
//! established by the same authoritative commit. What freezing deliberately
//! does NOT claim is that some out-of-band writer has stopped touching the
//! filesystem; that risk is answered by the confined capture boundary
//! (fail-closed races), not by the database.
//!
//! # Failure shape
//!
//! Any failure after the freeze retains the freeze and records typed
//! evidence through a separate durable command. Thawing after an
//! unexplained failure would hand mutation authority back while nobody
//! knows what the failure touched; rematerializing would destroy
//! worker-writable state that may be unsealed work. Recovery decides later,
//! from evidence.
//!
//! # Command identity under replay
//!
//! Every inner command identity is derived from the outer command id *and*
//! the authority facts (`run_id @ expected revision`) plus the observed
//! fence revision. A retry of the same request therefore replays the prior
//! outcome legitimately, while a retry presenting different authority
//! derives a different identity, executes afresh, and is refused by the
//! in-transaction revalidation — no ledger entry from an earlier decision
//! can authorize a later, different claim.
//!
//! # What this controller does not do
//!
//! No Candidate submission, no Task lifecycle transition, no shared-ref
//! work, no GC. A sealed Artifact is durable output; what accepts it is a
//! later boundary.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use pantheon_core::artifact::{
    CODE_CHANGESET_KIND, ChangesetEntry, ChangesetError, EntryKind, EntryState, FinalEntry,
    Operation, RepositoryPath, RevisionState, changeset_manifest,
};
use pantheon_core::config::Digest;
use pantheon_core::planning::{TaskDecodeError, TaskPhase, TaskSpec};
use pantheon_core::workspace::{Materialization, WorkspacePhase};
use pantheon_store::{Committed, SealAuthority, SealedChangeset, Store, StoreError};

use crate::workspace::MAX_COMMAND_ID;

/// A failure reported by a port implementation.
///
/// Carries a stable, namespaced code so callers can distinguish security
/// failures (`workspace.hostile-*`) from ordinary ones without parsing text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFault {
    pub code: String,
    pub detail: String,
}

impl fmt::Display for ExternalFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ExternalFault {}

/// One payload-bearing entry of the captured logical tree: its lossless
/// path, its canonical kind, and its exact bytes (file content, or link
/// target bytes for a symlink).
#[derive(Debug)]
pub struct CapturedEntry {
    pub path: RepositoryPath,
    pub kind: EntryKind,
    pub bytes: Vec<u8>,
}

/// One present object of the trusted base tree, addressed by its Git object
/// identity within the source repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseObject {
    pub kind: EntryKind,
    /// The validated hexadecimal object name of the blob.
    pub oid: String,
    pub size: u64,
}

/// A published CAS object reference: SHA-256 plus size, the Blob identity
/// the Artifact contract defines. Storage location is deliberately absent —
/// it is never identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    pub digest: Digest,
    pub size: u64,
}

/// The root-confined, no-follow Workspace capture boundary.
///
/// Implemented by a platform crate. The contract it must satisfy is the
/// canonical one: the trusted root is opened once from durable controller
/// state, descendants are enumerated relative to already-open trusted
/// directory objects, symlinks are captured as link-target bytes and never
/// dereferenced, regular-file payloads are read through the very descriptor
/// that was validated (no check-then-reopen by pathname), and unsupported
/// special objects or escapes fail closed with `workspace.hostile-
/// filesystem-state`.
pub trait WorkspaceTreeCapture {
    /// Streams every permitted entry of the tree beneath `root` to `sink`.
    ///
    /// An error from `sink` aborts the walk and is returned unchanged, so
    /// downstream CAS failures stop capture rather than being retried past.
    ///
    /// # Errors
    ///
    /// [`ExternalFault`], always fail-closed.
    fn capture_tree(
        &self,
        root: &Path,
        sink: &mut dyn FnMut(CapturedEntry) -> Result<(), ExternalFault>,
    ) -> Result<(), ExternalFault>;
}

/// Reads authoritative before-state from the resolved immutable base,
/// through controller-owned/trusted repository state.
///
/// The implementing platform crate runs its reads under a sterile profile;
/// it never consults the worker Workspace's `.git`, index, refs or object
/// database, because none of those is authority for preimage state.
pub trait TrustedBaseReader {
    /// The full logical tree of the base commit: raw path bytes to object.
    ///
    /// # Errors
    ///
    /// [`ExternalFault`] when the source cannot be read, the base is gone,
    /// or the tree names structures v1 does not support.
    fn base_tree(
        &self,
        source: &Path,
        base: &pantheon_core::workspace::ResolvedBase,
    ) -> Result<BTreeMap<Vec<u8>, BaseObject>, ExternalFault>;

    /// Computes, without writing any object anywhere, the Git object name
    /// each payload would carry as a blob under `source`'s own object
    /// format.
    ///
    /// This is the changed-path comparison signal. A captured entry whose
    /// canonical kind and size match its base entry is unchanged exactly
    /// when this name equals the base entry's recorded name, so sealing
    /// never needs the bytes of a same-size base blob that did not change.
    /// The names are compared against base metadata only; they never enter
    /// Artifact or CAS identity, which stay SHA-256 over exact bytes.
    /// Implementations must consult only controller-owned state and the
    /// source repository itself — never worker Workspace Git state — and
    /// must derive names from payload bytes alone, so the result is
    /// deterministic and independent of process-local cache warmth.
    ///
    /// Returns exactly one name per payload, in input order.
    ///
    /// # Errors
    ///
    /// [`ExternalFault`] when the source cannot be consulted or an answer
    /// is not a canonical object name.
    fn blob_object_names(
        &self,
        source: &Path,
        contents: &[&[u8]],
    ) -> Result<Vec<String>, ExternalFault>;

    /// The exact bytes of one base blob.
    ///
    /// # Errors
    ///
    /// [`ExternalFault`] when the object is missing or unreadable.
    fn blob_bytes(&self, source: &Path, oid: &str) -> Result<Vec<u8>, ExternalFault>;
}

/// The controller-owned content-addressed store.
///
/// Roots and temporary paths come only from controller composition, never
/// from Task or worker input, and are never exposed to a Sandbox.
pub trait ContentObjectStore {
    /// Hashes `bytes`, stages them durably, and publishes them atomically
    /// into the digest namespace, verifying what lands. Idempotent for a
    /// repeated digest; corruption of a pre-existing object fails closed.
    ///
    /// # Errors
    ///
    /// [`ExternalFault`] on any staging, durability, publication or
    /// verification failure.
    fn publish(&self, bytes: &[u8]) -> Result<ObjectRef, ExternalFault>;

    /// Confirms an object's bytes are present and match their claimed
    /// identity.
    ///
    /// # Errors
    ///
    /// [`ExternalFault`] when missing, unreadable, or corrupt.
    fn verify(&self, reference: &ObjectRef) -> Result<(), ExternalFault>;

    /// Reads one object's exact bytes back.
    ///
    /// # Errors
    ///
    /// [`ExternalFault`] when missing or corrupt.
    fn read(&self, reference: &ObjectRef) -> Result<Vec<u8>, ExternalFault>;
}

/// The durable command identity a sealing operation runs under.
#[derive(Debug, Clone, Copy)]
pub struct SealCommand<'a> {
    pub epoch: &'a str,
    pub id: &'a str,
    pub request_hash: &'a [u8; 32],
}

/// What is asked to be sealed.
#[derive(Debug, Clone)]
pub struct SealRequest<'a> {
    pub task_id: &'a str,
    /// The TaskSpec output slot this seal produces.
    pub output_slot: &'a str,
    /// The Run authority claimed for this seal. A claim, never a fact:
    /// every authoritative boundary re-reads and re-proves it.
    pub authority: SealAuthority,
}

/// Why a seal failed.
#[derive(Debug)]
pub enum SealError {
    Store(StoreError),
    /// The Task does not exist, or its durable specification could not be
    /// read and verified against its digest.
    TaskUnusable {
        task_id: String,
        detail: String,
    },
    /// The requested output slot does not exist in the immutable TaskSpec,
    /// or does not permit a `code.changeset`.
    OutputSlotInvalid {
        task_id: String,
        slot: String,
        detail: String,
    },
    /// The Task's Workspace is not in a capturable state, or the request
    /// contradicts its binding.
    WorkspaceState {
        workspace_id: String,
        detail: String,
    },
    /// A port failed closed. Security violations carry the canonical
    /// `workspace.hostile-*` codes.
    Capture(ExternalFault),
    /// A changed path falls outside the Task's declared resource scope.
    ScopeViolated {
        path: String,
    },
    /// Capture exceeded a configured ceiling (entries or total bytes).
    CeilingsExceeded {
        detail: String,
    },
    /// A derived command identity would exceed what the durable ledger
    /// accepts.
    CommandIdentityTooLong {
        id: String,
    },
}

impl fmt::Display for SealError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(err) => write!(f, "sealing store failure: {err}"),
            Self::TaskUnusable { task_id, detail } => {
                write!(f, "task {task_id} specification is unusable: {detail}")
            }
            Self::OutputSlotInvalid {
                task_id,
                slot,
                detail,
            } => write!(
                f,
                "task {task_id} output slot {slot:?} is invalid: {detail}"
            ),
            Self::WorkspaceState {
                workspace_id,
                detail,
            } => write!(f, "workspace {workspace_id} cannot be sealed: {detail}"),
            Self::Capture(fault) => write!(f, "capture failed: {fault}"),
            Self::ScopeViolated { path } => {
                write!(
                    f,
                    "changed path {path:?} is outside the Task's declared scope"
                )
            }
            Self::CeilingsExceeded { detail } => write!(f, "capture ceiling exceeded: {detail}"),
            Self::CommandIdentityTooLong { id } => {
                write!(f, "derived command identity {id:?} is too long")
            }
        }
    }
}

impl std::error::Error for SealError {}

impl From<StoreError> for SealError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

impl From<ChangesetError> for SealError {
    fn from(err: ChangesetError) -> Self {
        // Manifest construction failures are programming errors guarded by
        // construction elsewhere; surface them honestly as store-shaped
        // invariant failures rather than inventing a caller-facing class.
        Self::Store(StoreError::InvariantViolated(format!(
            "changeset construction failed: {err}"
        )))
    }
}

/// What a completed seal produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedArtifact {
    /// The canonical manifest digest — the Artifact identity.
    pub artifact_digest: Digest,
    /// The canonical manifest JSON.
    pub artifact_json: String,
    /// The immutable semantic-state digest the Artifact binds.
    pub revision_state_digest: Digest,
    /// Whether prior identical content was reused rather than created.
    pub artifact_reused: bool,
}

/// Capture ceilings. Failing closed beats an unbounded walk of whatever a
/// worker left in the Workspace; a Task needing more gets a wider ceiling as
/// an explicit configuration decision, not silently.
const MAX_ENTRIES: usize = 100_000;
const MAX_TOTAL_BYTES: u64 = 1 << 30;

/// How many captured bytes may sit in the identity batch before it is
/// converted to Git object names. Bounds peak memory during comparison
/// while keeping the number of identity computations proportional to
/// candidate bytes rather than to the number of unchanged files.
const MAX_IDENTITY_BATCH_BYTES: usize = 16 << 20;

/// The fault code the capture sink uses to report an out-of-scope path, so
/// the controller can surface it as the typed [`SealError::ScopeViolated`]
/// rather than a generic capture failure.
const SCOPE_FAULT_CODE: &str = "workspace.scope-violated";

/// Seals the Task's settled Workspace state into a CAS-complete
/// `code.changeset`.
pub struct ChangesetSealer<'a> {
    store: &'a Store,
    capture: &'a dyn WorkspaceTreeCapture,
    base: &'a dyn TrustedBaseReader,
    objects: &'a dyn ContentObjectStore,
    /// The controller-owned root every Workspace path is derived beneath —
    /// the same root the materialization controller derives from.
    workspace_root: PathBuf,
}

impl<'a> ChangesetSealer<'a> {
    #[must_use]
    pub fn new(
        store: &'a Store,
        capture: &'a dyn WorkspaceTreeCapture,
        base: &'a dyn TrustedBaseReader,
        objects: &'a dyn ContentObjectStore,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            store,
            capture,
            base,
            objects,
            workspace_root: workspace_root.into(),
        }
    }

    /// The deterministic capture root for a Workspace: a pure function of
    /// the controller root and the durable Workspace identity, mirroring
    /// `WorkspaceController::path_of`. Never a caller-supplied host path.
    #[must_use]
    pub fn capture_root_of(&self, workspace_id: &str) -> PathBuf {
        self.workspace_root.join(workspace_id).join("repo")
    }

    /// Runs the whole sealing order for one Task output slot.
    ///
    /// Idempotent under retry: the freeze or its revalidation replays, CAS
    /// publication is digest-idempotent, and the final publication replays
    /// or converges on existing content. Two different commands capturing
    /// identical state converge on one Artifact identity. Command identity
    /// is bound to the claimed authority facts, so a retry presenting a
    /// different Run revision derives a different command, executes afresh,
    /// and is refused by the in-transaction revalidation rather than
    /// inheriting an earlier decision's outcome.
    ///
    /// # Errors
    ///
    /// [`SealError`] as documented per variant. On every failure the
    /// Workspace stays frozen (or unfrozen-but-untouched) and no complete
    /// Artifact claim exists.
    pub fn seal(
        &self,
        command: &SealCommand<'_>,
        request: &SealRequest<'_>,
    ) -> Result<SealedArtifact, SealError> {
        // The claimed authority is never trusted here; each authoritative
        // boundary below re-reads durable state inside its own transaction
        // and refuses the seal unless the named Run is still current.

        // ---- Authority, read fresh from durable state. ----
        let spec = self.load_spec(request.task_id)?;
        let slot = spec
            .outputs
            .iter()
            .find(|output| output.name == request.output_slot)
            .ok_or_else(|| SealError::OutputSlotInvalid {
                task_id: request.task_id.to_string(),
                slot: request.output_slot.to_string(),
                detail: "no such output slot in the immutable specification".to_string(),
            })?;
        if slot.kind != CODE_CHANGESET_KIND {
            return Err(SealError::OutputSlotInvalid {
                task_id: request.task_id.to_string(),
                slot: request.output_slot.to_string(),
                detail: format!("it permits {}, not {CODE_CHANGESET_KIND}", slot.kind),
            });
        }

        let workspace = self
            .store
            .workspace_for_task(request.task_id)?
            .ok_or_else(|| SealError::WorkspaceState {
                workspace_id: "-".to_string(),
                detail: format!("task {} owns no current Workspace", request.task_id),
            })?;
        // The Workspace binds to the repository the Task declares, checked
        // against the same immutable spec the binding was made from.
        let repository_ref = spec
            .inputs
            .iter()
            .find(|input| input.name == pantheon_core::workspace::REPOSITORY_INPUT)
            .map(|input| input.reference.clone())
            .ok_or_else(|| SealError::TaskUnusable {
                task_id: request.task_id.to_string(),
                detail: "it declares no repository input to have worked in".to_string(),
            })?;
        if workspace.repository != repository_ref {
            return Err(SealError::WorkspaceState {
                workspace_id: workspace.id.clone(),
                detail: format!(
                    "it binds {} but the Task declares {repository_ref}",
                    workspace.repository
                ),
            });
        }

        // ---- Quiesce: establish or revalidate the fence under the
        // claimed Run authority. Both arms are authoritative validation
        // boundaries: capture never runs against a Workspace whose seal
        // authority was not proven inside a serialized transaction. ----
        let fence_revision = match workspace.phase {
            WorkspacePhase::Ready => {
                let id = self.derive(
                    command.id,
                    &format!(
                        "freeze:{}:{}",
                        workspace.revision.get(),
                        authority_tag(&request.authority)
                    ),
                )?;
                let committed = self.store.freeze_workspace(
                    &pantheon_store::Command {
                        epoch: command.epoch,
                        id: &id,
                        request_hash: command.request_hash,
                        event_type: "workspace.frozen",
                    },
                    &request.authority,
                    request.task_id,
                    request.output_slot,
                    &workspace.id,
                    workspace.revision,
                );
                self.settle(committed)?;
                // Re-read: the fence's own revision is what the final
                // publication will CAS against.
                self.frozen_revision(&workspace.task_id, &workspace.id)?
            }
            WorkspacePhase::Frozen => {
                if workspace.materialization != Materialization::Present {
                    return Err(SealError::WorkspaceState {
                        workspace_id: workspace.id.clone(),
                        detail: "it is frozen but its materialization is not verified present"
                            .to_string(),
                    });
                }
                // An already-fenced Workspace must not bypass current Run
                // authorization merely because its fence exists.
                let id = self.derive(
                    command.id,
                    &format!(
                        "validated:{}:{}",
                        workspace.revision.get(),
                        authority_tag(&request.authority)
                    ),
                )?;
                let committed = self.store.validate_seal_authority_command(
                    &pantheon_store::Command {
                        epoch: command.epoch,
                        id: &id,
                        request_hash: command.request_hash,
                        event_type: "workspace.seal-authority.validated",
                    },
                    &request.authority,
                    request.task_id,
                    request.output_slot,
                    &workspace.id,
                    workspace.revision,
                );
                self.settle(committed)?;
                workspace.revision
            }
            phase => {
                return Err(SealError::WorkspaceState {
                    workspace_id: workspace.id.clone(),
                    detail: format!(
                        "it is {phase}; capture requires a verified, mutable-or-frozen \
                         Workspace"
                    ),
                });
            }
        };
        let scope = pantheon_core::planning::scope::WorkspaceScope::compile(&spec.scope.resources)
            .map_err(|_| SealError::CeilingsExceeded {
                detail: "the Task's declared scope patterns are unusable".to_string(),
            })?;
        if scope.is_empty() {
            // An empty scope authorizes nothing: refusing up front keeps the
            // rule visible instead of discovered per-path.
            return Err(SealError::ScopeViolated {
                path: "(every path: the Task declares no resource scope)".to_string(),
            });
        }

        // ---- Capture, CAS, diff, manifest — all after the fence. ----
        match self.capture_and_seal(command, request, &workspace, fence_revision, &scope) {
            Ok(sealed) => Ok(sealed),
            Err(error) => {
                // Retain the freeze; record typed evidence separately. A
                // failure to record evidence must not mask the original
                // cause, but must also not pass unnoticed: it surfaces as
                // the store error instead.
                let id = self.derive(
                    command.id,
                    &format!("capture-failed:{}", fence_revision.get()),
                )?;
                self.store
                    .record_capture_failure(
                        &pantheon_store::Command {
                            epoch: command.epoch,
                            id: &id,
                            request_hash: command.request_hash,
                            event_type: "workspace.capture-failed",
                        },
                        &workspace.id,
                        fence_revision,
                    )
                    .map_err(SealError::Store)?;
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_and_seal(
        &self,
        command: &SealCommand<'_>,
        request: &SealRequest<'_>,
        workspace: &pantheon_store::WorkspaceRecord,
        fence_revision: pantheon_store::Revision,
        scope: &pantheon_core::planning::scope::WorkspaceScope,
    ) -> Result<SealedArtifact, SealError> {
        let capture_root = self.capture_root_of(&workspace.id);
        let source = PathBuf::from(&workspace.source_path);

        // Authoritative before-state metadata comes from the trusted
        // immutable base before capture runs: the walk then knows per entry
        // whether its Git identity is needed for comparison, and a base
        // that cannot be read fails before any Workspace byte is staged.
        let base_tree = self
            .base
            .base_tree(&source, &workspace.resolved_base)
            .map_err(SealError::Capture)?;

        // Confined capture streams straight into CAS: peak memory stays at
        // one object, and every payload is durable before anything
        // references it.
        let mut final_state: BTreeMap<Vec<u8>, (EntryKind, Digest, u64)> = BTreeMap::new();
        // Captured paths whose bytes were deliberately not retained because
        // they sit outside the declared scope. If any of them turns out to
        // have changed, the diff refuses the seal: an out-of-authority path
        // can never become changeset output.
        let mut unpublished: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
        let mut total_bytes: u64 = 0;
        // Captured entries whose content identity will be decided through
        // Git object names: present in the base at the same canonical kind
        // and size, so only content can still distinguish them. Their bytes
        // are batched here and converted in as few calls as the budget
        // allows; nothing is read from the base to compare them.
        let mut identity_batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut identity_batch_bytes = 0usize;
        // Path -> the Git object name of the captured bytes. Populated only
        // for entries that needed the comparison signal.
        let mut captured_identities: BTreeMap<Vec<u8>, String> = BTreeMap::new();
        self.capture
            .capture_tree(&capture_root, &mut |entry: CapturedEntry| {
                // Scope decides RETENTION, not existence. Every captured
                // entry is hashed — the diff needs content identity for the
                // whole tree — but bytes are made durable in CAS only for
                // in-scope paths, so content outside the Task's authority
                // is never written into Pantheon's CAS, not even as an
                // orphan. Whether an out-of-scope path actually changed is
                // only known at the diff below; if it did, that is an
                // authority violation and the seal fails there, before any
                // manifest or database row exists.
                if final_state.len() >= MAX_ENTRIES {
                    return Err(ExternalFault {
                        code: "workspace.capture-ceiling".to_string(),
                        detail: format!("more than {MAX_ENTRIES} entries"),
                    });
                }
                total_bytes = total_bytes.saturating_add(entry.bytes.len() as u64);
                if total_bytes > MAX_TOTAL_BYTES {
                    return Err(ExternalFault {
                        code: "workspace.capture-ceiling".to_string(),
                        detail: format!("more than {MAX_TOTAL_BYTES} captured bytes"),
                    });
                }
                let digest = Digest::of(&entry.bytes);
                let size = entry.bytes.len() as u64;
                if scope.authorizes(&entry.path) {
                    let published = self.objects.publish(&entry.bytes)?;
                    if published.size != size || published.digest != digest {
                        return Err(ExternalFault {
                            code: "cas.verify-failed".to_string(),
                            detail: format!(
                                "published {} bytes as {}, expected {} as {}",
                                published.size, published.digest, size, digest
                            ),
                        });
                    }
                } else {
                    unpublished.insert(entry.path.sort_key().to_vec());
                }
                final_state.insert(entry.path.sort_key().to_vec(), (entry.kind, digest, size));
                // A same-path, same-kind, same-size entry can still prove
                // itself unchanged through its Git object name alone. Its
                // bytes join the batch; no base blob is read for it here or
                // below.
                let comparison_candidate = base_tree
                    .get(entry.path.sort_key())
                    .is_some_and(|before| before.kind == entry.kind && before.size == size);
                if comparison_candidate {
                    identity_batch.push((entry.path.sort_key().to_vec(), entry.bytes.clone()));
                    identity_batch_bytes += entry.bytes.len();
                    if identity_batch_bytes >= MAX_IDENTITY_BATCH_BYTES {
                        flush_identity_batch(
                            &source,
                            self.base,
                            &mut identity_batch,
                            &mut captured_identities,
                        )?;
                        identity_batch_bytes = 0;
                    }
                }
                Ok(())
            })
            .map_err(|fault| {
                if fault.code == SCOPE_FAULT_CODE {
                    SealError::ScopeViolated { path: fault.detail }
                } else {
                    SealError::Capture(fault)
                }
            })?;
        // The last batch carries whatever the walk ended on.
        flush_identity_batch(
            &source,
            self.base,
            &mut identity_batch,
            &mut captured_identities,
        )
        .map_err(SealError::Capture)?;

        // Diff the two logical states. Keys are raw path bytes in both maps,
        // so merged iteration is canonical order by construction.
        let mut entries: Vec<ChangesetEntry> = Vec::new();
        for (path_bytes, after) in &final_state {
            let path = RepositoryPath::from_bytes(path_bytes)?;
            let (after_kind, after_digest, after_size) = after;
            let after_state = EntryState::Present {
                kind: *after_kind,
                blob: *after_digest,
                size: *after_size,
            };
            let entry = match base_tree.get(path_bytes) {
                None => {
                    // Scope gates changeset output. An unchanged
                    // out-of-scope file is preexisting tree content, not an
                    // attempt to produce output, so the refusal lands only
                    // on entries that would actually enter the changeset —
                    // via the retention gate below, which is the single
                    // authority check for added and modified paths.
                    refuse_unpublished_change(path_bytes, &unpublished)?;
                    ChangesetEntry::new(path, Operation::Add, EntryState::Absent, after_state)?
                }
                Some(before) => {
                    // Unchanged is provable without any base-blob read:
                    // same canonical kind, same size, and the Git object
                    // name of the captured bytes equals the name the base
                    // tree records. Equal names prove equal content; a
                    // same-size collision carries a different name and
                    // falls through as a Modify.
                    let unchanged = before.kind == *after_kind
                        && before.size == *after_size
                        && captured_identities
                            .get(path_bytes)
                            .is_some_and(|name| name == &before.oid);
                    if unchanged {
                        continue;
                    }
                    refuse_unpublished_change(path_bytes, &unpublished)?;
                    let before_state = self.before_state(&source, before)?;
                    ChangesetEntry::new(path, Operation::Modify, before_state, after_state)?
                }
            };
            entries.push(entry);
        }
        for (path_bytes, before) in &base_tree {
            if final_state.contains_key(path_bytes) {
                continue;
            }
            let path = RepositoryPath::from_bytes(path_bytes)?;
            if !scope.authorizes(&path) {
                return Err(SealError::ScopeViolated {
                    path: path.to_manifest_string(),
                });
            }
            let before_state = self.before_state(&source, before)?;
            entries.push(ChangesetEntry::new(
                path,
                Operation::Delete,
                before_state,
                EntryState::Absent,
            )?);
        }

        // Canonical identities: pure computation over the settled facts.
        let mut final_entries: Vec<FinalEntry> = Vec::with_capacity(final_state.len());
        for (path_bytes, (kind, digest, size)) in &final_state {
            final_entries.push(FinalEntry {
                path: RepositoryPath::from_bytes(path_bytes)?,
                kind: *kind,
                blob: *digest,
                size: *size,
            });
        }
        let revision_state = RevisionState::new(final_entries)?;
        let manifest = changeset_manifest(
            &workspace.repository,
            workspace.resolved_base.as_str(),
            revision_state.digest(),
            &entries,
        );
        let artifact_json = manifest.to_string();
        let artifact_digest = Digest::of(artifact_json.as_bytes());

        // ---- One authoritative publication. ----
        let members: Vec<(Digest, u64)> = entries
            .iter()
            .flat_map(|entry| match (entry.before(), entry.after()) {
                (EntryState::Present { blob, size, .. }, _) => Some((*blob, *size)),
                _ => None,
            })
            .chain(entries.iter().filter_map(|entry| match entry.after() {
                EntryState::Present { blob, size, .. } => Some((*blob, *size)),
                EntryState::Absent => None,
            }))
            .collect();
        let mut unique_members: Vec<(Digest, u64)> = Vec::new();
        for member in members {
            if !unique_members.iter().any(|(digest, _)| *digest == member.0) {
                unique_members.push(member);
            }
        }

        let id = self.derive(
            command.id,
            &format!(
                "sealed:{}:{}",
                fence_revision.get(),
                authority_tag(&request.authority)
            ),
        )?;
        let committed = self.store.commit_changeset_seal(
            &pantheon_store::Command {
                epoch: command.epoch,
                id: &id,
                request_hash: command.request_hash,
                event_type: "workspace.sealed",
            },
            &SealedChangeset {
                workspace_id: &workspace.id,
                task_id: &workspace.task_id,
                fence_revision,
                authority: &request.authority,
                output_slot: request.output_slot,
                repository: &workspace.repository,
                resolved_base: workspace.resolved_base.as_str(),
                revision_state_digest: revision_state.digest(),
                revision_state_json: &revision_state.to_canonical_json(),
                artifact_digest,
                artifact_json: &artifact_json,
                members: unique_members,
            },
        );

        let artifact_reused = match committed? {
            Committed::Executed { value, .. } => value.artifact_reused,
            // Replay carries no value by design: the manifest digest was
            // computed from content this process still holds, so confirming
            // the row exists is the reconciliation.
            Committed::Replayed { .. } => self.store.artifact(artifact_digest)?.is_some(),
        };

        Ok(SealedArtifact {
            artifact_digest,
            artifact_json,
            revision_state_digest: revision_state.digest(),
            artifact_reused,
        })
    }

    /// Builds the authoritative before-state for one base object, copying
    /// its preimage bytes into CAS first.
    fn before_state(&self, source: &Path, before: &BaseObject) -> Result<EntryState, SealError> {
        let bytes = self
            .base
            .blob_bytes(source, &before.oid)
            .map_err(SealError::Capture)?;
        let published = self.objects.publish(&bytes).map_err(SealError::Capture)?;
        if published.size != bytes.len() as u64 || published.size != before.size {
            return Err(SealError::Capture(ExternalFault {
                code: "cas.verify-failed".to_string(),
                detail: format!(
                    "preimage for {} published {} bytes, claimed {}",
                    before.oid, published.size, before.size
                ),
            }));
        }
        Ok(EntryState::Present {
            kind: before.kind,
            blob: published.digest,
            size: published.size,
        })
    }

    /// The frozen revision of a Workspace, refusing anything but a held
    /// fence with verified presence.
    fn frozen_revision(
        &self,
        task_id: &str,
        workspace_id: &str,
    ) -> Result<pantheon_store::Revision, SealError> {
        let record = self
            .store
            .workspace_for_task(task_id)?
            .filter(|record| record.id == workspace_id)
            .ok_or_else(|| SealError::WorkspaceState {
                workspace_id: workspace_id.to_string(),
                detail: "it vanished immediately after its own freeze".to_string(),
            })?;
        if record.phase != WorkspacePhase::Frozen
            || record.materialization != Materialization::Present
        {
            return Err(SealError::WorkspaceState {
                workspace_id: workspace_id.to_string(),
                detail: format!(
                    "after freezing it is {} ({})",
                    record.phase.as_str(),
                    record.materialization.as_str()
                ),
            });
        }
        Ok(record.revision)
    }

    fn settle(
        &self,
        committed: Result<pantheon_store::Committed<pantheon_store::WorkspaceRecord>, StoreError>,
    ) -> Result<(), SealError> {
        match committed? {
            Committed::Executed { .. } => Ok(()),
            Committed::Replayed { .. } => Ok(()),
        }
    }

    /// Loads and digest-verifies the Task's immutable specification.
    ///
    /// Sealing runs under a responsible Run, so the Task must be `Active`;
    /// the deeper Run facts (currency, ownership, ceiling) are re-proven
    /// inside the authoritative transactions, not here.
    fn load_spec(&self, task_id: &str) -> Result<TaskSpec, SealError> {
        let task = self
            .store
            .task(task_id)?
            .ok_or_else(|| SealError::TaskUnusable {
                task_id: task_id.to_string(),
                detail: "no such task".to_string(),
            })?;
        if task.phase != TaskPhase::Active {
            return Err(SealError::TaskUnusable {
                task_id: task_id.to_string(),
                detail: format!(
                    "the task is {}; sealing runs under its current Run",
                    task.phase.as_str()
                ),
            });
        }
        let canonical = self
            .store
            .task_spec_json(task.spec_digest)?
            .ok_or_else(|| SealError::TaskUnusable {
                task_id: task_id.to_string(),
                detail: "the specification the task names is not stored".to_string(),
            })?;
        let spec =
            TaskSpec::from_canonical_json(&canonical).map_err(|TaskDecodeError(detail)| {
                SealError::TaskUnusable {
                    task_id: task_id.to_string(),
                    detail,
                }
            })?;
        if spec.digest() != task.spec_digest {
            return Err(SealError::TaskUnusable {
                task_id: task_id.to_string(),
                detail: "the stored specification does not match its digest".to_string(),
            });
        }
        Ok(spec)
    }

    /// Derives an inner command identity from the outer command id and the
    /// step's facts. Authority-bearing steps embed the authority tag, so a
    /// retry claiming different authority derives a genuinely new command
    /// and is validated afresh instead of replaying a prior outcome.
    fn derive(&self, base: &str, suffix: &str) -> Result<String, SealError> {
        let id = format!("{base}:seal:{suffix}");
        if id.len() > MAX_COMMAND_ID {
            return Err(SealError::CommandIdentityTooLong { id });
        }
        Ok(id)
    }
}

/// The compact identity of a claimed seal authority: which Run, at which
/// observed status revision.
fn authority_tag(authority: &SealAuthority) -> String {
    format!(
        "{}@{}",
        authority.run_id,
        authority.expected_run_revision.get()
    )
}

#[cfg(test)]
mod tests;

/// Converts one batch of staged payloads into Git object names and records
/// them per path, draining the batch.
///
/// A count mismatch or unusable name from the port fails closed: without a
/// trustworthy name for a candidate entry there is no proof of either
/// state, and guessing would risk publishing a wrong changeset.
fn flush_identity_batch(
    source: &Path,
    base: &dyn TrustedBaseReader,
    batch: &mut Vec<(Vec<u8>, Vec<u8>)>,
    identities: &mut BTreeMap<Vec<u8>, String>,
) -> Result<(), ExternalFault> {
    if batch.is_empty() {
        return Ok(());
    }
    let contents: Vec<&[u8]> = batch.iter().map(|(_, bytes)| bytes.as_slice()).collect();
    let names = base.blob_object_names(source, &contents)?;
    if names.len() != contents.len() {
        return Err(ExternalFault {
            code: "workspace.base-unavailable".to_string(),
            detail: format!(
                "the base identity derivation returned {} names for {} payloads",
                names.len(),
                contents.len()
            ),
        });
    }
    for ((path, _), name) in batch.drain(..).zip(names) {
        identities.insert(path, name);
    }
    Ok(())
}

/// Refuses a changed path whose bytes were deliberately not retained
/// because they sit outside the declared scope: an attempt to produce
/// output the Task was never authorized to produce.
fn refuse_unpublished_change(
    path_bytes: &[u8],
    unpublished: &std::collections::HashSet<Vec<u8>>,
) -> Result<(), SealError> {
    if unpublished.contains(path_bytes) {
        return Err(SealError::ScopeViolated {
            path: RepositoryPath::from_bytes(path_bytes)
                .map(|p| p.to_manifest_string())
                .unwrap_or_else(|_| "(unrepresentable)".to_string()),
        });
    }
    Ok(())
}
