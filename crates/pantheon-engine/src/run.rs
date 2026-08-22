//! The Run Controller: committed-Run preparation, Attempt lifecycle, the
//! durable launch boundary and same-lineage reconciliation.
//!
//! `docs/architecture/execution/run-and-attempt.md`,
//! `docs/architecture/scheduling/scheduler-dispatch-and-run-intent-
//! reconciliation.md` ("Run Controller preparation", "Attempt creation",
//! "Pre-launch contact marker") and
//! `docs/architecture/persistence-and-recovery/recovery-policy.md` are
//! canonical for what this module drives. The Scheduler ends at T3; from
//! there *this* controller owns everything:
//!
//! ```text
//! committed Run (T3) + frozen source snapshot
//!   ↓ preparation gates: WorkspaceReady → SandboxReady → ContextReady → PolicyReady
//! LaunchReady
//!   ↓ T4  Attempt + ordinal + LaunchKey + AgentControlSession (one transaction)
//!   ↓ T4a optional pre-contact rekey when the bearer was lost with process memory
//!   ↓ T4b CONTACT_MAY_HAVE_OCCURRED (committed before the backend call)
//!   ↓ ensureExecution(LaunchKey)
//!   ↓ inspect / reconcile the SAME lineage forever after
//! ```
//!
//! # What this controller never does
//!
//! It never mutates a Run's Binding, source snapshot or attached plan; it
//! never launches from the Scheduler; it never creates a replacement
//! Attempt while `UNKNOWN`; it never rekeys a contacted session; and it
//! never persists or logs raw credential material. The bearer lives only in
//! process-local transient launch state ([`RunController`]'s own memory) and
//! in the launch package handed to the launcher port.
//!
//! Recovery is ordinary reconciliation: restart reconstructs the same flow
//! over durable state. There is no startup-only repair path.

use std::collections::HashMap;
use std::fmt;

use pantheon_core::attempt::{LaunchContactState, Observation};
use pantheon_core::config::canonical::Value;
use pantheon_core::config::model::{IsolationClass, NetworkMode, SandboxProfile};
use pantheon_core::config::{Digest, parse};
use pantheon_core::execution::LaunchSemantics;
use pantheon_store::{Command, Committed, ObservationUpdate, Revision, Store, StoreError};

/// How much of a digest an identifier carries.
const IDENTIFIER_HEX: usize = 16;

// ---------------------------------------------------------------------------
// Entropy boundary
// ---------------------------------------------------------------------------

/// The injectable production entropy source.
///
/// LaunchKeys and Agent Control bearers must be unpredictable and
/// rewind-resistant; tests substitute deterministic sources so crash-window
/// evidence never depends on timing or luck.
pub trait RandomBytes: fmt::Debug {
    /// Fills `dest` with cryptographically strong random bytes.
    ///
    /// # Errors
    ///
    /// Fails closed rather than returning weak material.
    fn fill(&self, dest: &mut [u8]) -> Result<(), RandomFailure>;
}

/// A failure drawing entropy. Never carries partial material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandomFailure {
    pub detail: String,
}

impl fmt::Display for RandomFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

/// The operating-system kernel CSPRNG (`/dev/urandom`).
///
/// Production-grade on both platforms Pantheon supports (Linux and macOS):
/// post-bootstrap `/dev/urandom` is the kernel cryptographic RNG, the same
/// generator `getrandom(2)` serves. Reading it through `std::fs` avoids a
/// third-party dependency for one syscall-shaped read, per the repository's
/// dependency policy; short reads fail closed instead of weakening output.
#[derive(Debug)]
pub struct OsRandom;

