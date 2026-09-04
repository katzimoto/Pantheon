//! Evidence for the root-confined, no-follow capture boundary and the
//! trusted base reader.
//!
//! The hostile fixtures here are real: outward symlinks, FIFOs, sockets,
//! sentinel files that must never enter CAS, non-UTF-8 names, and a
//! repository whose tree carries submodule semantics. Each assertion is
//! about containment — what capture may never touch — not convenience.

#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use pantheon_core::artifact::{EntryKind, RepositoryPath};
use pantheon_core::workspace::ResolvedBase;
use pantheon_engine::sealing::{TrustedBaseReader, WorkspaceTreeCapture};

use crate::capture::{ConfinedCapture, GitBaseReader, code};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-git-capture-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Captures `root` and returns `(entries, fault)` — exactly one is set.
/// One captured entry as the tests read it: manifest spelling, kind, bytes.
type Captured = (String, EntryKind, Vec<u8>);

fn capture(root: &Path) -> (Vec<Captured>, Option<String>) {
    let mut entries = Vec::new();
    // On failure the list holds whatever was captured before the refusal;
    // individual tests assert what must be absent. The seal as a whole
    // fails, which is what makes a partial list harmless.
    match ConfinedCapture.capture_tree(root, &mut |entry| {
        entries.push((entry.path.to_manifest_string(), entry.kind, entry.bytes));
        Ok(())
    }) {
        Ok(()) => (entries, None),
        Err(fault) => (entries, Some(fault.code)),
    }
}

fn entry<'a>(entries: &'a [Captured], path: &str) -> &'a Captured {
    entries
        .iter()
        .find(|(name, _, _)| name == path)
        .unwrap_or_else(|| panic!("{path} was not captured"))
}

#[test]
fn an_ordinary_tree_captures_files_modes_and_link_bytes() {
    let dir = TempDir::new("ordinary");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join("src/nested")).expect("dirs");
    let write = |rel: &str, bytes: &[u8]| {
        std::fs::write(root.join(rel), bytes).expect("write");
    };
    write("plain.txt", b"plain");
    write("script.sh", b"#!/bin/sh\necho hi\n");
    std::fs::set_permissions(
        root.join("script.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    write("src/nested/deep.rs", b"deep");
    #[cfg(unix)]
    std::os::unix::fs::symlink("../plain.txt", root.join("src/relative-link")).expect("symlink");

    let (entries, fault) = capture(&root);
    assert_eq!(fault, None);

    let plain = entry(&entries, "plain.txt");
    assert_eq!(plain.1, EntryKind::Regular);
    assert_eq!(plain.2, b"plain");

    let script = entry(&entries, "script.sh");
    assert_eq!(script.1, EntryKind::Executable);

    let deep = entry(&entries, "src/nested/deep.rs");
    assert_eq!(deep.1, EntryKind::Regular);
    assert_eq!(deep.2, b"deep");

    let link = entry(&entries, "src/relative-link");
    assert_eq!(link.1, EntryKind::Symlink);
    // The link's target *text* is the payload.
    assert_eq!(link.2, b"../plain.txt");

    // Directories themselves are traversal structure, not entries; empty
    // directories are not tree content.
    assert!(!entries.iter().any(|(name, _, _)| name == "src"));
}

#[test]
fn the_workspace_git_state_is_excluded_without_being_trusted_or_touched() {
    let dir = TempDir::new("git-excluded");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join(".git/hooks")).expect("git dirs");
    std::fs::write(root.join(".git/config"), b"hostile config").expect("config");
    std::fs::write(root.join(".git/HOSTILE-SENTINEL"), b"never in CAS").expect("sentinel");
    std::fs::write(root.join("app.txt"), b"app").expect("app");

    let (entries, fault) = capture(&root);
    assert_eq!(fault, None, "the exclusion is silent, not an error");
    assert!(entry(&entries, "app.txt").2 == b"app");
    assert!(
        !entries.iter().any(|(name, _, _)| name.starts_with(".git")),
        "administrative state must never become payload"
    );
}

