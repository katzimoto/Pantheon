//! Parsing what the operator typed.
//!
//! Hand-rolled rather than delegated to an argument-parsing crate. The command
//! set is small and closed, and `docs/development/implementation.md` admits a
//! dependency with the first code that genuinely needs one — a dozen flags is
//! not that.

use std::path::PathBuf;

pub(crate) const USAGE: &str = "\
pantheon — the Pantheon operator client

Usage:
  pantheon [--socket <path>] [--json] <command>

Commands:
  status                       Daemon identity, journal position and readiness
  version                      This client's version
  goal create --objective <text> [goal options]
  goal get <id>
  goal list
  goal cancel <id>
  dispatch status              Dispatch desired state and effective gates
  dispatch pause               Fence new Run-intent commits (durable)
  dispatch resume              Re-open Run-intent commits
  events watch [--after <cursor>]
  events list [--after <cursor>] [--limit <n>]

Goal options:
  --objective <text>           Required. The outcome, not the implementation
  --input <name>=<reference>   Repeatable
  --deliverable <name>:<kind>[:optional]
                               Repeatable. Required unless suffixed :optional
  --permit <effect>            Repeatable. The Goal's effect ceiling
  --forbid <effect>            Repeatable
  --resource <pattern>         Repeatable. The Goal's resource scope

Global options:
  --socket <path>   Operator Control socket.
                    Default: $PANTHEON_SOCKET, else pantheon-data/run/pantheond.sock
  --json            Print the daemon's JSON instead of a rendered summary
  --command-id <id> Reuse an exact command identity, making a repeat of a
                    mutation the same command rather than a second one
  -h, --help        Print this message

pantheon talks to pantheond over a local Unix socket and nowhere else. It has
no offline mode: a command that cannot reach the daemon fails.
";

/// One parsed invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invocation {
    pub socket: PathBuf,
    pub json: bool,
    /// An operator-supplied command identity, for retrying a mutation as the
    /// same command.
    pub command_id: Option<String>,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Status,
    Version,
    GoalCreate(GoalRequest),
    GoalGet {
        id: String,
    },
    GoalList,
    GoalCancel {
        id: String,
    },
    DispatchStatus,
    DispatchPause,
    DispatchResume,
    EventsList {
        after: Option<String>,
        limit: Option<i64>,
    },
    EventsWatch {
        after: Option<String>,
    },
}

/// The Goal an operator described on the command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GoalRequest {
    pub objective: String,
    pub inputs: Vec<(String, String)>,
    /// `(name, kind, required)`.
    pub deliverables: Vec<(String, String, bool)>,
    pub permitted_effects: Vec<String>,
    pub forbidden_effects: Vec<String>,
    pub permitted_resources: Vec<String>,
}

/// A usage failure.
#[derive(Debug)]
pub(crate) struct ArgsError(pub String);

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(detail: impl Into<String>) -> Result<T, ArgsError> {
    Err(ArgsError(detail.into()))
}

impl Invocation {
    /// Parses arguments. `Ok(None)` means help was requested.
    pub(crate) fn parse<I: IntoIterator<Item = String>>(
        args: I,
    ) -> Result<Option<Self>, ArgsError> {
        Self::parse_with(args, |name| std::env::var(name).ok())
    }

    /// Global options are recognized wherever they appear.
    ///
    /// `pantheon status --json` is what an operator types, and a parser that
    /// only accepted globals before the command would silently ignore it.
    /// This is unambiguous because no subcommand defines an option by any of
    /// these names.
    pub(crate) fn parse_with<I: IntoIterator<Item = String>>(
        args: I,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, ArgsError> {
        let mut socket: Option<PathBuf> = None;
        let mut json = false;
        let mut command_id: Option<String> = None;
        let mut rest: Vec<String> = Vec::new();

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => return Ok(None),
                "--json" => json = true,
                "--socket" => socket = Some(PathBuf::from(next(&mut args, "--socket")?)),
                "--command-id" => command_id = Some(next(&mut args, "--command-id")?),
                _ => rest.push(arg),
            }
        }

        let command = parse_command(rest)?;
        let socket = socket
            .or_else(|| env("PANTHEON_SOCKET").map(PathBuf::from))
            .unwrap_or_else(|| {
                PathBuf::from("pantheon-data")
                    .join("run")
                    .join("pantheond.sock")
            });
        Ok(Some(Self {
            socket,
            json,
            command_id,
            command,
        }))
    }
}

fn next<I: Iterator<Item = String>>(args: &mut I, name: &str) -> Result<String, ArgsError> {
    args.next()
        .ok_or_else(|| ArgsError(format!("{name} needs a value")))
}

