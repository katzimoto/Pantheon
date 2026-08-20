//! System metadata and health.

use serde::{Deserialize, Serialize};

/// `GET /api/v1/system`.
///
/// Deliberately absent: an installation identity. The architecture lists one,
/// but the only installation-scoped identifier Pantheon durably holds is the
/// RestoreGeneration, and that rotates on disaster restore — which is the one
/// property an installation identity must not have. Publishing it under both
/// names would make a distinction the same contract calls load-bearing
/// unobservable, so the field is omitted until a durable installation identity
/// exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemResponse {
    pub daemon_version: String,
    pub api_versions: Vec<String>,
    /// The database schema version this daemon has migrated to.
    pub schema_version: i64,
    /// The current RestoreGeneration. A mutation must carry this as its
    /// `commandEpoch`, so a client reads it here first.
    pub command_epoch: String,
    /// Event Journal continuity. Rotates independently of `commandEpoch`.
    pub journal: JournalResponse,
    pub active_configuration: Option<ActiveConfigurationResponse>,
    pub readiness: ReadinessResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalResponse {
    pub epoch: String,
    /// The last committed sequence, or absent when nothing has been committed
    /// in this history. Sequences start at 1, so `0` would be a lie rather
    /// than an empty answer.
    pub latest_sequence: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveConfigurationResponse {
    pub activation_sequence: i64,
    /// Hex content identity of the active ConfigurationRevision.
    pub content_digest: String,
    /// Whether the compiled semantics of that revision are loaded. `false`
    /// means the durable revision is active but its source drifted, so
    /// identity governs and no new authority-bearing work can be planned.
    pub semantics_loaded: bool,
}

/// `GET /health/ready`, and the `readiness` member of the system response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessResponse {
    pub ready: bool,
    pub components: Vec<ReadinessComponentResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessComponentResponse {
    pub name: String,
    /// `satisfied`, `unsatisfied`, or `unimplemented`.
    ///
    /// `unimplemented` is not padding: the readiness contract names conjuncts
    /// — a passed recovery barrier, a safe dispatch plane — that no code in
    /// this build establishes. Reporting them as unimplemented keeps the
    /// altitude of the `ready` flag visible instead of quietly asserting a
    /// barrier that does not exist.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `GET /health/live`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivenessResponse {
    pub live: bool,
}