impl RandomBytes for OsRandom {
    fn fill(&self, dest: &mut [u8]) -> Result<(), RandomFailure> {
        use std::io::Read;
        let mut file = std::fs::File::open("/dev/urandom").map_err(|err| RandomFailure {
            detail: format!("could not open the OS entropy source: {err}"),
        })?;
        file.read_exact(dest).map_err(|err| RandomFailure {
            detail: format!("the OS entropy source returned unusable output: {err}"),
        })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Credential material
// ---------------------------------------------------------------------------

/// One high-entropy opaque Agent Control bearer.
///
/// 256 bits of entropy rendered as 64 hex characters. There is no
/// [`fmt::Display`] impl at all, and the manual [`fmt::Debug`] redacts: a
/// bearer printed through any log or diagnostic path is a leaked credential,
/// so the type cannot be accidentally stringified. Delivery happens only
/// through [`Bearer::expose`] into the launch package.
pub struct Bearer(String);

impl Bearer {
    /// Draws a fresh bearer from `random`.
    ///
    /// # Errors
    ///
    /// [`RandomFailure`] when entropy fails; nothing weak is produced.
    pub fn generate(random: &dyn RandomBytes) -> Result<Self, RandomFailure> {
        let mut bytes = [0u8; 32];
        random.fill(&mut bytes)?;
        Ok(Self(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    /// The raw material, for delivery to the exact execution path only.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The persisted one-way verifier of this bearer.
    #[must_use]
    pub fn verifier(&self) -> [u8; 32] {
        *Digest::of(self.0.as_bytes()).as_bytes()
    }
}

impl fmt::Debug for Bearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Bearer([REDACTED])")
    }
}

// ---------------------------------------------------------------------------
// Backend ports (provider-neutral)
// ---------------------------------------------------------------------------

/// Why a launcher operation failed. After the contact marker any failure is
/// conservatively ambiguous, so no ambiguity flag exists to misuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherFailure {
    pub detail: String,
}

impl fmt::Display for LauncherFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

/// The exact launch package for one Attempt lineage.
///
/// Provider-neutral by contract: backend-private interpretation happens
/// behind the port, and the credential travels as an opaque string the model
/// never sees.
pub struct LaunchPackage<'a> {
    pub launch_key: &'a str,
    pub run_id: &'a str,
    pub attempt_id: &'a str,
    pub credential: &'a Bearer,
    pub context_plan_digest: &'a Digest,
}

/// The provider-neutral external-execution boundary.
///
/// Implementations advertise factual keyed-idempotency; the controller
/// refuses to cross this port for a backend that cannot address one logical
/// lineage per LaunchKey, because every recovery decision here assumes it.
pub trait ExecutionLauncher: fmt::Debug {
    fn backend_id(&self) -> &str;

    fn launch_semantics(&self) -> LaunchSemantics;