fn parse_command(words: Vec<String>) -> Result<Command, ArgsError> {
    let mut words = words.into_iter();
    let Some(head) = words.next() else {
        return err("no command given");
    };
    match head.as_str() {
        "status" => nothing_more(words, "status").map(|()| Command::Status),
        "version" => nothing_more(words, "version").map(|()| Command::Version),
        "goal" => parse_goal(words),
        "dispatch" => parse_dispatch(words),
        "events" => parse_events(words),
        other => err(format!("unknown command {other}")),
    }
}

fn parse_dispatch<I: Iterator<Item = String>>(mut words: I) -> Result<Command, ArgsError> {
    match words.next().as_deref() {
        Some("status") => nothing_more(words, "dispatch status").map(|()| Command::DispatchStatus),
        Some("pause") => nothing_more(words, "dispatch pause").map(|()| Command::DispatchPause),
        Some("resume") => nothing_more(words, "dispatch resume").map(|()| Command::DispatchResume),
        other => err(format!(
            "dispatch needs status, pause or resume, got {}",
            other.unwrap_or("nothing")
        )),
    }
}

/// Refuses trailing words a command has no meaning for.
///
/// Ignoring them would let a mistyped option look like it took effect.
fn nothing_more<I: Iterator<Item = String>>(mut words: I, command: &str) -> Result<(), ArgsError> {
    match words.next() {
        None => Ok(()),
        Some(extra) => err(format!("{command} takes no arguments, got {extra}")),
    }
}

fn parse_goal<I: Iterator<Item = String>>(mut words: I) -> Result<Command, ArgsError> {
    let Some(action) = words.next() else {
        return err("goal needs a subcommand: create, get, list or cancel");
    };
    match action.as_str() {
        "list" => nothing_more(words, "goal list").map(|()| Command::GoalList),
        "get" => {
            let id = next(&mut words, "goal get")?;
            nothing_more(words, "goal get").map(|()| Command::GoalGet { id })
        }
        "cancel" => {
            let id = next(&mut words, "goal cancel")?;
            nothing_more(words, "goal cancel").map(|()| Command::GoalCancel { id })
        }
        "create" => Ok(Command::GoalCreate(parse_goal_request(words)?)),
        other => err(format!("unknown goal subcommand {other}")),
    }
}

fn parse_goal_request<I: Iterator<Item = String>>(mut words: I) -> Result<GoalRequest, ArgsError> {
    let mut request = GoalRequest::default();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--objective" => request.objective = next(&mut words, "--objective")?,
            "--input" => {
                let raw = next(&mut words, "--input")?;
                let Some((name, reference)) = raw.split_once('=') else {
                    return err(format!("--input must be <name>=<reference>, got {raw}"));
                };
                request
                    .inputs
                    .push((name.to_string(), reference.to_string()));
            }
            "--deliverable" => {
                let raw = next(&mut words, "--deliverable")?;
                let mut parts = raw.split(':');
                let (Some(name), Some(kind)) = (parts.next(), parts.next()) else {
                    return err(format!(
                        "--deliverable must be <name>:<kind>[:optional], got {raw}"
                    ));
                };
                // A deliverable is required unless it says otherwise. Making
                // the safe reading the default matters: an accidentally
                // optional deliverable would let a plan that cannot produce it
                // validate.
                let required = match parts.next() {
                    None => true,
                    Some("optional") => false,
                    Some(other) => {
                        return err(format!(
                            "--deliverable suffix must be `optional`, got {other}"
                        ));
                    }
                };
                request
                    .deliverables
                    .push((name.to_string(), kind.to_string(), required));
            }
            "--permit" => request
                .permitted_effects
                .push(next(&mut words, "--permit")?),
            "--forbid" => request
                .forbidden_effects
                .push(next(&mut words, "--forbid")?),
            "--resource" => request
                .permitted_resources
                .push(next(&mut words, "--resource")?),
            other => return err(format!("unknown goal create option {other}")),
        }
    }
    if request.objective.trim().is_empty() {
        return err("goal create needs --objective");
    }
    Ok(request)
}

fn parse_events<I: Iterator<Item = String>>(mut words: I) -> Result<Command, ArgsError> {
    let Some(action) = words.next() else {
        return err("events needs a subcommand: watch or list");
    };
    let mut after = None;
    let mut limit = None;
    while let Some(word) = words.next() {
        match word.as_str() {
            "--after" => after = Some(next(&mut words, "--after")?),
            "--limit" => {
                let raw = next(&mut words, "--limit")?;
                limit = Some(
                    raw.parse::<i64>()
                        .map_err(|_| ArgsError(format!("--limit must be a number, got {raw}")))?,
                );
            }
            other => return err(format!("unknown events option {other}")),
        }
    }
    match action.as_str() {
        "watch" => Ok(Command::EventsWatch { after }),
        "list" => Ok(Command::EventsList { after, limit }),
        other => err(format!("unknown events subcommand {other}")),
    }
}

#[cfg(test)]
mod tests;
