//! `pantheon`: the operator command-line client.
//!
//! Built from package `pantheon-cli`; the binary is `pantheon`.
//!
//! # Owns
//!
//! The operator's command-line experience: argument parsing, output rendering,
//! exit codes, and speaking the Operator Control protocol to `pantheond`.
//!
//! # Must not own
//!
//! Control-plane authority of any kind. The CLI is a client, never a second
//! path into Pantheon's state, so it must not reach the database, the engine or
//! the daemon's internals. Its only permitted internal dependency is
//! `pantheon-operator-protocol`, which leaves it structurally unable to bypass
//! the daemon rather than merely discouraged from doing so —
//! `scripts/check-crate-deps.sh` enforces that ceiling.
//!
//! There is no offline mode and no "helpful" local fallback. A command that
//! cannot reach `pantheond` fails; it does not answer from somewhere else.
//!
//! # Command identity
//!
//! Every mutation carries `(commandEpoch, commandId)`. The epoch is read from
//! `GET /api/v1/system` immediately before the mutation, so a client that
//! slept through a disaster restore is refused rather than having its retry
//! accepted as new work. The id defaults to a fresh random value; `--command-id`
//! makes a retry *the same command* instead of a second one, which is the only
//! way to safely repeat a request whose outcome is unknown.

mod args;
mod client;
mod commands;
mod render;

use std::process::ExitCode;

use crate::args::{Invocation, USAGE};

/// Exit codes, so a script can branch without parsing output.
///
/// Separating "the daemon refused this" from "the daemon could not be
/// reached" is the distinction that matters most: the first means the request
/// was wrong, the second means nothing was attempted.
mod exit {
    /// Usage error: the arguments could not be understood.
    pub(crate) const USAGE: u8 = 2;
    /// The daemon returned a structured problem.
    pub(crate) const REFUSED: u8 = 3;
    /// The daemon could not be reached or did not speak the protocol.
    pub(crate) const UNREACHABLE: u8 = 4;
}

fn main() -> ExitCode {
    let invocation = match Invocation::parse(std::env::args().skip(1)) {
        Ok(Some(invocation)) => invocation,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("pantheon: {err}");
            eprint!("{USAGE}");
            return ExitCode::from(exit::USAGE);
        }
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("pantheon: could not start the runtime: {err}");
            return ExitCode::from(exit::UNREACHABLE);
        }
    };

    match runtime.block_on(commands::run(&invocation)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(commands::Failure::Refused(problem)) => {
            eprintln!("pantheon: {}", render::problem(&problem));
            ExitCode::from(exit::REFUSED)
        }
        Err(commands::Failure::Unreachable(detail)) => {
            eprintln!("pantheon: {detail}");
            ExitCode::from(exit::UNREACHABLE)
        }
        Err(commands::Failure::Usage(detail)) => {
            eprintln!("pantheon: {detail}");
            ExitCode::from(exit::USAGE)
        }
    }
}