    /// First contact for this lineage. Repeating the call with the same
    /// LaunchKey must address exactly one logical external execution.
    ///
    /// # Errors
    ///
    /// [`LauncherFailure`] when the call could not be completed. Whether the
    /// backend received anything is unknowable from here.
    fn ensure_execution(&self, package: &LaunchPackage<'_>)
    -> Result<Observation, LauncherFailure>;

    /// Inspects an existing lineage by its immutable key alone.
    ///
    /// # Errors
    ///
    /// [`LauncherFailure`] when existence cannot be established; callers
    /// treat this as [`Observation::Unknown`], never as absence.
    fn inspect_execution(&self, launch_key: &str) -> Result<Observation, LauncherFailure>;
}

/// One Sandbox readiness check for a Run under its frozen strategy.
#[derive(Debug, Clone, Copy)]
pub struct SandboxCheck<'a> {
    pub run_id: &'a str,
    pub sandbox_profile_digest: &'a Digest,
}

/// The MVP Sandbox gate.
///
/// **Test/fake infrastructure only.** The strict production container
/// SandboxBackend (#34) owns real isolation claims; nothing here may be
/// represented as production isolation or execution readiness. The port
/// exists so lifecycle semantics are exercisable before that mission lands.
pub trait SandboxReadiness: fmt::Debug {
    /// Verifies factual readiness for one Run under its frozen profile.
    ///
    /// # Errors
    ///
    /// A descriptive failure means the gate stays shut; the Run never
    /// reaches LaunchReady through a half-open Sandbox.
    fn verify_ready(&self, check: SandboxCheck<'_>) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Deterministic minimum recovery policy
// ---------------------------------------------------------------------------

/// The deterministic minimum Recovery Policy this mission implements.
///
/// `RECONCILE` is unconditional; `RETRY_ATTEMPT` requires a definitively
/// ended prior lineage plus an unchanged Run and permits at most
/// [`MinRecoveryPolicy::max_attempts_per_run`] Attempts per Run. Anything
/// richer belongs to later missions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinRecoveryPolicy {
    pub max_attempts_per_run: u32,
}

impl Default for MinRecoveryPolicy {
    fn default() -> Self {
        Self {
            max_attempts_per_run: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// One Run's reconciliation result from [`RunController::reconcile_all`].
pub type InventoryResult = (String, Result<RunOutcome, RunControllerError>);

/// What one reconciliation pass decided. Every variant is a normal outcome;
/// fencing and waiting are the controller working, not failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// No such Run.
    Unknown,
    /// The Run is not Active (Finalizing or terminal); nothing to launch.
    Inactive { phase: String },
    /// Preparation failed and the Run concluded with zero Attempts.
    ConcludedInPreparation { gate: String },
    /// T4 established the lineage durably; contact comes on a later pass.
    AttemptEstablished {
        attempt_id: String,
        launch_key: String,
    },
    /// T4b crossed the contact boundary and the backend was ensured.
    Launched {
        attempt_id: String,
        observation: Observation,
    },
    /// Same-lineage inspection reconciled a live observation.
    Reconciled {
        attempt_id: String,
        observation: Observation,
    },
    /// Existence could not be established; fenced, no replacement created.
    UnknownFenced { attempt_id: String },
    /// The lineage definitively ended; policy armed a same-Run retry.
    RetryArmed {
        attempt_id: String,
        next_ordinal: i64,
    },
    /// The lineage definitively ended; retries exhausted, Run concluded.
    ConcludedAfterFailure { attempt_id: String },
}

/// A failure along the control path.
#[derive(Debug)]
pub enum RunControllerError {
    Store(StoreError),
    Entropy(RandomFailure),
}

impl fmt::Display for RunControllerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(err) => write!(f, "run controller store failure: {err}"),
            Self::Entropy(err) => write!(f, "run controller entropy failure: {err}"),
        }
    }
}

impl std::error::Error for RunControllerError {}

impl From<StoreError> for RunControllerError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

impl From<RandomFailure> for RunControllerError {
    fn from(err: RandomFailure) -> Self {
        Self::Entropy(err)
    }
}

/// Everything one reconciliation pass may reach besides durable state.
#[derive(Debug)]
pub struct ReconciliationDeps<'a> {
    pub launcher: &'a dyn ExecutionLauncher,
    pub sandbox: &'a dyn SandboxReadiness,
    pub policy: &'a MinRecoveryPolicy,
}

/// Transient per-Attempt launch state: the raw bearer and the credential
/// revision it corresponds to. Deliberately process-local; losing it is what
/// makes T4a exist.
#[derive(Debug)]
struct BearerState {
    bearer: Bearer,
    credential_revision: i64,
}

/// Owns committed-Run preparation and Attempt lifecycle for the daemon.
///
/// Generic over the entropy source; production composes [`OsRandom`], tests
/// compose deterministic ones.
#[derive(Debug)]
pub struct RunController<'store, R: RandomBytes> {
    store: &'store Store,
    random: R,
    incarnation: String,
    bearers: HashMap<String, BearerState>,
}

impl<'store, R: RandomBytes> RunController<'store, R> {
    #[must_use]
    pub fn new(store: &'store Store, random: R, incarnation: impl Into<String>) -> Self {
        Self {
            store,
            random,
            incarnation: incarnation.into(),
            bearers: HashMap::new(),
        }
    }

