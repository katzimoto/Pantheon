//! Goal requests and representations.

use serde::{Deserialize, Serialize};

/// `POST /api/v1/goals`.
///
/// The command identity travels in headers, not here: it is identical for
/// every mutation regardless of body shape, and a body field would make it
/// look like part of the Goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGoalRequest {
    pub goal: GoalSpecPayload,
}

/// The semantic content of a Goal revision.
///
/// Structurally a copy of the domain's Goal specification rather than a
/// re-export of it. That duplication is the point: the domain type may be
/// refactored without that being a public breaking change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSpecPayload {
    /// The desired outcome, stated without prescribing an implementation.
    pub objective: String,
    #[serde(default)]
    pub inputs: Vec<GoalInputPayload>,
    #[serde(default)]
    pub deliverables: Vec<DeliverablePayload>,
    pub constraints: GoalConstraintsPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalInputPayload {
    pub name: String,
    /// An opaque URI-shaped reference. Pantheon does not interpret it here.
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverablePayload {
    pub name: String,
    pub kind: String,
    pub required: bool,
}

/// The authority ceiling for every Task planned from this Goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalConstraintsPayload {
    #[serde(default)]
    pub permitted_effects: Vec<String>,
    #[serde(default)]
    pub forbidden_effects: Vec<String>,
    #[serde(default)]
    pub permitted_resources: Vec<String>,
}

/// A Goal, as `GET /api/v1/goals/{id}` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalResponse {
    pub id: String,
    /// One of the canonical Goal phases.
    pub phase: String,
    /// The semantic GoalRevision currently being pursued.
    pub goal_revision: i64,
    /// The authoritative row revision the ETag is derived from. It advances on
    /// every authoritative mutation, including a lifecycle transition that
    /// leaves `goalRevision` alone — which is why the two are separate fields
    /// rather than one number doing both jobs.
    pub revision: i64,
    pub goal: GoalSpecPayload,
    /// The Goal's Tasks. Embedded because this API version exposes no Task
    /// resource.
    pub tasks: Vec<TaskResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
    pub id: String,
    pub phase: String,
    pub created_graph_revision: i64,
    /// Hex identity of the immutable Task specification. Not the
    /// specification itself: this API version exposes no Task resource.
    pub spec_digest: String,
}

/// `GET /api/v1/goals`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalListResponse {
    pub goals: Vec<GoalSummaryResponse>,
    /// The Event Journal position this list was read at, as an opaque string.
    ///
    /// Start an Event watch strictly after this and no Event that changed what
    /// the list shows can be missed. Obtained from the same durable read
    /// snapshot as `goals`, which is the only way that guarantee holds.
    pub snapshot_cursor: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSummaryResponse {
    pub id: String,
    pub phase: String,
    pub goal_revision: i64,
    pub revision: i64,
}
