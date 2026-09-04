//! Durable publication of a sealed `code.changeset`.
//!
//! This is the last step of the sealing order the Artifact contract fixes:
//!
//! ```text
//! ... CAS bytes durable  →  one authoritative SQLite transaction:
//!   re-read every mutable authority the seal depended on
//!   insert/reuse immutable rows
//!   append the Event through the command envelope
//!   COMMIT
//! ```
//!
//! Everything mutable is re-read *here*, inside the transaction, rather than
//! trusted from the caller's earlier observations: a Workspace frozen at
//! revision N may have been unfrozen, moved, or had its Task cancelled while
//! capture ran. The database constraints are the race-proof backstop behind
//! these typed checks, not a replacement for them — same as everywhere else
//! in this crate.
//!
//! Idempotence has two different meanings on this path and both are
//! implemented here rather than hoped for:
//!
//! - **replay**: the same command identity reaching this transaction again
//!   after a crash before commit replays from the ledger without running
//!   the mutation at all (`Store::execute_command`);
//! - **convergence**: a *different* command capturing identical state
//!   computes the same content digests and finds the existing
//!   blob/artifact/workspace-revision rows, which it verifies against what
//!   it computed instead of overwriting. Two captures of one state produce
//!   one content identity and two command records, never two contradictory
//!   immutable rows.

use pantheon_core::artifact::CODE_CHANGESET_KIND;
use pantheon_core::config::Digest;
use pantheon_core::workspace::WorkspacePhase;

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::seal::{SealAuthority, validate_seal_authority};
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

/// The authenticated producer context an Agent Control seal carries into
/// publication.
///
/// Content identity stays separate from production provenance: the Artifact
/// row records *what* exists, this records *which execution lineage produced
/// it, where*. The same Artifact digest may legitimately carry many
/// ProductionRecords from different Runs; one Run produces at most one per
/// output slot, which is what Candidate submission later resolves ownership
/// through.
#[derive(Debug)]
pub struct ProducerProvenance<'a> {
    /// The Attempt whose AgentControlSession requested the seal. Constrained
    /// holder-safely to belong to the sealing Run.
    pub attempt_id: &'a str,
}

/// Everything the final publication transaction needs to know.
///
/// Every field was computed *outside* any transaction — capture digests,
/// manifest digests — so the transaction treats them as claims to verify
/// against current authority, not facts.
#[derive(Debug)]
pub struct SealedChangeset<'a> {
    /// The Workspace whose state was captured.
    pub workspace_id: &'a str,
    /// The Task the Workspace must still belong to, still executing under
    /// the claimed Run authority.
    pub task_id: &'a str,
    /// The post-freeze Workspace revision the seal fence holds.
    pub fence_revision: Revision,
    /// The execution authority claimed for this seal, re-proven inside the
    /// publication transaction exactly as at the freeze boundary.
    pub authority: &'a SealAuthority,
    /// The TaskSpec output slot this seal produces, re-checked against the
    /// frozen specification inside the transaction.
    pub output_slot: &'a str,
    /// The repository identity recorded on the checkpoint.
    pub repository: &'a str,
    /// The resolved immutable base the before-state was derived from.
    pub resolved_base: &'a str,
    /// The immutable semantic-state digest of the captured logical tree.
    pub revision_state_digest: Digest,
    /// The canonical JSON document of that state.
    pub revision_state_json: &'a str,
    /// The digest of the canonical changeset manifest.
    pub artifact_digest: Digest,
    /// The canonical JSON manifest itself.
    pub artifact_json: &'a str,
    /// Every payload-bearing member: `(digest, size)` pairs already made
    /// durable in CAS before this transaction was attempted.
    pub members: Vec<(Digest, u64)>,
    /// The producer provenance to bind when the seal was requested through
    /// Agent Control. `None` keeps the pre-#33 controller-side behavior of
    /// publishing content without a ProductionRecord.
    pub producer: Option<ProducerProvenance<'a>>,
}

/// What the publication committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealOutcome {
    /// The workspace-revision checkpoint row this seal bound (created or
    /// reused).
    pub workspace_revision_id: String,
    /// Whether an Artifact with this exact manifest digest already existed.
    /// True means this command converged on prior content; false means it
    /// created the only row this manifest will ever have.
    pub artifact_reused: bool,
}

/// One stored Artifact row, read back by content identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub digest: Digest,
    pub kind: String,
    pub canonical_json: String,
}

