//! Binding and serving the Operator Control Unix-domain socket.
//!
//! There is no address, no port and no TCP listener anywhere in this module.
//! `docs/architecture/operations/public-daemon-api-and-cli.md` makes the local
//! socket the MVP boundary, and expressing that as "the only listener this
//! crate can construct" is stronger than expressing it as a configuration
//! default that a future flag could flip.
//!
//! # What the socket does and does not establish
//!
//! Directory and socket permissions restrict reachability to the daemon's own
//! uid. That is a containment measure, not an authorization decision: a caller
//! that reaches this socket is *not* thereby an authenticated operator, and
//! the Agent/operator trust boundary a later mission establishes must not be
//! satisfied by same-uid reachability.

use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

use axum::Router;
use tokio::net::{UnixListener, UnixStream};

/// Permissions for the directory the socket lives in.
///
/// The directory is the real gate. A Unix socket's own mode is honoured by
/// Linux but historically ignored by some BSD-derived kernels, and there is a
/// window between `bind` and `chmod` in which the socket exists at whatever
/// the process umask allowed. An unreachable parent directory closes both
/// gaps, so the directory mode is the load-bearing one and the socket mode is
/// defence in depth.
const DIRECTORY_MODE: u32 = 0o700;

/// Permissions for the socket file itself.
const SOCKET_MODE: u32 = 0o600;

/// A failure setting up or running the operator socket.
#[derive(Debug)]
pub enum SocketError {
    /// The socket path is already in use by a live daemon.
    AlreadyServing {
        path: PathBuf,
    },
    /// Something already exists at the socket path that is not a socket.
    ///
    /// Refused rather than removed: deleting an unrelated file because it sits
    /// where a socket was expected is a destructive guess.
    NotASocket {
        path: PathBuf,
    },
    /// The socket path has no parent directory to secure.
    NoParentDirectory {
        path: PathBuf,
    },
    Io {
        detail: String,
        source: io::Error,
    },
}

impl std::fmt::Display for SocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyServing { path } => {
                write!(f, "another daemon is already serving {}", path.display())
            }
            Self::NotASocket { path } => write!(
                f,
                "{} exists and is not a socket; refusing to remove it",
                path.display()
            ),
            Self::NoParentDirectory { path } => {
                write!(f, "{} has no parent directory", path.display())
            }
            Self::Io { detail, source } => write!(f, "{detail}: {source}"),
        }
    }
}

impl std::error::Error for SocketError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io(detail: impl Into<String>) -> impl FnOnce(io::Error) -> SocketError {
    let detail = detail.into();
    move |source| SocketError::Io { detail, source }
}

/// Binds `path` and serves `router` until `shutdown` resolves.
///
/// # Errors
///
/// [`SocketError`] when the path cannot be secured, is held by a live daemon,
/// or the server itself fails.
pub async fn serve(
    path: &Path,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), SocketError> {
    let listener = bind(path).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(io("operator server failed"))
}

/// Prepares the path and binds the listener.
///
/// Exposed within the crate so tests can bind without serving.
pub(crate) async fn bind(path: &Path) -> Result<UnixListener, SocketError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| SocketError::NoParentDirectory {
            path: path.to_path_buf(),
        })?;

    std::fs::create_dir_all(parent).map_err(io(format!(
        "could not create the operator socket directory {}",
        parent.display()
    )))?;
    // Applied unconditionally, not only on creation: an existing directory
    // left loose by an earlier version or by an operator must be tightened
    // before a socket appears inside it, not after.
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(DIRECTORY_MODE)).map_err(
        io(format!(
            "could not restrict the operator socket directory {}",
            parent.display()
        )),
    )?;

    clear_stale(path).await?;

    let listener = UnixListener::bind(path).map_err(io(format!(
        "could not bind the operator socket {}",
        path.display()
    )))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE)).map_err(io(
        format!("could not restrict the operator socket {}", path.display()),
    ))?;
    Ok(listener)
}

/// Removes a socket file left behind by a daemon that is no longer running.
///
/// Liveness is decided by *connecting*, not by a pid file or a lock: a socket
/// whose owner has died refuses connections, and a socket whose owner is alive
/// accepts them. Removing a socket without that check would let a second
/// daemon steal the path from a running one.
async fn clear_stale(path: &Path) -> Result<(), SocketError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(io(format!("could not inspect {}", path.display()))(err));
        }
    };
    if !metadata.file_type().is_socket() {
        return Err(SocketError::NotASocket {
            path: path.to_path_buf(),
        });
    }
    match UnixStream::connect(path).await {
        Ok(_) => Err(SocketError::AlreadyServing {
            path: path.to_path_buf(),
        }),
        Err(_) => std::fs::remove_file(path).map_err(io(format!(
            "could not remove the stale operator socket {}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests;
