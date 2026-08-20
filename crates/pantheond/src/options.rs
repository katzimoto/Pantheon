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

Options:
  --data-dir <path>  Directory holding the authoritative database.
                     Default: $PANTHEON_DATA_DIR, else ./pantheon-data
  --socket <path>    Operator Control Unix socket.
                     Default: <data-dir>/run/pantheond.sock
  --config <path>    Configuration source file.
                     Default: <data-dir>/configuration.json
  -h, --help         Print this message.

pantheond serves Operator Control on a local Unix-domain socket only. There is
no address or port to configure.
";

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

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| {
                args.next()
                    .map(PathBuf::from)
                    .ok_or_else(|| OptionsError(format!("{name} needs a path")))
            };
            match arg.as_str() {
                "-h" | "--help" => return Ok(None),
                "--data-dir" => data_dir = Some(value("--data-dir")?),
                "--socket" => socket = Some(value("--socket")?),
                "--config" => configuration = Some(value("--config")?),
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
