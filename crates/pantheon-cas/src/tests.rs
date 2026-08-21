//! Evidence for the CAS publication contract: durability before exposure,
//! idempotent concurrent publication, corruption failing closed, and
//! scratch tolerance.

use std::path::PathBuf;

use pantheon_core::config::Digest;
use pantheon_engine::sealing::ContentObjectStore;

use super::LocalFsCas;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-cas-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn published_bytes_are_durable_at_their_digest_path_and_verify() {
    let dir = TempDir::new("publish");
    let cas = LocalFsCas::open(dir.path()).expect("open");
    let bytes = b"payload bytes";

    let reference = cas.publish(bytes).expect("publishes");
    assert_eq!(reference.size, bytes.len() as u64);
    assert_eq!(reference.digest, Digest::of(bytes));
    // Storage location is not identity, but the bytes must be there.
    let stored = std::fs::read(cas.path_of(&reference.digest)).expect("stored");
    assert_eq!(stored, bytes);

    // No staging scratch is left behind by a clean publish.
    let entries: Vec<_> = std::fs::read_dir(cas.path_of(&reference.digest).parent().expect("dir"))
        .expect("readdir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !entries.iter().any(|name| name.starts_with("incoming-")),
        "a completed publish leaves no staged file: {entries:?}"
    );
}

#[test]
fn republication_and_concurrent_writers_are_idempotent() {
    let dir = TempDir::new("idempotent");
    let cas = LocalFsCas::open(dir.path()).expect("open");
    let first = cas.publish(b"same").expect("first");
    let second = cas.publish(b"same").expect("second");
    assert_eq!(first, second);

    // Distinct store handles on the same root model concurrent processes.
    let other = LocalFsCas::open(dir.path()).expect("open again");
    let third = other.publish(b"same").expect("third");
    assert_eq!(third, first);
}

#[test]
fn a_preexisting_corrupt_object_fails_closed_instead_of_being_trusted() {
    let dir = TempDir::new("corrupt");
    let cas = LocalFsCas::open(dir.path()).expect("open");
    let reference = cas.publish(b"honest bytes").expect("publish");

    // Corrupt the object behind the store's back.
    std::fs::write(cas.path_of(&reference.digest), b"tampered!!!").expect("tamper");

    let err = cas.publish(b"honest bytes").expect_err("must refuse");
    assert_eq!(err.code, "cas.corrupt-object", "{err}");
    // And reads fail closed too.
    let err = cas.read(&reference).expect_err("must refuse");
    assert_eq!(err.code, "cas.corrupt-object", "{err}");
}

#[test]
fn a_size_mismatch_is_corruption_not_availability() {
    let dir = TempDir::new("size");
    let cas = LocalFsCas::open(dir.path()).expect("open");
    let bytes = b"short";
    let reference = cas.publish(bytes).expect("publish");
    // Truncate in place: same prefix idea, wrong length.
    std::fs::write(cas.path_of(&reference.digest), &bytes[..3]).expect("truncate");

    let err = cas.verify(&reference).expect_err("must refuse");
    assert_eq!(err.code, "cas.corrupt-object", "{err}");
}

#[test]
fn a_missing_object_is_reported_as_unavailable() {
    let dir = TempDir::new("missing");
    let cas = LocalFsCas::open(dir.path()).expect("open");
    let reference = pantheon_engine::sealing::ObjectRef {
        digest: Digest::of(b"never published"),
        size: 4,
    };
    let err = cas.verify(&reference).expect_err("must refuse");
    assert_eq!(err.code, "cas.object-unavailable", "{err}");
}
