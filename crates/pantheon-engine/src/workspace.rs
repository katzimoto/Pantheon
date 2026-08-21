//! The Task-owned Workspace control path.
//!
//! Materializing a Workspace is the first place Pantheon's control plane
//! performs an irreversible external effect, so the ordering between durable
//! state and that effect is the whole design:
//!
//! ```text
//! resolve requested base against the source repository   (read-only)
//!   ↓
//! commit Workspace identity + immutable base   phase Requested
//!   ↓
//! commit the side-effect marker                phase Materializing
//!   ↓
//! create isolated Task-local repository state  (external effect)
//!   ↓
//! commit verified materialization              phase Ready
//! ```
//!
//! Resolution happens first because a Workspace binds to an immutable object
//! identity, not to a ref that may move afterwards. Identity is committed
//! before the marker, and the marker before the first effect, because
//! recovery has to be able to tell "no external state can exist" from "some
//! may". Nothing here infers ownership from a path, a Git worktree list or a
//! lock file: `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`
//! makes SQLite the authority and filesystem observations mere evidence.
//!
//! # What this controller does not do
//!
//! It does not create a Run, a Sandbox, an ExecutionBinding or a
//! WorkspaceRevision, and it does not seal anything. A Ready Workspace is a
//! mutable execution surface; it is not Candidate identity, and reaching
//! Ready authorizes no execution by itself.

use std::fmt;
use std::path::{Path, PathBuf};

use pantheon_core::planning::{TaskDecodeError, TaskPhase, TaskSpec};
use pantheon_core::workspace::{
    Materialization, REPOSITORY_INPUT, RequestedBase, ResolvedBase, WorkspacePhase,
};
use pantheon_store::{Command, Committed, Store, StoreError, WorkspaceBinding, WorkspaceRecord};

/// The external state one Workspace owns.
///
/// Every field comes from durable Pantheon state or from the controller's own
/// configuration. In particular `destination` is derived from the Workspace
/// identity and the controller-owned root, which is what the canonical
/// recovery contract means by a Workspace's idempotency identity being
/// "Workspace ID + deterministic desired path/base": two processes reconciling
/// the same Workspace compute the same path without consulting the filesystem.
#[derive(Debug, Clone, Copy)]
pub struct MaterializationTarget<'a> {
    pub workspace_id: &'a str,
    /// The controller-trusted source repository root.
    pub source: &'a Path,
    /// The deterministic desired path for this Workspace's repository state.
    pub destination: &'a Path,
    /// The immutable identity the Workspace is durably bound to.
    pub base: &'a ResolvedBase,
}

/// The port through which Workspace repository state is created and observed.
///
/// Every method here is an external effect, which is why this is a port and
/// not an implementation: `pantheon-engine` owns the ordering above and the
/// durable transitions, and a platform crate owns how a repository is
/// actually materialized. The trait is deliberately narrow — the controller
/// never asks for a repository handle, a command line or a path it did not
/// compute itself.
pub trait RepositoryMaterializer {
    /// Resolves a possibly mutable requested base against the source
    /// repository to an immutable object identity.
    ///
    /// Read-only with respect to the source repository.
    ///
    /// # Errors
    ///
    /// [`MaterializerError`] when the source cannot be read or the requested
    /// base does not name a commit.
    fn resolve_base(
        &self,
        source: &Path,
        requested: &RequestedBase,
    ) -> Result<ResolvedBase, MaterializerError>;