#[test]
fn outward_links_are_inert_data_and_target_content_never_enters_capture() {
    let dir = TempDir::new("escape-link");
    // A secret outside the Workspace...
    let secret = dir.path().join("secret.pem");
    std::fs::write(&secret, b"PRIVATE KEY MATERIAL").expect("secret");

    // ...and three links inside pointing at or past it.
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).expect("root");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&secret, root.join("abs-link")).expect("absolute link");
        std::os::unix::fs::symlink("../../secret.pem", root.join("rel-link"))
            .expect("relative link");
        std::os::unix::fs::symlink("/nonexistent/target", root.join("dangling")).expect("dangling");
    }
    std::fs::write(root.join("safe.txt"), b"safe").expect("safe");

    let (entries, fault) = capture(&root);
    assert_eq!(fault, None);

    let abs = entry(&entries, "abs-link");
    assert_eq!(abs.1, EntryKind::Symlink);
    assert_eq!(abs.2, secret.to_str().expect("utf-8 fixture").as_bytes());

    let rel = entry(&entries, "rel-link");
    assert_eq!(rel.2, b"../../secret.pem");

    let dangling = entry(&entries, "dangling");
    assert_eq!(dangling.2, b"/nonexistent/target");

    // The whole point: no captured payload anywhere contains what the
    // symlink points at.
    for (_, _, bytes) in &entries {
        assert_ne!(
            bytes.as_slice(),
            b"PRIVATE KEY MATERIAL",
            "target content leaked into capture"
        );
    }
}

#[test]
fn special_filesystem_objects_fail_closed_with_the_hostile_code() {
    let dir = TempDir::new("special");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).expect("root");

    // A FIFO, created through the platform tool since std does not expose it.
    let status = Process::new("mkfifo")
        .arg(root.join("pipe"))
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "fixture requires mkfifo");

    // A Unix socket file.
    let socket_path = root.join("sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket_path).expect("bind socket");

    let (entries, fault) = capture(&root);
    assert_eq!(
        fault.as_deref(),
        Some(code::HOSTILE_FILESYSTEM),
        "special objects must fail closed"
    );
    // Nothing from the hostile directory is reported as payload.
    assert!(entries.is_empty());

    // And the objects were never opened: opening a FIFO for read would
    // block forever, so reaching here at all proves the refusal arm ran.
}

#[test]
fn a_symlinked_capture_root_is_refused_rather_than_dereferenced() {
    let dir = TempDir::new("root-symlink");
    let real = dir.path().join("real");
    std::fs::create_dir_all(&real).expect("real");
    std::fs::write(real.join("data"), b"inside").expect("data");
    let link = dir.path().join("workspace-repo");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).expect("link");

    let (entries, fault) = capture(&link);
    assert_eq!(
        fault.as_deref(),
        Some(code::HOSTILE_FILESYSTEM),
        "the trusted root itself may not be indirection"
    );
    assert!(entries.is_empty());
}

#[test]
#[cfg(target_os = "linux")]
fn non_utf8_names_are_captured_byte_exactly() {
    // Linux filesystems store names as raw bytes; macOS's APFS refuses
    // names that are not valid UTF-8 ("Illegal byte sequence"), so there
    // the platform cannot produce this fixture at all — which is why the
    // acceptance criterion scopes losslessness to *supported* platforms.
    // The encoding itself is proven platform-independently in
    // `pantheon-core`.
    let dir = TempDir::new("non-utf8");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).expect("root");
    let name = std::ffi::OsStr::from_bytes(&[b'f', 0xE9, b'.', b't', b'x', b't']);
    std::fs::write(root.join(name), b"bytes").expect("write");

    let (entries, fault) = capture(&root);
    assert_eq!(fault, None);
    let (name_spelling, kind, bytes) = &entries[0];
    assert_eq!(bytes, b"bytes");
    assert_eq!(*kind, EntryKind::Regular);
    // The manifest spelling round-trips the exact raw bytes.
    let decoded = RepositoryPath::from_manifest_string(name_spelling).expect("decodes losslessly");
    assert_eq!(decoded.as_bytes(), &[b'f', 0xE9, b'.', b't', b'x', b't']);
}

