//! System metadata and health handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pantheon_engine::operator::{OperatorRuntime, ReadinessReport, SystemView};
use pantheon_operator_protocol::system::{
    ActiveConfigurationResponse, JournalResponse, LivenessResponse, ReadinessComponentResponse,
    ReadinessResponse, SystemResponse,
};

use crate::problem::ProblemResponse;

pub(crate) async fn system(State(runtime): State<Arc<OperatorRuntime>>) -> Response {
    match crate::blocking(runtime, |service| service.system()).await {
        Ok(view) => Json(system_response(view)).into_response(),
        Err(problem) => problem.into_response(),
    }
}

/// Liveness: is this process functioning.
///
/// Touches nothing durable and cannot fail. A database that is merely
/// unreachable is a readiness fact; reporting it here would ask an
/// orchestrator to restart a daemon that is still serving.
pub(crate) async fn live() -> Response {
    Json(LivenessResponse { live: true }).into_response()
}

/// Readiness: could the control plane safely take on new authority-bearing
/// work.
///
/// Returns 503 when it could not, per the contract's "during startup recovery,
/// live may be 200 while ready is 503".
pub(crate) async fn ready(State(runtime): State<Arc<OperatorRuntime>>) -> Response {
    let report = match crate::blocking(runtime, |service| Ok(service.readiness())).await {
        Ok(report) => report,
        Err(problem) => return problem.into_response(),
    };
    let status = if report.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(readiness_response(report))).into_response()
}

fn system_response(view: SystemView) -> SystemResponse {
    SystemResponse {
        daemon_version: view.daemon_version.to_string(),
        api_versions: view
            .api_versions
            .iter()
            .map(|version| (*version).to_string())
            .collect(),
        schema_version: view.schema_version,
        command_epoch: view.command_epoch,
        journal: JournalResponse {
            epoch: view.journal.epoch,
            latest_sequence: view.journal.latest_sequence,
        },
        active_configuration: view
            .active_configuration
            .map(|active| ActiveConfigurationResponse {
                activation_sequence: active.activation_sequence,
                content_digest: active.content_digest.to_hex(),
                semantics_loaded: active.semantics_loaded,
            }),
        readiness: readiness_response(view.readiness),
    }
}

fn readiness_response(report: ReadinessReport) -> ReadinessResponse {
    ReadinessResponse {
        ready: report.ready,
        components: report
            .components
            .into_iter()
            .map(|component| ReadinessComponentResponse {
                name: component.name.to_string(),
                state: component.state.as_str().to_string(),
                detail: component.detail,
            })
            .collect(),
    }
}

/// Unused import guard: `ProblemResponse` is named so the conversion above is
/// visible in this module's signature surface.
const _: fn(ProblemResponse) -> ProblemResponse = |problem| problem;
