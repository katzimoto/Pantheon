//! Durable [`ContextPlan`](pantheon_core::context::ContextPlan) state: the
//! content-addressed plan family, the one-time Run attachment transaction
//! (T3a), and the reads deterministic preparation reconstructs sources
//! through.
//!
//! `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
//! ("Context source snapshot and ContextPlan attachment", "T3a") is canonical.
//! The relational shape does the safety work this mission claims:
//!
//! - `run_context_plans.run_id` as primary key proves at most one attachment;
//! - the composite FK `(run_id, context_source_snapshot_digest)` proves an
//!   attachment names *its own Run's* frozen snapshot;
//! - the composite FK `(context_plan_digest, context_source_snapshot_digest)`
//!   proves the attached plan was built from that same snapshot;
//! - `context_plans` is keyed by digest, so a plan identity cannot silently
//!   come to mean different bytes.
//!
//! The controller-level checks inside T3a exist to turn each of those database
//! refusals into a typed failure a caller can distinguish without parsing
//! prose — and to reconcile the one idempotent case (same Run + same source +
//! same plan) before any insert would conflict.

use pantheon_core::config::Digest;

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::store::Store;
use crate::transaction::Value;

/// A committed Run's immutable identity row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub id: String,
    pub task_id: String,
    pub binding_digest: Digest,
    pub context_source_snapshot_digest: Digest,
}

/// The one-time Run-to-ContextPlan attachment, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContextPlanRecord {
    pub run_id: String,
    pub context_source_snapshot_digest: Digest,
    pub context_plan_digest: Digest,
}

/// The immutable Goal revision content preparation verifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRevisionContent {
    pub goal_id: String,
    pub revision: i64,
    pub content_digest: Digest,
    pub canonical_json: String,
}

/// One authoritative request to attach an already-built plan to its Run.
///
/// The plan travels with its canonical bytes so the transaction can verify
/// the claim `bytes → digest` before anything durable accepts it. Borrowed:
/// the caller already holds the built plan.
#[derive(Debug, Clone, Copy)]
pub struct ContextPlanAttachment<'a> {
    pub run_id: &'a str,
    pub source_snapshot_digest: &'a Digest,
    pub plan_digest: &'a Digest,
    pub builder_version: &'a str,
    pub plan_canonical_json: &'a str,
}

/// What one successful attachment established or reconciled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedContextPlan {
    pub run_id: String,
    pub context_plan_digest: Digest,
}

