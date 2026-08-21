//! Dispatch desired state over the real Operator path.
//!
//! The acceptance properties here are only meaningful end to end: pause is an
//! operator act through the socket, it must survive full daemon
//! reconstruction, and its optimistic concurrency must behave like every
//! other state-dependent mutation.

mod support;

use hyper::StatusCode;
use support::{Installation, get, request};

async fn dispatch(socket: &std::path::Path) -> serde_json::Value {
    get(socket, "/api/v1/dispatch").await.json()
}

/// One pause/resume mutation, with the identity and precondition headers a
/// real client sends.
async fn set_mode(
    socket: &std::path::Path,
    action: &str,
    expected_revision: i64,
    command_id: &str,
) -> support::Answer {
    let epoch = support::command_epoch(socket).await;
    let headers: Vec<(String, String)> = vec![
        (
            "If-Match".to_string(),
            format!("\"dispatch-{expected_revision}\""),
        ),
        ("pantheon-command-epoch".to_string(), epoch),
        ("pantheon-command-id".to_string(), command_id.to_string()),
    ];
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    request(
        socket,
        "POST",
        &format!("/api/v1/dispatch/actions/{action}"),
        &borrowed,
        None,
    )
    .await
}

#[tokio::test]
async fn a_fresh_daemon_dispatches_by_default_and_reports_effective_dispatchability() {
    let installation = Installation::new("dispatch-fresh");
    let daemon = installation.start().await;

    let body = dispatch(daemon.socket()).await;
    assert_eq!(body["desiredMode"], "RUNNING");
    assert_eq!(body["revision"], 1);
    assert_eq!(
        body["effectiveCanDispatch"], true,
        "a first start activates its configuration, so nothing blocks dispatch"
    );
    assert_eq!(body["blockedBy"].as_array().map(Vec::len), Some(0));

    daemon.stop();
}

#[tokio::test]
async fn pause_is_a_state_dependent_mutation_with_optimistic_concurrency() {
    let installation = Installation::new("dispatch-precondition");
    let daemon = installation.start().await;
    let revision = dispatch(daemon.socket()).await["revision"]
        .as_i64()
        .expect("revision");

    // Missing precondition: 428, before anything else is consulted.
    let missing = request(
        daemon.socket(),
        "POST",
        "/api/v1/dispatch/actions/pause",
        &[("pantheon-command-id", "pause-no-etag")],
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);

    // A lost race: 412, and nothing changed.
    let stale = set_mode(daemon.socket(), "pause", 999, "pause-stale").await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(dispatch(daemon.socket()).await["desiredMode"], "RUNNING");

    // The current ETag commits the pause.
    let paused = set_mode(daemon.socket(), "pause", revision, "pause-ok").await;
    assert_eq!(paused.status, StatusCode::OK);
    let body = paused.json();
    assert_eq!(body["desiredMode"], "PAUSED");
    assert_eq!(body["effectiveCanDispatch"], false);
    assert_eq!(body["blockedBy"][0], "operator-pause");

    daemon.stop();
}

#[tokio::test]
async fn a_paused_daemon_stays_paused_across_restart_and_resume_reopens_it() {
    let installation = Installation::new("dispatch-restart");
    let daemon = installation.start().await;
    let revision = dispatch(daemon.socket()).await["revision"]
        .as_i64()
        .expect("revision");
    let paused = set_mode(daemon.socket(), "pause", revision, "pause-durable").await;
    assert_eq!(paused.status, StatusCode::OK);
    daemon.stop();

    // Full reconstruction: new process, reopened database. The durable
    // desired mode must not silently become RUNNING.
    let daemon = installation.start().await;
    let after = dispatch(daemon.socket()).await;
    assert_eq!(
        after["desiredMode"], "PAUSED",
        "restart preserves the pause"
    );
    assert_eq!(after["revision"], revision + 1);
    assert_eq!(after["effectiveCanDispatch"], false);

    // Resume through the same path re-opens dispatch durably.
    let resumed = set_mode(
        daemon.socket(),
        "resume",
        revision + 1,
        "resume-after-restart",
    )
    .await;
    assert_eq!(resumed.status, StatusCode::OK);
    assert_eq!(resumed.json()["desiredMode"], "RUNNING");

    daemon.stop();
}

#[tokio::test]
async fn repeating_a_pause_command_is_the_same_command() {
    let installation = Installation::new("dispatch-replay");
    let daemon = installation.start().await;
    let revision = dispatch(daemon.socket()).await["revision"]
        .as_i64()
        .expect("revision");

    let first = set_mode(daemon.socket(), "pause", revision, "pause-once").await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.json()["revision"], revision + 1);

    // A retry whose response was lost reconciles instead of advancing again.
    let retry = set_mode(daemon.socket(), "pause", revision, "pause-once").await;
    assert_eq!(retry.status, StatusCode::OK);
    assert_eq!(retry.json(), first.json());

    daemon.stop();
}