#[test]
fn identity_binding_refuses_a_replaced_object_between_inspection_and_open() {
    // Deterministic unit evidence for the guard a race would exercise:
    // validation binds to (inode, device), so a descriptor opened onto any
    // other object fails even when O_NOFOLLOW alone would have passed it.
    use crate::capture::verify_identity;
    let dir = TempDir::new("identity");
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::write(&a, b"inspected object").expect("a");
    std::fs::write(&b, b"replacement object").expect("b");

    let open_stat = |path: &Path| {
        let fd = rustix::fs::openat(
            rustix::fs::CWD,
            path,
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        )
        .expect("open");
        let stat = rustix::fs::fstat(&fd).expect("fstat");
        (fd, stat)
    };
    let (fd_a, stat_a) = open_stat(&a);
    let (_fd_b, stat_b) = open_stat(&b);

    let path = RepositoryPath::from_bytes(b"swapped").expect("path");
    verify_identity(&fd_a, &stat_a, &path).expect("the true object verifies");
    let err = verify_identity(&fd_a, &stat_b, &path).expect_err("a foreign stat must fail");
    assert_eq!(err.code, code::HOSTILE_FILESYSTEM);
}

#[test]
fn swaps_during_capture_never_leak_external_bytes_and_always_fail_closed() {
    // Stress evidence, not proof-of-race: a mutator churns one path between
    // regular file / symlink / directory while repeated captures run. Every
    // run must either produce a coherent capture of *some* state of the
    // tree or fail closed; no run may ever report external content, and no
    // failure may be mislabeled as success.
    let dir = TempDir::new("swap-stress");
    let outside = dir.path();
    std::fs::write(outside.join("external-secret"), b"OUTSIDE BYTES").expect("outside");

    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join("d/stable")).expect("layout");
    std::fs::write(root.join("d/stable/s.txt"), b"stable").expect("stable");
    std::fs::write(root.join("d/churn"), b"churn-v1").expect("churn file");

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_writer = std::sync::Arc::clone(&stop);
    let churn = root.join("d/churn");
    let writer = std::thread::spawn(move || {
        let mut flip = false;
        while !stop_writer.load(std::sync::atomic::Ordering::Relaxed) {
            flip = !flip;
            let _ = std::fs::remove_file(&churn);
            if flip {
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink("../../external-secret", &churn);
            } else {
                let _ = std::fs::write(&churn, b"churn-v2");
            }
            std::thread::yield_now();
        }
    });

    let mut runs = 0;
    let mut succeeded = 0;
    let mut failed_closed = 0;
    while runs < 60 {
        runs += 1;
        let (entries, fault) = capture(&root);
        match fault {
            None => {
                succeeded += 1;
                for (_, _, bytes) in &entries {
                    assert_ne!(
                        bytes.as_slice(),
                        b"OUTSIDE BYTES",
                        "external content disclosed through a swap race"
                    );
                }
            }
            Some(observed) => {
                failed_closed += 1;
                assert!(
                    observed == code::HOSTILE_FILESYSTEM || observed == code::CAPTURE_IO,
                    "unexpected failure mode {observed}"
                );
            }
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().expect("writer joins");

    assert!(
        succeeded > 0 && failed_closed > 0,
        "both outcomes must be exercised by the stress ({succeeded} ok, \
         {failed_closed} refused over {runs} runs)"
    );
}

// ---- trusted base reader -------------------------------------------------

fn source_repository(dir: &TempDir) -> PathBuf {
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    git(&source, &["init", "--quiet", "-b", "main"]);
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    std::fs::write(source.join("run.sh"), b"#!/bin/sh\n").expect("write");
    std::fs::set_permissions(
        source.join("run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    #[cfg(unix)]
    std::os::unix::fs::symlink("app.txt", source.join("latest")).expect("symlink");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);
    source
}

fn reader(control: &Path) -> GitBaseReader {
    GitBaseReader::new(control).expect("reader")
}

#[test]
fn the_base_tree_maps_every_entry_with_its_canonical_kind_and_size() {
    let dir = TempDir::new("base-tree");
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).expect("control");
    let source = source_repository(&dir);
    let base = ResolvedBase::parse(&git(&source, &["rev-parse", "HEAD"])).expect("base");

    let tree = reader(&control)
        .base_tree(&source, &base)
        .expect("the base tree reads");

    let app = tree.get(b"app.txt".as_slice()).expect("app.txt");
    assert_eq!(app.kind, EntryKind::Regular);
    assert_eq!(app.size, b"original\n".len() as u64);

    let run = tree.get(b"run.sh".as_slice()).expect("run.sh");
    assert_eq!(run.kind, EntryKind::Executable);

    let latest = tree.get(b"latest".as_slice()).expect("latest");
    assert_eq!(latest.kind, EntryKind::Symlink);

    // Preimage bytes come back exactly, through the validated object name.
    let bytes = reader(&control)
        .blob_bytes(&source, &app.oid)
        .expect("blob reads");
    assert_eq!(bytes, b"original\n");
}

#[test]
fn a_base_tree_with_submodule_semantics_fails_closed() {
    let dir = TempDir::new("base-gitlink");
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).expect("control");
    let source = source_repository(&dir);

    // Forge a gitlink entry without creating a real submodule: plumbing
    // writes the index entry directly.
    let fake_commit = "1234567890123456789012345678901234567890";
    git(
        &source,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{fake_commit},vendored-sub"),
        ],
    );
    git(&source, &["commit", "--quiet", "-m", "add gitlink"]);
    let base = ResolvedBase::parse(&git(&source, &["rev-parse", "HEAD"])).expect("base");

    let fault = reader(&control)
        .base_tree(&source, &base)
        .expect_err("gitlinks are unsupported v1 semantics");
    assert_eq!(fault.code, code::HOSTILE_REPOSITORY);
}

