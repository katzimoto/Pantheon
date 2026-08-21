//! Revalidation of the execution authority a `code.changeset` seal runs
//! under.
//!
//! Since #29 a coding Task does its work under a durable Run: T3 commits
//! `Task Ready -> Active` with `active_run_id` pointing at the one
//! nonterminal Run that owns responsibility. Canonical sealing happens on
//! `task.submit_result`, under that same Run (`T6` revalidates "Run
//! Active/current responsible Run"), so once Runs exist there is no
//! production state in which sealing is authorized without one: a `Ready`
//! or `Waiting` Task provably owns zero nonterminal Runs and therefore has
//! no settled worker state to seal.
//!
//! [`SealAuthority`] is the claim a caller presents. Like every claim in
//! this crate it is never trusted: [`validate_seal_authority`] re-reads
//! authoritative state *inside* the caller's authoritative transaction and
//! refuses the seal unless every fact still holds. The same validation runs
//! at three boundaries — the freeze that fences the Workspace, the
//! revalidation of an already-frozen retry, and the final publication — so
//! an authority that goes stale between any two steps fails closed there.

use pantheon_core::artifact::CODE_CHANGESET_KIND;
use pantheon_core::planning::{TaskDecodeError, TaskPhase, TaskSpec};

use crate::error::StoreError;
use crate::transaction::{Revision, Value, Writer};

/// The execution authority a seal claims: the Run currently responsible for
/// the Task, named together with the `run_status` revision its proof was
/// read at.
///
/// The expected revision is what makes staleness detectable: a Run that has
/// been superseded, terminalized or otherwise moved since the caller looked
/// presents a different revision inside the validating transaction, and the
/// claim fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealAuthority {
    /// The durable identity of the responsible Run.
    pub run_id: String,
    /// The `run_status` revision observed when the claim was formed.
    pub expected_run_revision: Revision,
}

