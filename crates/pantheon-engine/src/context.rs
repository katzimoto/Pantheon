//! The Context Builder preparation path: reconstruct a committed Run's frozen
//! sources, deterministically build its provider-neutral [`ContextPlan`], and
//! attach it exactly once through the store's T3a transaction.
//!
//! `docs/architecture/agents-and-context/context-builder.md` ("Run boundary")
//! and `docs/architecture/scheduling/scheduler-dispatch-and-run-intent-
//! reconciliation.md` ("Context-source ownership") are canonical for what this
//! module may do: every attempt for a Run reads *only* the source universe
//! frozen at T3 — never the active ConfigurationRevision, never current Agent
//! versions, never mutable Workspace state — and either produces byte-for-byte
//! reproducible semantic content or fails closed.
//!
//! # What this controller never does
//!
//! It creates no Attempt, renders no prompt, calls no model or backend,
//! measures nothing for a provider, touches no repository, and grants no
//! authority. It also performs none of the Run Controller lifecycle
//! (`WorkspaceReady`, launch conditions, recovery ownership): it is the
//! context-preparation step that lifecycle will compose.

use pantheon_core::config::canonical::Value;
use pantheon_core::config::model::ContextComponent;
use pantheon_core::config::{Digest, parse};
use pantheon_core::context::{
    CONTEXT_BUILDER_VERSION, ContextPlan, FrozenSources, GuidanceSourceError, build_context_plan,
    frozen_agent_guidance, guidance_digest,
};
use pantheon_core::planning::TaskSpec;
use pantheon_core::scheduling::ContextSourceSnapshot;
use pantheon_store::{Command, ContextPlanAttachment, Store, StoreError};

/// What one successful preparation established.
///
/// The Run durably owns exactly one attachment naming these identities; the
/// typed plan is carried for later Run Controller work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedContext {
    pub run_id: String,
    pub source_snapshot_digest: Digest,
    pub context_plan_digest: Digest,
    pub plan: ContextPlan,
}

/// Why preparing one Run's context failed.
///
/// Every variant names the failing boundary so a caller can react — retry a
/// transient store condition, treat a frozen-source failure as fail-closed for
/// the Run, or surface corruption for reconciliation — without parsing prose.
/// Per the context contract, a missing or mismatched frozen source is never
/// healed by substituting a newer generation.
#[derive(Debug)]
pub enum ContextPreparationError {
    /// Durable state refused or could not perform an operation. Carries
    /// [`StoreError`] variants including [`StoreError::RunNotFound`],
    /// [`StoreError::ContextSourceMismatch`],
    /// [`StoreError::RunContextPlanConflict`] and
    /// [`StoreError::ContentIdentityConflict`], which are authoritative
    /// refusals rather than storage failures.
    Store(StoreError),
    /// The Run's frozen source snapshot row itself is unavailable.
    FrozenSnapshotUnavailable { digest: String },
    /// A persisted source's bytes no longer produce its recorded identity.
    SourceDigestMismatch {
        source: &'static str,
        expected: String,
        actual: String,
    },
    /// A required frozen source does not exist in durable state.
    ///
    /// Fail closed: the Run stays unprepared rather than silently selecting
    /// `latest`.
    RequiredSourceUnavailable { detail: String },
    /// A loaded frozen source does not belong to the relation the snapshot
    /// froze (wrong Goal, wrong Task, wrong base).
    WrongSourceRelation { detail: String },
    /// Persisted canonical state cannot be interpreted by this build.
    MalformedSource { detail: String },
    /// The frozen policy demands mandatory content the frozen sources cannot
    /// yield.
    PolicyUnsatisfiable { detail: String },
}

impl std::fmt::Display for ContextPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(err) => write!(f, "context preparation store failure: {err}"),
            Self::FrozenSnapshotUnavailable { digest } => write!(
                f,
                "the frozen context source snapshot {digest} is unavailable"
            ),
            Self::SourceDigestMismatch {
                source,
                expected,
                actual,
            } => write!(
                f,
                "frozen source {source} hashes to {actual}, not the {expected} the Run \
                 recorded"
            ),
            Self::RequiredSourceUnavailable { detail } => write!(
                f,
                "required frozen source is unavailable: {detail}; preparation fails \
                 closed instead of substituting a newer generation"
            ),
            Self::WrongSourceRelation { detail } => {
                write!(f, "frozen source does not belong to this Run: {detail}")
            }
            Self::MalformedSource { detail } => {
                write!(f, "persisted canonical source is malformed: {detail}")
            }
            Self::PolicyUnsatisfiable { detail } => {
                write!(f, "the frozen context policy cannot be satisfied: {detail}")
            }
        }
    }
}