impl Store {
    /// The committed Run's immutable identity row.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read or interpreted.
    pub fn run(&self, run_id: &str) -> Result<Option<RunRecord>, StoreError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT task_id, binding_digest, context_source_snapshot_digest
                     FROM runs WHERE id = ?1",
                    rusqlite::params![run_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            row.map(|(task_id, binding_digest, snapshot_digest)| {
                Ok(RunRecord {
                    id: run_id.to_string(),
                    task_id,
                    binding_digest: digest(&binding_digest, "binding_digest")?,
                    context_source_snapshot_digest: digest(
                        &snapshot_digest,
                        "context_source_snapshot_digest",
                    )?,
                })
            })
            .transpose()
        })
    }

    /// The Run's one-time ContextPlan attachment, if it exists yet.
    ///
    /// Preparation consults this to report reconciliation after restart; it is
    /// never authority for building a plan — construction always re-derives
    /// from the frozen sources.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read or interpreted.
    pub fn run_context_plan(
        &self,
        run_id: &str,
    ) -> Result<Option<RunContextPlanRecord>, StoreError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT context_source_snapshot_digest, context_plan_digest
                     FROM run_context_plans WHERE run_id = ?1",
                    rusqlite::params![run_id],
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            row.map(|(snapshot_digest, plan_digest)| {
                Ok(RunContextPlanRecord {
                    run_id: run_id.to_string(),
                    context_source_snapshot_digest: digest(
                        &snapshot_digest,
                        "context_source_snapshot_digest",
                    )?,
                    context_plan_digest: digest(&plan_digest, "context_plan_digest")?,
                })
            })
            .transpose()
        })
    }

    /// The frozen source snapshot's stored canonical JSON, by digest.
    ///
    /// Returns the bytes verbatim; the caller decodes and re-digests them, so
    /// a corrupted payload fails at the identity check rather than being
    /// served as authority.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read.
    pub fn context_source_snapshot_json(
        &self,
        digest: Digest,
    ) -> Result<Option<String>, StoreError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT canonical_json FROM context_source_snapshots WHERE digest = ?1",
                rusqlite::params![digest.as_bytes().to_vec()],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Sqlite(other)),
            })
        })
    }

    /// An immutable compiled configuration component by digest.
    ///
    /// Verifies the payload still hashes to the requested digest and that it
    /// is stored under the domain named, then returns both. This is how a
    /// Run's frozen policy and agents components are reconstructed exactly —
    /// through their content addresses, never through the active pointer.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when the stored domain disagrees or
    /// the bytes no longer hash to `digest`; [`StoreError::Sqlite`] on a
    /// storage failure.
    pub fn configuration_component_json(
        &self,
        digest: Digest,
    ) -> Result<Option<(String, String)>, StoreError> {
        self.read(|conn| {
            let found: Option<(String, String)> = conn
                .query_row(
                    "SELECT domain, canonical_json FROM configuration_components
                     WHERE digest = ?1",
                    rusqlite::params![digest.as_bytes().to_vec()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            let Some((domain, canonical_json)) = found else {
                return Ok(None);
            };
            // Content addressing is the read fence: damaged or swapped bytes
            // are refused here rather than decoded downstream.
            if pantheon_core::config::Digest::of(canonical_json.as_bytes()) != digest {
                return Err(StoreError::InvariantViolated(format!(
                    "{domain} component content no longer hashes to {digest}"
                )));
            }
            Ok(Some((domain, canonical_json)))
        })
    }

    /// The immutable agents component of one historical ConfigurationRevision,
    /// by activation sequence.
    ///
    /// Historical revisions stay addressable forever even after a newer one
    /// activated; this read is how a Run prepared under revision N keeps
    /// resolving revision N's Agent guidance after N+1 became active.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when the revision or its agents
    /// component cannot be resolved; [`StoreError::Sqlite`] on a storage
    /// failure.
    pub fn revision_agents_component_json(
        &self,
        activation_sequence: i64,
    ) -> Result<Option<String>, StoreError> {
        self.read(|conn| {
            let found: Option<(i64, String)> = conn
                .query_row(
                    "SELECT revision.activation_sequence, component.canonical_json
                     FROM configuration_revisions revision
                     JOIN configuration_components component
                       ON component.digest = revision.agents_digest
                     WHERE revision.activation_sequence = ?1
                       AND component.domain = 'agents'",
                    rusqlite::params![activation_sequence],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            found
                .map(|(sequence, canonical_json)| {
                    if sequence == activation_sequence {
                        Ok(canonical_json)
                    } else {
                        Err(StoreError::InvariantViolated(format!(
                            "configuration revision lookup returned sequence {sequence}, \
                         not {activation_sequence}"
                        )))
                    }
                })
                .transpose()
        })
    }

    /// The immutable content of one Goal revision, with its recorded digest.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when durable state cannot be read or interpreted.
    pub fn goal_revision_content(
        &self,
        goal_id: &str,
        revision: i64,
    ) -> Result<Option<GoalRevisionContent>, StoreError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT content_digest, canonical_json FROM goal_revisions
                     WHERE goal_id = ?1 AND revision = ?2",
                    rusqlite::params![goal_id, revision],
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;
            row.map(|(content_digest, canonical_json)| {
                Ok(GoalRevisionContent {
                    goal_id: goal_id.to_string(),
                    revision,
                    content_digest: digest(&content_digest, "content_digest")?,
                    canonical_json,
                })
            })
            .transpose()
        })
    }

    /// Commits one T3a ContextPlan attachment: the one-time boundary between
    /// deterministic preparation and durable Run readiness.
    ///
    /// Inside one authoritative transaction this re-reads the Run, refuses a
    /// plan whose claimed source snapshot is not the one the Run froze at T3,
    /// verifies the plan's canonical bytes against its own digest, inserts or
    /// verifies the immutable plan row, attaches it exactly once, and appends
    /// the Event atomically with the attachment. Nothing outside the database
    /// is touched.
    ///
    /// Idempotence follows the command envelope plus the attachment's own
    /// rules: the same `(run, source, plan)` under a fresh command identity
    /// reconciles successfully; a second *different* plan for an already
    /// attached Run fails closed; a plan built from another snapshot fails
    /// closed; and a digest that already names different bytes fails closed.
    /// Command replay of the original request reports the prior outcome
    /// without executing any of this again.
    ///
    /// # Errors
    ///
    /// - [`StoreError::RunNotFound`] when the Run does not exist;
    /// - [`StoreError::ContextSourceMismatch`] when the attachment names a
    ///   different source snapshot than the Run froze;
    /// - [`StoreError::ContentIdentityConflict`] when the plan bytes do not
    ///   hash to the claimed digest, or the digest already names different
    ///   stored content;
    /// - [`StoreError::RunContextPlanConflict`] when a different plan is
    ///   already attached to the Run;
    ///
    /// plus the command envelope's failures. In every failure case nothing is
    /// written.
    pub fn attach_run_context_plan(
        &self,
        command: &Command<'_>,
        attachment: &ContextPlanAttachment<'_>,
    ) -> Result<Committed<AttachedContextPlan>, StoreError> {
        self.execute_command(command, |writer| apply_attachment(writer, attachment))
    }
}

