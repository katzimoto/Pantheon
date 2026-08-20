//! What an ordinary daemon restart must and must not change.
//!
//! Two properties the mission names explicitly, and both are the kind that
//! only a real restart can establish: an in-process test would be asserting
//! about state that never left memory.

mod support;

use hyper::StatusCode;
use support::{GOAL_BODY, Installation, get, post};

#[tokio::test]
async fn a_restart_does_not_rotate_the_restore_generation() {
    // The RestoreGeneration fences command authority across disaster restore.
    // Rotating it on an ordinary restart would invalidate every in-flight
    // operator retry, and would make a restart indistinguishable from a
    // restore to any client holding a command epoch.
    let installation = Installation::new("generation");

    let daemon = installation.start().await;
    let before = get(daemon.socket(), "/api/v1/system").await.json();
    daemon.stop();

    let daemon = installation.start().await;
    let after = get(daemon.socket(), "/api/v1/system").await.json();

    assert_eq!(
        before["commandEpoch"], after["commandEpoch"],
        "an ordinary restart must not rotate the command epoch"
    );
    assert_eq!(
        before["journal"]["epoch"], after["journal"]["epoch"],
        "nor the journal history"
    );

    daemon.stop();
}

#[tokio::test]
async fn a_command_issued_before_a_restart_still_replays_after_it() {
    // The consequence that matters: a client whose response was lost can
    // safely retry across a restart, because its command epoch is still
    // current and its command id is still the same command.
    let installation = Installation::new("replay");

    let daemon = installation.start().await;
    let first = post(
        daemon.socket(),
        "/api/v1/goals",
        "survives",
        Some(GOAL_BODY),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);
    let created = first.json();
    daemon.stop();

    let daemon = installation.start().await;
    let retry = post(
        daemon.socket(),
        "/api/v1/goals",
        "survives",
        Some(GOAL_BODY),
    )
    .await;
    assert_eq!(retry.json(), created, "the retry reconciles the same Goal");
    assert_eq!(
        get(daemon.socket(), "/api/v1/goals").await.json()["goals"]
            .as_array()
            .expect("goals")
            .len(),
        1,
        "and creates nothing new"
    );

    daemon.stop();
}

#[tokio::test]
async fn a_restart_verifies_the_durable_configuration_rather_than_adopting_edited_source() {
    // A daemon that activated whatever the source file now says would make
    // every restart a silent configuration change, and an operator could not
    // tell an intentional activation from a restart after an accidental edit.
    let installation = Installation::new("drift");

    let daemon = installation.start().await;
    let before = get(daemon.socket(), "/api/v1/system").await.json();
    let before = before["activeConfiguration"].clone();
    assert_eq!(before["activationSequence"], 1);
    daemon.stop();

    // Edit the source into something that compiles to a different revision.
    let edited =
        support::CONFIGURATION.replace("\"memoryLimitTokens\":4000", "\"memoryLimitTokens\":8000");
    assert_ne!(
        edited,
        support::CONFIGURATION,
        "the edit must actually change the source"
    );
    std::fs::write(installation.configuration(), &edited).expect("write the edited source");

    let daemon = installation.start().await;
    let after = get(daemon.socket(), "/api/v1/system").await.json();
    let active = after["activeConfiguration"].clone();

    assert_eq!(
        active["activationSequence"], before["activationSequence"],
        "the restart must not have activated the edited source"
    );
    assert_eq!(
        active["contentDigest"], before["contentDigest"],
        "the active revision's identity is unchanged"
    );
    assert_eq!(
        active["semanticsLoaded"], false,
        "the drifted source cannot supply the active revision's semantics"
    );
    assert_eq!(
        after["readiness"]["ready"], false,
        "a daemon that cannot interpret its active revision is not ready for new work"
    );

    daemon.stop();
}

#[tokio::test]
async fn a_restart_after_the_source_is_put_back_recovers_the_active_revision() {
    // The other half of the drift rule: verification succeeds again as soon
    // as the source matches the revision that is durably active. Nothing had
    // to be re-activated.
    let installation = Installation::new("recover");

    let daemon = installation.start().await;
    let before = get(daemon.socket(), "/api/v1/system").await.json()["activeConfiguration"].clone();
    daemon.stop();

    let edited =
        support::CONFIGURATION.replace("\"memoryLimitTokens\":4000", "\"memoryLimitTokens\":8000");
    std::fs::write(installation.configuration(), &edited).expect("write");
    let daemon = installation.start().await;
    assert_eq!(
        get(daemon.socket(), "/api/v1/system").await.json()["readiness"]["ready"],
        false
    );
    daemon.stop();

    std::fs::write(installation.configuration(), support::CONFIGURATION).expect("restore");
    let daemon = installation.start().await;
    let after = get(daemon.socket(), "/api/v1/system").await.json();
    assert_eq!(after["activeConfiguration"], before);
    assert_eq!(after["readiness"]["ready"], true);

    daemon.stop();
}

#[tokio::test]
async fn a_daemon_starts_over_a_socket_left_behind_by_a_crash() {
    // `stop` kills the process, so the socket file it bound is still there —
    // which is exactly the state a crash leaves. Refusing to start would turn
    // every crash into manual cleanup.
    let installation = Installation::new("stale-socket");

    let daemon = installation.start().await;
    daemon.stop();
    assert!(
        installation.socket().exists(),
        "the killed daemon left its socket behind"
    );

    let daemon = installation.start().await;
    assert_eq!(
        get(daemon.socket(), "/health/live").await.status,
        StatusCode::OK
    );

    daemon.stop();
}
