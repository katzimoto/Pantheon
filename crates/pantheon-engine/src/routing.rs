//! Pre-Run Agent and Execution Fabric orchestration.
//!
//! The engine owns the abstract backend port and composes one captured
//! ConfigurationRevision with the durable Ready Task, pure core resolution and
//! side-effect-free backend offers. It never creates execution authority.

use pantheon_core::execution::{
    AgentResolutionError, BackendDescriptor, CandidateRejection, ConfigurationBinding,
    ControllerSafetyFacts, ExecutionOffer, ExecutionRequest, RequestBuildError, RoutingResult,
    SelectionError, build_execution_request, resolve_agents, select_execution_candidate,
    validate_execution_candidate,
};
use pantheon_core::planning::{TaskDecodeError, TaskPhase, TaskSpec};
use pantheon_store::{Store, StoreError, TaskRecord};

use crate::configuration::{ConfigurationAuthority, ConfigurationError, Snapshot};

/// The narrow pre-Run port an ExecutorBackend must implement.
pub trait ExecutorBackend {
    /// Publishes revisioned factual capabilities. This operation does not
    /// choose an Agent.
    fn describe(&self) -> BackendDescriptor;

    /// Produces zero or more side-effect-free offers for one request.
    fn offer(&self, request: &ExecutionRequest) -> Result<Vec<ExecutionOffer>, BackendError>;
}

/// Composition-owned backend port and safety evidence.
///
/// Backend descriptors/offers may report compatibility facts, but the
/// controller-owned safety evidence is supplied here by the composition root,
/// outside the backend trait. This keeps a backend from self-awarding physical
/// isolation or duplicate-prevention authority.
pub struct ExecutorBackendPort<'a> {
    pub backend: &'a dyn ExecutorBackend,
    pub safety: ControllerSafetyFacts,
}

impl<'a> ExecutorBackendPort<'a> {
    #[must_use]
    pub const fn new(backend: &'a dyn ExecutorBackend, safety: ControllerSafetyFacts) -> Self {
        Self { backend, safety }
    }
}

/// A backend-side failure while describing or offering, without exposing a
/// concrete backend implementation to core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub detail: String,
}

/// A diagnostic for an offer rejected before route selection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OfferRejection {
    pub agent: pantheon_core::execution::LogicalAgentVersion,
    pub backend_id: String,
    pub reason: CandidateRejection,
}

/// Failure along the non-authoritative routing path.
#[derive(Debug)]
pub enum RoutingError {
    Store(StoreError),
    Configuration(ConfigurationError),
    TaskNotFound {
        task_id: String,
    },
    TaskNotReady {
        task_id: String,
        phase: TaskPhase,
        active_run_id: Option<String>,
    },
    TaskSpecUnavailable {
        task_id: String,
    },
    InvalidTaskSpec {
        task_id: String,
        detail: String,
    },
    TaskSpecDigestMismatch {
        task_id: String,
    },
    AgentResolution(AgentResolutionError),
    RequestBuild(RequestBuildError),
    UnregisteredBackend {
        backend_id: String,
    },
    DuplicateBackendDescriptor {
        backend_id: String,
    },
    BackendOffer {
        backend_id: String,
        error: BackendError,
    },
    NoCompatibleOffers {
        task_id: String,
        rejections: Vec<OfferRejection>,
    },
    Selection(SelectionError),
    StaleConfiguration,
}