    /// Creates isolated writable Task-local repository state at
    /// `target.destination`, anchored to `target.base`.
    ///
    /// Returns the identity it verified on disk. The controller compares that
    /// against the durable binding before anything becomes Ready, so a
    /// materializer that checked out the wrong commit fails closed rather
    /// than producing a Workspace whose recorded base is a lie.
    ///
    /// # Errors
    ///
    /// [`MaterializerError`] when materialization could not be completed and
    /// verified. Partial external state may remain; resolving that is the
    /// controller's job, not this method's.
    fn materialize(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<ResolvedBase, MaterializerError>;

    /// Reports the strongest factual observation about `target`'s external
    /// state.
    ///
    /// Returning [`Materialization::Unknown`] is a legitimate answer and is
    /// always safer than guessing. An implementation must never report
    /// [`Materialization::Absent`] merely because an inspection failed.
    ///
    /// # Errors
    ///
    /// [`MaterializerError`] only when the observation itself could not be
    /// attempted.
    fn observe(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<Materialization, MaterializerError>;

    /// Discards external state for a Workspace that has never been mutable to
    /// an execution owner.
    ///
    /// The controller calls this only after durable state proves the
    /// Workspace never reached `Ready`, so what is discarded is
    /// controller-owned scratch.
    ///
    /// # Errors
    ///
    /// [`MaterializerError`] when the state could not be removed.
    fn discard(&self, target: &MaterializationTarget<'_>) -> Result<(), MaterializerError>;
}

/// A materializer-side failure, reported without exposing the concrete
/// implementation to the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializerError {
    /// A stable, namespaced code. `workspace.hostile-repository-state` is the
    /// canonical fail-closed code when a repository boundary cannot be
    /// established.
    pub code: String,
    pub detail: String,
}

impl fmt::Display for MaterializerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for MaterializerError {}

/// What the caller asks for.
///
/// The repository *reference* is deliberately absent: it is read from the
/// Task's own declared inputs rather than accepted here, so a Workspace can
/// only ever bind to the repository its Task declares. `source` is the
/// controller-trusted local path that reference resolves to, which the
/// composition root supplies because no canonical contract yet defines a
/// repository registry.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceRequest<'a> {
    pub source: &'a Path,
    pub requested_base: &'a RequestedBase,
}

/// The durable command identity a Workspace operation runs under.
///
/// One base identity per caller request. The controller derives a distinct
/// command id per durable transition, so a retried request replays each
/// transition it already committed instead of executing it twice.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceCommand<'a> {
    pub epoch: &'a str,
    pub id: &'a str,
    pub request_hash: &'a [u8; 32],
}

/// A failure along the Workspace control path.
#[derive(Debug)]
pub enum WorkspaceError {
    Store(StoreError),
    /// The Task does not exist, is not in a phase that may own a Workspace,
    /// or does not declare a repository to work in.
    Ineligible {
        task_id: String,
        reason: String,
    },
    /// The Task's durable specification could not be read or trusted.
    TaskSpecUnusable {
        task_id: String,
        detail: String,
    },
    /// Resolution or materialization failed. Durable state records the
    /// failure, and external state is whatever was observed — never assumed
    /// absent.
    Materialization {
        workspace_id: String,
        error: MaterializerError,
    },
    /// The Task already owns a Ready Workspace whose external state cannot be
    /// found.
    ///
    /// Fails closed rather than rebuilding: a Workspace that has been mutable
    /// may have held unsealed work, and silently recreating it would destroy
    /// the evidence that it is gone. Recovery policy decides what happens
    /// next.
    Missing {
        workspace_id: String,
        observed: Materialization,
    },
    /// The request does not match the Workspace the Task already owns.
    Conflict {
        workspace_id: String,
        detail: String,
    },
    /// A derived command identity would exceed what the durable ledger
    /// accepts.
    CommandIdentityTooLong {
        id: String,
    },
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(err) => write!(f, "workspace store failure: {err}"),
            Self::Ineligible { task_id, reason } => {
                write!(f, "task {task_id} may not own a workspace: {reason}")
            }
            Self::TaskSpecUnusable { task_id, detail } => {
                write!(f, "task {task_id} specification is unusable: {detail}")
            }
            Self::Materialization {
                workspace_id,
                error,
            } => write!(
                f,
                "workspace {workspace_id} materialization failed: {error}"
            ),
            Self::Missing {
                workspace_id,
                observed,
            } => write!(
                f,
                "workspace {workspace_id} is Ready but its materialization is {observed}"
            ),
            Self::Conflict {
                workspace_id,
                detail,
            } => write!(
                f,
                "workspace {workspace_id} conflicts with the request: {detail}"
            ),
            Self::CommandIdentityTooLong { id } => {
                write!(f, "derived command identity {id:?} is too long")
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<StoreError> for WorkspaceError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

/// The longest command identity the durable command ledger accepts.
pub(crate) const MAX_COMMAND_ID: usize = 128;

/// Drives a Task's Workspace toward Ready against durable authority.
pub struct WorkspaceController<'a> {
    store: &'a Store,
    materializer: &'a dyn RepositoryMaterializer,
    /// The controller-owned root every Workspace path is derived beneath.
    root: PathBuf,
}

impl<'a> WorkspaceController<'a> {
    #[must_use]
    pub fn new(
        store: &'a Store,
        materializer: &'a dyn RepositoryMaterializer,
        root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            store,
            materializer,
            root: root.into(),
        }
    }

