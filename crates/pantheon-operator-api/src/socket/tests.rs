//! Evidence that the operator transport is local-only and that a socket path
//! is handled without guessing.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::socket::{DIRECTORY_MODE, SOCKET_MODE, SocketError, bind};

/// The modes asserted below are written out rather than taken from the
/// constants they check. A test that compared a constant with itself would
/// pass no matter what the constant was changed to, which is exactly the
/// change worth catching.
const OWNER_ONLY_DIRECTORY: u32 = 0o700;
const OWNER_ONLY_SOCKET: u32 = 0o600;

struct TempDir(PathBuf);

impl TempDir {
    /// Deliberately short. A Unix socket address is bounded by `SUN_LEN`
    /// (about 104 bytes), and the platform temporary directory is long enough
    /// on macOS that a conventional fixture path exceeds it — which would
    /// make these tests fail for a reason that has nothing to do with what
    /// they assert.
    fn new(label: &str) -> Self {
        let path = PathBuf::from(format!("/tmp/pn-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("r").join("s.sock")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn the_socket_and_its_directory_are_reachable_only_by_their_owner() {
    let dir = TempDir::new("modes");
    let socket = dir.socket();
    let listener = bind(&socket).await.expect("binds");

    assert_eq!(mode(socket.parent().expect("parent")), OWNER_ONLY_DIRECTORY);
    assert_eq!(mode(&socket), OWNER_ONLY_SOCKET);
    // And the constants the implementation applies are those modes, so a
    // change to either is caught here rather than at whatever reads them.
    assert_eq!(DIRECTORY_MODE, OWNER_ONLY_DIRECTORY);
    assert_eq!(SOCKET_MODE, OWNER_ONLY_SOCKET);
    assert_eq!(
        mode(&socket) & 0o077,
        0,
        "no group or other permission may remain on the operator socket"
    );
    drop(listener);
}

#[tokio::test]
async fn a_directory_left_loose_is_tightened_before_the_socket_appears() {
    // An existing directory from an earlier version, or one an operator
    // created by hand, must not be trusted to already be private.
    let dir = TempDir::new("loose");
    let socket = dir.socket();
    let parent = socket.parent().expect("parent");
    std::fs::create_dir_all(parent).expect("create");
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o777)).expect("loosen");
    assert_eq!(mode(parent), 0o777);

    let listener = bind(&socket).await.expect("binds");
    assert_eq!(mode(parent), OWNER_ONLY_DIRECTORY);
    assert_eq!(
        mode(parent) & 0o077,
        0,
        "the directory is the gate; nothing outside the owner may traverse it"
    );
    drop(listener);
}

#[tokio::test]
async fn a_socket_left_behind_by_a_dead_daemon_is_replaced() {
    let dir = TempDir::new("stale");
    let socket = dir.socket();

    let listener = bind(&socket).await.expect("first bind");
    drop(listener);
    // Dropping the listener does not remove the file, which is exactly the
    // stale state a crashed daemon leaves.
    assert!(socket.exists(), "the socket file outlives its listener");

    let replacement = bind(&socket)
        .await
        .expect("a stale socket must not block startup");
    drop(replacement);
}

#[tokio::test]
async fn a_socket_a_live_daemon_is_serving_is_never_stolen() {
    // The difference between this and the stale case is only that the first
    // listener is still accepting. Deciding by connection rather than by file
    // existence is what makes that distinguishable at all.
    let dir = TempDir::new("live");
    let socket = dir.socket();
    let listener = bind(&socket).await.expect("first bind");

    let err = bind(&socket)
        .await
        .expect_err("a live socket must not be taken");
    assert!(
        matches!(err, SocketError::AlreadyServing { .. }),
        "unexpected: {err}"
    );
    drop(listener);
}

#[tokio::test]
async fn a_regular_file_at_the_socket_path_is_refused_rather_than_deleted() {
    let dir = TempDir::new("regular-file");
    let socket = dir.socket();
    std::fs::create_dir_all(socket.parent().expect("parent")).expect("create");
    std::fs::write(&socket, b"not a socket").expect("write");

    let err = bind(&socket).await.expect_err("must refuse");
    assert!(
        matches!(err, SocketError::NotASocket { .. }),
        "unexpected: {err}"
    );
    assert_eq!(
        std::fs::read(&socket).expect("still there"),
        b"not a socket",
        "refusing must not be destructive"
    );
}

/// The transport cannot silently fall back to TCP, because nothing in this
/// crate knows how to open one.
///
/// A structural check rather than a behavioural one: a behavioural test can
/// only prove that the configuration in front of it did not produce a TCP
/// listener today. This proves there is no code that could.
#[test]
fn no_source_in_this_crate_can_construct_a_network_listener() {
    const FORBIDDEN: &[&str] = &[
        "TcpListener",
        "TcpStream",
        "SocketAddr",
        "ToSocketAddrs",
        "0.0.0.0",
        "127.0.0.1",
        "::1",
    ];

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    let mut pending = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            // This file names the forbidden strings in order to forbid them.
            if path.file_name().is_some_and(|name| name == "tests.rs")
                && path
                    .parent()
                    .is_some_and(|parent| parent.ends_with("socket"))
            {
                checked += 1;
                continue;
            }
            for needle in FORBIDDEN {
                assert!(
                    !text.contains(needle),
                    "{} names {needle}; the operator transport is Unix-socket only",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 5, "the scan must actually have read the crate");
}