impl std::fmt::Display for RoutingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(f, "routing store failure: {error}"),
            Self::Configuration(error) => write!(f, "routing configuration failure: {error}"),
            Self::TaskNotFound { task_id } => write!(f, "Task {task_id:?} does not exist"),
            Self::TaskNotReady {
                task_id,
                phase,
                active_run_id,
            } => write!(
                f,
                "Task {task_id:?} is not Ready (phase {}, active run {:?})",
                phase.as_str(),
                active_run_id
            ),
            Self::TaskSpecUnavailable { task_id } => {
                write!(f, "Task {task_id:?} has no stored specification")
            }
            Self::InvalidTaskSpec { task_id, detail } => {
                write!(f, "Task {task_id:?} specification is invalid: {detail}")
            }
            Self::TaskSpecDigestMismatch { task_id } => write!(
                f,
                "Task {task_id:?} specification bytes do not match its durable digest"
            ),
            Self::AgentResolution(error) => write!(f, "Agent resolution failed: {error:?}"),
            Self::RequestBuild(error) => {
                write!(f, "ExecutionRequest construction failed: {error:?}")
            }
            Self::UnregisteredBackend { backend_id } => {
                write!(
                    f,
                    "backend {backend_id:?} is not registered in the captured configuration"
                )
            }
            Self::DuplicateBackendDescriptor { backend_id } => {
                write!(f, "more than one backend descriptor uses id {backend_id:?}")
            }
            Self::BackendOffer { backend_id, error } => {
                write!(
                    f,
                    "backend {backend_id:?} could not produce offers: {error:?}"
                )
            }
            Self::NoCompatibleOffers { task_id, .. } => {
                write!(f, "Task {task_id:?} has no compatible ExecutionOffer")
            }
            Self::Selection(error) => write!(f, "candidate selection failed: {error:?}"),
            Self::StaleConfiguration => {
                f.write_str("the captured ConfigurationRevision changed during routing")
            }
        }
    }
}

impl std::error::Error for RoutingError {}

impl From<StoreError> for RoutingError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ConfigurationError> for RoutingError {
    fn from(error: ConfigurationError) -> Self {
        Self::Configuration(error)
    }
}

impl From<AgentResolutionError> for RoutingError {
    fn from(error: AgentResolutionError) -> Self {
        Self::AgentResolution(error)
    }
}

impl From<RequestBuildError> for RoutingError {
    fn from(error: RequestBuildError) -> Self {
        Self::RequestBuild(error)
    }
}

impl From<SelectionError> for RoutingError {
    fn from(error: SelectionError) -> Self {
        Self::Selection(error)
    }
}

/// Orchestrates one recomputable route attempt against current authority.
#[derive(Debug)]
pub struct RoutingController<'store, 'authority> {
    store: &'store Store,
    configuration: &'authority ConfigurationAuthority<'store>,
}

impl<'store, 'authority> RoutingController<'store, 'authority> {
    #[must_use]
    pub const fn new(
        store: &'store Store,
        configuration: &'authority ConfigurationAuthority<'store>,
    ) -> Self {
        Self {
            store,
            configuration,
        }
    }

