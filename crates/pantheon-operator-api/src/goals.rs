//! Goal handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value;
use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_engine::operator::{GoalView, GoalsPage, OperatorRuntime};
use pantheon_operator_protocol::goals::{
    CreateGoalRequest, DeliverablePayload, GoalConstraintsPayload, GoalInputPayload,
    GoalListResponse, GoalResponse, GoalSpecPayload, GoalSummaryResponse, TaskResponse,
};

use crate::command::identity_from_headers;
use crate::problem::ProblemResponse;

pub(crate) async fn list(State(runtime): State<Arc<OperatorRuntime>>) -> Response {
    match crate::blocking(runtime, |service| service.goals()).await {
        Ok(page) => Json(list_response(page)).into_response(),
        Err(problem) => problem.into_response(),
    }
}

pub(crate) async fn get(
    State(runtime): State<Arc<OperatorRuntime>>,
    Path(goal_id): Path<String>,
) -> Response {
    match crate::blocking(runtime, move |service| service.goal(&goal_id)).await {
        Ok(view) => goal_response(StatusCode::OK, view),
        Err(problem) => problem.into_response(),
    }
}

/// `POST /api/v1/goals`.
///
/// Returns 201: the contract reserves 201 for a resource created
/// synchronously, and this path does not return until the Goal exists, its
/// planning decision is recorded and its Ready Task is materialized.
pub(crate) async fn create(
    State(runtime): State<Arc<OperatorRuntime>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let request: CreateGoalRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(err) => return crate::problem::invalid(err.to_string()).into_response(),
    };
    let spec = match spec_from(request.goal) {
        Ok(spec) => spec,
        Err(problem) => return problem.into_response(),
    };
    // The request hash is taken over the canonical encoding of the request's
    // *meaning* — method, path and body — not over the raw bytes. Two byte
    // sequences that decode to the same request must not be treated as a
    // conflicting reuse of a command id, and two different requests must not
    // collide. Recomputing it from the decoded body is what makes that true.
    let hash = request_hash("POST", "/api/v1/goals", &spec.to_value());
    let identity = match identity_from_headers(&headers, hash) {
        Ok(identity) => identity,
        Err(problem) => return problem.into_response(),
    };

    match crate::blocking(runtime, move |service| {
        service.create_goal(&identity, &spec)
    })
    .await
    {
        Ok(view) => goal_response(StatusCode::CREATED, view),
        Err(problem) => problem.into_response(),
    }
}

/// `POST /api/v1/goals/{id}/actions/cancel`.
///
/// Returns 202, not 200: the contract reserves 202 for a durable intent that
/// has been accepted while processing continues, and cancellation commits
/// `Finalizing / terminalTarget=Cancelled`. The Goal is not Cancelled when
/// this returns, and saying 200 would imply it was.
///
/// Carries no `If-Match`. The contract names cancel-if-nonterminal as the
/// example of a state-independent idempotent command that need not require a
/// client-supplied prior revision.
pub(crate) async fn cancel(
    State(runtime): State<Arc<OperatorRuntime>>,
    Path(goal_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let hash = request_hash(
        "POST",
        "/api/v1/goals/{id}/actions/cancel",
        &Value::string(&goal_id),
    );
    let identity = match identity_from_headers(&headers, hash) {
        Ok(identity) => identity,
        Err(problem) => return problem.into_response(),
    };

    match crate::blocking(runtime, move |service| {
        service.cancel_goal(&identity, &goal_id)
    })
    .await
    {
        Ok(view) => goal_response(StatusCode::ACCEPTED, view),
        Err(problem) => problem.into_response(),
    }
}

/// The non-sensitive digest the command ledger stores.
///
/// Composed over canonical values rather than concatenated text, so no choice
/// of path and body can be read as a different pair.
fn request_hash(method: &str, path: &str, body: &Value) -> [u8; 32] {
    let value = Value::object([
        ("method", Value::string(method)),
        ("path", Value::string(path)),
        ("body", body.clone()),
    ]);
    *Digest::of(&value.to_canonical_bytes()).as_bytes()
}

/// A Goal response with its ETag.
///
/// The ETag is derived from the authoritative row revision, not the semantic
/// GoalRevision: only the row revision advances on *every* authoritative
/// mutation, so only it is a sound concurrency token. A lifecycle transition
/// such as cancellation leaves `goalRevision` alone and must still invalidate
/// a cached representation.
fn goal_response(status: StatusCode, view: GoalView) -> Response {
    let etag = format!("\"{}-{}\"", view.id, view.revision);
    let header = HeaderValue::from_str(&etag).ok();
    let body = Json(response(view));
    match header {
        Some(value) => (status, [(header::ETAG, value)], body).into_response(),
        None => (status, body).into_response(),
    }
}

fn response(view: GoalView) -> GoalResponse {
    GoalResponse {
        id: view.id,
        phase: view.phase.as_str().to_string(),
        goal_revision: view.goal_revision,
        revision: view.revision,
        goal: payload_from(view.spec),
        tasks: view
            .tasks
            .into_iter()
            .map(|task| TaskResponse {
                id: task.id,
                phase: task.phase.as_str().to_string(),
                created_graph_revision: task.created_graph_revision,
                spec_digest: task.spec_digest,
            })
            .collect(),
    }
}

fn list_response(page: GoalsPage) -> GoalListResponse {
    GoalListResponse {
        goals: page
            .goals
            .into_iter()
            .map(|goal| GoalSummaryResponse {
                id: goal.id,
                phase: goal.phase.as_str().to_string(),
                goal_revision: goal.goal_revision,
                revision: goal.revision,
            })
            .collect(),
        snapshot_cursor: page.snapshot_cursor.to_wire(),
    }
}

fn payload_from(spec: GoalSpec) -> GoalSpecPayload {
    GoalSpecPayload {
        objective: spec.objective,
        inputs: spec
            .inputs
            .into_iter()
            .map(|input| GoalInputPayload {
                name: input.name,
                reference: input.reference,
            })
            .collect(),
        deliverables: spec
            .deliverables
            .into_iter()
            .map(|deliverable| DeliverablePayload {
                name: deliverable.name,
                kind: deliverable.kind,
                required: deliverable.required,
            })
            .collect(),
        constraints: GoalConstraintsPayload {
            permitted_effects: spec.constraints.permitted_effects,
            forbidden_effects: spec.constraints.forbidden_effects,
            permitted_resources: spec.constraints.permitted_resources,
        },
    }
}

/// Translates a wire Goal into the domain type, rejecting what cannot be a
/// Goal at all.
///
/// Only shape is checked here. Whether the Goal's constraints permit what a
/// plan proposes is a semantic decision the planner and validator own, and
/// duplicating it in a handler would be a second, divergent authority.
fn spec_from(payload: GoalSpecPayload) -> Result<GoalSpec, ProblemResponse> {
    if payload.objective.trim().is_empty() {
        return Err(crate::problem::invalid("goal.objective must not be empty"));
    }
    Ok(GoalSpec {
        objective: payload.objective,
        inputs: payload
            .inputs
            .into_iter()
            .map(|input| GoalInput {
                name: input.name,
                reference: input.reference,
            })
            .collect(),
        deliverables: payload
            .deliverables
            .into_iter()
            .map(|deliverable| Deliverable {
                name: deliverable.name,
                kind: deliverable.kind,
                required: deliverable.required,
            })
            .collect(),
        constraints: GoalConstraints {
            permitted_effects: payload.constraints.permitted_effects,
            forbidden_effects: payload.constraints.forbidden_effects,
            permitted_resources: payload.constraints.permitted_resources,
        },
    })
}