    /// Inventories nonterminal Runs and reconciles each through the ordinary
    /// path. Restart calls exactly this.
    ///
    /// # Errors
    ///
    /// Per-Run failures are reported per entry; only an inventory read
    /// failure surfaces as `Err`.
    pub fn reconcile_all(
        &mut self,
        deps: &ReconciliationDeps<'_>,
    ) -> Result<Vec<InventoryResult>, StoreError> {
        let inventory = self.store.nonterminal_run_inventory()?;
        Ok(inventory
            .into_iter()
            .map(|entry| {
                let run_id = entry.run_id.clone();
                (run_id.clone(), self.reconcile_run(&run_id, deps))
            })
            .collect())
    }

    /// Drives one Run one decisive step forward.
    ///
    /// # Errors
    ///
    /// [`RunControllerError`] when durable state or entropy fails in a way
    /// that is not an ordinary fenced outcome.
    pub fn reconcile_run(
        &mut self,
        run_id: &str,
        deps: &ReconciliationDeps<'_>,
    ) -> Result<RunOutcome, RunControllerError> {
        let Some(view) = self.store.run_execution_view(run_id)? else {
            return Ok(RunOutcome::Unknown);
        };
        if view.phase != "Active" {
            return Ok(RunOutcome::Inactive { phase: view.phase });
        }

        match view.attempt {
            None => self.establish_attempt(run_id, &view, deps),
            Some(lineage) => {
                let attempt_id = lineage.attempt.id.clone();
                if lineage.terminal {
                    return self.decide_after_terminal(run_id, &lineage.attempt.id, deps);
                }
                match lineage.launch_contact_state {
                    // Pre-contact: the raw bearer is load-bearing, so lost
                    // memory rotates through T4a before anything launches.
                    LaunchContactState::NotContacted => {
                        self.ensure_current_bearer(run_id, &lineage)?;
                        let refreshed = self.current_lineage(run_id, &attempt_id)?;
                        self.launch(run_id, &refreshed, deps)
                    }
                    // Post-contact the credential is frozen and inspection
                    // addresses the lineage by LaunchKey alone; no bearer is
                    // needed, and none may be minted for this Attempt.
                    LaunchContactState::ContactMayHaveOccurred => {
                        self.reconcile_contacted(&lineage, deps)
                    }
                }
            }
        }
    }

    // -- preparation and T4 --------------------------------------------------