#[test]
fn a_base_that_no_longer_exists_fails_as_unavailable() {
    let dir = TempDir::new("base-gone");
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).expect("control");
    let source = source_repository(&dir);
    let base = ResolvedBase::parse("1111111111111111111111111111111111111111").expect("base");

    let fault = reader(&control)
        .base_tree(&source, &base)
        .expect_err("an absent base cannot yield preimages");
    assert_eq!(fault.code, code::BASE_UNAVAILABLE);
}

#[test]
fn a_nested_repository_gitfile_fails_closed_like_a_nested_git_directory() {
    // Modern `git submodule add` leaves `sub/.git` as a *regular file*
    // carrying `gitdir: ...` indirection. The refusal is type-independent:
    // directory, gitfile, and symlink spellings all mean nested-repository
    // semantics v1 does not support.
    let dir = TempDir::new("gitfile");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join("sub")).expect("sub");
    std::fs::write(root.join("sub/.git"), "gitdir: ../.git/modules/sub\n").expect("gitfile");
    std::fs::write(root.join("sub/inner.rs"), b"submodule interior").expect("interior");
    std::fs::write(root.join("app.txt"), b"app").expect("app");

    let (entries, fault) = capture(&root);
    assert_eq!(
        fault.as_deref(),
        Some(code::HOSTILE_FILESYSTEM),
        "a gitfile below the root must fail closed"
    );
    assert!(
        !entries.iter().any(|(name, _, _)| name.contains("inner.rs")),
        "submodule interior must never become payload"
    );
}