    /// The deterministic desired path for a Workspace's repository state.
    ///
    /// A pure function of the controller root and the Workspace identity, so
    /// a process that has only read durable state can compute where the
    /// Workspace should be without discovering it.
    #[must_use]
    pub fn path_of(&self, workspace_id: &str) -> PathBuf {
        self.root.join(workspace_id).join("repo")
    }

    /// Brings the Task's one current Workspace to Ready, creating it if the
    /// Task does not own one yet.
    ///
    /// Idempotent by construction: it reads durable state first and performs
    /// only the transitions that state says are still outstanding. Calling it
    /// again after a daemon restart reopens the same Workspace bound to the
    /// same base rather than creating a second one.
    ///
    /// Idempotent is not the same as mutually exclusive, and the difference
    /// matters to whoever supervises this. Command replay proves an identity
    /// already committed, not that no other caller is running now: two live
    /// callers can both observe `Committed::Replayed` and reach the external
    /// discard/materialize steps against the same destination concurrently.
    /// The blast radius is bounded — external state at that point is
    /// never-`Ready` controller-owned scratch, rebuilding it is deterministic,
    /// and whichever caller loses the revisioned CAS surfaces
    /// [`StoreError::RevisionConflict`] rather than committing — but a
    /// controller that needs one materialization at a time must serialize its
    /// own dispatch. Nothing wires a production caller yet.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError`] as documented on each variant. On every failure the
    /// Task still owns at most one Workspace, and any Workspace that was
    /// already Ready is untouched.
    pub fn ensure(
        &self,
        command: &WorkspaceCommand<'_>,
        workspace_id: &str,
        task_id: &str,
        request: &WorkspaceRequest<'_>,
    ) -> Result<WorkspaceRecord, WorkspaceError> {
        let existing = self.store.workspace_for_task(task_id)?;
        let current = match existing {
            Some(record) => {
                self.check_matches(&record, request)?;
                record
            }
            None => self.open(command, workspace_id, task_id, request)?,
        };

        match current.phase {
            // Already usable, provided its external state is actually there.
            WorkspacePhase::Ready => self.reopen(&current),
            // Never handed to an execution owner, so whatever is on disk is
            // controller-owned scratch that may be rebuilt at the same
            // identity and base.
            WorkspacePhase::Requested | WorkspacePhase::Materializing | WorkspacePhase::Error => {
                self.materialize(command, &current)
            }
            // Frozen, Releasing and Released are lifecycle states no mission
            // yet drives; refusing them is honest, and reaching one would
            // mean a controller this mission does not implement moved the
            // Workspace.
            phase => Err(WorkspaceError::Conflict {
                workspace_id: current.id.clone(),
                detail: format!("workspace is {phase} and cannot be brought to Ready here"),
            }),
        }
    }

    /// Resolves the base and commits durable identity, before any effect.
    fn open(
        &self,
        command: &WorkspaceCommand<'_>,
        workspace_id: &str,
        task_id: &str,
        request: &WorkspaceRequest<'_>,
    ) -> Result<WorkspaceRecord, WorkspaceError> {
        let repository = self.repository_of(task_id)?;

        // Read-only against the source repository, and performed before any
        // durable Workspace row exists: a Workspace is never created bound to
        // a base nobody could resolve.
        let resolved = self
            .materializer
            .resolve_base(request.source, request.requested_base)
            .map_err(|error| WorkspaceError::Materialization {
                workspace_id: workspace_id.to_string(),
                error,
            })?;

        let source = path_text(request.source).ok_or_else(|| WorkspaceError::Ineligible {
            task_id: task_id.to_string(),
            reason: "the source repository path is not valid UTF-8".to_string(),
        })?;

        let id = self.derive(command.id, "open", None)?;
        let committed = self.store.open_workspace(
            &Command {
                epoch: command.epoch,
                id: &id,
                request_hash: command.request_hash,
                event_type: "workspace.requested",
            },
            workspace_id,
            &WorkspaceBinding {
                task_id,
                repository: &repository,
                source_path: &source,
                requested_base: request.requested_base,
                resolved_base: &resolved,
            },
        );

        self.settle(committed, task_id)
    }

