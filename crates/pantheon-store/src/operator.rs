//! Read paths the Operator Control surface needs.
//!
//! Everything here is a query over authoritative state. The Event Journal is
//! append-only observation, so a watcher reads history from it while
//! controllers continue to read the authoritative tables — the journal never
//! becomes the source of truth for what a Goal *is*.
//!
//! # Why the snapshot carries a cursor
//!
//! A client that lists Goals and then starts watching from "now" loses every
//! Event committed between the two reads. [`Store::goal_snapshot`] therefore
//! reads the Goals and the journal head *in one read transaction*, so the
//! cursor it returns is the exact position the returned state corresponds to.
//! Watching after that cursor cannot skip an Event and cannot miss one.

use pantheon_core::planning::GoalPhase;

use crate::error::StoreError;
use crate::planning::GoalRecord;
use crate::store::Store;
use crate::transaction::Revision;

/// A durable position in the Event Journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub journal_epoch: String,
    /// The last sequence included in the corresponding snapshot. A watch
    /// starts strictly after this.
    pub sequence: i64,
}

impl Cursor {
    /// The wire spelling, `<epoch>:<sequence>`.
    #[must_use]
    pub fn to_wire(&self) -> String {
        format!("{}:{}", self.journal_epoch, self.sequence)
    }

    /// Parses the wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (epoch, sequence) = text.split_once(':')?;
        if epoch.is_empty() {
            return None;
        }
        Some(Self {
            journal_epoch: epoch.to_string(),
            sequence: sequence.parse().ok()?,
        })
    }
}

/// One durable Event, as history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub event_id: String,
    pub journal_epoch: String,
    pub sequence: i64,
    pub event_type: String,
    pub recorded_at: i64,
    pub command_epoch: Option<String>,
    pub command_id: Option<String>,
}

impl EventRecord {
    /// This Event's resumable cursor.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor {
            journal_epoch: self.journal_epoch.clone(),
            sequence: self.sequence,
        }
    }
}

/// A consistent view of Goals plus the journal position it corresponds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSnapshot {
    pub goals: Vec<GoalRecord>,
    pub cursor: Cursor,
}

/// Why a cursor cannot be resumed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    /// The cursor names a journal history this installation is not on.
    UnknownEpoch { supplied: String, current: String },
    /// The cursor is ahead of anything committed.
    AheadOfJournal { supplied: i64, head: i64 },
}

impl std::fmt::Display for CursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEpoch { supplied, current } => write!(
                f,
                "cursor names journal epoch {supplied}, but this installation is on {current}"
            ),
            Self::AheadOfJournal { supplied, head } => write!(
                f,
                "cursor sequence {supplied} is ahead of the journal head {head}"
            ),
        }
    }
}

impl Store {
    /// The current journal head.
    pub fn journal_head(&self) -> Result<Cursor, StoreError> {
        self.read(read_head)
    }

    /// Every Goal, with the journal cursor the list corresponds to.
    ///
    /// Both reads happen on the same connection inside one implicit read
    /// transaction, so the cursor is the position of *this* state — which is
    /// what closes the list-then-watch gap.
    pub fn goal_snapshot(&self) -> Result<GoalSnapshot, StoreError> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, phase, current_revision, revision FROM goals ORDER BY id")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            let mut goals = Vec::new();
            for row in rows {
                let (id, phase, current_revision, revision) = row?;
                let phase = GoalPhase::parse(&phase).ok_or_else(|| {
                    StoreError::InvariantViolated(format!("goal {id} has unknown phase"))
                })?;
                goals.push(GoalRecord {
                    id,
                    phase,
                    current_revision,
                    revision: Revision::new(revision),
                });
            }
            let cursor = read_head(conn)?;
            Ok(GoalSnapshot { goals, cursor })
        })
    }

    /// Events committed strictly after `cursor`, oldest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] wrapping a [`CursorError`] when the
    /// cursor names another journal history or is ahead of the head. Failing
    /// closed matters: silently restarting at the head would drop exactly the
    /// Events the caller asked not to miss.
    pub fn events_after(
        &self,
        cursor: &Cursor,
        limit: i64,
    ) -> Result<Result<Vec<EventRecord>, CursorError>, StoreError> {
        self.read(|conn| {
            let head = read_head(conn)?;
            if cursor.journal_epoch != head.journal_epoch {
                return Ok(Err(CursorError::UnknownEpoch {
                    supplied: cursor.journal_epoch.clone(),
                    current: head.journal_epoch,
                }));
            }
            if cursor.sequence > head.sequence {
                return Ok(Err(CursorError::AheadOfJournal {
                    supplied: cursor.sequence,
                    head: head.sequence,
                }));
            }

            let mut stmt = conn.prepare(
                "SELECT event_id, journal_epoch, sequence, event_type, recorded_at,
                        command_epoch, command_id
                 FROM event_journal
                 WHERE journal_epoch = ?1 AND sequence > ?2
                 ORDER BY sequence
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![cursor.journal_epoch, cursor.sequence, limit],
                |row| {
                    Ok(EventRecord {
                        event_id: row.get(0)?,
                        journal_epoch: row.get(1)?,
                        sequence: row.get(2)?,
                        event_type: row.get(3)?,
                        recorded_at: row.get(4)?,
                        command_epoch: row.get(5)?,
                        command_id: row.get(6)?,
                    })
                },
            )?;
            let mut events = Vec::new();
            for row in rows {
                events.push(row?);
            }
            Ok(Ok(events))
        })
    }

    /// The schema version this database is migrated to.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        self.read(|conn| {
            conn.query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(StoreError::Sqlite)
        })
    }
}

/// The journal head: the current epoch and the last committed sequence.
///
/// `next_sequence` is what the allocator will hand out, so the last committed
/// sequence is one less. A fresh installation therefore reports 0, which is a
/// legitimate cursor meaning "before anything".
fn read_head(conn: &rusqlite::Connection) -> Result<Cursor, StoreError> {
    let (epoch, next): (String, i64) = conn.query_row(
        "SELECT epoch, next_sequence FROM journal_epochs WHERE is_current = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(Cursor {
        journal_epoch: epoch,
        sequence: next - 1,
    })
}

#[cfg(test)]
mod tests;