impl Store {
    /// Publishes a sealed changeset: revalidates authority, inserts or
    /// reuses the immutable rows, appends the Event — all in the command
    /// envelope's single authoritative transaction.
    ///
    /// No filesystem, Git, process or CAS work happens here; the contract
    /// forbids external effects inside a transaction, and every byte this
    /// claims was already made durable before the call.
    ///
    /// Authority is re-read *inside this transaction*, independently of the
    /// validation that preceded filesystem capture: the claimed Run must
    /// still be the Task's current, nonterminal, revision-current
    /// responsible Run bound to exactly this Workspace at its immutable
    /// base, and the frozen specification must still permit a
    /// `code.changeset` on the requested slot. Validation performed before
    /// capture says nothing about now.
    ///
    /// # Errors
    ///
    /// [`StoreError::SealAuthorityInvalid`] when the Workspace is no longer
    /// frozen at `fence_revision`, no longer owned by `task_id`, or the
    /// claimed Run authority no longer holds (see the crate-private
    /// `validate_seal_authority`);
    /// [`StoreError::RevisionConflict`] when the Workspace row is gone
    /// outright; [`StoreError::ContentIdentityConflict`] when a computed
    /// digest names an existing row holding different content — corruption
    /// or a broken hash, never overwritten; plus the command envelope's
    /// failures. Nothing is written in any failure case.
    #[allow(clippy::too_many_lines)]
    pub fn commit_changeset_seal(
        &self,
        command: &Command<'_>,
        seal: &SealedChangeset<'_>,
    ) -> Result<Committed<SealOutcome>, StoreError> {
        self.execute_command(command, |writer| {
            // ---- Authority revalidation, before any write. ----

            let workspace = writer
                .query_optional(
                    "SELECT task_id, phase, revision FROM workspaces WHERE id = ?1",
                    &[Value::from(seal.workspace_id)],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )?
                .ok_or_else(|| StoreError::RevisionConflict {
                    table: "workspaces",
                    id: seal.workspace_id.to_string(),
                    expected: seal.fence_revision.get(),
                    actual: None,
                })?;
            let (owner, phase_text, revision) = workspace;
            if owner != seal.task_id {
                return writer.fail(StoreError::SealAuthorityInvalid {
                    workspace_id: seal.workspace_id.to_string(),
                    detail: format!("the workspace is owned by {owner}, not {}", seal.task_id),
                });
            }
            if phase_text != WorkspacePhase::Frozen.as_str() {
                return writer.fail(StoreError::SealAuthorityInvalid {
                    workspace_id: seal.workspace_id.to_string(),
                    detail: format!("the freeze did not hold: the workspace is {phase_text}"),
                });
            }
            if revision != seal.fence_revision.get() {
                return writer.fail(StoreError::SealAuthorityInvalid {
                    workspace_id: seal.workspace_id.to_string(),
                    detail: format!(
                        "the freeze moved: fenced at revision {}, found {revision}",
                        seal.fence_revision.get()
                    ),
                });
            }

            // The same Run facts validated before capture are re-proven here,
            // inside this publication's own transaction.
            validate_seal_authority(
                writer,
                seal.authority,
                seal.task_id,
                seal.output_slot,
                seal.workspace_id,
            )?;

            // ---- Immutable rows: reuse verified, create once. ----

            for (digest, size) in &seal.members {
                ensure_blob(writer, digest, *size)?;
            }

            let revision_row = writer.query_optional(
                "SELECT id FROM workspace_revisions WHERE workspace_id = ?1 AND state_digest = ?2",
                &[
                    Value::from(seal.workspace_id),
                    Value::Blob(seal.revision_state_digest.as_bytes().to_vec()),
                ],
                |row| row.get::<_, String>(0),
            )?;
            let workspace_revision_id = match revision_row {
                Some(id) => id,
                None => {
                    let id = fresh_id(writer)?;
                    writer.execute(
                        "INSERT INTO workspace_revisions (
                             id, workspace_id, repository, resolved_base,
                             state_digest, canonical_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())",
                        &[
                            Value::from(id.as_str()),
                            Value::from(seal.workspace_id),
                            Value::from(seal.repository),
                            Value::from(seal.resolved_base),
                            Value::Blob(seal.revision_state_digest.as_bytes().to_vec()),
                            Value::from(seal.revision_state_json),
                        ],
                    )?;
                    id
                }
            };

            let existing_artifact = writer.query_optional(
                "SELECT artifact_kind, canonical_json FROM artifacts WHERE digest = ?1",
                &[Value::Blob(seal.artifact_digest.as_bytes().to_vec())],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            let artifact_reused = match existing_artifact {
                Some((kind, json)) => {
                    if kind != CODE_CHANGESET_KIND || json != seal.artifact_json {
                        return writer.fail(StoreError::ContentIdentityConflict {
                            table: "artifacts",
                            id: seal.artifact_digest.to_string(),
                            detail: "an artifact with this digest holds a different \
                                     manifest"
                                .to_string(),
                        });
                    }
                    true
                }
                None => {
                    writer.execute(
                        "INSERT INTO artifacts (digest, artifact_kind, canonical_json, created_at)
                         VALUES (?1, ?2, ?3, unixepoch())",
                        &[
                            Value::Blob(seal.artifact_digest.as_bytes().to_vec()),
                            Value::from(CODE_CHANGESET_KIND),
                            Value::from(seal.artifact_json),
                        ],
                    )?;
                    false
                }
            };

            for (digest, _) in &seal.members {
                writer.execute(
                    "INSERT OR IGNORE INTO artifact_members (artifact_digest, blob_digest)
                     VALUES (?1, ?2)",
                    &[
                        Value::Blob(seal.artifact_digest.as_bytes().to_vec()),
                        Value::Blob(digest.as_bytes().to_vec()),
                    ],
                )?;
            }

            // Production provenance binds the content to the execution
            // lineage that produced it — inside this same transaction, so a
            // published Artifact never exists without its record and a retry
            // converges instead of minting conflicting provenance.
            if let Some(producer) = &seal.producer {
                let existing: Option<(String, String, String, Vec<u8>)> = writer.query_optional(
                    "SELECT task_id, attempt_id, workspace_revision_id, artifact_digest
                     FROM production_records WHERE run_id = ?1 AND output_slot = ?2",
                    &[
                        Value::from(seal.authority.run_id.as_str()),
                        Value::from(seal.output_slot),
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
                let claimed = (
                    seal.task_id.to_string(),
                    producer.attempt_id.to_string(),
                    workspace_revision_id.clone(),
                    seal.artifact_digest.as_bytes().to_vec(),
                );
                match existing {
                    Some(recorded) => {
                        if recorded != claimed {
                            return writer.fail(StoreError::ContentIdentityConflict {
                                table: "production_records",
                                id: format!(
                                    "{}/{}/{}",
                                    seal.authority.run_id, seal.output_slot, seal.artifact_digest
                                ),
                                detail: "the run already produced different content \
                                         for this output slot"
                                    .to_string(),
                            });
                        }
                    }
                    None => {
                        writer.execute(
                            "INSERT INTO production_records
                                 (run_id, output_slot, task_id, attempt_id,
                                  workspace_revision_id, artifact_digest, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())",
                            &[
                                Value::from(seal.authority.run_id.as_str()),
                                Value::from(seal.output_slot),
                                Value::from(seal.task_id),
                                Value::from(producer.attempt_id),
                                Value::from(workspace_revision_id.as_str()),
                                Value::Blob(seal.artifact_digest.as_bytes().to_vec()),
                            ],
                        )?;
                    }
                }
            }

            Ok(SealOutcome {
                workspace_revision_id,
                artifact_reused,
            })
        })
    }

    /// One stored Artifact by its content digest.
    ///
    /// The read half of replay handling: a sealed command that returns
    /// [`Committed::Replayed`] carries no value, so the controller confirms
    /// the seal by looking up the manifest digest it computed itself.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when a stored row cannot be
    /// interpreted; [`StoreError::Sqlite`] on a storage failure.
    pub fn artifact(&self, digest: Digest) -> Result<Option<ArtifactRecord>, StoreError> {
        self.read(|conn| {
            conn.query_row(
                "SELECT artifact_kind, canonical_json FROM artifacts WHERE digest = ?1",
                rusqlite::params![digest.as_bytes().to_vec()],
                |row| {
                    Ok(ArtifactRecord {
                        digest,
                        kind: row.get(0)?,
                        canonical_json: row.get(1)?,
                    })
                },
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Sqlite(other)),
            })
        })
    }
}

