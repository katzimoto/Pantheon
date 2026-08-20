//! Durable configuration authority: immutable revisions and the active pointer.
//!
//! `docs/architecture/operations/configuration-and-policy-revisions.md` §9
//! specifies activation as one transaction:
//!
//! ```text
//! insert immutable revision/components if needed
//! update active_configuration pointer
//! append ConfigurationActivated event
//! ```
//!
//! [`Store::activate_configuration`] is exactly that, and it gets the shape for
//! free by running inside [`Store::execute_command`]: the command envelope from
//! Issue #18 already commits the authoritative mutation, the durable command
//! outcome and the Event together, so configuration does not need — and must
//! not have — a second write path.
//!
//! Moving the pointer is an ordinary revisioned CAS through the Issue #17
//! primitive rather than a bespoke compare. That is why `active_configuration`
//! carries a `revision` column: a caller that observed the pointer and then
//! activated against a stale observation loses deterministically, with the
//! typed conflict every other revisioned mutation produces.

use pantheon_core::config::revision::COMPILER_VERSION;
use pantheon_core::config::{CompiledConfiguration, ComponentDigests, Digest};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::store::Store;
use crate::transaction::{Revision, Value, Writer};

/// The identity of the active configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveConfiguration {
    /// Historical activation identity. Monotonic; re-activating identical
    /// content produces a later sequence.
    pub activation_sequence: i64,
    /// Semantic content identity.
    pub content_digest: Digest,
    /// The per-domain digests an immutable decision may bind.
    pub components: ComponentDigests,
    /// Provenance of the source set this revision was compiled from.
    pub source_set_digest: Digest,
    /// The pointer row's revision, for the next activation's CAS.
    pub pointer_revision: Revision,
}

/// The pointer state of an installation with no configuration yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationPointer {
    pub active: Option<ActiveConfiguration>,
    /// The pointer revision, present whether or not anything is active.
    pub revision: Revision,
}

const POINTER_ID: &str = "singleton";

impl Store {
    /// Reads the durable active configuration.
    ///
    /// Returns a pointer whose `active` is `None` on a fresh installation that
    /// has not yet activated a revision — the state in which the daemon is not
    /// ready for authority-bearing work.
    pub fn configuration_pointer(&self) -> Result<ConfigurationPointer, StoreError> {
        self.read(|conn| {
            let (revision, sequence): (i64, Option<i64>) = conn.query_row(
                "SELECT revision, activation_sequence FROM active_configuration WHERE id = ?1",
                rusqlite::params![POINTER_ID],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let revision = Revision::new(revision);
            let Some(sequence) = sequence else {
                return Ok(ConfigurationPointer {
                    active: None,
                    revision,
                });
            };
            let active = read_revision_row(conn, sequence, revision)?;
            Ok(ConfigurationPointer {
                active: Some(active),
                revision,
            })
        })
    }

    /// Activates `compiled` as the new authoritative configuration.
    ///
    /// The whole activation — component inserts, the immutable revision row,
    /// the pointer CAS, the durable command outcome and the activation Event —
    /// commits in one authoritative transaction, so no component can become
    /// active independently and a failure leaves the previous revision
    /// completely authoritative.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionConflict`] when `expected_pointer_revision` is not
    /// the pointer's current revision, meaning another activation intervened;
    /// the stale/conflict and epoch failures of
    /// [`Store::execute_command`]; or a storage failure. In every case the
    /// active revision is unchanged.
    pub fn activate_configuration(
        &self,
        command: &Command<'_>,
        compiled: &CompiledConfiguration,
        source_set_digest: Digest,
        expected_pointer_revision: Revision,
    ) -> Result<Committed<ActiveConfiguration>, StoreError> {
        self.execute_command(command, |writer| {
            write_activation(
                writer,
                compiled,
                source_set_digest,
                expected_pointer_revision,
            )
        })
    }
}

fn write_activation(
    writer: &Writer<'_>,
    compiled: &CompiledConfiguration,
    source_set_digest: Digest,
    expected_pointer_revision: Revision,
) -> Result<ActiveConfiguration, StoreError> {
    // Components are content-addressed and immutable, so re-inserting one that
    // an earlier revision already stored is a no-op rather than a conflict.
    for (domain, digest, canonical_json) in compiled.component_records() {
        writer.execute(
            "INSERT INTO configuration_components (digest, domain, canonical_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (digest) DO NOTHING",
            &[
                Value::Blob(digest.as_bytes().to_vec()),
                Value::from(domain),
                Value::from(canonical_json),
            ],
        )?;
    }

    // Allocated inside this transaction on the one serialized writer, so no
    // second activation can claim the same sequence.
    let next_sequence = writer
        .query_optional(
            "SELECT COALESCE(MAX(activation_sequence), 0) + 1 FROM configuration_revisions",
            &[],
            |row| row.get::<_, i64>(0),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated("could not allocate an activation sequence".to_string())
        })?;

    let components = compiled.component_digests();
    let content_digest = compiled.revision_digest();
    let recorded_at = writer
        .query_optional("SELECT unixepoch()", &[], |row| row.get::<_, i64>(0))?
        .ok_or_else(|| {
            StoreError::InvariantViolated("could not read the current time".to_string())
        })?;