impl std::error::Error for ContextPreparationError {}

impl From<StoreError> for ContextPreparationError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

/// Prepares Run context against durable authority.
///
/// Holds no cache of its own: every call re-reads the Run, its frozen source
/// snapshot and each frozen source by immutable identity, so daemon restart
/// changes nothing about what the same Run prepares to.
#[derive(Debug)]
pub struct ContextPreparationController<'store> {
    store: &'store Store,
}

impl<'store> ContextPreparationController<'store> {
    #[must_use]
    pub const fn new(store: &'store Store) -> Self {
        Self { store }
    }

    /// Deterministically prepares the initial ContextPlan for one committed
    /// Run and attaches it exactly once.
    ///
    /// Idempotent and restart-safe: a retry after a crash before attachment
    /// rebuilds the identical plan from the identical frozen sources and
    /// attaches it; a retry after attachment reconciles against the durable
    /// attachment without replacing anything. A material change to any source
    /// requires a new Run — this method can never observe one for this Run.
    ///
    /// # Errors
    ///
    /// [`ContextPreparationError`] when the Run, its frozen snapshot, any
    /// frozen source, the plan construction or the attachment fails.
    pub fn prepare_run_context(
        &self,
        run_id: &str,
    ) -> Result<PreparedContext, ContextPreparationError> {
        // 1. The committed Run and the exact snapshot identity it froze at T3.
        let Some(run) = self.store.run(run_id)? else {
            return Err(ContextPreparationError::Store(StoreError::RunNotFound {
                run_id: run_id.to_string(),
            }));
        };

        // 2. Reconstruct the frozen source universe from its own canonical
        //    bytes and prove those bytes are the Run's frozen identity.
        let stored_snapshot = self
            .store
            .context_source_snapshot_json(run.context_source_snapshot_digest)?;
        let snapshot = decode_frozen_snapshot(stored_snapshot, run.context_source_snapshot_digest)?;

        // 3. The frozen context policy — by digest, not by active pointer.
        let stored_policy = self
            .store
            .configuration_component_json(snapshot.context_policy_digest)?;
        let policy = decode_frozen_policy(stored_policy, snapshot.context_policy_digest)?;

        // 4. The immutable Task specification, verified and related.
        let stored_spec = self.store.task_spec_json(snapshot.task_spec_digest)?;
        let spec = decode_task_spec(stored_spec, snapshot.task_spec_digest)?;
        verify_task_relation(&spec, &snapshot)?;

        // 5. The exact Goal revision the Task was created from.
        let goal = self
            .store
            .goal_revision_content(&snapshot.goal_id, snapshot.goal_revision)?
            .ok_or_else(|| ContextPreparationError::RequiredSourceUnavailable {
                detail: format!(
                    "goal revision {}@{}",
                    snapshot.goal_id, snapshot.goal_revision
                ),
            })?;
        verify_goal_content(&goal)?;

        // 6. The selected Agent version's approved guidance from the frozen
        //    revision's agents component — never from the current one.
        let agents_json = self
            .store
            .revision_agents_component_json(snapshot.configuration_activation_sequence)?
            .ok_or_else(|| ContextPreparationError::RequiredSourceUnavailable {
                detail: format!(
                    "agents component of configuration revision {}",
                    snapshot.configuration_activation_sequence
                ),
            })?;
        let guidance = decode_agent_guidance(&agents_json, &snapshot)?;

        // 7. The frozen Workspace's ownership and immutable base. Phase is
        //    deliberately not consulted: lifecycle may legitimately have
        //    advanced since T3, and only the frozen facts belong to context.
        let workspace = self.store.workspace_record(&snapshot.workspace_id)?;
        let workspace = verify_workspace_relation(workspace, &run.task_id, &snapshot)?;

        // 8. Pure deterministic selection over the verified frozen inputs.
        let plan = build_context_plan(
            &snapshot,
            &FrozenSources {
                task_spec: &spec,
                goal_content_digest: goal.content_digest,
                soul: &guidance.soul,
                behavior: &guidance.behavior,
                workspace_repository: &workspace.repository,
            },
            &policy,
        )
        .map_err(|error| ContextPreparationError::PolicyUnsatisfiable {
            detail: error.to_string(),
        })?;
        let plan_digest = plan.digest();
        let plan_canonical =
            String::from_utf8(plan.to_value().to_canonical_bytes()).map_err(|_| {
                ContextPreparationError::MalformedSource {
                    detail: "the built plan's canonical encoding is not UTF-8".to_string(),
                }
            })?;

        // 9. Reconciliation first: an identical durable attachment means this
        //    attempt has nothing left to make durable. A different attached
        //    plan falls through to T3a, which is the authority that refuses
        //    replacement with its typed conflict.
        if self
            .store
            .run_context_plan(&run.id)?
            .is_some_and(|existing| existing.context_plan_digest == plan_digest)
        {
            return Ok(PreparedContext {
                run_id: run.id,
                source_snapshot_digest: run.context_source_snapshot_digest,
                context_plan_digest: plan_digest,
                plan,
            });
        }

        // 10. One-time attachment under a command identity derived from the
        //     exact content being attached: the same frozen world is the same
        //     command (a lost response replays), and a different attempt is a
        //     different command by construction.
        let epoch = self.store.restore_generation()?;
        let request_hash = Digest::of(
            &Value::object([
                ("kind", Value::string("run.context-plan-attachment")),
                ("runId", Value::string(&run.id)),
                (
                    "sourceSnapshotDigest",
                    Value::string(run.context_source_snapshot_digest.to_string()),
                ),
                ("planDigest", Value::string(plan_digest.to_string())),
            ])
            .to_canonical_bytes(),
        );
        let mut command_hex = [0u8; 16];
        command_hex.copy_from_slice(&request_hash.as_bytes()[..16]);
        let command_id: String = command_hex.iter().map(|b| format!("{b:02x}")).collect();
        let command = Command {
            epoch: epoch.as_str(),
            id: &format!("t3a-{command_id}"),
            request_hash: request_hash.as_bytes(),
            event_type: "run.context.attached",
        };
        let _ = self.store.attach_run_context_plan(
            &command,
            &ContextPlanAttachment {
                run_id: &run.id,
                source_snapshot_digest: &run.context_source_snapshot_digest,
                plan_digest: &plan_digest,
                builder_version: CONTEXT_BUILDER_VERSION,
                plan_canonical_json: &plan_canonical,
            },
        )?;

        Ok(PreparedContext {
            run_id: run.id,
            source_snapshot_digest: run.context_source_snapshot_digest,
            context_plan_digest: plan_digest,
            plan,
        })
    }
}

