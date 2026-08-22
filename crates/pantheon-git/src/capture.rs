//! Root-confined, no-follow capture of a Workspace's logical tree, and
//! authoritative before-state reads against the trusted immutable base.
//!
//! # The boundary this module implements
//!
//! Everything below is the canonical capture invariant made of syscalls:
//! validation and payload reads must bind to the *same* filesystem object,
//! so no Agent-controlled swap between two steps can redirect a privileged
//! read (`docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`,
//! "Hostile filesystem state"). Concretely:
//!
//! - the trusted root is opened once, `O_NOFOLLOW`, from a path derived by
//!   the controller — never discovered;
//! - every descendant is inspected through `fstatat(AT_SYMLINK_NOFOLLOW)`
//!   relative to an already-open directory descriptor;
//! - directories are descended only through a freshly opened, `O_NOFOLLOW`
//!   child descriptor whose inode/device are verified equal to what the
//!   inspection saw — so a swap to a symlink fails (`ELOOP`) and a swap to
//!   another file fails the identity comparison;
//! - a regular file's payload is read from the very descriptor that was
//!   validated, never reopened by pathname, and its byte count is pinned to
//!   what that descriptor says;
//! - a symlink contributes its link-target *bytes*; the target is never
//!   opened;
//! - FIFOs, sockets, devices, undeclared mount boundaries, and any `.git`
//!   entry below the Workspace root fail closed with stable
//!   `workspace.hostile-*` codes rather than being interpreted.
//!
//! This is defense where `std` cannot go: `std::fs` opens by pathname and
//! follows symlinks, which would reintroduce exactly the TOCTOU race the
//! contract forbids as a security boundary. `rustix` provides the `*at`
//! family as safe wrappers, so no `unsafe` lives here.
//!
//! What confinement does *not* promise: atomicity of files rewritten in
//! place while being read, which no handle discipline can detect without
//! cooperation from the writer. Quiescence (the durable freeze plus the
//! proof that no execution owner exists) is what makes that window small;
//! containment is what this module guarantees regardless.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use pantheon_core::artifact::{EntryKind, RepositoryPath};
use pantheon_core::workspace::ResolvedBase;
use pantheon_engine::sealing::{
    BaseObject, CapturedEntry, ExternalFault, TrustedBaseReader, WorkspaceTreeCapture,
};

use crate::sterile::SterileProfile;

/// Fail-closed codes this module reports. They are part of the observable
/// contract: security violations keep their own names instead of collapsing
/// into generic I/O.
pub mod code {
    /// A filesystem object or structure capture must not interpret: special
    /// file types, escapes, races won by an Agent, nested repositories.
    pub const HOSTILE_FILESYSTEM: &str = "workspace.hostile-filesystem-state";
    /// A repository boundary could not be established, or the trusted base
    /// tree names structures v1 refuses.
    pub const HOSTILE_REPOSITORY: &str = "workspace.hostile-repository-state";
    /// The trusted base could not be read at all.
    pub const BASE_UNAVAILABLE: &str = "workspace.base-unavailable";
    /// An ordinary I/O failure during capture.
    pub const CAPTURE_IO: &str = "workspace.capture-io";
    /// A configured capture ceiling was exceeded.
    pub const CEILING: &str = "workspace.capture-ceiling";
}

fn fault(code: &str, detail: impl Into<String>) -> ExternalFault {
    ExternalFault {
        code: code.to_string(),
        detail: detail.into(),
    }
}

/// The deepest directory nesting capture will descend.
///
/// Bounded because [`RepositoryPath`] bounds total path length; this bound
/// simply stops the descent before pathological trees exhaust the stack.
const MAX_DEPTH: usize = 64;

/// The largest single payload capture will stage in memory.
///
/// Per-object bound, deliberately smaller than the sealing controller's
/// whole-tree budget: one enormous file fails fast instead of allocating
/// first and accounting later.
const MAX_OBJECT_BYTES: u64 = 1 << 29;

