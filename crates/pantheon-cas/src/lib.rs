//! Pantheon's local content-addressed object store.
//!
//! # Owns
//!
//! The concrete local CAS backend behind `pantheon-engine`'s
//! `ContentObjectStore`
//! port: hash-and-stage, durable finalize, atomic publication into the
//! digest namespace, and verification of what lands.
//!
//! # Must not own
//!
//! Durable authority — the database owns every claim that bytes exist and
//! what they mean. This crate stores payloads and verifies identities; a
//! committed Artifact referencing missing content is a recovery/storage
//! fault discovered *against* this store, never a state it records. It also
//! owns no orchestration: ordering between CAS durability and DB commits
//! belongs to the engine's sealing controller.
//!
//! # Why this is a separate crate
//!
//! Raw CAS storage is a platform boundary in both directions the
//! implementation map names: it is a concrete implementation behind an
//! abstract port, and it is material an untrusted Sandbox must never reach
//! (`docs/architecture/security/sandbox-broker-and-isolation.md`). Keeping
//! it out of `pantheon-git` preserves that crate's single purpose, and out
//! of `pantheon-engine` keeps concrete filesystem effects off the control
//! plane.
//!
//! # The publication order, and why each step is there
//!
//! ```text
//! hash the bytes                        identity first: SHA-256 + size
//! stage under a private O_EXCL name     never touch a live object path
//! fsync the staged file                 the bytes survive before they exist
//! hard-link into the digest namespace   atomic; EEXIST means prior/concurrent publish
//! verify whatever sits at that path     pre-existing objects are verified, not trusted
//! unlink the staged name                one namespace entry per digest
//! fsync the directory                   the linkage survives crashes too
//! ```
//!
//! A crash after publication but before any database commit leaves an
//! orphan object — harmless and GC-able by design. Overwriting a digest
//! path with different bytes is impossible here: publication only ever
//! creates, and a colliding existing object must prove its identity or fail
//! closed. Abandoned `incoming-*` files from crashed stagings are scratch,
//! never objects, and are ignored by every read path.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use pantheon_core::config::Digest;
use pantheon_engine::sealing::{ContentObjectStore, ExternalFault, ObjectRef};

/// The controller-owned local CAS root.
///
/// Constructed from composition configuration only — never from Task,
/// Workspace or worker input — and never exposed to a Sandbox.
#[derive(Debug, Clone)]
pub struct LocalFsCas {
    /// `<root>/objects/sha256/<hex>`; the layout mirrors the conceptual one
    /// in the Artifact contract while keeping every object under one root.
    objects_dir: PathBuf,
}

impl LocalFsCas {
    /// Opens (creating if necessary) the CAS beneath `root`.
    ///
    /// # Errors
    ///
    /// Any I/O failure creating the directory layout.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let objects_dir = root.as_ref().join("objects").join("sha256");
        std::fs::create_dir_all(&objects_dir)?;
        Ok(Self { objects_dir })
    }

    /// The storage path for one digest. Storage path is not identity; this
    /// exists so tooling can point corruption attempts at real locations.
    #[must_use]
    pub fn path_of(&self, digest: &Digest) -> PathBuf {
        self.objects_dir.join(digest.to_hex())
    }

    fn fault(code: &'static str, detail: impl Into<String>) -> ExternalFault {
        ExternalFault {
            code: code.to_string(),
            detail: detail.into(),
        }
    }

    /// Reads back and hashes an object file, checking size and digest.
    fn verify_at(&self, path: &Path, reference: &ObjectRef) -> Result<(), ExternalFault> {
        let bytes = std::fs::read(path).map_err(|err| {
            Self::fault(
                "cas.object-unavailable",
                format!("could not read {}: {err}", path.display()),
            )
        })?;
        if bytes.len() as u64 != reference.size {
            return Err(Self::fault(
                "cas.corrupt-object",
                format!(
                    "{} holds {} bytes, expected {}",
                    path.display(),
                    bytes.len(),
                    reference.size
                ),
            ));
        }
        let actual = Digest::of(&bytes);
        if actual != reference.digest {
            return Err(Self::fault(
                "cas.corrupt-object",
                format!(
                    "{} hashes to {actual}, expected {}",
                    path.display(),
                    reference.digest
                ),
            ));
        }
        Ok(())
    }

    /// Durably links verified staged bytes into the digest namespace.
    fn finalize(
        &self,
        staged: &Path,
        final_path: &Path,
        reference: &ObjectRef,
    ) -> Result<(), ExternalFault> {
        match std::fs::hard_link(staged, final_path) {
            Ok(()) => {}
            // Someone published concurrently (or earlier). That is fine —
            // but their bytes must prove themselves; a filename proves
            // nothing.
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                self.verify_at(final_path, reference)?;
                discard_staged(staged)?;
                return Ok(());
            }
            Err(err) => {
                return Err(Self::fault(
                    "cas.publish-failed",
                    format!("could not link into {}: {err}", final_path.display()),
                ));
            }
        }
        discard_staged(staged)?;
        sync_dir(&self.objects_dir)?;
        Ok(())
    }
}