/// Decodes and identity-verifies the frozen source snapshot.
///
/// The whole decision is pure over the loaded bytes so every fail-closed
/// branch — absent row, tampered bytes, decoder drift — is provable without
/// durable-state surgery.
fn decode_frozen_snapshot(
    stored: Option<String>,
    expected: Digest,
) -> Result<ContextSourceSnapshot, ContextPreparationError> {
    let stored = stored.ok_or_else(|| ContextPreparationError::FrozenSnapshotUnavailable {
        digest: expected.to_string(),
    })?;
    if Digest::of(stored.as_bytes()) != expected {
        return Err(ContextPreparationError::SourceDigestMismatch {
            source: "context-source-snapshot",
            expected: expected.to_string(),
            actual: Digest::of(stored.as_bytes()).to_string(),
        });
    }
    let snapshot = ContextSourceSnapshot::from_canonical_json(&stored).map_err(|error| {
        ContextPreparationError::MalformedSource {
            detail: error.to_string(),
        }
    })?;
    // Decoding proved shape; re-digesting proves the decoded content is the
    // identity the Run froze, closing any decoder drift gap.
    if snapshot.digest() != expected {
        return Err(ContextPreparationError::SourceDigestMismatch {
            source: "context-source-snapshot",
            expected: expected.to_string(),
            actual: snapshot.digest().to_string(),
        });
    }
    Ok(snapshot)
}

/// Decodes the frozen context policy from its content-addressed component.
fn decode_frozen_policy(
    stored: Option<(String, String)>,
    expected: Digest,
) -> Result<ContextComponent, ContextPreparationError> {
    let Some((domain, canonical_json)) = stored else {
        return Err(ContextPreparationError::RequiredSourceUnavailable {
            detail: format!("context policy component {expected}"),
        });
    };
    if domain != "context" {
        return Err(ContextPreparationError::MalformedSource {
            detail: format!("digest {expected} is stored as the {domain} component, not context"),
        });
    }
    let value = parse::parse(&canonical_json).map_err(|error| {
        ContextPreparationError::MalformedSource {
            detail: error.to_string(),
        }
    })?;
    ContextComponent::from_canonical_value(&value).map_err(|error| {
        ContextPreparationError::MalformedSource {
            detail: error.to_string(),
        }
    })
}