/// The root-confined, no-follow [`WorkspaceTreeCapture`] implementation.
#[derive(Debug, Clone)]
pub struct ConfinedCapture;

impl ConfinedCapture {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ConfinedCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// Raw `st_mode` file-type field, masked the way POSIX defines it. The
/// platform width of `st_mode` differs (u16 on macOS, u32 elsewhere); both
/// widen losslessly.
fn file_type_field(mode: impl Into<u64>) -> u64 {
    Into::<u64>::into(mode) & 0o170_000
}

/// The device id of a stat result, normalized across platforms: `i32` on
/// macOS/BSD, `u64` on Linux.
fn dev_of(stat: &rustix::fs::Stat) -> u64 {
    (stat.st_dev as i64) as u64
}

const FT_DIRECTORY: u64 = 0o040_000;
const FT_REGULAR: u64 = 0o100_000;
const FT_SYMLINK: u64 = 0o120_000;
// FIFO/socket/block/character devices share one refusal arm, so they need no
// individual constants here.

impl WorkspaceTreeCapture for ConfinedCapture {
    fn capture_tree(
        &self,
        root: &Path,
        sink: &mut dyn FnMut(CapturedEntry) -> Result<(), ExternalFault>,
    ) -> Result<(), ExternalFault> {
        // Pin the trusted root: opened once, never followed, never
        // re-resolved. A symlink at the root path itself is refused rather
        // than dereferenced — the controller derived this path from durable
        // state, and anything standing in the way is not the Workspace.
        let root_fd = rustix::fs::openat(
            rustix::fs::CWD,
            root,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|errno| match errno {
            // Linux answers ELOOP for O_NOFOLLOW onto a symlink; BSD/macOS
            // answer ENOTDIR when O_DIRECTORY also applies. Both mean the
            // same thing here: the trusted root is filesystem indirection.
            rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR => fault(
                code::HOSTILE_FILESYSTEM,
                format!(
                    "{} is a symlink or not a directory, not the trusted capture root",
                    root.display()
                ),
            ),
            other => fault(
                code::CAPTURE_IO,
                format!(
                    "could not open the capture root {}: {other}",
                    root.display()
                ),
            ),
        })?;

        let root_stat = rustix::fs::fstat(&root_fd).map_err(|errno| {
            fault(
                code::CAPTURE_IO,
                format!("could not inspect the capture root: {errno}"),
            )
        })?;
        debug_assert_eq!(
            file_type_field(root_stat.st_mode),
            FT_DIRECTORY,
            "an O_DIRECTORY open yields a directory"
        );

        self.walk_directory(root_fd, Vec::new(), dev_of(&root_stat), 0, sink)
    }
}

impl ConfinedCapture {
    /// Enumerates and captures one directory's descendants, all relative to
    /// its already-open descriptor.
    #[allow(clippy::too_many_arguments)]
    fn walk_directory(
        &self,
        dir_fd: std::os::fd::OwnedFd,
        prefix: Vec<u8>,
        dir_dev: u64,
        depth: usize,
        sink: &mut dyn FnMut(CapturedEntry) -> Result<(), ExternalFault>,
    ) -> Result<(), ExternalFault> {
        if depth > MAX_DEPTH {
            return Err(fault(
                code::CEILING,
                format!("directory nesting deeper than {MAX_DEPTH} levels"),
            ));
        }

        // Names are collected and sorted before any entry is processed, so
        // traversal order is deterministic and independent of readdir order.
        let mut names: Vec<Vec<u8>> = Vec::new();
        {
            let mut entries = rustix::fs::Dir::read_from(&dir_fd).map_err(|errno| {
                fault(code::CAPTURE_IO, format!("could not enumerate: {errno}"))
            })?;
            loop {
                match entries.next() {
                    Some(Ok(entry)) => {
                        let name = entry.file_name();
                        let bytes = name.to_bytes();
                        if bytes == b"." || bytes == b".." {
                            continue;
                        }
                        names.push(bytes.to_vec());
                    }
                    Some(Err(errno)) => {
                        return Err(fault(
                            code::CAPTURE_IO,
                            format!("directory enumeration failed: {errno}"),
                        ));
                    }
                    None => break,
                }
            }
        }
        names.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));