    /// Runs the materialization half: durable marker, external effect,
    /// durable verification.
    fn materialize(
        &self,
        command: &WorkspaceCommand<'_>,
        current: &WorkspaceRecord,
    ) -> Result<WorkspaceRecord, WorkspaceError> {
        let destination = self.path_of(&current.id);
        let source = PathBuf::from(&current.source_path);
        let target = MaterializationTarget {
            workspace_id: &current.id,
            source: &source,
            destination: &destination,
            base: &current.resolved_base,
        };

        // The marker commits before the first effect. After this, durable
        // state no longer claims that nothing exists at the Workspace path.
        let id = self.derive(command.id, "materializing", Some(current.revision.get()))?;
        let marked = self.settle(
            self.store.begin_workspace_materialization(
                &Command {
                    epoch: command.epoch,
                    id: &id,
                    request_hash: command.request_hash,
                    event_type: "workspace.materializing",
                },
                &current.id,
                current.revision,
            ),
            &current.task_id,
        )?;

        // A retry may be resuming after a crash that left partial state.
        // Durable state has already established this Workspace was never
        // mutable to a worker, so discarding is safe and is what makes the
        // retry deterministic rather than dependent on what survived.
        if let Err(error) = self.materializer.discard(&target) {
            return Err(self.record_failure(command, &marked, &target, error));
        }

        let verified = match self.materializer.materialize(&target) {
            Ok(verified) => verified,
            Err(error) => return Err(self.record_failure(command, &marked, &target, error)),
        };

        let id = self.derive(command.id, "ready", Some(marked.revision.get()))?;
        self.settle(
            self.store.complete_workspace_materialization(
                &Command {
                    epoch: command.epoch,
                    id: &id,
                    request_hash: command.request_hash,
                    event_type: "workspace.ready",
                },
                &marked.id,
                marked.revision,
                &verified,
            ),
            &marked.task_id,
        )
    }

    /// Confirms a Ready Workspace's external state still exists.
    fn reopen(&self, current: &WorkspaceRecord) -> Result<WorkspaceRecord, WorkspaceError> {
        let destination = self.path_of(&current.id);
        let source = PathBuf::from(&current.source_path);
        let observed = self
            .materializer
            .observe(&MaterializationTarget {
                workspace_id: &current.id,
                source: &source,
                destination: &destination,
                base: &current.resolved_base,
            })
            .map_err(|error| WorkspaceError::Materialization {
                workspace_id: current.id.clone(),
                error,
            })?;

        if observed == Materialization::Present {
            Ok(current.clone())
        } else {
            // Canonical recovery: mark it and stop. Do not silently recreate
            // a Workspace that may have held unsealed work.
            Err(WorkspaceError::Missing {
                workspace_id: current.id.clone(),
                observed,
            })
        }
    }

    /// Records a materialization failure durably, preserving what is actually
    /// known about external state.
    fn record_failure(
        &self,
        command: &WorkspaceCommand<'_>,
        current: &WorkspaceRecord,
        target: &MaterializationTarget<'_>,
        error: MaterializerError,
    ) -> WorkspaceError {
        // An error is not evidence of absence. Ask, and fall back to Unknown
        // when the observation itself cannot be made.
        let observed = self
            .materializer
            .observe(target)
            .unwrap_or(Materialization::Unknown);
        let observed = match observed {
            // The materializer may believe its own output is complete; this
            // path is a failure, so nothing here may claim verification.
            Materialization::Present | Materialization::Unknown => Materialization::Unknown,
            Materialization::Absent => Materialization::Absent,
        };

        let id = match self.derive(command.id, "failed", Some(current.revision.get())) {
            Ok(id) => id,
            Err(err) => return err,
        };
        if let Err(store_error) = self.store.fail_workspace_materialization(
            &Command {
                epoch: command.epoch,
                id: &id,
                request_hash: command.request_hash,
                event_type: "workspace.failed",
            },
            &current.id,
            current.revision,
            observed,
        ) {
            return WorkspaceError::Store(store_error);
        }

        WorkspaceError::Materialization {
            workspace_id: current.id.clone(),
            error,
        }
    }

