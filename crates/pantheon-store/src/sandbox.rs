//! Durable Run-owned Sandbox authority.
//!
//! `docs/architecture/security/sandbox-broker-and-isolation.md` is canonical
//! for what a Sandbox is; this module owns only how one is stored. Each
//! authoritative mutation is a single call to [`Store::execute_command`],
//! inheriting the one authoritative transaction, durable command outcome and
//! Event append.
//!
//! # What the database refuses, rather than this code
//!
//! Three invariants are `CREATE TABLE`/`CREATE INDEX` constraints in
//! migration 15:
//!
//! - `phase = 'Ready'` requires `observed_presence = 'Present'`;
//! - `phase = 'Requested'` requires `observed_presence = 'Absent'`;
//! - `phase = 'Released'` requires `observed_presence = 'Absent'`;
//! - `sandbox_one_current_per_run` is a partial unique index, so a Run cannot
//!   acquire a second non-`Released` Sandbox even if two callers race.

use pantheon_core::sandbox::{SandboxKey, SandboxPhase, SandboxPresence};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

const TABLE: &str = "sandbox_instances";

/// One durable SandboxInstance row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRecord {
    pub id: String,
    pub run_id: String,
    pub sandbox_plan_digest: [u8; 32],
    pub environment_identity: String,
    pub phase: SandboxPhase,
    pub observed_presence: SandboxPresence,
    pub revision: Revision,
}

/// What a Sandbox is durably bound to at creation.
#[derive(Debug, Clone, Copy)]
pub struct SandboxBinding<'a> {
    pub run_id: &'a str,
    pub sandbox_plan_digest: &'a [u8; 32],
    pub environment_identity: &'a str,
}

/// One controller-owned probe evidence record to persist.
#[derive(Debug, Clone, Copy)]
pub struct SandboxProbeEvidence<'a> {
    pub sandbox_id: &'a str,
    pub probe_name: &'a str,
    pub expected: &'a str,
    pub observed: &'a str,
    pub passed: bool,
    pub backend_descriptor: &'a str,
    pub backend_version: &'a str,
    pub platform: &'a str,
    pub architecture: &'a str,
    pub probe_implementation_version: &'a str,
}

/// One durable row from `sandbox_probe_results`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProbeRecord {
    pub id: i64,
    pub sandbox_id: String,
    pub probe_name: String,
    pub expected: String,
    pub observed: String,
    pub passed: bool,
    pub backend_descriptor: String,
    pub backend_version: String,
    pub platform: String,
    pub architecture: String,
    pub probe_implementation_version: String,
    pub recorded_at: i64,
}

impl Store {
    /// Commits durable Sandbox identity and intention for a Run, before any
    /// container-runtime side effect.
    ///
    /// # Errors
    ///
    /// [`StoreError::SandboxAlreadyCurrent`] when the Run already owns one;
    /// plus SQLite foreign-key violation when the Run does not exist, and the
    /// command envelope's stale-epoch and conflict failures.
    pub fn create_sandbox(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        binding: &SandboxBinding<'_>,
    ) -> Result<Committed<SandboxRecord>, StoreError> {
        self.execute_command(command, |writer| {
            if let Some(existing) = current_sandbox_id(writer, binding.run_id)? {
                return writer.fail(StoreError::SandboxAlreadyCurrent {
                    run_id: binding.run_id.to_string(),
                    sandbox_id: existing,
                });
            }

            let now = now(writer)?;
            writer.execute(
                "INSERT INTO sandbox_instances (
                     id, run_id, sandbox_plan_digest, environment_identity,
                     phase, observed_presence, revision, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
                &[
                    Value::from(sandbox_key.as_str()),
                    Value::from(binding.run_id),
                    Value::Blob(binding.sandbox_plan_digest.to_vec()),
                    Value::from(binding.environment_identity),
                    Value::from(SandboxPhase::Requested.as_str()),
                    Value::from(SandboxPresence::Absent.as_str()),
                    Value::Integer(now),
                ],
            )?;

            Ok(SandboxRecord {
                id: sandbox_key.as_str().to_string(),
                run_id: binding.run_id.to_string(),
                sandbox_plan_digest: *binding.sandbox_plan_digest,
                environment_identity: binding.environment_identity.to_string(),
                phase: SandboxPhase::Requested,
                observed_presence: SandboxPresence::Absent,
                revision: Revision::new(1),
            })
        })
    }

