//! Turning failures into `application/problem+json`.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pantheon_engine::operator::OperatorError;
use pantheon_operator_protocol::PROBLEM_MEDIA_TYPE;
use pantheon_operator_protocol::problem::{Problem, ProblemCode};

/// A response carrying a structured error.
pub(crate) struct ProblemResponse(pub Problem);

impl IntoResponse for ProblemResponse {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_vec(&self.0).unwrap_or_else(|_| {
            // The body is built from owned primitives, so this cannot fail in
            // practice. Falling back to a literal rather than panicking keeps
            // one malformed error from taking down the daemon.
            br#"{"type":"urn:pantheon:problem:internal","title":"Internal error","status":500,"detail":"the error could not be serialized","code":"internal"}"#.to_vec()
        });
        (
            status,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static(PROBLEM_MEDIA_TYPE),
            )],
            body,
        )
            .into_response()
    }
}

impl From<OperatorError> for ProblemResponse {
    /// Maps every operation failure onto exactly one problem code.
    ///
    /// [`OperatorError::Internal`] is the only variant whose detail is
    /// replaced rather than forwarded: it carries storage and invariant text
    /// that would put internal structure on a public surface, and an operator
    /// can act on none of it.
    fn from(err: OperatorError) -> Self {
        let (code, detail) = match &err {
            OperatorError::NotFound { .. } => (ProblemCode::NotFound, err.to_string()),
            OperatorError::Conflict(_) => (ProblemCode::Conflict, err.to_string()),
            OperatorError::StaleRevision { detail } => (ProblemCode::StaleRevision, detail.clone()),
            OperatorError::StaleCommandEpoch { .. } => {
                (ProblemCode::StaleCommandEpoch, err.to_string())
            }
            OperatorError::CommandConflict { .. } => (ProblemCode::Conflict, err.to_string()),
            OperatorError::CursorGone(_) => (ProblemCode::CursorGone, err.to_string()),
            OperatorError::NotReady(_) => (ProblemCode::TemporarilyUnavailable, err.to_string()),
            OperatorError::Invalid(_) => (ProblemCode::Validation, err.to_string()),
            OperatorError::Internal(_) => (
                ProblemCode::Internal,
                "the daemon could not complete this request".to_string(),
            ),
        };
        Self(Problem::new(code, detail))
    }
}

/// A request-shaped failure, before any operation runs.
pub(crate) fn invalid(detail: impl Into<String>) -> ProblemResponse {
    ProblemResponse(Problem::new(ProblemCode::Validation, detail))
}

/// A missing mandatory precondition, per the contract's 428.
pub(crate) fn precondition_required(detail: impl Into<String>) -> ProblemResponse {
    ProblemResponse(Problem::new(ProblemCode::PreconditionRequired, detail))
}

/// A failure inside the transport itself, with nothing internal disclosed.
pub(crate) fn internal() -> ProblemResponse {
    ProblemResponse(Problem::new(
        ProblemCode::Internal,
        "the daemon could not complete this request",
    ))
}