    /// Routes one currently Ready Task to a compatible Agent+Offer pair.
    ///
    /// The method performs reads and backend offer calls only. It does not write
    /// the store, reserve resources, prepare isolation or contact an executor
    /// for launch.
    pub fn route_ready_task(
        &self,
        task_id: &str,
        backends: &[ExecutorBackendPort<'_>],
    ) -> Result<RoutingResult, RoutingError> {
        let snapshot = self.configuration.snapshot()?;
        let binding = binding_from_snapshot(&snapshot);
        let compiled = snapshot.compiled().ok_or_else(|| {
            RoutingError::Configuration(ConfigurationError::Unavailable(
                "the captured configuration has no usable compiled semantics".to_string(),
            ))
        })?;

        let task = self
            .store
            .task(task_id)?
            .ok_or_else(|| RoutingError::TaskNotFound {
                task_id: task_id.to_string(),
            })?;
        ensure_ready(&task)?;
        let task = read_task_spec(self.store, &task)?;

        let resolution = resolve_agents(&task.spec, compiled, binding)?;
        let mut requests = Vec::with_capacity(resolution.eligible.len());
        for agent in &resolution.eligible {
            requests.push((
                agent.clone(),
                build_execution_request(task_id, &task.spec, agent, compiled, binding)?,
            ));
        }

        let mut descriptors = Vec::with_capacity(backends.len());
        let mut descriptor_ids = Vec::with_capacity(backends.len());
        for backend in backends {
            let descriptor = backend.backend.describe();
            if descriptor_ids.contains(&descriptor.backend_id) {
                return Err(RoutingError::DuplicateBackendDescriptor {
                    backend_id: descriptor.backend_id,
                });
            }
            descriptor_ids.push(descriptor.backend_id.clone());
            let registration = compiled
                .execution()
                .backends
                .iter()
                .find(|registration| registration.backend_id == descriptor.backend_id)
                .ok_or_else(|| RoutingError::UnregisteredBackend {
                    backend_id: descriptor.backend_id.clone(),
                })?;
            descriptors.push(BackendView {
                backend: backend.backend,
                descriptor,
                enabled: registration.enabled,
                safety: backend.safety.clone(),
            });
        }
        descriptors.sort_by(|left, right| {
            (&left.descriptor.backend_id, left.descriptor.revision)
                .cmp(&(&right.descriptor.backend_id, right.descriptor.revision))
        });

        let mut candidates = Vec::new();
        let mut rejections = Vec::new();
        for (agent, request) in &requests {
            for backend in &descriptors {
                if !backend.enabled || !backend.descriptor.available_for_offers {
                    continue;
                }
                let offers =
                    backend
                        .backend
                        .offer(request)
                        .map_err(|error| RoutingError::BackendOffer {
                            backend_id: backend.descriptor.backend_id.clone(),
                            error,
                        })?;
                for offer in offers {
                    match validate_execution_candidate(
                        request,
                        agent,
                        &backend.descriptor,
                        &offer,
                        backend.enabled,
                        &backend.safety,
                    ) {
                        Ok(candidate) => candidates.push(candidate),
                        Err(reason) => rejections.push(OfferRejection {
                            agent: agent.identity.clone(),
                            backend_id: backend.descriptor.backend_id.clone(),
                            reason,
                        }),
                    }
                }
            }
        }

        let candidate = select_execution_candidate(&candidates).map_err(|_| {
            rejections.sort();
            RoutingError::NoCompatibleOffers {
                task_id: task_id.to_string(),
                rejections,
            }
        })?;
        let request = requests
            .iter()
            .find(|(_, request)| request.digest() == candidate.request_digest)
            .map(|(_, request)| request.clone())
            .ok_or(RoutingError::Selection(SelectionError::NoCandidates))?;

        let current = self.configuration.snapshot()?;
        if !binding.matches(binding_from_snapshot(&current)) {
            return Err(RoutingError::StaleConfiguration);
        }

        Ok(RoutingResult {
            task_id: task_id.to_string(),
            task_revision: task.revision,
            task_spec_digest: task.spec.digest(),
            configuration: binding,
            request,
            candidate,
        })
    }
}

struct BackendView<'a> {
    backend: &'a dyn ExecutorBackend,
    descriptor: BackendDescriptor,
    enabled: bool,
    safety: ControllerSafetyFacts,
}

struct LoadedTask {
    spec: TaskSpec,
    revision: i64,
}

fn ensure_ready(task: &TaskRecord) -> Result<(), RoutingError> {
    if task.phase != TaskPhase::Ready || task.active_run_id.is_some() {
        return Err(RoutingError::TaskNotReady {
            task_id: task.id.clone(),
            phase: task.phase,
            active_run_id: task.active_run_id.clone(),
        });
    }
    Ok(())
}

fn read_task_spec(store: &Store, task: &TaskRecord) -> Result<LoadedTask, RoutingError> {
    let canonical = store.task_spec_json(task.spec_digest)?.ok_or_else(|| {
        RoutingError::TaskSpecUnavailable {
            task_id: task.id.clone(),
        }
    })?;
    let spec = TaskSpec::from_canonical_json(&canonical).map_err(|error| match error {
        TaskDecodeError(detail) => RoutingError::InvalidTaskSpec {
            task_id: task.id.clone(),
            detail,
        },
    })?;
    if spec.digest() != task.spec_digest {
        return Err(RoutingError::TaskSpecDigestMismatch {
            task_id: task.id.clone(),
        });
    }
    if spec.goal_id != task.goal_id {
        return Err(RoutingError::InvalidTaskSpec {
            task_id: task.id.clone(),
            detail: "Task specification names a different Goal".to_string(),
        });
    }
    Ok(LoadedTask {
        spec,
        revision: task.revision.get(),
    })
}

fn binding_from_snapshot(snapshot: &Snapshot) -> ConfigurationBinding {
    ConfigurationBinding::new(
        snapshot.active().activation_sequence,
        snapshot.active().content_digest,
        snapshot.active().components,
    )
}

#[cfg(test)]
mod tests;
