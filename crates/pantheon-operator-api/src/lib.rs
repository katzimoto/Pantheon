//! The Operator Control transport adapter.
//!
//! # Owns
//!
//! Serving the operator surface: routes, the Unix-socket HTTP server,
//! middleware, conversion between `pantheon-operator-protocol` wire types and
//! domain types, handling of sensitive requests, and API description assembly.
//!
//! # Must not own
//!
//! Business decisions. A handler validates and translates a request, then calls
//! an operation on `pantheon-engine`; logic that lives in a handler is logic
//! that no other entry point can reuse and no test can reach without HTTP.
//!
//! It does not own persistence either: it must reach durable state through the
//! engine rather than talking to `pantheon-store` directly.
//!
//! # Local only, structurally
//!
//! [`serve`] takes a filesystem path and binds a Unix-domain socket. There is
//! no address type, no port, and no code path in this crate that constructs a
//! TCP listener — the crate cannot fall back to one, because it never learned
//! how. `docs/architecture/operations/public-daemon-api-and-cli.md` makes
//! local-socket-only the MVP boundary, and the same-UID reachability a Unix
//! socket gives is emphatically *not* the Agent/operator trust boundary a
//! later mission establishes.
//!
//! # Blocking work
//!
//! Durable reads and writes are synchronous SQLite calls. Every handler that
//! touches the store runs them on [`tokio::task::spawn_blocking`], so the
//! serialized authoritative writer cannot stall the async executor that is
//! also serving reads and Event streams.

mod command;
mod description;
mod events;
mod goals;
mod problem;
mod socket;
mod system;

pub use socket::{SocketError, serve};

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use pantheon_engine::operator::{OperatorError, OperatorRuntime, RuntimeService};
use pantheon_operator_protocol::API_PREFIX;

/// The complete operator surface.
///
/// Returned as a plain [`Router`] so the whole API can be exercised in tests
/// through `tower::Service` without binding a socket — and so the socket code
/// below has nothing to do with what any route means.
pub fn router(runtime: Arc<OperatorRuntime>) -> Router {
    // Health probes are process-level endpoints, deliberately outside the
    // version prefix: `docs/architecture/operations/public-daemon-api-and-cli.md`
    // ("Health/readiness") makes `/health/live` and `/health/ready` canonical
    // so probe and supervisor configuration survives a future `/api/v2`
    // transition, and — Pantheon being unreleased — no `/api/v1/health/...`
    // alias is kept.
    let unversioned = Router::new()
        .route("/health/live", get(system::live))
        .route("/health/ready", get(system::ready));

    let versioned = Router::new()
        .route("/system", get(system::system))
        .route("/goals", get(goals::list).post(goals::create))
        .route("/goals/{goalId}", get(goals::get))
        .route("/goals/{goalId}/actions/cancel", post(goals::cancel))
        .route("/events", get(events::list))
        .route("/events/watch", get(events::watch));

    unversioned
        .merge(Router::new().nest(API_PREFIX, versioned))
        .with_state(runtime)
}

/// Runs one durable operation off the async executor.
///
/// Every store call is synchronous SQLite. Running one directly in a handler
/// would block a runtime thread for the duration of an authoritative
/// transaction, and the authoritative writer is serialized — so one slow
/// mutation would stall reads and Event streams that have nothing to do with
/// it.
async fn blocking<T, F>(
    runtime: Arc<OperatorRuntime>,
    operation: F,
) -> Result<T, problem::ProblemResponse>
where
    T: Send + 'static,
    F: FnOnce(RuntimeService<'_>) -> Result<T, OperatorError> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || operation(runtime.service())).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.into()),
        // The blocking task panicked or was cancelled. Nothing internal is
        // disclosed: an operator can act on none of it.
        Err(_) => Err(problem::internal()),
    }
}