    writer.execute(
        "INSERT INTO configuration_revisions (
             activation_sequence, content_digest, compiler_version, source_set_digest,
             agents_digest, routing_digest, execution_profile_digest,
             evaluator_registry_digest, context_policy_digest, authorization_digest,
             recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        &[
            Value::Integer(next_sequence),
            Value::Blob(content_digest.as_bytes().to_vec()),
            Value::from(COMPILER_VERSION),
            Value::Blob(source_set_digest.as_bytes().to_vec()),
            Value::Blob(components.agents.as_bytes().to_vec()),
            Value::Blob(components.routing.as_bytes().to_vec()),
            Value::Blob(components.execution_profile.as_bytes().to_vec()),
            Value::Blob(components.evaluator_registry.as_bytes().to_vec()),
            Value::Blob(components.context_policy.as_bytes().to_vec()),
            Value::Blob(components.authorization.as_bytes().to_vec()),
            Value::Integer(recorded_at),
        ],
    )?;

    // The pointer move is the moment the revision becomes authority, and it is
    // the same CAS every other revisioned mutation uses.
    let pointer_revision = writer.update_revisioned(
        "active_configuration",
        POINTER_ID,
        expected_pointer_revision,
        &[("activation_sequence", Value::Integer(next_sequence))],
    )?;

    Ok(ActiveConfiguration {
        activation_sequence: next_sequence,
        content_digest,
        components,
        source_set_digest,
        pointer_revision,
    })
}

fn read_revision_row(
    conn: &rusqlite::Connection,
    sequence: i64,
    pointer_revision: Revision,
) -> Result<ActiveConfiguration, StoreError> {
    let row = conn.query_row(
        "SELECT content_digest, source_set_digest, compiler_version, agents_digest, routing_digest,
                execution_profile_digest, evaluator_registry_digest,
                context_policy_digest, authorization_digest
         FROM configuration_revisions WHERE activation_sequence = ?1",
        rusqlite::params![sequence],
        |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Vec<u8>>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, Vec<u8>>(7)?,
                row.get::<_, Vec<u8>>(8)?,
            ))
        },
    );
    let row = match row {
        Ok(row) => row,
        // The pointer names a revision that is not there. Fail closed rather
        // than serve work against a configuration Pantheon cannot read.
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(StoreError::InvariantViolated(format!(
                "active configuration points at revision {sequence}, which does not exist"
            )));
        }
        Err(err) => return Err(StoreError::Sqlite(err)),
    };

    let components = ComponentDigests {
        agents: digest(&row.3, "agents_digest")?,
        routing: digest(&row.4, "routing_digest")?,
        execution_profile: digest(&row.5, "execution_profile_digest")?,
        evaluator_registry: digest(&row.6, "evaluator_registry_digest")?,
        context_policy: digest(&row.7, "context_policy_digest")?,
        authorization: digest(&row.8, "authorization_digest")?,
    };
    let content_digest = digest(&row.0, "content_digest")?;
    let compiler_version = row.2;

    // Verifying each component against its own digest column proves the
    // payloads are intact, but not that the manifest as a whole is the
    // revision it claims to be. Recomputing the revision identity from those
    // component digests is what closes that gap: without it, a `content_digest`
    // swapped to another valid revision would let a caller recover *that*
    // revision's semantics while the durable component bindings still belong
    // to this one — exactly the mixed generation the contract forbids.
    let recomputed = components.revision_digest(&compiler_version);
    if recomputed != content_digest {
        return Err(StoreError::InvariantViolated(format!(
            "revision {sequence} names content digest {content_digest}, but its components \
             under compiler {compiler_version} produce {recomputed}"
        )));
    }

    // The configuration contract requires startup to verify the component
    // hashes, not merely to read them. Each stored component is re-digested
    // from its canonical bytes and checked against the digest the revision
    // names, so a damaged or swapped payload fails closed instead of being
    // served as authority.
    for (domain, expected) in [
        ("agents", components.agents),
        ("routing", components.routing),
        ("executionProfiles", components.execution_profile),
        ("evaluators", components.evaluator_registry),
        ("context", components.context_policy),
        ("authorization", components.authorization),
    ] {
        verify_component(conn, domain, expected)?;
    }

    Ok(ActiveConfiguration {
        activation_sequence: sequence,
        content_digest,
        source_set_digest: digest(&row.1, "source_set_digest")?,
        components,
        pointer_revision,
    })
}

/// Re-digests one stored component and checks it against what the revision
/// names, including that it is stored under the domain it is used as.
fn verify_component(
    conn: &rusqlite::Connection,
    domain: &str,
    expected: Digest,
) -> Result<(), StoreError> {
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT domain, canonical_json FROM configuration_components WHERE digest = ?1",
            rusqlite::params![expected.as_bytes().to_vec()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(StoreError::Sqlite(other)),
        })?;

    let Some((stored_domain, canonical_json)) = found else {
        return Err(StoreError::InvariantViolated(format!(
            "active configuration names {domain} component {expected}, which is not stored"
        )));
    };

    if stored_domain != domain {
        return Err(StoreError::InvariantViolated(format!(
            "component {expected} is stored as {stored_domain}, but the revision uses it as {domain}"
        )));
    }

    // The check that actually catches a tampered or corrupted payload: the
    // bytes must still hash to the identity the revision was built on.
    let recomputed = Digest::of(canonical_json.as_bytes());
    if recomputed != expected {
        return Err(StoreError::InvariantViolated(format!(
            "{domain} component content hashes to {recomputed}, not the {expected} the active revision names"
        )));
    }

    Ok(())
}

fn digest(bytes: &[u8], column: &str) -> Result<Digest, StoreError> {
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