    fn establish_attempt(
        &mut self,
        run_id: &str,
        view: &pantheon_store::RunExecutionView,
        deps: &ReconciliationDeps<'_>,
    ) -> Result<RunOutcome, RunControllerError> {
        // WorkspaceReady against the frozen snapshot's own Workspace.
        match self.store.workspace_readiness(run_id)? {
            Some((phase, materialization)) if phase == "Ready" && materialization == "Present" => {}
            Some((phase, materialization)) => {
                return self.conclude_in_preparation(
                    run_id,
                    view.revision,
                    format!("workspace:{phase}/{materialization}"),
                );
            }
            None => {
                return self.conclude_in_preparation(
                    run_id,
                    view.revision,
                    "workspace:frozen-workspace-unavailable".to_string(),
                );
            }
        }

        // SandboxReady — fake/test infrastructure until #34. A refusal keeps
        // the gate shut and concludes the Run with zero Attempts; it is never
        // retried into existence.
        let binding = self.binding_profile_identity(view)?;
        if let Err(detail) = deps.sandbox.verify_ready(SandboxCheck {
            run_id,
            sandbox_profile_digest: &binding.sandbox_profile_digest,
        }) {
            return self.conclude_in_preparation(
                run_id,
                view.revision,
                format!("sandbox:{detail}"),
            );
        }

        // ContextReady: deterministic preparation against the exact frozen
        // snapshot, attaching exactly once. Idempotent on retry. A frozen-
        // source failure concludes the Run rather than substituting anything;
        // a genuine storage failure propagates without concluding anything.
        if let Err(error) = crate::context::ContextPreparationController::new(self.store)
            .prepare_run_context(run_id)
        {
            return match error {
                crate::context::ContextPreparationError::Store(store) => {
                    Err(RunControllerError::Store(store))
                }
                other => {
                    self.conclude_in_preparation(
                        run_id,
                        // The status revision did not move while preparing.
                        view.revision,
                        format!("context:{other}"),
                    )
                }
            };
        }

        // PolicyReady: the frozen revision still yields the exact sandbox
        // profile identity the Binding froze.
        self.verify_policy_ready(run_id, &binding)?;

        // LaunchReady reached. T4/T8: one authoritative lineage creation.
        let ordinal = self.store.attempt_history_count(run_id)? + 1;
        let attempt_id = derive_identifier("attempt", run_id, ordinal);
        let session_id = derive_identifier("acs", run_id, ordinal);
        let launch_key = self.random_hex_key()?;
        let bearer = Bearer::generate(&self.random)?;
        let request_hash = Digest::of(
            &Value::object([
                ("kind", Value::string("attempt.creation")),
                ("runId", Value::string(run_id)),
                ("ordinal", Value::Integer(ordinal)),
            ])
            .to_canonical_bytes(),
        );
        let epoch = self.store.restore_generation()?;
        let command = Command {
            epoch: epoch.as_str(),
            id: &format!("t4-{run_id}-{ordinal}"),
            request_hash: request_hash.as_bytes(),
            event_type: "run.attempt.created",
        };

        let committed = self.store.create_attempt(
            &command,
            &pantheon_store::AttemptCreation {
                run_id,
                attempt_id: &attempt_id,
                launch_key: &launch_key,
                session_id: &session_id,
                credential_verifier: &bearer.verifier(),
                expected_run_status_revision: view.revision,
            },
        )?;

        match committed {
            Committed::Executed { .. } => {
                self.bearers.insert(
                    attempt_id.clone(),
                    BearerState {
                        bearer,
                        credential_revision: 1,
                    },
                );
            }
            // The lineage already existed when this command committed; the
            // freshly drawn material is not its credential. Dropping it and
            // holding no bearer sends the next pass through T4a, which is
            // exactly right while the Attempt is NOT_CONTACTED.
            Committed::Replayed { .. } => {}
        }

        Ok(RunOutcome::AttemptEstablished {
            attempt_id,
            launch_key,
        })
    }

    fn conclude_in_preparation(
        &mut self,
        run_id: &str,
        revision: Revision,
        gate: String,
    ) -> Result<RunOutcome, RunControllerError> {
        self.store.conclude_run(run_id, "Failed", revision)?;
        Ok(RunOutcome::ConcludedInPreparation { gate })
    }

    /// Extracts the frozen sandbox-profile identity facts from the stored
    /// immutable Binding.
    fn binding_profile_identity(
        &self,
        view: &pantheon_store::RunExecutionView,
    ) -> Result<FrozenBindingIdentity, RunControllerError> {
        let json = self
            .store
            .binding_canonical_json(view.binding_digest)?
            .ok_or_else(|| {
                RunControllerError::Store(StoreError::InvariantViolated(format!(
                    "run {} binds {:?} which is not stored",
                    view.run_id, view.binding_digest
                )))
            })?;
        let value = parse::parse(&json).map_err(|error| {
            RunControllerError::Store(StoreError::InvariantViolated(format!(
                "stored binding is unreadable: {error}"
            )))
        })?;
        let Some(components) = value.get("componentDigests") else {
            return Err(RunControllerError::Store(StoreError::InvariantViolated(
                "stored binding has no componentDigests".to_string(),
            )));
        };
        let field = |name: &str, source: &Value| -> Result<Digest, RunControllerError> {
            Digest::from_display(string_field(source, name).ok_or_else(|| {
                StoreError::InvariantViolated(format!("stored binding has no readable {name}"))
            })?)
            .ok_or_else(|| {
                StoreError::InvariantViolated(format!(
                    "stored binding {name} is not a sha256 digest"
                ))
            })
            .map_err(RunControllerError::Store)
        };
        Ok(FrozenBindingIdentity {
            sandbox_profile_digest: field("sandboxProfileDigest", &value)?,
            execution_profiles_component: field("executionProfiles", components)?,
        })
    }

