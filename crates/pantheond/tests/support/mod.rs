//! A real daemon on a real socket, and a client that speaks to it.
//!
//! Nothing here calls the engine or the store directly. The point of these
//! tests is the transport an operator actually uses, and a test that reached
//! around it would prove nothing about it.

// A shared test module is compiled into each integration test binary that
// declares it, so items only one binary uses look dead, and `pub` items look
// unreachable from outside a crate that has no outside. Both are artefacts of
// how integration tests share code, not defects.
#![allow(dead_code, unreachable_pub)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

/// The smallest configuration that compiles and resolves the DIRECT planner's
/// evaluator reference.
pub const CONFIGURATION: &str = r#"{
  "agents": [{"name":"builder","version":1,"accepts":["code-change"],"competencies":["rust"],
    "routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"]}],
  "routing": {"policies":[{"name":"default","ordering":["featureMatch"],"tieBreak":"backendId"}]},
  "execution": {
    "profiles":[{"name":"strict","isolationClass":"CONTAINER",
      "guarantees":["isolation.control-plane"],"networkMode":"NONE",
      "environmentIdentity":"sha256:image"}],
    "backends":[{"backendId":"fake-local","enabled":true,"selector":"fake"}]},
  "evaluators": {
    "versions":[{"id":"unit-v1","kind":"check","argv":["/bin/check"],"timeoutMs":1000,
      "sandboxProfile":"strict","resultProtocol":"p-v1"}],
    "refs":[{"ref":"check://project/unit-tests","currentVersion":"unit-v1"}]},
  "context": {"schemaVersion":1,"mandatorySections":["task"],"preloadPriority":["task"],
    "memoryLimitTokens":4000,"workspaceOrientationLimitTokens":2000,
    "safetyMarginTokens":512,"optionalDropOrder":["memory"]},
  "authorization": {"schemaVersion":1,"rules":[{"action":"filesystem.read","effect":"permit"}]}
}"#;

/// An installation directory that survives across daemon restarts.
pub struct Installation {
    pub dir: PathBuf,
}

impl Installation {
    /// Deliberately short: a Unix socket address is bounded by `SUN_LEN`, and
    /// the platform temporary directory is long enough on some systems that a
    /// conventional fixture path exceeds it.
    pub fn new(label: &str) -> Self {
        let dir = PathBuf::from(format!("/tmp/pd-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the installation directory");
        std::fs::write(dir.join("configuration.json"), CONFIGURATION).expect("write configuration");
        Self { dir }
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("s.sock")
    }

    pub fn configuration(&self) -> PathBuf {
        self.dir.join("configuration.json")
    }

    /// Starts a daemon against this installation and waits for its socket.
    // The child is moved into `Daemon` before this function can fail, and
    // `Daemon` both kills and waits in `Drop`. Clippy cannot see through the
    // struct field, so the lint is opted out of here rather than satisfied by
    // restructuring ownership in a way that would make the panic path leak.
    #[expect(
        clippy::zombie_processes,
        reason = "Daemon::drop kills and waits on every path"
    )]
    pub async fn start(&self) -> Daemon {
        let child = Command::new(env!("CARGO_BIN_EXE_pantheond"))
            .arg("--data-dir")
            .arg(&self.dir)
            .arg("--socket")
            .arg(self.socket())
            // Discarded rather than piped: nothing reads these, and a daemon
            // that filled a pipe nobody drains would block on its own output.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pantheond");

        let socket = self.socket();
        for _ in 0..200 {
            if UnixStream::connect(&socket).await.is_ok() {
                return Daemon { child, socket };
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("pantheond did not start serving {}", socket.display());
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A running daemon.
pub struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Stops the daemon and waits for it to exit.
    ///
    /// Explicit rather than left to `Drop`: a test that restarts the daemon
    /// must know the first one is gone before the second binds the socket.
    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One HTTP response, decoded far enough to assert on.
pub struct Answer {
    pub status: StatusCode,
    pub etag: Option<String>,
    pub body: Bytes,
}

impl Answer {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|err| {
            panic!(
                "response was not JSON ({err}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The Pantheon problem code, when the answer carried one.
    pub fn code(&self) -> String {
        self.json()["code"]
            .as_str()
            .unwrap_or_else(|| panic!("no problem code in {}", String::from_utf8_lossy(&self.body)))
            .to_string()
    }
}

/// Sends one request over the daemon's socket.
pub async fn request(
    socket: &Path,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Answer {
    let stream = UnixStream::connect(socket)
        .await
        .unwrap_or_else(|err| panic!("connect {}: {err}", socket.display()));
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .expect("handshake");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(hyper::header::HOST, "localhost");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Full::new(Bytes::from(body.unwrap_or_default().to_string())))
        .expect("build request");

    let response: Response<hyper::body::Incoming> =
        sender.send_request(request).await.expect("send request");
    let status = response.status();
    let etag = response
        .headers()
        .get(hyper::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    Answer { status, etag, body }
}

pub async fn get(socket: &Path, path: &str) -> Answer {
    request(socket, "GET", path, &[], None).await
}

/// The status of a request whose body must not be read to completion.
///
/// An Event stream never ends, so collecting its body would hang forever.
/// Reading only the head is enough to establish that a route exists.
pub async fn status_only(
    socket: &Path,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> StatusCode {
    let stream = UnixStream::connect(socket)
        .await
        .unwrap_or_else(|err| panic!("connect {}: {err}", socket.display()));
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .expect("handshake");
    let pump = tokio::spawn(async move {
        let _ = connection.await;
    });

    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(hyper::header::HOST, "localhost");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Full::new(Bytes::from(body.unwrap_or_default().to_string())))
        .expect("build request");
    let response = sender.send_request(request).await.expect("send request");
    let status = response.status();
    // Dropping the response body closes the connection, which is what ends a
    // stream the test is not going to read.
    drop(response);
    drop(sender);
    pump.abort();
    status
}

/// The status of a mutation, without reading its body.
pub async fn post_status(
    socket: &Path,
    path: &str,
    command_id: &str,
    body: Option<&str>,
) -> StatusCode {
    let epoch = command_epoch(socket).await;
    let mut headers = vec![
        ("pantheon-command-epoch", epoch.as_str()),
        ("pantheon-command-id", command_id),
    ];
    if body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    status_only(socket, "POST", path, &headers, body).await
}

/// Issues a mutation, reading the command epoch first exactly as a client does.
pub async fn post(socket: &Path, path: &str, command_id: &str, body: Option<&str>) -> Answer {
    let epoch = command_epoch(socket).await;
    let mut headers = vec![
        ("pantheon-command-epoch", epoch.as_str()),
        ("pantheon-command-id", command_id),
    ];
    if body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    request(socket, "POST", path, &headers, body).await
}

pub async fn command_epoch(socket: &Path) -> String {
    get(socket, "/api/v1/system").await.json()["commandEpoch"]
        .as_str()
        .expect("commandEpoch")
        .to_string()
}

pub const GOAL_BODY: &str = r#"{"goal":{
  "objective":"Fix the checkout timeout with the smallest safe change.",
  "inputs":[{"name":"repository","reference":"repo://whiskyshop"}],
  "deliverables":[{"name":"changeset","kind":"code.changeset","required":true}],
  "constraints":{
    "permittedEffects":["filesystem.read","filesystem.write"],
    "forbiddenEffects":["git.push"],
    "permittedResources":["workspace://src/**"]}}}"#;
