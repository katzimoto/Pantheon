//! The durable command mutation kernel.
//!
//! An operator mutation is not just a state change: it carries an identity
//! that must survive retries, daemon restarts, and disaster restore. This
//! module is the envelope that turns "mutate this state" into "execute this
//! command exactly once, durably, under an identity a restored snapshot
//! cannot silently recycle".
//!
//! The contract is
//! `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
//! ("Commands", "Event Journal") together with
//! `docs/architecture/operations/event-and-observability-model.md` ("Durable
//! Event Journal", "Transactional event rule", "Journal epoch and
//! sequence"). Three rules from those documents shape everything here:
//!
//! 1. Command identity is `(commandEpoch, commandId)`, where `commandEpoch`
//!    **is** the installation's current RestoreGeneration.
//! 2. The epoch is compared against the current RestoreGeneration *before*
//!    the command ledger is consulted. A restored snapshot may have lost the
//!    row for a command that already produced an effect, so row absence can
//!    never make an old-epoch request new.
//! 3. The authoritative state mutation, the durable command outcome, the
//!    Event append and the journal sequence allocation commit together or
//!    not at all.
//!
//! # One transaction, structurally
//!
//! [`crate::Store::execute_command`] calls [`crate::Store::write`] exactly
//! once and runs this entire envelope inside that closure. There is no
//! second transaction to get wrong: [`crate::Writer`] exposes no way to
//! commit, and the whole envelope either reaches the single commit at the
//! end of `Store::write` or is rolled back with everything else.
//!
//! Every call takes the authoritative transaction, including a replay that
//! writes nothing. That is deliberate: the epoch fence and the ledger
//! decision must be read from the transaction's own snapshot, not from a
//! pre-transaction one. The accepted cost is that duplicate retries serialize
//! behind the single writer.
//!
//! # Why a replay cannot execute the caller's mutation
//!
//! The mutation is an `FnOnce` moved into [`execute`]. It is named only
//! inside the branch that has already established the command is new, and
//! the result type `T` exists only in [`Committed::Executed`] — a replay
//! has no `T` to return, so the replay branch could not invoke and discard
//! the mutation even by mistake.

use crate::error::StoreError;
use crate::transaction::{Value, Writer};

/// The durable identity of one operator command, plus the Event it records.
///
/// Borrowed rather than owned: the caller already has these, and the store
/// keeps nothing beyond what it persists.
#[derive(Debug, Clone, Copy)]
pub struct Command<'a> {
    /// The `commandEpoch` the caller believes is current — that is, the
    /// RestoreGeneration it read before building the request.
    pub epoch: &'a str,
    /// Single-use within its epoch. The same text under a different epoch is
    /// a different command.
    pub id: &'a str,
    /// A non-sensitive digest of the request, computed by the caller.
    ///
    /// This crate never hashes anything and never sees a request body, which
    /// is what keeps request payloads and secret material out of the durable
    /// command ledger entirely.
    pub request_hash: &'a [u8; 32],
    /// The Event this command appends when it executes. Opaque to the store.
    pub event_type: &'a str,
}

/// A durable position in the Event Journal: the ordering cursor
/// `(journalEpoch, sequence)`.
///
/// `sequence` is ordering metadata within one journal history, not Event
/// identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCursor {
    epoch: String,
    sequence: i64,
}

impl JournalCursor {
    #[must_use]
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    #[must_use]
    pub const fn sequence(&self) -> i64 {
        self.sequence
    }
}

/// What a command committed.
///
/// [`Committed::Replayed`] deliberately carries no `T`: the durable ledger
/// records where the command's Event landed and when, not the value some
/// earlier process returned. Reconstructing a `T` would require a durable
/// result representation this mission does not define.
#[derive(Debug)]
pub enum Committed<T> {
    /// This command ID was new in this epoch. The mutation ran exactly once.
    Executed { value: T, cursor: JournalCursor },
    /// This exact identity and request hash was already committed. The
    /// mutation did not run again.
    Replayed {
        cursor: JournalCursor,
        recorded_at: i64,
    },
}