/// Re-reads and requires, inside the caller's authoritative transaction,
/// every fact a seal depends on:
///
/// ```text
/// the Task exists and is Active
/// the Run exists and belongs to that Task
/// the Run is nonterminal and at the claimed revision
/// the Run is the Task's current responsible Run
/// the Run froze this Task's exact TaskSpec digest
/// the Run froze exactly this Workspace at exactly its bound base
/// the frozen specification permits code.changeset on the requested slot
/// ```
///
/// Every refusal is [`StoreError::SealAuthorityInvalid`]: the seal's
/// authority is gone, not merely moved. Nothing is written.
///
/// # Errors
///
/// [`StoreError::SealAuthorityInvalid`] on any failed comparison above;
/// [`StoreError::InvariantViolated`] when stored rows cannot be interpreted.
pub(crate) fn validate_seal_authority(
    writer: &Writer<'_>,
    authority: &SealAuthority,
    task_id: &str,
    output_slot: &str,
    workspace_id: &str,
) -> Result<(), StoreError> {
    let invalid = |detail: String| {
        writer.fail(StoreError::SealAuthorityInvalid {
            workspace_id: workspace_id.to_string(),
            detail,
        })
    };

    // ---- The Task and its current responsibility. ----
    let task: Option<(String, i64, Option<String>, Vec<u8>)> = writer.query_optional(
        "SELECT phase, revision, active_run_id, spec_digest FROM tasks WHERE id = ?1",
        &[Value::from(task_id)],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let Some((phase_text, _task_revision, active_run_id, spec_digest)) = task else {
        return invalid(format!("task {task_id} no longer exists"));
    };
    if phase_text != TaskPhase::Active.as_str() {
        return invalid(format!(
            "task {task_id} is {phase_text}, not Active under a responsible Run"
        ));
    }
    if active_run_id.as_deref() != Some(authority.run_id.as_str()) {
        return invalid(match active_run_id {
            Some(current) => format!(
                "run {} is not task {task_id}'s current responsible run ({current})",
                authority.run_id
            ),
            None => format!(
                "run {} is not task {task_id}'s current responsible run (none)",
                authority.run_id
            ),
        });
    }

    // ---- The Run itself: existence, holder, currency, terminality. ----
    let run: Option<(String, Vec<u8>)> = writer.query_optional(
        "SELECT task_id, context_source_snapshot_digest FROM runs WHERE id = ?1",
        &[Value::from(authority.run_id.as_str())],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some((run_task_id, snapshot_digest)) = run else {
        return invalid(format!("run {} does not exist", authority.run_id));
    };
    if run_task_id != task_id {
        return invalid(format!(
            "run {} belongs to task {run_task_id}, not {task_id}",
            authority.run_id
        ));
    }

    let status: Option<(String, i64)> = writer.query_optional(
        "SELECT phase, revision FROM run_status WHERE run_id = ?1",
        &[Value::from(authority.run_id.as_str())],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some((run_phase, run_revision)) = status else {
        return writer.fail(StoreError::InvariantViolated(format!(
            "run {} has no status row",
            authority.run_id
        )));
    };
    // run_status.task_id is not re-checked against the sealing Task here:
    // its composite foreign key into runs already proves holder identity,
    // and the runs.task_id comparison above is that proof's transactional
    // form.
    if run_phase != "Active" && run_phase != "Finalizing" {
        return invalid(format!(
            "run {} is terminal ({run_phase})",
            authority.run_id
        ));
    }
    if Revision::new(run_revision) != authority.expected_run_revision {
        return invalid(format!(
            "run {} moved: authority was read at revision {}, found {run_revision}",
            authority.run_id,
            authority.expected_run_revision.get()
        ));
    }

    // ---- What the Run froze must be exactly what this seal claims. ----
    let snapshot: Option<(Vec<u8>, String, String)> = writer.query_optional(
        "SELECT task_spec_digest, workspace_id, workspace_resolved_base
         FROM context_source_snapshots WHERE digest = ?1",
        &[Value::Blob(snapshot_digest.clone())],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let Some((frozen_spec_digest, frozen_workspace_id, frozen_base)) = snapshot else {
        return writer.fail(StoreError::InvariantViolated(format!(
            "run {} names source snapshot {:?} which is not stored",
            authority.run_id,
            DigestRef(&snapshot_digest),
        )));
    };
    if frozen_spec_digest.as_slice() != spec_digest.as_slice() {
        return invalid(format!(
            "run {} froze task-spec digest {:?} but task {task_id} stores a different one",
            authority.run_id,
            DigestRef(&frozen_spec_digest),
        ));
    }
    if frozen_workspace_id != workspace_id {
        return invalid(format!(
            "run {} is bound to workspace {frozen_workspace_id}, not {workspace_id}",
            authority.run_id
        ));
    }

    let workspace_base: Option<String> = writer.query_optional(
        "SELECT resolved_base FROM workspaces WHERE id = ?1",
        &[Value::from(workspace_id)],
        |row| row.get(0),
    )?;
    let Some(workspace_base) = workspace_base else {
        return writer.fail(StoreError::InvariantViolated(format!(
            "workspace {workspace_id} disappeared inside its own transaction"
        )));
    };
    if workspace_base != frozen_base {
        return invalid(format!(
            "run {} is bound to base {frozen_base:?} but workspace {workspace_id} \
             resolved to {workspace_base:?}",
            authority.run_id
        ));
    }

    // ---- The frozen output ceiling must permit this exact seal. ----
    let spec_json: Option<String> = writer.query_optional(
        "SELECT canonical_json FROM task_specs WHERE digest = ?1",
        &[Value::Blob(spec_digest.clone())],
        |row| row.get(0),
    )?;
    let Some(spec_json) = spec_json else {
        return writer.fail(StoreError::InvariantViolated(
            "the task's stored specification is not present".to_string(),
        ));
    };
    let spec = TaskSpec::from_canonical_json(&spec_json).map_err(|TaskDecodeError(detail)| {
        StoreError::InvariantViolated(format!(
            "the task's stored specification cannot be decoded: {detail}"
        ))
    })?;
    let slot = spec
        .outputs
        .iter()
        .find(|output| output.name == output_slot);
    let Some(slot) = slot else {
        return invalid(format!("no such output slot {output_slot:?}"));
    };
    if slot.kind != CODE_CHANGESET_KIND {
        return invalid(format!(
            "output slot {output_slot:?} permits {}, not {CODE_CHANGESET_KIND}",
            slot.kind
        ));
    }

    Ok(())
}

/// Formats a stored digest for failure text without pulling the digest type
/// into every arm.
struct DigestRef<'a>(&'a [u8]);

impl std::fmt::Debug for DigestRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}
