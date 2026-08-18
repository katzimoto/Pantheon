//! A minimal, dependency-free isolated-directory helper for tests.
//!
//! Pantheon's dependency policy adds a crate only when code needs it,
//! including for tests: this mission's tests need one isolated temporary
//! directory per test, which `std::env::temp_dir` plus a per-process,
//! per-call unique suffix provides without taking on a `tempfile`
//! dependency.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pantheon-store-test-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated test directory");
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
