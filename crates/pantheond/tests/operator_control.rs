//! The acceptance path of Issue #26, over the transport an operator uses.
//!
//! Every assertion here goes through a real `pantheond` process and a real
//! Unix socket. Nothing calls the engine or the store: a test that did would
//! prove the operations work while saying nothing about whether they are
//! reachable.

mod support;

use hyper::StatusCode;
use support::{GOAL_BODY, Installation, get, post, post_status, request, status_only};

#[tokio::test]
async fn the_whole_operator_path_works_over_the_socket() {
    let installation = Installation::new("path");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    // system: the client learns the command epoch it must carry.
    let system = get(socket, "/api/v1/system").await;
    assert_eq!(system.status, StatusCode::OK);
    let system = system.json();
    assert_eq!(system["apiVersions"], serde_json::json!(["v1"]));
    assert!(system["readiness"]["ready"].as_bool().expect("ready"));
    assert_ne!(
        system["commandEpoch"], system["journal"]["epoch"],
        "the command epoch and the journal epoch are different continuity facts"
    );

    // create: one request reaches a Ready Task.
    let created = post(socket, "/api/v1/goals", "create-1", Some(GOAL_BODY)).await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert!(created.etag.is_some(), "a mutable resource carries an ETag");
    let goal = created.json();
    let goal_id = goal["id"].as_str().expect("id").to_string();
    assert_eq!(goal["phase"], "Active");
    assert_eq!(goal["tasks"].as_array().expect("tasks").len(), 1);
    assert_eq!(goal["tasks"][0]["phase"], "Ready");

    // get: the same Goal, and an ETag that matches its row revision.
    let fetched = get(socket, &format!("/api/v1/goals/{goal_id}")).await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(fetched.etag, created.etag);
    assert_eq!(fetched.json(), goal);

    // list: with the cursor the list corresponds to.
    let listed = get(socket, "/api/v1/goals").await.json();
    assert_eq!(listed["goals"].as_array().expect("goals").len(), 1);
    let cursor = listed["snapshotCursor"]
        .as_str()
        .expect("cursor")
        .to_string();

    // Nothing has happened since the list, so watching after its cursor is
    // empty — which is what makes the cursor safe to watch from.
    let empty = get(socket, &format!("/api/v1/events?after={cursor}"))
        .await
        .json();
    assert_eq!(empty["events"].as_array().expect("events").len(), 0);
    assert_eq!(empty["nextCursor"], cursor);

    // cancel: accepted, and visibly not completed.
    let cancelled = post(
        socket,
        &format!("/api/v1/goals/{goal_id}/actions/cancel"),
        "cancel-1",
        None,
    )
    .await;
    assert_eq!(cancelled.status, StatusCode::ACCEPTED);
    // The ETag must move even though the semantic GoalRevision did not. This
    // is the whole reason it is derived from the authoritative row revision:
    // a cached representation of a Goal that has since been fenced is stale,
    // and an ETag tracking goalRevision would still call it fresh.
    assert_ne!(
        cancelled.etag, created.etag,
        "a lifecycle transition must invalidate the cached representation"
    );
    let cancelled = cancelled.json();
    assert_eq!(cancelled["phase"], "Finalizing");
    assert_eq!(cancelled["tasks"][0]["phase"], "Finalizing");
    assert_eq!(
        cancelled["goalRevision"], goal["goalRevision"],
        "cancellation is a lifecycle transition, not a semantic revision"
    );
    assert_ne!(
        cancelled["revision"], goal["revision"],
        "but the authoritative row revision did advance"
    );

    // events: the cancellation is reachable from the cursor taken before it.
    let after = get(socket, &format!("/api/v1/events?after={cursor}"))
        .await
        .json();
    let events = after["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["eventType"], "goal.cancel.requested");
    assert_eq!(events[0]["commandId"], "cancel-1");
    assert_ne!(
        events[0]["commandEpoch"], events[0]["cursor"],
        "an Event's command epoch is not its journal position"
    );

    daemon.stop();
}

#[tokio::test]
async fn a_retried_create_reconciles_the_first_outcome_instead_of_creating_a_second_goal() {
    // The command kernel's replay carries no value, so this can only work if
    // the Goal identity is derivable from the command identity.
    let installation = Installation::new("retry");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    let first = post(socket, "/api/v1/goals", "same", Some(GOAL_BODY)).await;
    let second = post(socket, "/api/v1/goals", "same", Some(GOAL_BODY)).await;
    assert_eq!(first.status, second.status);
    assert_eq!(first.json(), second.json());
    assert_eq!(first.etag, second.etag);
    assert_eq!(
        get(socket, "/api/v1/goals").await.json()["goals"]
            .as_array()
            .expect("goals")
            .len(),
        1
    );

    daemon.stop();
}

#[tokio::test]
async fn reusing_a_command_id_for_a_different_request_fails_closed() {
    let installation = Installation::new("conflict");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    post(socket, "/api/v1/goals", "reused", Some(GOAL_BODY)).await;
    let different = GOAL_BODY.replace("Fix the checkout timeout", "Something else entirely");
    let answer = post(socket, "/api/v1/goals", "reused", Some(&different)).await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.code(), "conflict");
    assert_eq!(
        get(socket, "/api/v1/goals").await.json()["goals"]
            .as_array()
            .expect("goals")
            .len(),
        1,
        "the conflicting request must not have created anything"
    );

    daemon.stop();
}

