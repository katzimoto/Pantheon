//! Event reads and the Event stream.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use pantheon_engine::operator::{
    Cursor, EventPage, EventView, MAX_EVENTS, OperatorError, OperatorRuntime,
};
use pantheon_operator_protocol::events::{EventListResponse, EventResponse};
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;

use crate::problem::ProblemResponse;

/// How long the watch task waits before looking for new Events.
///
/// A poll rather than a database notification: SQLite has no cross-connection
/// change feed Pantheon could subscribe to, and the alternative — an
/// in-process broadcast from the write path — would make delivery depend on
/// process memory rather than on the durable journal, which is precisely what
/// the Event contract says delivery must not do.
const WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// How many Events one page of a stream carries.
const WATCH_PAGE: i64 = 64;

/// How many pending Events the stream buffers before the producer waits.
///
/// Bounded so a client that stops reading applies backpressure to the poller
/// instead of growing the daemon's memory.
const WATCH_BUFFER: usize = 32;

#[derive(Debug, Deserialize)]
pub(crate) struct EventQuery {
    /// The opaque cursor to read strictly after. Absent means "from the
    /// journal head", which is the only sensible default for a watcher that
    /// has no prior position — and is never used to *recover* from a bad
    /// cursor, which fails closed instead.
    after: Option<String>,
    limit: Option<i64>,
}

pub(crate) async fn list(
    State(runtime): State<Arc<OperatorRuntime>>,
    Query(query): Query<EventQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(WATCH_PAGE);
    let after = query.after.clone();
    match crate::blocking(runtime, move |service| {
        let cursor = resolve(&after, || service.journal_head())?;
        service.events_after(&cursor, limit)
    })
    .await
    {
        Ok(page) => Json(list_response(page)).into_response(),
        Err(problem) => problem.into_response(),
    }
}

/// `GET /api/v1/events/watch`.
///
/// The SSE `id` line is the resumable journal cursor and the Event's own id
/// stays inside the payload, exactly as the contract specifies. A client that
/// loses the stream resumes by passing the last `id` back as `after`; an
/// unreachable position fails closed as `cursor-gone` rather than restarting
/// at the head, because restarting would drop the Events the watcher asked
/// not to miss.
pub(crate) async fn watch(
    State(runtime): State<Arc<OperatorRuntime>>,
    Query(query): Query<EventQuery>,
) -> Response {
    // Resolve and validate the starting cursor *before* the stream begins.
    // An SSE response has already committed to 200 by the time the first
    // event is written, so a bad cursor discovered later could not be
    // reported as 410 at all.
    let after = query.after.clone();
    let start = match crate::blocking(Arc::clone(&runtime), move |service| {
        let cursor = resolve(&after, || service.journal_head())?;
        // Reading zero events is enough to establish the cursor is reachable.
        service.events_after(&cursor, 1)?;
        Ok(cursor)
    })
    .await
    {
        Ok(cursor) => cursor,
        Err(problem) => return problem.into_response(),
    };

    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(WATCH_BUFFER);
    tokio::spawn(async move {
        let mut cursor = start;
        loop {
            let at = cursor.clone();
            let runtime = Arc::clone(&runtime);
            let page = tokio::task::spawn_blocking(move || {
                runtime.service().events_after(&at, WATCH_PAGE)
            })
            .await;
            let Ok(Ok(page)) = page else {
                // Either the blocking task failed or the journal moved out
                // from under this cursor. Ending the stream is the honest
                // answer: the client reconnects, and a fresh request gets a
                // real status code instead of a silent gap.
                break;
            };
            cursor = page.next.clone();
            for event in page.events {
                let message = Event::default()
                    .id(event.cursor.to_wire())
                    .event(event.event_type.clone())
                    .json_data(response(event));
                let Ok(message) = message else { break };
                if sender.send(Ok(message)).await.is_err() {
                    return;
                }
            }
            tokio::time::sleep(WATCH_INTERVAL).await;
        }
    });

    Sse::new(ReceiverStream::new(receiver))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Interprets an `after` parameter.
///
/// A malformed cursor is a `cursor-gone` failure, not a silent fallback to the
/// head. The whole point of the parameter is that the client knows where it
/// stopped; guessing on its behalf is how a watcher loses Events.
fn resolve(
    after: &Option<String>,
    head: impl FnOnce() -> Result<Cursor, OperatorError>,
) -> Result<Cursor, OperatorError> {
    match after {
        None => head(),
        Some(text) => Cursor::parse(text)
            .ok_or_else(|| OperatorError::CursorGone(format!("{text} is not a journal cursor"))),
    }
}

fn list_response(page: EventPage) -> EventListResponse {
    EventListResponse {
        next_cursor: page.next.to_wire(),
        events: page.events.into_iter().map(response).collect(),
    }
}

fn response(event: EventView) -> EventResponse {
    EventResponse {
        event_id: event.event_id,
        cursor: event.cursor.to_wire(),
        event_type: event.event_type,
        recorded_at: event.recorded_at,
        command_epoch: event.command_epoch,
        command_id: event.command_id,
    }
}

/// Keeps the page cap visible at this layer: a client-supplied `limit` is
/// clamped by the operations layer, and this is the constant it clamps to.
const _: i64 = MAX_EVENTS;

/// Named so the conversion into a response is part of this module's surface.
const _: fn(ProblemResponse) -> ProblemResponse = |problem| problem;