fn apply_attachment(
    writer: &crate::transaction::Writer<'_>,
    attachment: &ContextPlanAttachment<'_>,
) -> Result<AttachedContextPlan, StoreError> {
    // 1. The Run, re-read on the transaction's own snapshot. Its frozen
    //    snapshot digest is the only source identity this attachment may
    //    claim; the runs FK guarantees the snapshot row itself exists.
    let run = writer.query_optional(
        "SELECT context_source_snapshot_digest FROM runs WHERE id = ?1",
        &[Value::from(attachment.run_id)],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let Some(run_snapshot) = run else {
        return writer.fail(StoreError::RunNotFound {
            run_id: attachment.run_id.to_string(),
        });
    };
    if run_snapshot.as_slice() != attachment.source_snapshot_digest.as_bytes() {
        return writer.fail(StoreError::ContextSourceMismatch {
            run_id: attachment.run_id.to_string(),
            frozen: Digest::from_bytes(run_snapshot.as_slice().try_into().map_err(|_| {
                StoreError::InvariantViolated(format!(
                    "run {} stores a malformed snapshot digest",
                    attachment.run_id
                ))
            })?)
            .to_string(),
            proposed: attachment.source_snapshot_digest.to_string(),
        });
    }

    // 2. The plan's own claim must hold before anything durable accepts it:
    //    the bytes travel with the digest, and they must actually produce it.
    //    This is the one place the store hashes caller-provided content, and
    //    it is deliberate: content-addressed identity is only meaningful if
    //    the identity is verified where the row is written, not merely
    //    upstream in the caller.
    let computed = Digest::of(attachment.plan_canonical_json.as_bytes());
    if computed != *attachment.plan_digest {
        return writer.fail(StoreError::ContentIdentityConflict {
            table: "context_plans",
            id: attachment.plan_digest.to_string(),
            detail: format!(
                "the plan's canonical bytes hash to {computed}, not the claimed {}",
                attachment.plan_digest
            ),
        });
    }

    // 3. Insert-or-verify the immutable plan row. Reaching an existing row
    //    with identical bytes is convergence (another Run legitimately
    //    selected the same semantics from the same frozen universe); reaching
    //    one with different bytes means either corruption or a broken hash,
    //    and neither is overwritten.
    writer.execute(
        "INSERT INTO context_plans
             (digest, source_snapshot_digest, builder_version, canonical_json, created_at)
         VALUES (?1, ?2, ?3, ?4, unixepoch())
         ON CONFLICT (digest) DO NOTHING",
        &[
            Value::Blob(attachment.plan_digest.as_bytes().to_vec()),
            Value::Blob(attachment.source_snapshot_digest.as_bytes().to_vec()),
            Value::from(attachment.builder_version),
            Value::from(attachment.plan_canonical_json),
        ],
    )?;
    let stored: Option<(Vec<u8>, String)> = writer.query_optional(
        "SELECT source_snapshot_digest, canonical_json FROM context_plans WHERE digest = ?1",
        &[Value::Blob(attachment.plan_digest.as_bytes().to_vec())],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some((stored_snapshot, stored_json)) = stored else {
        return writer.fail(StoreError::InvariantViolated(
            "context_plans insert reported success but the row is unreadable".to_string(),
        ));
    };
    if stored_json != attachment.plan_canonical_json {
        return writer.fail(StoreError::ContentIdentityConflict {
            table: "context_plans",
            id: attachment.plan_digest.to_string(),
            detail: "an existing plan row under this digest holds different canonical content"
                .to_string(),
        });
    }
    if stored_snapshot.as_slice() != attachment.source_snapshot_digest.as_bytes() {
        return writer.fail(StoreError::InvariantViolated(
            "context_plans row pairs the claimed digest with another source snapshot".to_string(),
        ));
    }

    // 4. Attach exactly once. Same Run + same source + same plan reconciles;
    //    anything else already attached fails closed rather than replacing.
    let existing: Option<Vec<u8>> = writer.query_optional(
        "SELECT context_plan_digest FROM run_context_plans WHERE run_id = ?1",
        &[Value::from(attachment.run_id)],
        |row| row.get(0),
    )?;
    if let Some(existing) = existing {
        if existing.as_slice() == attachment.plan_digest.as_bytes() {
            return Ok(AttachedContextPlan {
                run_id: attachment.run_id.to_string(),
                context_plan_digest: *attachment.plan_digest,
            });
        }
        return writer.fail(StoreError::RunContextPlanConflict {
            run_id: attachment.run_id.to_string(),
            attached_plan: Digest::from_bytes(existing.as_slice().try_into().map_err(|_| {
                StoreError::InvariantViolated(format!(
                    "run {} has a malformed attached plan digest",
                    attachment.run_id
                ))
            })?)
            .to_string(),
            proposed_plan: attachment.plan_digest.to_string(),
        });
    }
    writer.execute(
        "INSERT INTO run_context_plans
             (run_id, context_source_snapshot_digest, context_plan_digest, attached_at)
         VALUES (?1, ?2, ?3, unixepoch())",
        &[
            Value::from(attachment.run_id),
            Value::Blob(attachment.source_snapshot_digest.as_bytes().to_vec()),
            Value::Blob(attachment.plan_digest.as_bytes().to_vec()),
        ],
    )?;

    Ok(AttachedContextPlan {
        run_id: attachment.run_id.to_string(),
        context_plan_digest: *attachment.plan_digest,
    })
}

pub(crate) fn digest(bytes: &[u8], column: &str) -> Result<Digest, StoreError> {
    let array: [u8; 32] = bytes.try_into().map_err(|_| {
        StoreError::InvariantViolated(format!(
            "{column} is {} bytes, not a 32-byte digest",
            bytes.len()
        ))
    })?;
    Ok(Digest::from_bytes(array))
}

#[cfg(test)]
mod tests;