impl ContentObjectStore for LocalFsCas {
    fn publish(&self, bytes: &[u8]) -> Result<ObjectRef, ExternalFault> {
        let reference = ObjectRef {
            digest: Digest::of(bytes),
            size: bytes.len() as u64,
        };
        let final_path = self.path_of(&reference.digest);

        // Already present? Verify rather than trust, then done: publication
        // of one digest is idempotent however many callers race it.
        if final_path.exists() {
            self.verify_at(&final_path, &reference)?;
            return Ok(reference);
        }

        // Stage under a private name. A leftover incoming-* file means some
        // process crashed mid-stage; it is scratch, so a fresh unique name
        // sidesteps it rather than adopting or deleting it blindly.
        let staged = loop {
            let candidate = self.objects_dir.join(format!(
                "incoming-{}-{}",
                std::process::id(),
                next_temp_name()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(mut file) => {
                    file.write_all(bytes).map_err(|err| {
                        Self::fault(
                            "cas.write-failed",
                            format!("could not stage object bytes: {err}"),
                        )
                    })?;
                    file.sync_all().map_err(|err| {
                        Self::fault(
                            "cas.durability-failed",
                            format!("could not make staged bytes durable: {err}"),
                        )
                    })?;
                    break candidate;
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(Self::fault(
                        "cas.write-failed",
                        format!("could not stage an object: {err}"),
                    ));
                }
            }
        };

        self.finalize(&staged, &final_path, &reference)?;
        Ok(reference)
    }

    fn verify(&self, reference: &ObjectRef) -> Result<(), ExternalFault> {
        self.verify_at(&self.path_of(&reference.digest), reference)
    }

    fn read(&self, reference: &ObjectRef) -> Result<Vec<u8>, ExternalFault> {
        let path = self.path_of(&reference.digest);
        self.verify_at(&path, reference)?;
        std::fs::read(&path).map_err(|err| {
            Self::fault(
                "cas.object-unavailable",
                format!("could not read {}: {err}", path.display()),
            )
        })
    }
}

/// Removes a staged scratch name after it has served its purpose.
fn discard_staged(staged: &Path) -> Result<(), ExternalFault> {
    std::fs::remove_file(staged).map_err(|err| {
        LocalFsCas::fault(
            "cas.publish-failed",
            format!("could not remove the staged object: {err}"),
        )
    })
}

/// Flushes a directory entry so a just-created name survives a crash.
fn sync_dir(dir: &Path) -> Result<(), ExternalFault> {
    File::open(dir)
        .and_then(|handle| handle.sync_all())
        .map_err(|err| {
            LocalFsCas::fault(
                "cas.durability-failed",
                format!("could not flush the object directory: {err}"),
            )
        })
}

/// Process-local unique suffix for staging names.
fn next_temp_name() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests;