impl<T> Committed<T> {
    /// The journal position of this command's Event, however it resolved.
    #[must_use]
    pub const fn cursor(&self) -> &JournalCursor {
        match self {
            Self::Executed { cursor, .. } | Self::Replayed { cursor, .. } => cursor,
        }
    }

    /// Whether the mutation body ran on this call.
    #[must_use]
    pub const fn was_executed(&self) -> bool {
        matches!(self, Self::Executed { .. })
    }
}

/// The durable ledger row for one command identity.
struct LedgerEntry {
    request_hash: Vec<u8>,
    cursor: JournalCursor,
    recorded_at: i64,
}

/// Runs `mutation` under durable command identity inside the caller's
/// authoritative transaction.
///
/// See the module documentation for the ordering guarantees. In outline:
/// fence the epoch, classify the command, and only for a genuinely new
/// command run the mutation, allocate a journal sequence, append the Event
/// and record the durable outcome — all before the single commit.
pub(crate) fn execute<T>(
    writer: &Writer<'_>,
    command: &Command<'_>,
    mutation: impl FnOnce(&Writer<'_>) -> Result<T, StoreError>,
) -> Result<Committed<T>, StoreError> {
    // 1. The authority fence, first and unconditionally. Nothing about the
    //    command ledger is consulted until the epoch is known current, so no
    //    code path exists in which a missing row could make a stale request
    //    look new.
    fence_epoch(writer, command.epoch)?;

    // 2. Classify against the durable ledger, on this transaction's snapshot.
    if let Some(entry) = lookup(writer, command)? {
        if entry.request_hash.as_slice() != command.request_hash.as_slice() {
            // Same identity, different request. Fail closed without touching
            // the stored identity: a command ID is single-use, and rewriting
            // its hash would let a second request inherit the first's
            // authority.
            return writer.fail(StoreError::CommandConflict {
                command_id: command.id.to_string(),
            });
        }
        // Same identity, same request: reconcile the durable prior outcome.
        // `mutation` is never named on this path.
        return Ok(Committed::Replayed {
            cursor: entry.cursor,
            recorded_at: entry.recorded_at,
        });
    }

    // 3. Genuinely new. Only now does the caller's mutation run.
    let value = mutation(writer)?;

    let cursor = allocate_sequence(writer)?;
    let recorded_at = append_event(writer, command, &cursor)?;
    record_command(writer, command, &cursor, recorded_at)?;

    Ok(Committed::Executed { value, cursor })
}

/// Rejects any command whose epoch is not the installation's current
/// RestoreGeneration.
///
/// Read through the writer, so the comparison is against the same snapshot
/// the rest of the transaction acts on. Reading it from the store's
/// read-only connection would place this fence outside the transaction that
/// depends on it.
fn fence_epoch(writer: &Writer<'_>, supplied: &str) -> Result<(), StoreError> {
    let current: String = writer
        .query_optional(
            "SELECT restore_generation FROM system_state WHERE id = 1",
            &[],
            |row| row.get(0),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated(
                "system_state has no installation identity row".to_string(),
            )
        })?;

    if supplied == current {
        Ok(())
    } else {
        writer.fail(StoreError::StaleCommandEpoch {
            supplied: supplied.to_string(),
            current,
        })
    }
}

fn lookup(writer: &Writer<'_>, command: &Command<'_>) -> Result<Option<LedgerEntry>, StoreError> {
    writer.query_optional(
        "SELECT request_hash, journal_epoch, sequence, recorded_at
         FROM commands WHERE command_epoch = ?1 AND command_id = ?2",
        &[Value::from(command.epoch), Value::from(command.id)],
        |row| {
            Ok(LedgerEntry {
                request_hash: row.get(0)?,
                cursor: JournalCursor {
                    epoch: row.get(1)?,
                    sequence: row.get(2)?,
                },
                recorded_at: row.get(3)?,
            })
        },
    )
}

