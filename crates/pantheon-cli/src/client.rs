//! Speaking Operator Control over a Unix-domain socket.
//!
//! There is exactly one way for this crate to reach Pantheon, and it is here:
//! connect to a filesystem path, speak HTTP/1.1, read JSON. No TCP, no URL
//! authority, no DNS, no connection pool, and no path that answers a question
//! without asking the daemon.
//!
//! A connection per request. A CLI issues one or two, and a pool would buy
//! nothing while adding state that could outlive a command.

use std::path::Path;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderValue, IF_MATCH};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use pantheon_operator_protocol::problem::Problem;
use pantheon_operator_protocol::{COMMAND_EPOCH_HEADER, COMMAND_ID_HEADER, PROBLEM_MEDIA_TYPE};
use tokio::net::UnixStream;

/// What went wrong talking to the daemon.
#[derive(Debug)]
pub(crate) enum ClientError {
    /// The daemon answered with a structured problem.
    Refused(Box<Problem>),
    /// The daemon could not be reached, or did not answer in the protocol.
    Unreachable(String),
}

impl ClientError {
    fn unreachable(detail: impl Into<String>) -> Self {
        Self::Unreachable(detail.into())
    }
}

/// A connection to one daemon socket.
pub(crate) struct Client<'a> {
    socket: &'a Path,
}

/// A response body plus the status it arrived with.
pub(crate) struct Received {
    pub status: StatusCode,
    pub body: Bytes,
}

impl<'a> Client<'a> {
    pub(crate) const fn new(socket: &'a Path) -> Self {
        Self { socket }
    }

    /// Reads a JSON resource.
    pub(crate) async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, ClientError> {
        let received = self.send(self.request("GET", path).body(empty())?).await?;
        decode(received)
    }

    /// Issues a mutation under an explicit command identity.
    ///
    /// The epoch is supplied by the caller, which read it from
    /// `GET /api/v1/system` moments earlier. This client never invents one:
    /// an epoch the daemon has moved past must be refused, and a client that
    /// guessed would have that refusal turned into a silent success.
    pub(crate) async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        epoch: &str,
        command_id: &str,
        if_match: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<T, ClientError> {
        let mut builder = self
            .request("POST", path)
            .header(COMMAND_EPOCH_HEADER, header(epoch)?)
            .header(COMMAND_ID_HEADER, header(command_id)?);
        if let Some(expected) = if_match {
            builder = builder.header(IF_MATCH, header(expected)?);
        }
        let body = match body {
            Some(bytes) => {
                builder =
                    builder.header(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                Full::new(Bytes::from(bytes))
            }
            None => empty(),
        };
        let received = self.send(builder.body(body)?).await?;
        decode(received)
    }

    /// Opens a streaming response, leaving the body unread.
    ///
    /// Used by the Event watch, which must consume frames as they arrive
    /// rather than waiting for a body that never ends.
    pub(crate) async fn stream(&self, path: &str) -> Result<Response<Incoming>, ClientError> {
        let request = self.request("GET", path).body(empty())?;
        let (mut sender, connection) = self.connect().await?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let response = sender
            .send_request(request)
            .await
            .map_err(|err| ClientError::unreachable(format!("request failed: {err}")))?;
        if response.status().is_success() {
            return Ok(response);
        }
        // A stream that never started is an ordinary failure, and the status
        // is still available because SSE has not begun.
        let status = response.status();
        let body = read_body(response).await?;
        Err(refusal(Received { status, body }))
    }

    fn request(&self, method: &str, path: &str) -> hyper::http::request::Builder {
        // A Unix socket has no authority, but HTTP/1.1 requires a Host header.
        // `localhost` is a placeholder that names nothing routable, which is
        // the honest spelling for a connection that never left the machine.
        Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, "localhost")
    }

    async fn connect(
        &self,
    ) -> Result<
        (
            hyper::client::conn::http1::SendRequest<Full<Bytes>>,
            hyper::client::conn::http1::Connection<TokioIo<UnixStream>, Full<Bytes>>,
        ),
        ClientError,
    > {
        let stream = UnixStream::connect(self.socket).await.map_err(|err| {
            ClientError::unreachable(format!(
                "could not reach pantheond at {}: {err}",
                self.socket.display()
            ))
        })?;
        hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|err| ClientError::unreachable(format!("protocol handshake failed: {err}")))
    }

    async fn send(&self, request: Request<Full<Bytes>>) -> Result<Received, ClientError> {
        let (mut sender, connection) = self.connect().await?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let response = sender
            .send_request(request)
            .await
            .map_err(|err| ClientError::unreachable(format!("request failed: {err}")))?;
        let status = response.status();
        let body = read_body(response).await?;
        let received = Received { status, body };
        if status.is_success() {
            Ok(received)
        } else {
            Err(refusal(received))
        }
    }
}

async fn read_body(response: Response<Incoming>) -> Result<Bytes, ClientError> {
    Ok(response
        .into_body()
        .collect()
        .await
        .map_err(|err| ClientError::unreachable(format!("could not read the response: {err}")))?
        .to_bytes())
}

/// Interprets a non-success response.
///
/// A body that does not decode as a Problem is not turned into one. The
/// daemon's error vocabulary is a contract, and inventing a problem code for
/// something that did not carry one would make a transport failure
/// indistinguishable from a refusal.
fn refusal(received: Received) -> ClientError {
    match serde_json::from_slice::<Problem>(&received.body) {
        Ok(problem) => ClientError::Refused(Box::new(problem)),
        Err(_) => ClientError::Unreachable(format!(
            "pantheond answered {} without a {PROBLEM_MEDIA_TYPE} body",
            received.status
        )),
    }
}

fn decode<T: serde::de::DeserializeOwned>(received: Received) -> Result<T, ClientError> {
    serde_json::from_slice(&received.body).map_err(|err| {
        ClientError::unreachable(format!("could not understand pantheond's response: {err}"))
    })
}

fn empty() -> Full<Bytes> {
    Full::new(Bytes::new())
}

fn header(value: &str) -> Result<HeaderValue, ClientError> {
    HeaderValue::from_str(value)
        .map_err(|_| ClientError::unreachable(format!("{value} is not a valid header value")))
}

impl From<hyper::http::Error> for ClientError {
    fn from(err: hyper::http::Error) -> Self {
        Self::unreachable(format!("could not build the request: {err}"))
    }
}

#[cfg(test)]
mod tests;
