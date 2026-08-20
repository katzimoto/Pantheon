//! Reading durable Events.
//!
//! Observation only. `docs/architecture/operations/event-and-observability-model.md`
//! is explicit that the Event Journal is not the authoritative state model, so
//! nothing here decides anything: a watcher reads history, and controllers
//! keep reading the authoritative tables.

use std::borrow::Borrow;

use pantheon_store::{Cursor, EventRecord, Store};

use crate::operator::{OperatorError, OperatorService};

/// The largest page of Events one read returns.
///
/// A cap rather than a client-chosen limit: an unbounded read would let one
/// operator request pull the whole journal into memory.
pub const MAX_EVENTS: i64 = 256;

/// One durable Event, as an operator sees it.
///
/// Carries identity, type, ordering and command causality — and no payload.
/// The Event contract keeps secret material and backend-private state out of
/// Events, and #26 defines no payload schema; emitting an unspecified body
/// would make it a compatibility commitment by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventView {
    pub event_id: String,
    /// The resumable journal position of this Event.
    pub cursor: Cursor,
    pub event_type: String,
    pub recorded_at: i64,
    /// The command this Event was caused by, when it had one. `command_epoch`
    /// is that command's RestoreGeneration, never the journal epoch.
    pub command_epoch: Option<String>,
    pub command_id: Option<String>,
}

/// A page of Events plus the position to continue from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPage {
    pub events: Vec<EventView>,
    /// Where to resume. Equal to the supplied cursor when the page is empty,
    /// so a caller that polls does not silently skip forward.
    pub next: Cursor,
}

impl<S: Borrow<Store>> OperatorService<'_, S> {
    /// The current journal head, for a client that wants to watch only what
    /// happens from now on.
    ///
    /// # Errors
    ///
    /// [`OperatorError::Internal`] when durable state cannot be read.
    pub fn journal_head(&self) -> Result<Cursor, OperatorError> {
        Ok(self.store.journal_head()?)
    }

    /// Events committed strictly after `cursor`.
    ///
    /// # Errors
    ///
    /// [`OperatorError::CursorGone`] when the cursor names another journal
    /// history or is ahead of the head. This fails closed rather than
    /// restarting at the head, because restarting would drop exactly the
    /// Events the caller asked not to miss.
    pub fn events_after(&self, cursor: &Cursor, limit: i64) -> Result<EventPage, OperatorError> {
        let limit = limit.clamp(1, MAX_EVENTS);
        let events = self.store.events_after(cursor, limit)??;
        let next = events
            .last()
            .map_or_else(|| cursor.clone(), EventRecord::cursor);
        Ok(EventPage {
            events: events.into_iter().map(view).collect(),
            next,
        })
    }
}

fn view(record: EventRecord) -> EventView {
    EventView {
        cursor: record.cursor(),
        event_id: record.event_id,
        event_type: record.event_type,
        recorded_at: record.recorded_at,
        command_epoch: record.command_epoch,
        command_id: record.command_id,
    }
}
