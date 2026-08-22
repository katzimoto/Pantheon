//! What the daemon is told at startup.

use std::path::PathBuf;

/// Where the daemon keeps state and what it listens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// The directory holding the authoritative database.
    pub data_dir: PathBuf,
    /// The Operator Control socket.
    pub socket: PathBuf,
    /// The configuration source this installation compiles from.
    pub configuration: PathBuf,
    /// Run the fake executor: a deterministic test backend that lets the
    /// Scheduler commit T3 and the Run Controller exercise full Attempt
    /// lineage without any production executor. Never a production
    /// isolation or execution claim.
    pub fake_executor: bool,
    /// The controller tick period in milliseconds. Diagnostics/testing knob:
    /// production cadence is the default; shorter ticks let integration
    /// tests observe multi-pass lifecycles without wall-clock sleeps of
    /// their own.
    pub tick_millis: u64,
}

/// A refusal to start.
#[derive(Debug)]
pub(crate) struct OptionsError(pub String);

impl std::fmt::Display for OptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The longest socket path a `sockaddr_un` can hold on the platforms Pantheon
/// targets, minus room for the trailing NUL.
///
/// Checked here rather than left to `bind`, so an operator who points the
/// daemon at a deep directory is told what is wrong instead of receiving an
/// `InvalidInput` from the kernel.
const MAX_SOCKET_PATH: usize = 100;

pub(crate) const USAGE: &str = "\
pantheond — the Pantheon daemon

Usage:
  pantheond [--data-dir <path>] [--socket <path>] [--config <path>]
            [--executor fake] [--tick-millis <ms>]

Options:
  --data-dir <path>  Directory holding the authoritative database.
                     Default: $PANTHEON_DATA_DIR, else ./pantheon-data
  --socket <path>    Operator Control Unix socket.
                     Default: <data-dir>/run/pantheond.sock
  --config <path>    Configuration source file.
                     Default: <data-dir>/configuration.json
  --executor fake    Run the deterministic fake execution backend. Test
                     infrastructure only: it makes no production isolation
                     or execution claim.
  --tick-millis <ms> Controller tick period (default 10000). Diagnostics
                     and integration testing; not a correctness input.
  -h, --help         Print this message.

pantheond serves Operator Control on a local Unix-domain socket only. There is
no address or port to configure.
";

/// The default controller tick period.
const DEFAULT_TICK_MILLIS: u64 = 10_000;

impl Options {
    /// Parses command-line arguments over environment defaults.
    ///
    /// Hand-rolled rather than delegated to an argument-parsing crate: three
    /// options with no subcommands is not enough surface to justify a
    /// dependency, and `docs/development/implementation.md` admits a
    /// dependency with the first code that needs it.
    pub(crate) fn parse<I>(
        args: I,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, OptionsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut data_dir: Option<PathBuf> = None;
        let mut socket: Option<PathBuf> = None;
        let mut configuration: Option<PathBuf> = None;
        let mut fake_executor = false;
        let mut tick_millis: Option<u64> = None;

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| {
                args.next()
                    .ok_or_else(|| OptionsError(format!("{name} needs a value")))
            };
            match arg.as_str() {
                "-h" | "--help" => return Ok(None),
                "--data-dir" => data_dir = Some(value("--data-dir")?.into()),
                "--socket" => socket = Some(value("--socket")?.into()),
                "--config" => configuration = Some(value("--config")?.into()),
                "--executor" => match value("--executor")?.as_str() {
                    "fake" => fake_executor = true,
                    other => {
                        return Err(OptionsError(format!(
                            "unknown executor {other}; only 'fake' exists"
                        )));
                    }
                },
                "--tick-millis" => {
                    let raw = value("--tick-millis")?;
                    tick_millis = Some(raw.parse().map_err(|_| {
                        OptionsError(format!("--tick-millis expects a number, got {raw}"))
                    })?);
                }
                other => {
                    return Err(OptionsError(format!("unrecognized argument {other}")));
                }
            }
        }

        let data_dir = data_dir
            .or_else(|| env("PANTHEON_DATA_DIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("pantheon-data"));
        let options = Self {
            socket: socket.unwrap_or_else(|| data_dir.join("run").join("pantheond.sock")),
            configuration: configuration.unwrap_or_else(|| data_dir.join("configuration.json")),
            fake_executor,
            tick_millis: tick_millis.unwrap_or(DEFAULT_TICK_MILLIS),
            data_dir,
        };

        let length = options.socket.as_os_str().len();
        if length > MAX_SOCKET_PATH {
            return Err(OptionsError(format!(
                "the socket path is {length} bytes; a Unix socket address holds at most \
                 {MAX_SOCKET_PATH}. Pass a shorter --socket."
            )));
        }
        Ok(Some(options))
    }

    /// The authoritative database file.
    pub(crate) fn database(&self) -> PathBuf {
        self.data_dir.join("pantheon.db")
    }
}

#[cfg(test)]
mod tests;