#[test]
fn an_open_swapped_to_a_fifo_cannot_block_and_still_fails_identity() {
    // Deterministic evidence for the NONBLOCK guard: opening a FIFO with
    // O_RDONLY|NONBLOCK returns immediately where a blocking open would
    // hang until a writer appears — so a swap between inspection and open
    // reaches verify_identity and is refused instead of freezing capture
    // while it holds the freeze.
    let dir = TempDir::new("fifo-nonblock");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).expect("root");
    let fifo = root.join("pipe");
    let status = Process::new("mkfifo").arg(&fifo).status().expect("mkfifo");
    assert!(status.success());

    // "Inspected" stat from a real regular file; the descriptor opened is
    // the FIFO's. Exactly the post-swap shape.
    let regular = root.join("real.txt");
    std::fs::write(&regular, b"inspected object").expect("write");
    let inspected_fd = rustix::fs::openat(
        rustix::fs::CWD,
        &regular,
        rustix::fs::OFlags::RDONLY,
        rustix::fs::Mode::empty(),
    )
    .expect("open inspected");
    let inspected = rustix::fs::fstat(&inspected_fd).expect("fstat");

    let opened = rustix::fs::openat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .expect("NONBLOCK open of a FIFO returns without a writer");
    let path = RepositoryPath::from_bytes(b"pipe").expect("path");
    let err = super::capture::verify_identity(&opened, &inspected, &path)
        .expect_err("the FIFO is not the inspected object");
    assert_eq!(err.code, code::HOSTILE_FILESYSTEM);
}

#[test]
fn bounded_reads_refuse_growth_shrink_and_ceiling_exactly() {
    use super::capture::{ReadFault, read_bounded};
    let mut cursor: &[u8] = b"exact";
    assert_eq!(read_bounded(&mut cursor, 5, 512).expect("exact"), b"exact");

    // Grown past the validated size: refused, not silently truncated.
    let mut grown: &[u8] = b"now-longer";
    assert!(matches!(
        read_bounded(&mut grown, 5, 512),
        Err(ReadFault::Grew)
    ));

    // Shrunk: same refusal.
    let mut shrunk: &[u8] = b"ab";
    assert!(matches!(
        read_bounded(&mut shrunk, 5, 512),
        Err(ReadFault::Grew)
    ));

    // Over ceiling: refused at max+1 bytes read, whatever was expected.
    let mut huge: &[u8] = &[0u8; 64];
    assert!(matches!(
        read_bounded(&mut huge, 64, 8),
        Err(ReadFault::OverCeiling)
    ));
}

// ---- captured-identity substrate (#75) -------------------------------------
//
// The changed-path comparison rests on one claim about Git rather than
// Pantheon code: the object name `git hash-object` computes for raw bytes,
// in a repository's own object format and without filters or writes, is the
// same name the repository's trees record for that content. These tests pin
// exactly that, because if it stopped holding, sealing's unchanged verdicts
// would be built on sand.

#[test]
fn captured_payload_names_match_the_base_trees_recorded_identities() {
    let dir = TempDir::new("identity-consistency");
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).expect("control");
    let source = source_repository(&dir);
    let base = ResolvedBase::parse(&git(&source, &["rev-parse", "HEAD"])).expect("base");

    let tree = reader(&control)
        .base_tree(&source, &base)
        .expect("the base tree reads");

    // The exact bytes of each recorded entry — regular, executable and
    // symlink alike — must name to exactly what the tree records.
    let names = reader(&control)
        .blob_object_names(&source, &[b"original\n", b"#!/bin/sh\n", b"app.txt"])
        .expect("names compute");
    assert_eq!(names.len(), 3, "one name per payload");
    assert_eq!(names[0], tree.get(b"app.txt".as_slice()).expect("app").oid);
    assert_eq!(names[1], tree.get(b"run.sh".as_slice()).expect("sh").oid);
    assert_eq!(
        names[2],
        tree.get(b"latest".as_slice()).expect("link").oid,
        "a symlink's target bytes carry its identity"
    );

    // And different bytes cannot pass as the same content.
    let other = reader(&control)
        .blob_object_names(&source, &[b"different bytes entirely"])
        .expect("name");
    assert_ne!(other[0], tree.get(b"app.txt".as_slice()).expect("app").oid);
}

fn sha256_source_repository(dir: &TempDir) -> PathBuf {
    let source = dir.path().join("source256");
    std::fs::create_dir_all(&source).expect("source");
    git(
        &source,
        &["init", "--quiet", "--object-format=sha256", "-b", "main"],
    );
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "--quiet", "-m", "base"]);
    source
}

