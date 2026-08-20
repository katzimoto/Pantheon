//! Event reads and the Event stream.

use serde::{Deserialize, Serialize};

/// One durable Event.
///
/// Carries identity, type, ordering and command causality, and no payload.
/// The Event contract keeps secret material and backend-private state out of
/// Events, and this API version defines no payload schema — emitting an
/// unspecified body would turn it into a compatibility commitment by accident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventResponse {
    /// Stable Event identity, unchanged across retries and re-delivery.
    pub event_id: String,
    /// The resumable journal position of this Event, as an opaque string.
    /// This is also the SSE `id` line, so a dropped stream resumes by handing
    /// the last `id` back as `after`.
    pub cursor: String,
    pub event_type: String,
    /// Unix seconds at which the Event was committed.
    pub recorded_at: i64,
    /// The command this Event was caused by, when it had one.
    ///
    /// `commandEpoch` is that command's RestoreGeneration and is deliberately
    /// not the journal epoch: the two rotate for different reasons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_epoch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
}

/// `GET /api/v1/events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventListResponse {
    pub events: Vec<EventResponse>,
    /// Where to resume. Equal to the requested cursor when the page is empty,
    /// so a polling client cannot skip forward past Events it never saw.
    pub next_cursor: String,
}
