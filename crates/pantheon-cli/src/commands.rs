//! Running one operator command.

mod events;
mod goals;

use pantheon_operator_protocol::API_PREFIX;
use pantheon_operator_protocol::problem::Problem;
use pantheon_operator_protocol::system::SystemResponse;

use crate::args::{Command, Invocation};
use crate::client::{Client, ClientError};
use crate::render;

/// Why a command did not succeed.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The daemon refused the request and said why, in its own vocabulary.
    Refused(Box<Problem>),
    /// The daemon could not be reached, or did not speak the protocol.
    Unreachable(String),
    /// The operator asked for something that cannot be built into a request.
    Usage(String),
}

impl From<ClientError> for Failure {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::Refused(problem) => Self::Refused(problem),
            ClientError::Unreachable(detail) => Self::Unreachable(detail),
        }
    }
}

pub(crate) async fn run(invocation: &Invocation) -> Result<(), Failure> {
    // `version` is the one command that answers without the daemon, because
    // it is a fact about this binary. Everything else asks.
    if matches!(invocation.command, Command::Version) {
        println!("pantheon {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let client = Client::new(&invocation.socket);
    match &invocation.command {
        Command::Version => unreachable!("handled above"),
        Command::Status => {
            let system: SystemResponse = client.get(&path("/system")).await?;
            emit(invocation, &system, || render::system(&system));
            Ok(())
        }
        Command::GoalCreate(request) => goals::create(invocation, &client, request).await,
        Command::GoalGet { id } => goals::get(invocation, &client, id).await,
        Command::GoalList => goals::list(invocation, &client).await,
        Command::GoalCancel { id } => goals::cancel(invocation, &client, id).await,
        Command::EventsList { after, limit } => {
            events::list(invocation, &client, after.as_deref(), *limit).await
        }
        Command::EventsWatch { after } => {
            events::watch(invocation, &client, after.as_deref()).await
        }
    }
}

/// The full path of a versioned resource.
pub(crate) fn path(suffix: &str) -> String {
    format!("{API_PREFIX}{suffix}")
}

/// Prints either the daemon's own JSON or a rendered summary.
///
/// `--json` prints the *response*, re-serialized from the typed value rather
/// than echoed from the socket: what a script consumes is then exactly what
/// this client understood, not bytes it may have ignored fields from.
pub(crate) fn emit<T: serde::Serialize>(
    invocation: &Invocation,
    value: &T,
    rendered: impl FnOnce() -> String,
) {
    if invocation.json {
        match serde_json::to_string_pretty(value) {
            Ok(text) => println!("{text}"),
            Err(err) => eprintln!("pantheon: could not render JSON: {err}"),
        }
    } else {
        println!("{}", rendered());
    }
}

/// The `commandEpoch` a mutation must carry, read immediately before issuing
/// it.
///
/// Read every time rather than cached: the epoch is what fences a command
/// against a disaster restore, and a cached one would defeat exactly the case
/// the fence exists for.
pub(crate) async fn command_epoch(client: &Client<'_>) -> Result<String, Failure> {
    let system: SystemResponse = client.get(&path("/system")).await?;
    Ok(system.command_epoch)
}

/// A fresh single-use command id.
///
/// Drawn from the operating system's randomness. A weak id would collide with
/// another command, and the daemon would fail that closed as a conflict —
/// correct, but a failure the client should not be manufacturing. There is no
/// fallback to a counter or a timestamp for that reason.
pub(crate) fn fresh_command_id() -> Result<String, Failure> {
    use std::fmt::Write as _;
    use std::io::Read as _;

    let mut file = std::fs::File::open("/dev/urandom").map_err(random_unavailable)?;
    let mut bytes = [0u8; 16];
    file.read_exact(&mut bytes).map_err(random_unavailable)?;

    let mut id = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }
    Ok(id)
}

fn random_unavailable(err: std::io::Error) -> Failure {
    Failure::Unreachable(format!(
        "could not draw a command id from /dev/urandom: {err}"
    ))
}

/// The command identity for one mutation: the operator's, or a fresh one.
pub(crate) fn command_id(invocation: &Invocation) -> Result<String, Failure> {
    match &invocation.command_id {
        Some(id) if id.trim().is_empty() => {
            Err(Failure::Usage("--command-id must not be empty".to_string()))
        }
        Some(id) => Ok(id.clone()),
        None => fresh_command_id(),
    }
}