#[tokio::test]
async fn a_superseded_command_epoch_is_refused_rather_than_corrected() {
    let installation = Installation::new("epoch");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    let answer = request(
        socket,
        "POST",
        "/api/v1/goals",
        &[
            ("pantheon-command-epoch", &"0".repeat(32)),
            ("pantheon-command-id", "stale"),
            ("content-type", "application/json"),
        ],
        Some(GOAL_BODY),
    )
    .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.code(), "stale-command-epoch");
    assert!(
        get(socket, "/api/v1/goals").await.json()["goals"]
            .as_array()
            .expect("goals")
            .is_empty(),
        "a fenced command must write nothing"
    );

    daemon.stop();
}

#[tokio::test]
async fn a_mutation_without_command_identity_is_refused_as_a_missing_precondition() {
    let installation = Installation::new("no-identity");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    let answer = request(
        socket,
        "POST",
        "/api/v1/goals",
        &[("content-type", "application/json")],
        Some(GOAL_BODY),
    )
    .await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(answer.code(), "precondition-required");

    daemon.stop();
}

#[tokio::test]
async fn an_unreachable_journal_position_is_reported_rather_than_restarted_from_the_head() {
    // Silently restarting at the head would drop exactly the Events the
    // caller asked not to miss.
    let installation = Installation::new("cursor");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    for query in [
        format!("after={}:1", "0".repeat(32)),
        "after=not-a-cursor".to_string(),
    ] {
        let answer = get(socket, &format!("/api/v1/events?{query}")).await;
        assert_eq!(answer.status, StatusCode::GONE, "{query}");
        assert_eq!(answer.code(), "cursor-gone", "{query}");
    }

    // The stream validates its starting position before committing to a 200,
    // because after that there is no status left to send.
    let answer = get(
        socket,
        &format!("/api/v1/events/watch?after={}:1", "0".repeat(32)),
    )
    .await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(answer.code(), "cursor-gone");
    // A watch that *is* reachable opens a stream, so only its head is read.
    assert_eq!(
        status_only(socket, "GET", "/api/v1/events/watch", &[], None).await,
        StatusCode::OK
    );

    daemon.stop();
}

#[tokio::test]
async fn an_unknown_goal_and_an_unknown_route_are_both_ordinary_refusals() {
    let installation = Installation::new("not-found");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    let missing = get(socket, "/api/v1/goals/goal-nope").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.code(), "not-found");

    // A route the mission does not serve answers 404 rather than something
    // that looks like an unimplemented feature.
    assert_eq!(
        get(socket, "/api/v1/dispatch").await.status,
        StatusCode::NOT_FOUND
    );

    daemon.stop();
}