    /// PolicyReady: the frozen execution-profiles component still contains
    /// exactly the profile identity the Binding froze.
    fn verify_policy_ready(
        &self,
        run_id: &str,
        identity: &FrozenBindingIdentity,
    ) -> Result<(), RunControllerError> {
        let Some((domain, json)) = self
            .store
            .configuration_component_json(identity.execution_profiles_component)?
        else {
            return Err(RunControllerError::Store(StoreError::InvariantViolated(
                format!("run {run_id}: frozen execution-profiles component is unavailable"),
            )));
        };
        debug_assert_eq!(domain, "executionProfiles");
        let value = parse::parse(&json).map_err(|error| {
            RunControllerError::Store(StoreError::InvariantViolated(format!(
                "frozen execution profiles unreadable: {error}"
            )))
        })?;
        let entries = array_field(&value, "profiles");
        let Some(entries) = entries else {
            return Err(RunControllerError::Store(StoreError::InvariantViolated(
                "frozen execution profiles carry no profiles array".to_string(),
            )));
        };
        let frozen = entries
            .iter()
            .filter_map(|entry| decode_sandbox_profile(entry).ok())
            .find(|profile| profile.digest() == identity.sandbox_profile_digest);
        match frozen {
            Some(_) => {}
            None => {
                return Err(RunControllerError::Store(StoreError::InvariantViolated(
                    format!(
                        "run {run_id}: frozen profiles do not contain the Binding's \
                         sandbox identity"
                    ),
                )));
            }
        }
        Ok(())
    }

    // -- bearer / T4a --------------------------------------------------------

    /// Makes sure process memory holds the current credential revision,
    /// rotating through T4a exactly when it does not.
    fn ensure_current_bearer(
        &mut self,
        run_id: &str,
        lineage: &pantheon_store::AttemptLineageView,
    ) -> Result<(), RunControllerError> {
        let attempt_id = &lineage.attempt.id;
        let current_revision = lineage.session.credential_revision;
        let known = self
            .bearers
            .get(attempt_id)
            .is_some_and(|state| state.credential_revision == current_revision);
        if known {
            return Ok(());
        }

        // Lost bearer material (restart, replay adoption). While durably
        // NOT_CONTACTED in the current generation, T4a rotates the same
        // session; the store refuses everything else and this controller
        // never retries a refusal blindly.
        let bearer = Bearer::generate(&self.random)?;
        let new_revision = self.store.rekey_agent_control_session(
            attempt_id,
            &bearer.verifier(),
            current_revision,
        )?;
        self.bearers.insert(
            attempt_id.clone(),
            BearerState {
                bearer,
                credential_revision: new_revision,
            },
        );
        let _ = run_id;
        Ok(())
    }

    // -- T4b + first contact -------------------------------------------------