        for name in names {
            let component = component_bytes(&name, &prefix)?;
            if prefix.is_empty() && component == b".git" {
                // The Workspace's own Git administrative state: excluded from
                // logical payload unconditionally. Not skipped because it is
                // trusted — precisely because it never is — but because it is
                // not tree content. No indirection inside it (gitfiles,
                // alternates, config) can matter: nothing in it is ever
                // opened.
                continue;
            }

            let name_cstr = std::ffi::CString::new(component).map_err(|_| {
                fault(
                    code::HOSTILE_FILESYSTEM,
                    "an entry name contains a NUL byte",
                )
            })?;

            // Inspect the entry itself, without following it.
            let stat =
                rustix::fs::statat(&dir_fd, &name_cstr, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|errno| match errno {
                        rustix::io::Errno::NOENT => fault(
                            code::HOSTILE_FILESYSTEM,
                            format!(
                                "entry {}{} vanished during capture",
                                String::from_utf8_lossy(&prefix),
                                String::from_utf8_lossy(component)
                            ),
                        ),
                        other => fault(
                            code::CAPTURE_IO,
                            format!(
                                "could not inspect {}: {other}",
                                String::from_utf8_lossy(component)
                            ),
                        ),
                    })?;

            let mut path_bytes = prefix.clone();
            if !prefix.is_empty() {
                path_bytes.push(b'/');
            }
            path_bytes.extend_from_slice(component);
            let path = RepositoryPath::from_bytes(&path_bytes).map_err(|err| {
                fault(
                    code::HOSTILE_FILESYSTEM,
                    format!("a captured path cannot be represented: {err}"),
                )
            })?;

            // A nested repository is submodule semantics v1 does not
            // support, whatever filesystem type the `.git` entry has: a
            // directory for `git submodule add` in old layouts, a *gitfile*
            // (`gitdir: ...`) in modern ones, or a symlink. Silently
            // flattening any of them would publish one repository's
            // interior — or its indirection target's identity — as plain
            // content of another.
            if component == b".git" {
                return Err(fault(
                    code::HOSTILE_FILESYSTEM,
                    format!(
                        "{} nests a repository below the Workspace root",
                        path.to_manifest_string()
                    ),
                ));
            }

            match file_type_field(stat.st_mode) {
                FT_DIRECTORY => {
                    // An undeclared mount boundary: crossing onto another
                    // filesystem is an escape the Workspace binding did not
                    // declare.
                    if dev_of(&stat) != dir_dev {
                        return Err(fault(
                            code::HOSTILE_FILESYSTEM,
                            format!(
                                "{} crosses onto another filesystem",
                                path.to_manifest_string()
                            ),
                        ));
                    }
                    let child = rustix::fs::openat(
                        &dir_fd,
                        &name_cstr,
                        rustix::fs::OFlags::RDONLY
                            | rustix::fs::OFlags::DIRECTORY
                            | rustix::fs::OFlags::NOFOLLOW,
                        rustix::fs::Mode::empty(),
                    )
                    .map_err(|errno| match errno {
                        // Swapped for a symlink between inspection and open.
                        rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR => fault(
                            code::HOSTILE_FILESYSTEM,
                            format!(
                                "{} was replaced by non-directory state during capture",
                                path.to_manifest_string()
                            ),
                        ),
                        other => fault(
                            code::CAPTURE_IO,
                            format!(
                                "could not descend into {}: {other}",
                                path.to_manifest_string()
                            ),
                        ),
                    })?;
                    // Bind the opened descriptor to what was inspected: a
                    // swap to a *different directory* passes O_NOFOLLOW and
                    // dies here instead.
                    verify_identity(&child, &stat, &path)?;
                    self.walk_directory(child, path_bytes, dev_of(&stat), depth + 1, sink)?;
                }
                FT_SYMLINK => {
                    // Link-target bytes are data. The target is never opened;
                    // whether it points outward, inward, nowhere, or at
                    // /etc/shadows is irrelevant here by construction.
                    let target = rustix::fs::readlinkat(&dir_fd, &name_cstr, Vec::new()).map_err(
                        |errno| {
                            fault(
                                code::CAPTURE_IO,
                                format!(
                                    "could not read link {}: {errno}",
                                    path.to_manifest_string()
                                ),
                            )
                        },
                    )?;
                    sink(CapturedEntry {
                        path,
                        kind: EntryKind::Symlink,
                        bytes: target.into_bytes(),
                    })?;
                }
                FT_REGULAR => {
                    if u64::try_from(stat.st_size).unwrap_or(u64::MAX) > MAX_OBJECT_BYTES {
                        return Err(fault(
                            code::CEILING,
                            format!(
                                "{} exceeds the per-object ceiling",
                                path.to_manifest_string()
                            ),
                        ));
                    }
                    // NONBLOCK is load-bearing in the race case: an entry
                    // swapped for a FIFO between inspection and open would
                    // otherwise block this open until a writer appears —
                    // before verify_identity can reject it. On a FIFO,
                    // O_RDONLY|NONBLOCK returns immediately; on a regular
                    // file it is a no-op.
                    let file_fd = rustix::fs::openat(
                        &dir_fd,
                        &name_cstr,
                        rustix::fs::OFlags::RDONLY
                            | rustix::fs::OFlags::NOFOLLOW
                            | rustix::fs::OFlags::NONBLOCK,
                        rustix::fs::Mode::empty(),
                    )
                    .map_err(|errno| match errno {
                        // Swapped for a symlink (or directory) between
                        // inspection and open.
                        rustix::io::Errno::LOOP | rustix::io::Errno::ISDIR => fault(
                            code::HOSTILE_FILESYSTEM,
                            format!(
                                "{} was replaced by non-file state during capture",
                                path.to_manifest_string()
                            ),
                        ),
                        other => fault(
                            code::CAPTURE_IO,
                            format!("could not open {}: {other}", path.to_manifest_string()),
                        ),
                    })?;
                    // The payload comes from this descriptor, bound to the
                    // inspected object — never a second pathname lookup.
                    verify_identity(&file_fd, &stat, &path)?;
                    let mut file = std::fs::File::from(file_fd);
                    let bytes =
                        read_bounded(&mut file, stat.st_size.max(0) as u64, MAX_OBJECT_BYTES)
                            .map_err(|fault_kind| match fault_kind {
                                ReadFault::Io(err) => fault(
                                    code::CAPTURE_IO,
                                    format!("could not read {}: {err}", path.to_manifest_string()),
                                ),
                                ReadFault::Grew => fault(
                                    code::HOSTILE_FILESYSTEM,
                                    format!(
                                        "{} changed size while being captured",
                                        path.to_manifest_string()
                                    ),
                                ),
                                ReadFault::OverCeiling => fault(
                                    code::CEILING,
                                    format!(
                                        "{} exceeds the per-object ceiling",
                                        path.to_manifest_string()
                                    ),
                                ),
                            })?;
                    let kind = if stat.st_mode & 0o111 != 0 {
                        EntryKind::Executable
                    } else {
                        EntryKind::Regular
                    };
                    sink(CapturedEntry { path, kind, bytes })?;
                }
                // FIFOs, Unix sockets, block and character devices — and any
                // future file type — are refused, never interacted with.
                _ => {
                    return Err(fault(
                        code::HOSTILE_FILESYSTEM,
                        format!(
                            "{} is a special filesystem object capture must not touch",
                            path.to_manifest_string()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Validates one entry-name component against the path rules.
fn component_bytes<'a>(name: &'a [u8], prefix: &[u8]) -> Result<&'a [u8], ExternalFault> {
    match name {
        b"" | b"." | b".." => Err(fault(
            code::HOSTILE_FILESYSTEM,
            "a directory yielded a relative-path component",
        )),
        _ => {
            let _ = prefix;
            Ok(name)
        }
    }
}

/// Proves the opened descriptor is the very object the earlier inspection
/// saw.
///
/// This is the seam a pathname-based implementation would have left open:
/// opening by path again would resolve whatever sits there *now*. Identity
/// is inode plus device, the pair a filesystem guarantees for one object.
pub(crate) fn verify_identity(
    fd: &std::os::fd::OwnedFd,
    inspected: &rustix::fs::Stat,
    path: &RepositoryPath,
) -> Result<(), ExternalFault> {
    let opened = rustix::fs::fstat(fd).map_err(|errno| {
        fault(
            code::CAPTURE_IO,
            format!("could not verify identity: {errno}"),
        )
    })?;
    if opened.st_ino != inspected.st_ino || opened.st_dev != inspected.st_dev {
        return Err(fault(
            code::HOSTILE_FILESYSTEM,
            format!(
                "{} was replaced between inspection and open",
                path.to_manifest_string()
            ),
        ));
    }
    Ok(())
}

/// Reads authoritative before-state from the resolved immutable base.
///
/// The reader runs Git against the **source** repository — the
/// operator-supplied input the Workspace's own base resolution already
/// trusts at materialization time — under the sterile profile, and never
/// touches the worker Workspace's `.git`, index, refs or object database.
/// Worker-local commits, staging state and objects are untrusted input and
/// play no part here.
#[derive(Debug, Clone)]
pub struct GitBaseReader {
    sterile: SterileProfile,
}

impl GitBaseReader {
    /// Creates the controller-owned scratch state the sterile profile needs.
    ///
    /// # Errors
    ///
    /// Any I/O failure creating that state.
    pub fn new(control_root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            sterile: SterileProfile::create(control_root.as_ref())?,
        })
    }

    fn git(
        &self,
        code: &'static str,
        what: &str,
        args: &[&std::ffi::OsStr],
    ) -> Result<Vec<u8>, ExternalFault> {
        let output = self
            .sterile
            .run(args)
            .map_err(|err| fault(code, format!("could not run git while {what}: {err}")))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(fault(
                code,
                format!(
                    "git failed while {what}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ))
        }
    }

    /// Stages each payload as a scratch file and asks Git to name them all
    /// in one invocation.
    fn hash_staged_blobs(
        &self,
        source: &Path,
        stage: &Path,
        contents: &[&[u8]],
    ) -> Result<Vec<String>, ExternalFault> {
        let mut paths = String::new();
        for (index, bytes) in contents.iter().enumerate() {
            let staged = stage.join(format!("payload-{index:06}"));
            std::fs::write(&staged, bytes).map_err(|err| {
                fault(
                    code::CAPTURE_IO,
                    format!("could not stage comparison payload: {err}"),
                )
            })?;
            paths.push_str(&staged.to_string_lossy());
            paths.push('\n');
        }

        // The path list is written from a thread so neither pipe can fill
        // and block the other; a write failure surfaces through Git's own
        // exit status, which is checked below anyway.
        let mut child = self
            .sterile
            .command()
            .args([
                std::ffi::OsStr::new("-C"),
                source.as_os_str(),
                std::ffi::OsStr::new("hash-object"),
                std::ffi::OsStr::new("--stdin-paths"),
                std::ffi::OsStr::new("--no-filters"),
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| {
                fault(
                    code::BASE_UNAVAILABLE,
                    format!("could not run git while naming captured blobs: {err}"),
                )
            })?;
        let writer = {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| fault(code::BASE_UNAVAILABLE, "git stdin was not piped"))?;
            std::thread::spawn(move || {
                use std::io::Write;
                stdin.write_all(paths.as_bytes())
            })
        };
        let output = child.wait_with_output().map_err(|err| {
            fault(
                code::BASE_UNAVAILABLE,
                format!("could not read object names from git: {err}"),
            )
        })?;
        if writer.join().is_err() {
            return Err(fault(
                code::BASE_UNAVAILABLE,
                "could not feed paths to git hash-object",
            ));
        }
        if !output.status.success() {
            return Err(fault(
                code::BASE_UNAVAILABLE,
                format!(
                    "git failed while naming captured blobs: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        parse_object_names(&output.stdout).ok_or_else(|| {
            fault(
                code::BASE_UNAVAILABLE,
                "git hash-object output is not one canonical object name per payload",
            )
        })
    }
}

impl TrustedBaseReader for GitBaseReader {
    fn blob_object_names(
        &self,
        source: &Path,
        contents: &[&[u8]],
    ) -> Result<Vec<String>, ExternalFault> {
        if contents.is_empty() {
            return Ok(Vec::new());
        }

        // The payloads are staged as files beneath controller-owned scratch
        // state — paths this process derived, never anything an Agent can
        // write — so one Git invocation can name all of them. `hash-object`
        // without `-w` writes no object anywhere, in the source or out of
        // it; it consults the source repository only for its configured
        // object format, which is exactly what makes the names comparable
        // with the base tree's. `--no-filters` pins the identity to raw
        // bytes alone: no clean filter, CRLF conversion or attribute rule
        // may influence what a captured payload's name is. The exclusion is
        // safe because its failure direction is harmless: a filter that
        // *would* have transformed content can only make an unchanged file
        // look changed — costing one extra preimage fetch that then
        // classifies correctly — it can never make a changed file look
        // unchanged.
        let stage = self.sterile.scratch_dir("base-compare").map_err(|err| {
            fault(
                code::CAPTURE_IO,
                format!("could not create comparison staging: {err}"),
            )
        })?;
        let result = self.hash_staged_blobs(source, &stage, contents);
        let _ = std::fs::remove_dir_all(&stage);
        result
    }

    fn base_tree(
        &self,
        source: &Path,
        base: &ResolvedBase,
    ) -> Result<BTreeMap<Vec<u8>, BaseObject>, ExternalFault> {
        let revision = base.as_str().to_string();
        let out = self.git(
            code::BASE_UNAVAILABLE,
            "reading the immutable base tree",
            &[
                std::ffi::OsStr::new("-C"),
                source.as_os_str(),
                std::ffi::OsStr::new("ls-tree"),
                std::ffi::OsStr::new("--full-tree"),
                std::ffi::OsStr::new("-r"),
                std::ffi::OsStr::new("-l"),
                std::ffi::OsStr::new("-z"),
                std::ffi::OsStr::new(&revision),
            ],
        )?;

        let mut tree = BTreeMap::new();
        for record in out.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let Some((meta, path_bytes)) = record
                .iter()
                .position(|b| *b == b'\t')
                .map(|tab| (&record[..tab], &record[tab + 1..]))
            else {
                return Err(fault(
                    code::HOSTILE_REPOSITORY,
                    "the base tree listing is not shaped like ls-tree output",
                ));
            };
            let fields: Vec<&[u8]> = meta
                .split(|b| *b == b' ')
                .filter(|field| !field.is_empty())
                .collect();
            let [mode, kind, oid, size] = fields.as_slice() else {
                return Err(fault(
                    code::HOSTILE_REPOSITORY,
                    "a base tree record does not have four fields",
                ));
            };

            let kind = match (*kind, *mode) {
                (b"blob", b"100644") => EntryKind::Regular,
                (b"blob", b"100755") => EntryKind::Executable,
                (b"blob", b"120000") => EntryKind::Symlink,
                // Submodule/gitlink entries: explicit v1 policy is refusal.
                (b"commit", _) | (b"blob", b"160000") => {
                    return Err(fault(
                        code::HOSTILE_REPOSITORY,
                        "the base tree contains a submodule entry, which v1 capture \
                         does not support",
                    ));
                }
                _ => {
                    return Err(fault(
                        code::HOSTILE_REPOSITORY,
                        "the base tree contains an unsupported entry type",
                    ));
                }
            };

            // Lossless validation: a base path that cannot be represented is
            // refused, never normalized.
            RepositoryPath::from_bytes(path_bytes).map_err(|err| {
                fault(
                    code::HOSTILE_REPOSITORY,
                    format!("the base tree names an unusable path: {err}"),
                )
            })?;

            let oid = std::str::from_utf8(oid)
                .map_err(|_| fault(code::HOSTILE_REPOSITORY, "object name is not UTF-8"))?
                .to_string();
            let size = std::str::from_utf8(size)
                .ok()
                .and_then(|text| text.trim_start_matches('-').parse::<u64>().ok())
                .ok_or_else(|| {
                    fault(
                        code::HOSTILE_REPOSITORY,
                        "a base tree record has no usable size",
                    )
                })?;

            tree.insert(path_bytes.to_vec(), BaseObject { kind, oid, size });
        }
        Ok(tree)
    }

    fn blob_bytes(&self, source: &Path, oid: &str) -> Result<Vec<u8>, ExternalFault> {
        // Validated before it can reach a command line: it came from our own
        // listing, and refusing non-canonical forms costs nothing.
        if !is_canonical_object_name(oid) {
            return Err(fault(
                code::HOSTILE_REPOSITORY,
                format!("{oid:?} is not a canonical object name"),
            ));
        }
        self.git(
            code::BASE_UNAVAILABLE,
            "reading a base blob",
            &[
                std::ffi::OsStr::new("-C"),
                source.as_os_str(),
                std::ffi::OsStr::new("cat-file"),
                std::ffi::OsStr::new("blob"),
                std::ffi::OsStr::new(oid),
            ],
        )
    }
}

/// Whether `name` is a canonical Git object name: 40 hex digits for a
/// SHA-1 repository, 64 for a SHA-256 one. The reader accepts both because
/// the source repository's object format is its own decision, never an
/// assumption.
pub(crate) fn is_canonical_object_name(name: &str) -> bool {
    matches!(name.len(), 40 | 64)
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Parses `git hash-object --stdin-paths` output: one canonical object
/// name per line, in input order. Any other shape is unusable — a partial
/// or malformed answer must fail closed rather than let a candidate entry
/// pass as unchanged.
pub(crate) fn parse_object_names(output: &[u8]) -> Option<Vec<String>> {
    let text = std::str::from_utf8(output).ok()?;
    let mut names = Vec::new();
    for line in text.lines() {
        if !is_canonical_object_name(line) {
            return None;
        }
        names.push(line.to_string());
    }
    Some(names)
}

/// Why a bounded payload read refused.
#[derive(Debug)]
pub(crate) enum ReadFault {
    /// The descriptor could not be read.
    Io(std::io::Error),
    /// The object's byte count no longer matches what its validated
    /// descriptor reported: it changed while being captured.
    Grew,
    /// The read exceeded the per-object ceiling before reaching the
    /// expected end — bounds allocation regardless of what the writer does.
    OverCeiling,
}

/// Reads exactly `expected` bytes, refusing to buffer more than `max`.
///
/// The ceiling check at inspection time is not enough on its own: a file
/// grown after `fstat` would otherwise be read to EOF in full before any
/// size comparison runs. Reading through a `Take` limited to `max + 1`
/// keeps the allocation bound real; the one extra byte distinguishes "at
/// the ceiling" from "over it".
pub(crate) fn read_bounded(
    file: &mut impl std::io::Read,
    expected: u64,
    max: u64,
) -> Result<Vec<u8>, ReadFault> {
    let mut bytes = Vec::with_capacity(expected.min(max + 1) as usize);
    let mut limited = file.take(max.saturating_add(1));
    limited.read_to_end(&mut bytes).map_err(ReadFault::Io)?;
    if bytes.len() as u64 > max {
        return Err(ReadFault::OverCeiling);
    }
    if bytes.len() as u64 != expected {
        return Err(ReadFault::Grew);
    }
    Ok(bytes)
}