#[tokio::test]
async fn cancelling_a_goal_that_is_already_cancelling_stays_accepted_and_changes_nothing() {
    let installation = Installation::new("cancel-twice");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    let goal = post(socket, "/api/v1/goals", "create", Some(GOAL_BODY))
        .await
        .json();
    let goal_id = goal["id"].as_str().expect("id").to_string();
    let path = format!("/api/v1/goals/{goal_id}/actions/cancel");

    let once = post(socket, &path, "cancel-1", None).await;
    // A different command id, so this is a second execution rather than a
    // replay of the first.
    let twice = post(socket, &path, "cancel-2", None).await;

    assert_eq!(once.status, StatusCode::ACCEPTED);
    assert_eq!(twice.status, StatusCode::ACCEPTED);
    assert_eq!(once.json(), twice.json());
    assert_eq!(once.etag, twice.etag, "no second revision was burned");

    daemon.stop();
}

#[tokio::test]
async fn health_probes_are_unversioned_and_the_versioned_spelling_is_gone() {
    // The contract settles the spelling #26 had to serve both ways: the
    // canonical probes are `/health/live` and `/health/ready`, outside the
    // version prefix, and — Pantheon being unreleased — no
    // `/api/v1/health/...` alias is kept. An operator still carrying the old
    // document must get a plain 404, not a silently working second spelling.
    let installation = Installation::new("health");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    let live = get(socket, "/health/live").await;
    assert_eq!(live.status, StatusCode::OK);
    assert_eq!(live.json()["live"], serde_json::json!(true));

    let ready = get(socket, "/health/ready").await;
    assert_eq!(ready.status, StatusCode::OK);
    assert_eq!(ready.json()["ready"], serde_json::json!(true));

    for alias in ["/api/v1/health/live", "/api/v1/health/ready"] {
        assert_eq!(
            get(socket, alias).await.status,
            StatusCode::NOT_FOUND,
            "{alias} must not be served"
        );
    }

    daemon.stop();
}

#[tokio::test]
async fn every_described_operation_actually_routes() {
    // The direction the API description's own tests cannot prove: that each
    // documented operation reaches a handler over the real transport rather
    // than answering 404 or 405.
    let installation = Installation::new("described");
    let daemon = installation.start().await;
    let socket = daemon.socket();

    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schemas/operator-control-v1.openapi.json"),
    )
    .expect("read the API description");
    let description: serde_json::Value = serde_json::from_str(&text).expect("JSON");

    let goal = post(socket, "/api/v1/goals", "create", Some(GOAL_BODY))
        .await
        .json();
    let goal_id = goal["id"].as_str().expect("id").to_string();

    let mut checked = 0;
    for (path, operations) in description["paths"].as_object().expect("paths") {
        let concrete = path.replace("{goalId}", &goal_id);
        for method in operations.as_object().expect("operations").keys() {
            // Only the head is read. The Event stream never ends, so
            // collecting its body would hang rather than fail.
            let status = match method.as_str() {
                "get" => status_only(socket, "GET", &concrete, &[], None).await,
                "post" => {
                    let body = (concrete == "/api/v1/goals").then_some(GOAL_BODY);
                    post_status(socket, &concrete, &format!("probe-{checked}"), body).await
                }
                other => panic!("undescribed method {other}"),
            };
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {concrete} is described but does not route"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {concrete} is described but the method is not served"
            );
            checked += 1;
        }
    }
    // Nine: two health probes, system, goals (list/create/get/cancel), and
    // events (list/watch). The versioned health aliases the contract once
    // served both ways are gone, so this is one fewer operation per spelling.
    assert!(checked >= 9, "the sweep must actually have run: {checked}");

    daemon.stop();
}