    fn launch(
        &mut self,
        run_id: &str,
        lineage: &pantheon_store::AttemptLineageView,
        deps: &ReconciliationDeps<'_>,
    ) -> Result<RunOutcome, RunControllerError> {
        if deps.launcher.launch_semantics() != LaunchSemantics::KeyedIdempotent {
            return Err(RunControllerError::Store(StoreError::InvariantViolated(
                format!(
                    "backend {} does not factually advertise KEYED_IDEMPOTENT semantics",
                    deps.launcher.backend_id()
                ),
            )));
        }

        let attempt_id = lineage.attempt.id.clone();
        let launch_key = lineage.attempt.launch_key.clone();
        let plan_digest = self.attached_plan_digest(run_id)?;
        let bearer_state = self.bearers.get(&attempt_id).ok_or_else(|| {
            RunControllerError::Store(StoreError::InvariantViolated(format!(
                "no current bearer for attempt {attempt_id}; refusing to launch"
            )))
        })?;
        let package = LaunchPackage {
            launch_key: &launch_key,
            run_id,
            attempt_id: &attempt_id,
            credential: &bearer_state.bearer,
            context_plan_digest: &plan_digest,
        };

        // T4b: commit the conservative boundary BEFORE the external call,
        // binding the exact credential revision being delivered.
        self.store.mark_launch_contact(
            run_id,
            &attempt_id,
            &self.incarnation,
            lineage.status_revision,
            lineage.session.credential_revision,
        )?;

        // First external contact. Any failure here is conservatively
        // ambiguous: the marker is already durable.
        let observation = match deps.launcher.ensure_execution(&package) {
            Ok(observation @ (Observation::Starting | Observation::Running)) => observation,
            Ok(Observation::Exited) => {
                // Acknowledged and already finished: definitive end.
                let status_now = self.current_status_revision(&attempt_id)?;
                self.record(
                    status_now,
                    &attempt_id,
                    ObservationUpdate::Terminal(Observation::Exited),
                )?;
                return Ok(RunOutcome::Launched {
                    attempt_id,
                    observation: Observation::Exited,
                });
            }
            Ok(other) => other,
            Err(failure) => {
                let _ = failure;
                Observation::Unknown
            }
        };
        let update = match observation {
            Observation::Exited => ObservationUpdate::Terminal(Observation::Exited),
            other => ObservationUpdate::Observe(other),
        };
        let status_now = self.current_status_revision(&attempt_id)?;
        self.record(status_now, &attempt_id, update)?;
        Ok(RunOutcome::Launched {
            attempt_id,
            observation,
        })
    }

    // -- same-lineage reconciliation ------------------------------------------

    fn reconcile_contacted(
        &mut self,
        lineage: &pantheon_store::AttemptLineageView,
        deps: &ReconciliationDeps<'_>,
    ) -> Result<RunOutcome, RunControllerError> {
        let attempt_id = lineage.attempt.id.clone();
        let observation = deps
            .launcher
            .inspect_execution(&lineage.attempt.launch_key)
            .unwrap_or(Observation::Unknown);

        match observation {
            Observation::Starting | Observation::Running => {
                let status_now = self.current_status_revision(&attempt_id)?;
                self.record(
                    status_now,
                    &attempt_id,
                    ObservationUpdate::Observe(observation),
                )?;
                Ok(RunOutcome::Reconciled {
                    attempt_id,
                    observation,
                })
            }
            Observation::Unknown => {
                let status_now = self.current_status_revision(&attempt_id)?;
                self.record(
                    status_now,
                    &attempt_id,
                    ObservationUpdate::Observe(Observation::Unknown),
                )?;
                Ok(RunOutcome::UnknownFenced { attempt_id })
            }
            Observation::Absent | Observation::Exited => {
                // Definitive end proven by inspection (or proven absence).
                let status_now = self.current_status_revision(&attempt_id)?;
                self.record(
                    status_now,
                    &attempt_id,
                    ObservationUpdate::Terminal(observation),
                )?;
                self.decide_after_terminal(&lineage.attempt.run_id, &attempt_id, deps)
            }
        }
    }

    /// The deterministic minimum Recovery Policy decision once a lineage is
    /// definitively terminal.
    fn decide_after_terminal(
        &mut self,
        run_id: &str,
        attempt_id: &str,
        deps: &ReconciliationDeps<'_>,
    ) -> Result<RunOutcome, RunControllerError> {
        let used = self.store.attempt_history_count(run_id)?;
        if used < i64::from(deps.policy.max_attempts_per_run) {
            return Ok(RunOutcome::RetryArmed {
                attempt_id: attempt_id.to_string(),
                next_ordinal: used + 1,
            });
        }
        let revision = self
            .store
            .run_execution_view(run_id)?
            .map(|view| view.revision)
            .ok_or_else(|| {
                RunControllerError::Store(StoreError::InvariantViolated(format!(
                    "run {run_id} vanished during recovery decision"
                )))
            })?;
        self.store.conclude_run(run_id, "Failed", revision)?;
        Ok(RunOutcome::ConcludedAfterFailure {
            attempt_id: attempt_id.to_string(),
        })
    }

