//! Evidence that every route this crate serves is described.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::description::DESCRIPTION;
use pantheon_operator_protocol::API_PREFIX;
use pantheon_operator_protocol::problem::ProblemCode;

fn description() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DESCRIPTION);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()));
    serde_json::from_str(&text).expect("the API description is JSON")
}

fn documented_paths() -> BTreeSet<String> {
    description()["paths"]
        .as_object()
        .expect("paths is an object")
        .keys()
        .cloned()
        .collect()
}

/// The paths [`crate::router`] registers, read from its own source.
///
/// Read rather than introspected because axum's `Router` does not expose its
/// route table. The alternative — a hand-maintained list beside the router —
/// is a second thing to forget, and forgetting it would make this test pass
/// while the router drifted.
fn routed_paths() -> BTreeSet<String> {
    let source =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
            .expect("read the router source");

    let mut paths = BTreeSet::new();
    let mut rest = source.as_str();
    while let Some(at) = rest.find(".route(\"") {
        rest = &rest[at + ".route(\"".len()..];
        let end = rest.find('"').expect("an unterminated route path");
        let path = &rest[..end];
        // The versioned routes are registered under a `nest`, so their
        // registered path is the suffix. Both spellings are recorded; the
        // health routes are registered unversioned only, and their prefixed
        // spellings simply match nothing.
        paths.insert(path.to_string());
        paths.insert(format!("{API_PREFIX}{path}"));
        rest = &rest[end..];
    }
    paths
}

#[test]
fn every_route_the_daemon_serves_is_described() {
    let documented = documented_paths();
    let routed = routed_paths();

    // A versioned route is described if either its suffixed or its
    // fully-spelled form appears in the document. Health is registered
    // unversioned only and never carries the prefix.
    let mut missing = Vec::new();
    for path in &routed {
        let Some(bare) = path.strip_prefix(API_PREFIX) else {
            continue;
        };
        if !documented.contains(path) && !documented.contains(bare) {
            missing.push(path.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "these routes are served but not described in {DESCRIPTION}: {missing:?}"
    );
}

#[test]
fn every_described_path_is_one_this_crate_actually_registers() {
    // The reverse direction. A path in the document that no route registers
    // would promise an operation that answers 404.
    let routed = routed_paths();
    let undescribed: Vec<String> = documented_paths()
        .into_iter()
        .filter(|path| !routed.contains(path))
        .collect();
    assert!(
        undescribed.is_empty(),
        "these paths are described but not routed: {undescribed:?}"
    );
}

/// Every problem code this build's surface can return.
///
/// One list so the vocabulary test and the status tests cannot disagree about
/// how many codes exist.
fn implemented_codes() -> &'static [ProblemCode] {
    &[
        ProblemCode::NotFound,
        ProblemCode::Validation,
        ProblemCode::PreconditionRequired,
        ProblemCode::StaleRevision,
        ProblemCode::StaleCommandEpoch,
        ProblemCode::Conflict,
        ProblemCode::CursorGone,
        ProblemCode::TemporarilyUnavailable,
        ProblemCode::Internal,
    ]
}

#[test]
fn the_described_problem_codes_are_exactly_the_ones_this_build_can_return() {
    // The error vocabulary is a compatibility promise. A code in the document
    // that nothing returns is a promise no path keeps; a code this build
    // returns that is absent from the document is one a client cannot prepare
    // for.
    let described: BTreeSet<String> =
        description()["components"]["schemas"]["Problem"]["properties"]["code"]["enum"]
            .as_array()
            .expect("the problem code enum")
            .iter()
            .map(|value| value.as_str().expect("a code").to_string())
            .collect();

    let implemented: BTreeSet<String> = implemented_codes()
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();

    assert_eq!(described, implemented);
}

#[test]
fn every_described_status_is_one_the_problem_code_actually_carries() {
    // The status a code is served with is decided once, in the canonical
    // contract's status table (`docs/architecture/operations/
    // public-daemon-api-and-cli.md`, "Problem Details"), implemented by
    // `ProblemCode::status`. Pinning each value here means a mapping that
    // changes in either place without the other fails this test.
    for (code, expected) in [
        (ProblemCode::NotFound, 404),
        (ProblemCode::Validation, 400),
        (ProblemCode::CursorGone, 410),
        (ProblemCode::StaleRevision, 412),
        (ProblemCode::PreconditionRequired, 428),
        (ProblemCode::StaleCommandEpoch, 409),
        (ProblemCode::Conflict, 409),
        (ProblemCode::Internal, 500),
        (ProblemCode::TemporarilyUnavailable, 503),
    ] {
        assert_eq!(code.status(), expected, "{}", code.as_str());
    }

    // And the document's own response keys use those statuses.
    let paths = description();
    let create = &paths["paths"]["/api/v1/goals"]["post"]["responses"];
    for status in ["201", "400", "409", "428", "500", "503"] {
        assert!(
            create.get(status).is_some(),
            "POST /api/v1/goals must describe {status}"
        );
    }
    let cancel = &paths["paths"]["/api/v1/goals/{goalId}/actions/cancel"]["post"]["responses"];
    assert!(
        cancel.get("202").is_some(),
        "cancellation is accepted, not completed, and must be described as 202"
    );
    assert!(
        cancel.get("200").is_none(),
        "a 200 would imply the Goal reached Cancelled"
    );
}

#[test]
fn no_operation_promises_a_problem_status_no_code_carries() {
    // Every response documented with a problem body must use one of the
    // statuses the codes above actually map to. A `403` problem response, say,
    // would promise an error shape nothing can produce.
    let carried: BTreeSet<u16> = implemented_codes().iter().map(|c| c.status()).collect();

    let mut offenders = Vec::new();
    for (path, operations) in description()["paths"]
        .as_object()
        .expect("paths is an object")
    {
        for (method, operation) in operations.as_object().expect("operations are objects") {
            for key in operation["responses"]
                .as_object()
                .expect("responses")
                .keys()
            {
                let promises_problem = operation["responses"][key]["content"]
                    .get("application/problem+json")
                    .is_some();
                if !promises_problem {
                    continue;
                }
                let status: u16 = key.parse().expect("a numeric response key");
                if !carried.contains(&status) {
                    offenders.push(format!("{method} {path} -> {status}"));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these operations document problem responses under statuses no code carries: {offenders:?}"
    );
}