#[test]
fn captured_names_follow_the_repositories_own_object_format() {
    let dir = TempDir::new("identity-sha256");
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).expect("control");
    let source = sha256_source_repository(&dir);
    assert_eq!(
        git(&source, &["rev-parse", "--show-object-format"]),
        "sha256",
        "fixture really is a SHA-256 repository"
    );
    let base = ResolvedBase::parse(&git(&source, &["rev-parse", "HEAD"])).expect("base");

    let tree = reader(&control)
        .base_tree(&source, &base)
        .expect("the base tree reads");
    let app = tree.get(b"app.txt".as_slice()).expect("app.txt");
    assert_eq!(app.oid.len(), 64, "the base names are 64 hex digits");

    let names = reader(&control)
        .blob_object_names(&source, &[b"original\n"])
        .expect("names compute");
    assert_eq!(
        names[0], app.oid,
        "the comparison signal works through the repository's actual format"
    );
}

#[test]
fn naming_captured_blobs_writes_nothing_into_the_source() {
    let dir = TempDir::new("identity-nowrite");
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).expect("control");
    let source = source_repository(&dir);

    let inventory_before = git(
        &source,
        &["cat-file", "--batch-all-objects", "--batch-check"],
    );
    let refs_before = git(&source, &["show-ref"]);

    // Novel payloads that exist nowhere in the source's object database.
    reader(&control)
        .blob_object_names(
            &source,
            &[b"never committed anywhere", b"nor this one either"],
        )
        .expect("names compute");

    assert_eq!(
        git(
            &source,
            &["cat-file", "--batch-all-objects", "--batch-check"]
        ),
        inventory_before,
        "no object appeared in the source"
    );
    assert_eq!(git(&source, &["show-ref"]), refs_before, "refs untouched");
}

#[test]
fn a_source_configured_filter_cannot_influence_captured_identity() {
    // The identity of captured bytes must be their raw content alone. The
    // filter here is wired repo-wide (info/attributes matches any path) so
    // it would apply even to staged scratch paths if filters were in play;
    // if it ever ran during naming, the computed name would stop matching
    // the OID the tree already recorded for these very bytes.
    let dir = TempDir::new("identity-filter");
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).expect("control");
    let source = source_repository(&dir);
    let base = ResolvedBase::parse(&git(&source, &["rev-parse", "HEAD"])).expect("base");
    let recorded = reader(&control)
        .base_tree(&source, &base)
        .expect("tree")
        .get(b"app.txt".as_slice())
        .expect("app")
        .oid
        .clone();

    std::fs::create_dir_all(source.join(".git/info")).expect("info dir");
    std::fs::write(source.join(".git/info/attributes"), b"* filter=ident\n").expect("attributes");
    git(&source, &["config", "filter.ident.clean", "sed s/$/FILTH/"]);

    let names = reader(&control)
        .blob_object_names(&source, &[b"original\n"])
        .expect("names compute");
    assert_eq!(
        names[0], recorded,
        "attributes and configured filters stay out of the identity"
    );
}

#[test]
fn malformed_identity_output_fails_closed() {
    use super::capture::{is_canonical_object_name, parse_object_names};

    assert_eq!(
        parse_object_names(b"0123456789abcdef0123456789abcdef01234567\n"),
        Some(vec!["0123456789abcdef0123456789abcdef01234567".to_string()])
    );
    assert_eq!(
        parse_object_names(
            b"0123456789abcdef0123456789abcdef01234567\n\
              0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"
        )
        .map(|names| names.len()),
        Some(2),
        "both formats parse"
    );
    assert_eq!(
        parse_object_names(b""),
        Some(Vec::new()),
        "an empty answer parses as empty"
    );
    // Any non-canonical shape is refused wholesale.
    assert_eq!(parse_object_names(b"zz\n"), None);
    assert_eq!(parse_object_names(b"0123\n"), None);
    assert_eq!(
        parse_object_names(b"0123456789ABCDEF0123456789abcdef01234567\n"),
        None,
        "uppercase is not canonical"
    );
    assert_eq!(
        parse_object_names(b"not even hex\n0123456789abcdef0123456789abcdef01234567\n"),
        None
    );
    assert!(!is_canonical_object_name(""));
}