/// Decodes and identity-verifies the immutable Task specification.
fn decode_task_spec(
    stored: Option<String>,
    expected: Digest,
) -> Result<TaskSpec, ContextPreparationError> {
    let canonical = stored.ok_or_else(|| ContextPreparationError::RequiredSourceUnavailable {
        detail: format!("task specification {expected}"),
    })?;
    let spec = TaskSpec::from_canonical_json(&canonical).map_err(|error| {
        ContextPreparationError::MalformedSource {
            detail: error.to_string(),
        }
    })?;
    if spec.digest() != expected {
        return Err(ContextPreparationError::SourceDigestMismatch {
            source: "task-specification",
            expected: expected.to_string(),
            actual: spec.digest().to_string(),
        });
    }
    Ok(spec)
}

/// Proves the spec belongs to the Goal relation the snapshot froze.
///
/// The spec carries its owner; only comparing closes the swap a bare digest
/// lookup would accept.
fn verify_task_relation(
    spec: &TaskSpec,
    snapshot: &ContextSourceSnapshot,
) -> Result<(), ContextPreparationError> {
    if spec.goal_id != snapshot.goal_id || spec.goal_revision != snapshot.goal_revision {
        return Err(ContextPreparationError::WrongSourceRelation {
            detail: format!(
                "spec names goal {}@{} but the snapshot froze goal {}@{}",
                spec.goal_id, spec.goal_revision, snapshot.goal_id, snapshot.goal_revision
            ),
        });
    }
    Ok(())
}

/// Verifies the loaded Goal revision content against its own recorded digest.
fn verify_goal_content(
    content: &pantheon_store::GoalRevisionContent,
) -> Result<(), ContextPreparationError> {
    let actual = Digest::of(content.canonical_json.as_bytes());
    if actual != content.content_digest {
        return Err(ContextPreparationError::SourceDigestMismatch {
            source: "goal-revision",
            expected: content.content_digest.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}

/// Extracts the frozen Agent version's guidance and proves it reproduces the
/// digests frozen at T3.
///
/// Defense in depth on top of T3's own validation: a corrupted component or a
/// drifted extraction rule fails closed here rather than preparing a Run with
/// substituted instruction semantics.
fn decode_agent_guidance(
    agents_component_json: &str,
    snapshot: &ContextSourceSnapshot,
) -> Result<pantheon_core::context::AgentGuidance, ContextPreparationError> {
    let value = parse::parse(agents_component_json).map_err(|error| {
        ContextPreparationError::MalformedSource {
            detail: error.to_string(),
        }
    })?;
    let guidance = frozen_agent_guidance(&value, &snapshot.agent).map_err(|error| match error {
        GuidanceSourceError::VersionAbsent { agent } => {
            ContextPreparationError::RequiredSourceUnavailable {
                detail: format!(
                    "agent {}@{} in the frozen agents component",
                    agent.name, agent.version
                ),
            }
        }
        GuidanceSourceError::Malformed(detail) => {
            ContextPreparationError::MalformedSource { detail }
        }
    })?;
    if guidance_digest(&guidance.soul) != snapshot.agent_soul_digest
        || guidance_digest(&guidance.behavior) != snapshot.agent_behavior_digest
    {
        return Err(ContextPreparationError::SourceDigestMismatch {
            source: "agent-guidance",
            expected: format!(
                "soul {}, behavior {}",
                snapshot.agent_soul_digest, snapshot.agent_behavior_digest
            ),
            actual: format!(
                "soul {}, behavior {}",
                guidance_digest(&guidance.soul),
                guidance_digest(&guidance.behavior)
            ),
        });
    }
    Ok(guidance)
}

/// Proves the Workspace row satisfies the ownership/base relation the
/// snapshot froze. Phase is deliberately not consulted: lifecycle may
/// legitimately have advanced since T3, and only the frozen facts belong to
/// context.
fn verify_workspace_relation(
    workspace: Option<pantheon_store::WorkspaceRecord>,
    task_id: &str,
    snapshot: &ContextSourceSnapshot,
) -> Result<pantheon_store::WorkspaceRecord, ContextPreparationError> {
    let workspace =
        workspace.ok_or_else(|| ContextPreparationError::RequiredSourceUnavailable {
            detail: format!("workspace {}", snapshot.workspace_id),
        })?;
    if workspace.task_id != task_id {
        return Err(ContextPreparationError::WrongSourceRelation {
            detail: format!(
                "workspace {} belongs to task {} but this Run commits for {task_id}",
                snapshot.workspace_id, workspace.task_id
            ),
        });
    }
    if workspace.resolved_base.as_str() != snapshot.workspace_resolved_base {
        return Err(ContextPreparationError::WrongSourceRelation {
            detail: format!(
                "workspace {} resolved to {}, not the frozen base {}",
                snapshot.workspace_id,
                workspace.resolved_base.as_str(),
                snapshot.workspace_resolved_base
            ),
        });
    }
    Ok(workspace)
}

#[cfg(test)]
mod tests;
