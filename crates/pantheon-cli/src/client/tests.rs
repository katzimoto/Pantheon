//! Evidence for the transport: it reaches a Unix socket and nothing else, and
//! it does not invent an outcome the daemon did not send.

use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

use crate::client::{Client, ClientError};

struct Stub {
    dir: PathBuf,
    socket: PathBuf,
}

impl Stub {
    /// A one-shot HTTP/1.1 responder on a Unix socket.
    ///
    /// Hand-written rather than borrowed from a server crate: the point is to
    /// exercise this client's behaviour against exact bytes, including bytes
    /// no real server would send.
    fn serve(label: &str, response: &'static str) -> (Self, tokio::task::JoinHandle<Vec<u8>>) {
        // Short by necessity: a Unix socket address is bounded by `SUN_LEN`.
        let dir = PathBuf::from(format!("/tmp/pc-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let socket = dir.join("s.sock");

        let listener = UnixListener::bind(&socket).expect("bind stub");
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0u8; 4096];
            let read = stream.read(&mut request).await.expect("read request");
            request.truncate(read);
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            stream.shutdown().await.expect("shutdown");
            request
        });
        (Self { dir, socket }, handle)
    }

    fn path(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ok_body(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn a_get_decodes_the_daemons_json() {
    let response: &'static str = Box::leak(ok_body(r#"{"live":true}"#).into_boxed_str());
    let (stub, handle) = Stub::serve("get", response);
    let client = Client::new(stub.path());

    let value: pantheon_operator_protocol::system::LivenessResponse =
        client.get("/health/live").await.expect("decodes");
    assert!(value.live);

    let request = String::from_utf8(handle.await.expect("stub")).expect("utf-8");
    assert!(
        request.starts_with("GET /health/live HTTP/1.1"),
        "{request}"
    );
    // HTTP/1.1 requires a Host header even though a Unix socket has no
    // authority; a request without one is not a valid HTTP/1.1 request.
    assert!(
        request.to_lowercase().contains("host: localhost"),
        "{request}"
    );
}

#[tokio::test]
async fn a_mutation_carries_the_command_identity_headers_it_was_given() {
    // These two headers are the entire idempotency and authority fence. A
    // client that omitted or renamed one would have every mutation refused —
    // or worse, accepted as a new command on a retry.
    let response: &'static str = Box::leak(ok_body(r#"{"live":true}"#).into_boxed_str());
    let (stub, handle) = Stub::serve("post-headers", response);
    let client = Client::new(stub.path());

    let _: pantheon_operator_protocol::system::LivenessResponse = client
        .post(
            "/api/v1/goals",
            "epoch-1",
            "command-1",
            None,
            Some(b"{}".to_vec()),
        )
        .await
        .expect("decodes");

    let request = String::from_utf8(handle.await.expect("stub")).expect("utf-8");
    let lower = request.to_lowercase();
    assert!(
        lower.contains("pantheon-command-epoch: epoch-1"),
        "{request}"
    );
    assert!(
        lower.contains("pantheon-command-id: command-1"),
        "{request}"
    );
    assert!(
        lower.contains("content-type: application/json"),
        "{request}"
    );
    assert!(request.ends_with("{}"), "{request}");
}

#[tokio::test]
async fn a_structured_refusal_is_reported_as_the_daemons_own_problem() {
    let body = r#"{"type":"urn:pantheon:problem:not-found","title":"Resource not found","status":404,"detail":"no such goal: x","code":"not-found"}"#;
    let response: &'static str = Box::leak(
        format!(
            "HTTP/1.1 404 Not Found\r\ncontent-type: application/problem+json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_boxed_str(),
    );
    let (stub, _handle) = Stub::serve("refusal", response);
    let client = Client::new(stub.path());

    let err = client
        .get::<serde_json::Value>("/api/v1/goals/x")
        .await
        .expect_err("must refuse");
    match err {
        ClientError::Refused(problem) => {
            assert_eq!(
                problem.code,
                pantheon_operator_protocol::problem::ProblemCode::NotFound
            );
            assert_eq!(problem.status, 404);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn an_error_without_a_problem_body_is_not_turned_into_one() {
    // Manufacturing a problem code for a body that carried none would make a
    // broken intermediary indistinguishable from a deliberate refusal, and
    // the error vocabulary is a contract.
    let response: &'static str = "HTTP/1.1 502 Bad Gateway\r\ncontent-length: 3\r\n\r\nbad";
    let (stub, _handle) = Stub::serve("no-problem", response);
    let client = Client::new(stub.path());

    let err = client
        .get::<serde_json::Value>("/api/v1/system")
        .await
        .expect_err("must fail");
    assert!(
        matches!(err, ClientError::Unreachable(detail) if detail.contains("502")),
        "a non-problem error must stay a transport failure"
    );
}

#[tokio::test]
async fn a_socket_that_is_not_there_is_a_transport_failure_not_a_refusal() {
    let client = Client::new(Path::new("/tmp/pantheon-absent.sock"));
    let err = client
        .get::<serde_json::Value>("/api/v1/system")
        .await
        .expect_err("must fail");
    assert!(
        matches!(err, ClientError::Unreachable(_)),
        "an unreachable daemon is never a refusal"
    );
}