    // -- small helpers ---------------------------------------------------------

    fn current_lineage(
        &self,
        run_id: &str,
        attempt_id: &str,
    ) -> Result<pantheon_store::AttemptLineageView, RunControllerError> {
        let view = self.store.run_execution_view(run_id)?.ok_or_else(|| {
            RunControllerError::Store(StoreError::InvariantViolated(format!(
                "run {run_id} vanished mid-reconciliation"
            )))
        })?;
        let lineage = view.attempt.ok_or_else(|| {
            RunControllerError::Store(StoreError::InvariantViolated(format!(
                "attempt {attempt_id} is no longer current for run {run_id}"
            )))
        })?;
        debug_assert_eq!(lineage.attempt.id, attempt_id);
        Ok(lineage)
    }

    fn current_status_revision(&self, attempt_id: &str) -> Result<Revision, RunControllerError> {
        self.store
            .attempt_status_revision(attempt_id)
            .map_err(RunControllerError::Store)
    }

    fn record(
        &self,
        expected: Revision,
        attempt_id: &str,
        update: ObservationUpdate,
    ) -> Result<(), RunControllerError> {
        let _new_revision = self
            .store
            .record_execution_observation(attempt_id, expected, update)?;
        Ok(())
    }

    fn attached_plan_digest(&self, run_id: &str) -> Result<Digest, RunControllerError> {
        self.store
            .run_execution_view(run_id)?
            .and_then(|view| view.context_plan_digest)
            .ok_or_else(|| {
                RunControllerError::Store(StoreError::InvariantViolated(format!(
                    "run {run_id} has no attached plan at launch time"
                )))
            })
    }

    fn random_hex_key(&self) -> Result<String, RunControllerError> {
        let mut bytes = [0u8; 32];
        self.random.fill(&mut bytes)?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}

#[derive(Debug, Clone, Copy)]
struct FrozenBindingIdentity {
    sandbox_profile_digest: Digest,
    execution_profiles_component: Digest,
}

fn derive_identifier(kind: &str, run_id: &str, ordinal: i64) -> String {
    let digest = Digest::of(
        &Value::object([
            ("kind", Value::string(kind)),
            ("runId", Value::string(run_id)),
            ("ordinal", Value::Integer(ordinal)),
        ])
        .to_canonical_bytes(),
    );
    format!(
        "{kind}-{}",
        digest
            .as_bytes()
            .iter()
            .take(IDENTIFIER_HEX)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// Decodes one frozen profile entry enough to compute core's canonical
/// content digest for it.
fn decode_sandbox_profile(value: &Value) -> Result<SandboxProfile, String> {
    let string = |name: &str| -> Result<String, String> {
        string_field(value, name)
            .map(str::to_string)
            .ok_or_else(|| format!("profile entry has no readable {name}"))
    };
    let isolation_class = match string("isolationClass")?.as_str() {
        "TRUSTED_HOST" => IsolationClass::TrustedHost,
        "CONTAINER" => IsolationClass::Container,
        other => return Err(format!("unknown isolation class {other:?}")),
    };
    let network_mode = match string("networkMode")?.as_str() {
        "NONE" => NetworkMode::None,
        other => return Err(format!("unknown network mode {other:?}")),
    };
    let guarantees = array_field(value, "guarantees")
        .map(|items| {
            items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(SandboxProfile {
        name: string("name")?,
        isolation_class,
        guarantees,
        network_mode,
        environment_identity: string("environmentIdentity")?,
    })
}

/// Reads a string-valued object member.
fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value.get(key) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

/// Reads an array-valued object member.
fn array_field<'a>(value: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    match value.get(key) {
        Some(Value::Array(items)) => Some(items),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
