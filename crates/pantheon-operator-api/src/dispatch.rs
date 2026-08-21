//! Dispatch handlers.
//!
//! Pause/resume are state-dependent mutations over the scheduler singleton,
//! so they require `If-Match` per the Operator contract's optimistic-
//! concurrency rule: a missing precondition is 428, a lost race is 412.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pantheon_core::config::canonical::Value;
use pantheon_engine::operator::{DispatchView, OperatorRuntime};
use pantheon_operator_protocol::dispatch::DispatchResponse;

use crate::command::identity_from_headers;
use crate::problem::{ProblemResponse, precondition_required};

pub(crate) async fn status(State(runtime): State<Arc<OperatorRuntime>>) -> Response {
    match crate::blocking(runtime, |service| service.dispatch_status()).await {
        Ok(view) => dispatch_response(StatusCode::OK, view),
        Err(problem) => problem.into_response(),
    }
}

/// The shared shape of `POST .../actions/pause` and `.../actions/resume`.
///
/// Returns 200, not 202: the durable desired-state mutation has completed
/// when this returns, and the contract reserves 202 for intents whose
/// processing continues.
async fn set_mode(
    runtime: Arc<OperatorRuntime>,
    headers: HeaderMap,
    path: &'static str,
    apply: fn(
        &pantheon_engine::operator::RuntimeService<'_>,
        &pantheon_engine::operator::CommandIdentity,
        i64,
    ) -> Result<DispatchView, pantheon_engine::operator::OperatorError>,
) -> Response {
    let expected = match expected_revision(&headers) {
        Ok(revision) => revision,
        Err(problem) => return problem.into_response(),
    };
    let hash = request_hash("POST", path, &Value::string(path));
    let identity = match identity_from_headers(&headers, hash) {
        Ok(identity) => identity,
        Err(problem) => return problem.into_response(),
    };

    match crate::blocking(runtime, move |service| apply(&service, &identity, expected)).await {
        Ok(view) => dispatch_response(StatusCode::OK, view),
        Err(problem) => problem.into_response(),
    }
}

pub(crate) async fn pause(
    State(runtime): State<Arc<OperatorRuntime>>,
    headers: HeaderMap,
) -> Response {
    set_mode(
        runtime,
        headers,
        "/api/v1/dispatch/actions/pause",
        |service, identity, expected| service.pause_dispatch(identity, expected),
    )
    .await
}

pub(crate) async fn resume(
    State(runtime): State<Arc<OperatorRuntime>>,
    headers: HeaderMap,
) -> Response {
    set_mode(
        runtime,
        headers,
        "/api/v1/dispatch/actions/resume",
        |service, identity, expected| service.resume_dispatch(identity, expected),
    )
    .await
}

/// The ETag a dispatch representation carries.
fn etag(view: &DispatchView) -> String {
    format!("\"dispatch-{}\"", view.revision)
}

/// The current singleton revision an `If-Match` must name.
///
/// The validator is deliberately opaque to clients: they echo back what a
/// read handed them rather than constructing one.
fn expected_revision(headers: &HeaderMap) -> Result<i64, ProblemResponse> {
    let supplied = headers
        .get(header::IF_MATCH)
        .ok_or_else(|| precondition_required("dispatch desired-state changes require If-Match"))?;
    let text = supplied
        .to_str()
        .map_err(|_| crate::problem::invalid("If-Match must be a valid ETag string"))?;
    let unprefixed = text
        .trim()
        .strip_prefix("\"dispatch-")
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| {
            crate::problem::invalid(format!("If-Match {text} is not this resource's ETag"))
        })?;
    unprefixed.parse::<i64>().map_err(|_| {
        crate::problem::invalid(format!("If-Match {text} is not this resource's ETag"))
    })
}

fn dispatch_response(status: StatusCode, view: DispatchView) -> Response {
    let header = HeaderValue::from_str(&etag(&view)).ok();
    let body = Json(DispatchResponse {
        desired_mode: view.desired_mode.as_str().to_string(),
        revision: view.revision,
        effective_can_dispatch: view.effective_can_dispatch,
        blocked_by: view
            .blocked_by
            .iter()
            .map(|gate| (*gate).to_string())
            .collect(),
    });
    match header {
        Some(value) => (status, [(header::ETAG, value)], body).into_response(),
        None => (status, body).into_response(),
    }
}

/// Composed over canonical values rather than concatenated text, exactly as
/// every other mutation does.
fn request_hash(method: &str, path: &str, body: &Value) -> [u8; 32] {
    let value = Value::object([
        ("method", Value::string(method)),
        ("path", Value::string(path)),
        ("body", body.clone()),
    ]);
    *pantheon_core::config::Digest::of(&value.to_canonical_bytes()).as_bytes()
}