/// Makes one Blob row exist with exactly the claimed size.
fn ensure_blob(writer: &Writer<'_>, digest: &Digest, size: u64) -> Result<(), StoreError> {
    let stored_size = i64::try_from(size).map_err(|_| {
        StoreError::InvariantViolated(format!("blob size {size} exceeds what SQLite can store"))
    })?;
    let existing: Option<i64> = writer.query_optional(
        "SELECT size FROM blobs WHERE digest = ?1",
        &[Value::Blob(digest.as_bytes().to_vec())],
        |row| row.get(0),
    )?;
    match existing {
        Some(stored) => {
            if stored != stored_size {
                return writer.fail(StoreError::ContentIdentityConflict {
                    table: "blobs",
                    id: digest.to_string(),
                    detail: format!("stored size {stored} disagrees with published {size}"),
                });
            }
            Ok(())
        }
        None => {
            writer.execute(
                "INSERT INTO blobs (digest, size, created_at) VALUES (?1, ?2, unixepoch())",
                &[
                    Value::Blob(digest.as_bytes().to_vec()),
                    Value::Integer(stored_size),
                ],
            )?;
            Ok(())
        }
    }
}

/// A fresh opaque row identity, drawn like Event ids from SQLite's OS-seeded
/// randomblob inside the same transaction that uses it.
fn fresh_id(writer: &Writer<'_>) -> Result<String, StoreError> {
    writer
        .query_optional("SELECT lower(hex(randomblob(16)))", &[], |row| row.get(0))?
        .ok_or_else(|| StoreError::InvariantViolated("could not generate an id".to_string()))
}

#[cfg(test)]
mod tests;
