//! Dispatch desired state and effective dispatchability.

use serde::{Deserialize, Serialize};

/// `GET /api/v1/dispatch`, and the body pause/resume answer with.
///
/// `desiredMode` is the operator's durable intent; `effectiveCanDispatch` is
/// the current factual ability to commit new Runs. They are deliberately
/// different fields: `desiredMode=RUNNING` can coexist with
/// `effectiveCanDispatch=false` while configuration is unusable, while
/// `desiredMode=PAUSED` stays paused even after every other gate is healthy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchResponse {
    /// `RUNNING` or `PAUSED`.
    pub desired_mode: String,
    /// The scheduler singleton revision. Pause/resume carry it as `If-Match`.
    pub revision: i64,
    pub effective_can_dispatch: bool,
    /// Normalized current factual gates, e.g. `operator-pause`,
    /// `configuration`. Never a second desired-state field.
    pub blocked_by: Vec<String>,
}