    /// Records that provisioning is authorized and external state may now
    /// exist.
    ///
    /// This must commit *before* the first container-runtime side effect.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionConflict`] when the Sandbox does not exist or
    /// has moved; plus the command envelope's failures.
    pub fn begin_sandbox_preparation(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
    ) -> Result<Committed<SandboxRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let phase = sandbox_phase(writer, sandbox_key.as_str(), expected)?;
            if phase != SandboxPhase::Requested {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "sandbox {} is {phase} and cannot become Preparing",
                    sandbox_key.as_str()
                )));
            }
            transition(
                writer,
                sandbox_key.as_str(),
                expected,
                SandboxPhase::Preparing,
                SandboxPresence::Unknown,
            )
        })
    }

    /// Records verified provisioning: the Sandbox becomes `Ready`.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when the Sandbox is not `Preparing`;
    /// [`StoreError::RevisionConflict`] when it has moved; plus the command
    /// envelope's failures.
    pub fn complete_sandbox_preparation(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
    ) -> Result<Committed<SandboxRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let phase = sandbox_phase(writer, sandbox_key.as_str(), expected)?;
            if phase != SandboxPhase::Preparing {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "sandbox {} is {phase} and cannot become Ready",
                    sandbox_key.as_str()
                )));
            }
            transition(
                writer,
                sandbox_key.as_str(),
                expected,
                SandboxPhase::Ready,
                SandboxPresence::Present,
            )
        })
    }

    /// Records that release is authorized.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when the Sandbox is not `Ready`;
    /// [`StoreError::RevisionConflict`] when it has moved; plus the command
    /// envelope's failures.
    pub fn begin_sandbox_release(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
    ) -> Result<Committed<SandboxRecord>, StoreError> {
        self.execute_command(command, |writer| {
            let phase = sandbox_phase(writer, sandbox_key.as_str(), expected)?;
            if phase != SandboxPhase::Ready {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "sandbox {} is {phase} and cannot become Releasing",
                    sandbox_key.as_str()
                )));
            }
            transition(
                writer,
                sandbox_key.as_str(),
                expected,
                SandboxPhase::Releasing,
                SandboxPresence::Unknown,
            )
        })
    }

    /// Records established external absence and frees the holder slot.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when `observed` is `Present`;
    /// [`StoreError::RevisionConflict`] when the Sandbox has moved; plus the
    /// command envelope's failures.
    pub fn complete_sandbox_release(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
        observed: SandboxPresence,
    ) -> Result<Committed<SandboxRecord>, StoreError> {
        self.execute_command(command, |writer| {
            if observed == SandboxPresence::Present {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "sandbox {} cannot record Present on a release path",
                    sandbox_key.as_str()
                )));
            }
            let phase = sandbox_phase(writer, sandbox_key.as_str(), expected)?;
            if phase != SandboxPhase::Releasing {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "sandbox {} is {phase} and cannot become Released",
                    sandbox_key.as_str()
                )));
            }
            transition(
                writer,
                sandbox_key.as_str(),
                expected,
                SandboxPhase::Released,
                observed,
            )
        })
    }

    /// Records a failed provisioning or release together with what is
    /// actually known about external state.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] when `observed` is `Present`;
    /// [`StoreError::RevisionConflict`] when the Sandbox has moved; plus the
    /// command envelope's failures.
    pub fn fail_sandbox(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
        observed: SandboxPresence,
    ) -> Result<Committed<SandboxRecord>, StoreError> {
        self.execute_command(command, |writer| {
            if observed == SandboxPresence::Present {
                return writer.fail(StoreError::InvariantViolated(format!(
                    "sandbox {} cannot record Present on a failure path",
                    sandbox_key.as_str()
                )));
            }
            transition(
                writer,
                sandbox_key.as_str(),
                expected,
                SandboxPhase::Error,
                observed,
            )
        })
    }

    /// Updates the observed presence without changing lifecycle phase.
    ///
    /// Used by reconciliation when the external runtime is inspected.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionConflict`] when the Sandbox has moved; plus the
    /// command envelope's failures.
    pub fn update_sandbox_presence(
        &self,
        command: &Command<'_>,
        sandbox_key: &SandboxKey,
        expected: Revision,
        presence: SandboxPresence,
    ) -> Result<Committed<SandboxRecord>, StoreError> {
        self.execute_command(command, |writer| {
            transition_presence(writer, sandbox_key.as_str(), expected, presence)
        })
    }

    /// The Run's current Sandbox, if it owns one.
    ///
    /// "Current" means any phase other than `Released`, matching the partial
    /// unique index that makes at most one such row possible.
    pub fn sandbox_for_run(&self, run_id: &str) -> Result<Option<SandboxRecord>, StoreError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, sandbox_plan_digest, environment_identity,
                            phase, observed_presence, revision
                     FROM sandbox_instances WHERE run_id = ?1 AND phase != ?2",
                    rusqlite::params![run_id, SandboxPhase::Released.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;

            row.map(|(id, digest, env, phase, presence, revision)| {
                let digest: [u8; 32] = digest.try_into().map_err(|_| {
                    StoreError::InvariantViolated(format!(
                        "sandbox {id} has a malformed plan digest"
                    ))
                })?;
                Ok(SandboxRecord {
                    id,
                    run_id: run_id.to_string(),
                    sandbox_plan_digest: digest,
                    environment_identity: env,
                    phase: SandboxPhase::parse(&phase).ok_or_else(|| {
                        StoreError::InvariantViolated(format!(
                            "sandbox {run_id} has unknown phase {phase}"
                        ))
                    })?,
                    observed_presence: SandboxPresence::parse(&presence).ok_or_else(|| {
                        StoreError::InvariantViolated(format!(
                            "sandbox {run_id} has unknown presence {presence}"
                        ))
                    })?,
                    revision: Revision::new(revision),
                })
            })
            .transpose()
        })
    }

    /// One Sandbox by its durable id, regardless of phase.
    pub fn sandbox_by_id(&self, sandbox_id: &str) -> Result<Option<SandboxRecord>, StoreError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT run_id, sandbox_plan_digest, environment_identity,
                            phase, observed_presence, revision
                     FROM sandbox_instances WHERE id = ?1",
                    rusqlite::params![sandbox_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })?;

            row.map(|(run_id, digest, env, phase, presence, revision)| {
                let digest: [u8; 32] = digest.try_into().map_err(|_| {
                    StoreError::InvariantViolated(format!(
                        "sandbox {sandbox_id} has a malformed plan digest"
                    ))
                })?;
                Ok(SandboxRecord {
                    id: sandbox_id.to_string(),
                    run_id,
                    sandbox_plan_digest: digest,
                    environment_identity: env,
                    phase: SandboxPhase::parse(&phase).ok_or_else(|| {
                        StoreError::InvariantViolated(format!(
                            "sandbox {sandbox_id} has unknown phase {phase}"
                        ))
                    })?,
                    observed_presence: SandboxPresence::parse(&presence).ok_or_else(|| {
                        StoreError::InvariantViolated(format!(
                            "sandbox {sandbox_id} has unknown presence {presence}"
                        ))
                    })?,
                    revision: Revision::new(revision),
                })
            })
            .transpose()
        })
    }

    /// Every Sandbox whose phase is not `Released`.
    ///
    /// Used by recovery to inventory external obligations.
    pub fn nonreleased_sandbox_inventory(&self) -> Result<Vec<SandboxRecord>, StoreError> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, run_id, sandbox_plan_digest, environment_identity,
                        phase, observed_presence, revision
                 FROM sandbox_instances WHERE phase != ?1",
            )?;
            let rows =
                stmt.query_map(rusqlite::params![SandboxPhase::Released.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                })?;

            let mut out = Vec::new();
            for row in rows {
                let (id, run_id, digest, env, phase, presence, revision) = row?;
                let digest: [u8; 32] = digest.try_into().map_err(|_| {
                    StoreError::InvariantViolated(format!(
                        "sandbox {id} has a malformed plan digest"
                    ))
                })?;
                out.push(SandboxRecord {
                    id: id.clone(),
                    run_id,
                    sandbox_plan_digest: digest,
                    environment_identity: env,
                    phase: SandboxPhase::parse(&phase).ok_or_else(|| {
                        StoreError::InvariantViolated(format!(
                            "sandbox {id} has unknown phase {phase}"
                        ))
                    })?,
                    observed_presence: SandboxPresence::parse(&presence).ok_or_else(|| {
                        StoreError::InvariantViolated(format!(
                            "sandbox {id} has unknown presence {presence}"
                        ))
                    })?,
                    revision: Revision::new(revision),
                });
            }
            Ok(out)
        })
    }

    /// Records one controller-owned probe evidence row durably.
    ///
    /// Appends an Event and commits in the same authoritative transaction.
    pub fn record_sandbox_probe_evidence(
        &self,
        command: &Command<'_>,
        evidence: &SandboxProbeEvidence<'_>,
    ) -> Result<Committed<()>, StoreError> {
        self.execute_command(command, |writer| {
            let recorded_at: i64 = writer
                .query_optional("SELECT unixepoch()", &[], |row| row.get(0))?
                .ok_or_else(|| {
                    StoreError::InvariantViolated("could not read the current time".to_string())
                })?;
            writer.execute(
                "INSERT INTO sandbox_probe_results (
                     sandbox_id, probe_name, expected, observed, passed,
                     backend_descriptor, backend_version, platform, architecture,
                     probe_implementation_version, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                &[
                    Value::from(evidence.sandbox_id),
                    Value::from(evidence.probe_name),
                    Value::from(evidence.expected),
                    Value::from(evidence.observed),
                    Value::Integer(i64::from(evidence.passed)),
                    Value::from(evidence.backend_descriptor),
                    Value::from(evidence.backend_version),
                    Value::from(evidence.platform),
                    Value::from(evidence.architecture),
                    Value::from(evidence.probe_implementation_version),
                    Value::Integer(recorded_at),
                ],
            )?;
            Ok(())
        })
    }

    /// Every probe evidence row for a given Sandbox, oldest first.
    pub fn sandbox_probe_results(
        &self,
        sandbox_id: &str,
    ) -> Result<Vec<SandboxProbeRecord>, StoreError> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, sandbox_id, probe_name, expected, observed, passed,
                        backend_descriptor, backend_version, platform, architecture,
                        probe_implementation_version, recorded_at
                 FROM sandbox_probe_results
                 WHERE sandbox_id = ?1
                 ORDER BY recorded_at ASC",
            )?;
            let rows = stmt.query_map(rusqlite::params![sandbox_id], |row| {
                Ok(SandboxProbeRecord {
                    id: row.get(0)?,
                    sandbox_id: row.get(1)?,
                    probe_name: row.get(2)?,
                    expected: row.get(3)?,
                    observed: row.get(4)?,
                    passed: row.get::<_, i64>(5)? != 0,
                    backend_descriptor: row.get(6)?,
                    backend_version: row.get(7)?,
                    platform: row.get(8)?,
                    architecture: row.get(9)?,
                    probe_implementation_version: row.get(10)?,
                    recorded_at: row.get(11)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }
}

fn current_sandbox_id(writer: &Writer<'_>, run_id: &str) -> Result<Option<String>, StoreError> {
    writer.query_optional(
        "SELECT id FROM sandbox_instances WHERE run_id = ?1 AND phase != ?2",
        &[
            Value::from(run_id),
            Value::from(SandboxPhase::Released.as_str()),
        ],
        |row| row.get(0),
    )
}

fn sandbox_phase(
    writer: &Writer<'_>,
    sandbox_id: &str,
    expected: Revision,
) -> Result<SandboxPhase, StoreError> {
    let row: Option<(String, i64)> = writer.query_optional(
        "SELECT phase, revision FROM sandbox_instances WHERE id = ?1",
        &[Value::from(sandbox_id)],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;

    let Some((phase, revision)) = row else {
        return writer.fail(StoreError::RevisionConflict {
            table: TABLE,
            id: sandbox_id.to_string(),
            expected: expected.get(),
            actual: None,
        });
    };
    if revision != expected.get() {
        return writer.fail(StoreError::RevisionConflict {
            table: TABLE,
            id: sandbox_id.to_string(),
            expected: expected.get(),
            actual: Some(revision),
        });
    }

    SandboxPhase::parse(&phase).ok_or_else(|| {
        StoreError::InvariantViolated(format!("sandbox {sandbox_id} has unknown phase {phase}"))
    })
}

fn transition(
    writer: &Writer<'_>,
    sandbox_id: &str,
    expected: Revision,
    phase: SandboxPhase,
    presence: SandboxPresence,
) -> Result<SandboxRecord, StoreError> {
    let revision = writer.update_revisioned(
        TABLE,
        sandbox_id,
        expected,
        &[
            ("phase", Value::from(phase.as_str())),
            ("observed_presence", Value::from(presence.as_str())),
        ],
    )?;
    let mut record = read_in_transaction(writer, sandbox_id)?;
    record.revision = revision;
    Ok(record)
}

fn transition_presence(
    writer: &Writer<'_>,
    sandbox_id: &str,
    expected: Revision,
    presence: SandboxPresence,
) -> Result<SandboxRecord, StoreError> {
    let revision = writer.update_revisioned(
        TABLE,
        sandbox_id,
        expected,
        &[("observed_presence", Value::from(presence.as_str()))],
    )?;
    let mut record = read_in_transaction(writer, sandbox_id)?;
    record.revision = revision;
    Ok(record)
}

fn read_in_transaction(writer: &Writer<'_>, sandbox_id: &str) -> Result<SandboxRecord, StoreError> {
    let row = writer
        .query_optional(
            "SELECT run_id, sandbox_plan_digest, environment_identity,
                    phase, observed_presence, revision
             FROM sandbox_instances WHERE id = ?1",
            &[Value::from(sandbox_id)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated(format!(
                "sandbox {sandbox_id} disappeared inside its own transaction"
            ))
        })?;

    let (run_id, digest, env, phase, presence, revision) = row;
    let digest: [u8; 32] = digest.try_into().map_err(|_| {
        StoreError::InvariantViolated(format!("sandbox {sandbox_id} has a malformed plan digest"))
    })?;
    Ok(SandboxRecord {
        id: sandbox_id.to_string(),
        run_id,
        sandbox_plan_digest: digest,
        environment_identity: env,
        phase: SandboxPhase::parse(&phase).ok_or_else(|| {
            StoreError::InvariantViolated(format!("sandbox {sandbox_id} has unknown phase {phase}"))
        })?,
        observed_presence: SandboxPresence::parse(&presence).ok_or_else(|| {
            StoreError::InvariantViolated(format!(
                "sandbox {sandbox_id} has unknown presence {presence}"
            ))
        })?,
        revision: Revision::new(revision),
    })
}

fn now(writer: &Writer<'_>) -> Result<i64, StoreError> {
    writer
        .query_optional("SELECT unixepoch()", &[], |row| row.get::<_, i64>(0))?
        .ok_or_else(|| StoreError::InvariantViolated("could not read the current time".to_string()))
}

#[cfg(test)]
mod tests;