    /// The repository reference the Task declares, read from its durable
    /// immutable specification.
    ///
    /// Taken from the Task rather than from the caller so a Workspace can
    /// only bind to the repository its Task actually names.
    fn repository_of(&self, task_id: &str) -> Result<String, WorkspaceError> {
        let task = self
            .store
            .task(task_id)?
            .ok_or_else(|| WorkspaceError::Ineligible {
                task_id: task_id.to_string(),
                reason: "no such task".to_string(),
            })?;
        if task.phase != TaskPhase::Ready {
            return Err(WorkspaceError::Ineligible {
                task_id: task_id.to_string(),
                reason: format!("task is {}", task.phase.as_str()),
            });
        }

        let canonical = self
            .store
            .task_spec_json(task.spec_digest)?
            .ok_or_else(|| WorkspaceError::TaskSpecUnusable {
                task_id: task_id.to_string(),
                detail: "the specification the task names is not stored".to_string(),
            })?;
        let spec =
            TaskSpec::from_canonical_json(&canonical).map_err(|TaskDecodeError(detail)| {
                WorkspaceError::TaskSpecUnusable {
                    task_id: task_id.to_string(),
                    detail,
                }
            })?;
        // The same content fence the routing path applies: a stored
        // specification is trusted only if it still hashes to the digest the
        // Task row names.
        if spec.digest() != task.spec_digest {
            return Err(WorkspaceError::TaskSpecUnusable {
                task_id: task_id.to_string(),
                detail: "the stored specification does not match its digest".to_string(),
            });
        }

        spec.inputs
            .iter()
            .find(|input| input.name == REPOSITORY_INPUT)
            .map(|input| input.reference.clone())
            .ok_or_else(|| WorkspaceError::Ineligible {
                task_id: task_id.to_string(),
                reason: format!("it declares no {REPOSITORY_INPUT} input to work in"),
            })
    }

    /// Refuses a request that does not describe the Workspace the Task
    /// already owns.
    fn check_matches(
        &self,
        current: &WorkspaceRecord,
        request: &WorkspaceRequest<'_>,
    ) -> Result<(), WorkspaceError> {
        if current.requested_base.as_str() != request.requested_base.as_str() {
            return Err(WorkspaceError::Conflict {
                workspace_id: current.id.clone(),
                detail: format!(
                    "it was opened for {} and the request asks for {}",
                    current.requested_base, request.requested_base
                ),
            });
        }
        match path_text(request.source) {
            Some(source) if source == current.source_path => Ok(()),
            Some(source) => Err(WorkspaceError::Conflict {
                workspace_id: current.id.clone(),
                detail: format!(
                    "it was opened against {} and the request names {source}",
                    current.source_path
                ),
            }),
            None => Err(WorkspaceError::Conflict {
                workspace_id: current.id.clone(),
                detail: "the source repository path is not valid UTF-8".to_string(),
            }),
        }
    }

    /// Turns a committed transition into the row it produced.
    ///
    /// A replay carries no value, by design: the durable ledger records where
    /// a command's Event landed, not what some earlier process returned. So a
    /// replay re-reads current durable state, which is the answer that was
    /// wanted anyway.
    fn settle(
        &self,
        committed: Result<Committed<WorkspaceRecord>, StoreError>,
        task_id: &str,
    ) -> Result<WorkspaceRecord, WorkspaceError> {
        match committed? {
            Committed::Executed { value, .. } => Ok(value),
            Committed::Replayed { .. } => {
                self.store
                    .workspace_for_task(task_id)?
                    .ok_or_else(|| WorkspaceError::Ineligible {
                        task_id: task_id.to_string(),
                        reason: "a replayed workspace command left no current workspace"
                            .to_string(),
                    })
            }
        }
    }

    /// Derives one transition's durable command identity from the caller's.
    ///
    /// `from` is the revision the transition CASes against, and it is part of
    /// the identity on purpose. A state-dependent transition retried from the
    /// *same* revision is the same command and must replay; the same
    /// transition attempted again from a *different* revision — a retry after
    /// a recorded failure, say — is a different command and must execute.
    /// Without the revision, the second attempt would replay the first and
    /// silently skip the transition it was asked to make.
    fn derive(
        &self,
        base: &str,
        suffix: &str,
        from: Option<i64>,
    ) -> Result<String, WorkspaceError> {
        let id = match from {
            Some(revision) => format!("{base}:workspace:{suffix}:{revision}"),
            None => format!("{base}:workspace:{suffix}"),
        };
        if id.len() > MAX_COMMAND_ID {
            return Err(WorkspaceError::CommandIdentityTooLong { id });
        }
        Ok(id)
    }
}

/// A path as durable text, or `None` when it cannot be represented.
///
/// Durable Workspace state is text in a STRICT column, so a path that is not
/// UTF-8 cannot be recorded and must be refused at the boundary rather than
/// lossily converted into a different path.
fn path_text(path: &Path) -> Option<String> {
    path.to_str().map(str::to_string)
}

#[cfg(test)]
mod tests;