/// Advances the singleton next-sequence allocator and returns the cursor it
/// yielded.
///
/// The allocator is a mutable authoritative row that is deliberately not
/// revisioned: `journal_epochs` has neither an `id` nor a `revision` column,
/// and the row is never optimistically contended because only the one
/// serialized authoritative writer reaches it, inside `BEGIN IMMEDIATE`. The
/// compare-and-set shape is kept anyway — the `UPDATE` re-states the value
/// the `SELECT` returned — but note that its exactly-one-row check is
/// defensive rather than reachable: within one `BEGIN IMMEDIATE` transaction
/// on the only writer, nothing can change the row in between.
fn allocate_sequence(writer: &Writer<'_>) -> Result<JournalCursor, StoreError> {
    let (epoch, sequence) = writer
        .query_optional(
            "SELECT epoch, next_sequence FROM journal_epochs WHERE is_current = 1",
            &[],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?
        .ok_or_else(|| {
            StoreError::InvariantViolated("no current journal epoch exists".to_string())
        })?;

    let affected = writer.execute(
        "UPDATE journal_epochs SET next_sequence = next_sequence + 1
         WHERE is_current = 1 AND next_sequence = ?1",
        &[Value::Integer(sequence)],
    )?;
    if affected != 1 {
        return writer.fail(StoreError::InvariantViolated(format!(
            "journal sequence allocation affected {affected} rows, not exactly one"
        )));
    }

    Ok(JournalCursor { epoch, sequence })
}

/// Appends the command's Event, returning the time it was recorded.
fn append_event(
    writer: &Writer<'_>,
    command: &Command<'_>,
    cursor: &JournalCursor,
) -> Result<i64, StoreError> {
    // Drawn from SQLite's OS-seeded randomblob, like the installation
    // identity in migration 2. Event identity is separate from the ordering
    // sequence, per the Event model, and needs no new dependency.
    let event_id: String = writer
        .query_optional("SELECT lower(hex(randomblob(16)))", &[], |row| row.get(0))?
        .ok_or_else(|| {
            StoreError::InvariantViolated("could not generate an event id".to_string())
        })?;

    let recorded_at: i64 = writer
        .query_optional("SELECT unixepoch()", &[], |row| row.get(0))?
        .ok_or_else(|| {
            StoreError::InvariantViolated("could not read the current time".to_string())
        })?;

    let appended = writer.execute(
        "INSERT INTO event_journal
             (event_id, journal_epoch, sequence, event_type, recorded_at,
              command_epoch, command_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        &[
            Value::from(event_id),
            Value::from(cursor.epoch.as_str()),
            Value::Integer(cursor.sequence),
            Value::from(command.event_type),
            Value::Integer(recorded_at),
            Value::from(command.epoch),
            Value::from(command.id),
        ],
    );
    let affected = match appended {
        Ok(affected) => affected,
        // The allocator just handed out this slot, so finding it occupied
        // means the journal disagrees with its own allocator. That is a
        // violated invariant, not an ordinary storage failure, and saying so
        // keeps it distinguishable from a disk error.
        //
        // Matched on the *extended* code, not the primary one. Every
        // constraint on this table reports `ConstraintViolation`, so keying
        // on that alone would report a caller's over-long `event_type` — a
        // CHECK failure — as an occupied journal slot. Only the
        // `UNIQUE (journal_epoch, sequence)` violation means what this arm
        // claims; anything else stays an ordinary storage error.
        Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(inner, _)))
            if inner.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            return writer.fail(StoreError::InvariantViolated(format!(
                "journal slot ({}, {}) is already occupied",
                cursor.epoch, cursor.sequence
            )));
        }
        Err(other) => return Err(other),
    };
    debug_assert_eq!(affected, 1, "a single-row INSERT affects exactly one row");

    Ok(recorded_at)
}

fn record_command(
    writer: &Writer<'_>,
    command: &Command<'_>,
    cursor: &JournalCursor,
    recorded_at: i64,
) -> Result<(), StoreError> {
    let affected = writer.execute(
        "INSERT INTO commands
             (command_epoch, command_id, request_hash, journal_epoch, sequence, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        &[
            Value::from(command.epoch),
            Value::from(command.id),
            Value::Blob(command.request_hash.to_vec()),
            Value::from(cursor.epoch.as_str()),
            Value::Integer(cursor.sequence),
            Value::Integer(recorded_at),
        ],
    )?;
    debug_assert_eq!(affected, 1, "a single-row INSERT affects exactly one row");
    Ok(())
}

#[cfg(test)]
mod tests;
