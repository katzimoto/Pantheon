//! The immutable Artifact vocabulary for `code.changeset` sealing.
//!
//! `docs/architecture/artifacts-and-workspaces/artifact-model.md` is
//! canonical for what an Artifact is; this module holds only the pure part:
//! the provider-neutral types a captured Workspace state and its canonical
//! `code.changeset` manifest are built from, and the rules that make their
//! identity deterministic.
//!
//! Nothing here touches a filesystem, a repository or a database. Deciding
//! *which* bytes to capture, from where, and under what authority belongs to
//! `pantheon-engine` and the platform crates behind its ports; this module
//! only guarantees that once the semantic state is fixed, its identity is too.
//!
//! # Determinism contract
//!
//! Two captures of the same permitted logical state must produce the same
//! revision-state digest and Artifact identity regardless of directory
//! iteration order, file mtimes, staging/index state, worker-local commits,
//! branch names, capture time or command identity. That is made true here by
//! construction rather than by discipline:
//!
//! - every collection that reaches a digest is sorted by canonical path
//!   *bytes* on construction;
//! - nothing derived from wall-clock time, random row identity, host paths,
//!   CAS locations or command provenance enters any digest — provenance is
//!   relational (`created_at`, owning rows), never manifest content;
//! - hashing reuses [`crate::config::canonical::Value`] and
//!   [`crate::config::Digest`], so there is exactly one canonical JSON
//!   encoding in Pantheon and it is not reimplemented here.

use std::fmt;

use crate::config::Digest;
use crate::config::canonical::Value;

/// The `schemaVersion` of the canonical `code.changeset` manifest.
///
/// Part of manifest identity on purpose: a future format change is a new
/// schema version, never a silent reinterpretation of the old one.
pub const CHANGESET_SCHEMA_VERSION: i64 = 1;

/// The Artifact kind sealed by Workspace capture.
pub const CODE_CHANGESET_KIND: &str = "code.changeset";

/// Why a changeset entry or path was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesetError {
    /// A repository path is not representable losslessly or violates the
    /// canonical path rules.
    InvalidPath { value: String, reason: &'static str },
    /// An operation contradicts its before/after states.
    OperationStateMismatch { detail: String },
}

impl fmt::Display for ChangesetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath { value, reason } => {
                write!(f, "repository path {value:?} is invalid: {reason}")
            }
            Self::OperationStateMismatch { detail } => {
                write!(f, "changeset entry is inconsistent: {detail}")
            }
        }
    }
}

impl std::error::Error for ChangesetError {}

/// A repository path held as raw bytes, losslessly.
///
/// Git paths are byte sequences, not strings: a Unix working tree may name a
/// file with bytes that are not valid UTF-8, and the Artifact contract
/// requires those paths to survive capture without lossy normalization. This
/// type therefore carries the raw bytes and validates only what makes a path
/// unrepresentable or ambiguous.
///
/// # Validation rules
///
/// A path is refused when it is empty, contains a NUL byte, has more than
/// [`MAX_PATH_BYTES`] bytes or a component longer than 255 bytes (the usual
/// filesystem component bound, applied uniformly), or when any component is
/// empty, `.`, or `..`. Leading `/` is refused because capture produces
/// root-relative paths by construction; anything else would claim an
/// absolute location inside a relative namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepositoryPath(Vec<u8>);

/// The longest repository path Pantheon accepts, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;

impl RepositoryPath {
    /// Validates raw path bytes into a [`RepositoryPath`].
    ///
    /// # Errors
    ///
    /// [`ChangesetError::InvalidPath`] with the specific rule broken.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ChangesetError> {
        let refuse = |reason: &'static str| {
            Err(ChangesetError::InvalidPath {
                // Diagnostics render the path losslessly through the same
                // encoding the manifest uses, even when refusing it.
                value: encode_path(bytes),
                reason,
            })
        };
        if bytes.is_empty() {
            return refuse("it is empty");
        }
        if bytes.len() > MAX_PATH_BYTES {
            return refuse("it is longer than 4096 bytes");
        }
        if bytes.contains(&0) {
            return refuse("it contains a NUL byte");
        }
        if bytes[0] == b'/' {
            return refuse("it is absolute");
        }
        for component in bytes.split(|b| *b == b'/') {
            match component {
                b"" => return refuse("it has an empty path component"),
                b"." => return refuse("a component is `.`"),
                b".." => return refuse("a component is `..`"),
                _ => {}
            }
            if component.len() > 255 {
                return refuse("a component is longer than 255 bytes");
            }
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Rebuilds a path from its manifest spelling.
    ///
    /// Inverse of [`RepositoryPath::to_manifest_string`] and subject to the
    /// same validation, so a decoded path can never be more permissive than a
    /// captured one.
    ///
    /// # Errors
    ///
    /// [`ChangesetError::InvalidPath`] for malformed escapes or a path that
    /// fails validation after decoding.
    pub fn from_manifest_string(text: &str) -> Result<Self, ChangesetError> {
        let bytes = decode_manifest_string(text).ok_or_else(|| ChangesetError::InvalidPath {
            value: text.to_string(),
            reason: "it is not a valid manifest spelling",
        })?;
        Self::from_bytes(&bytes)
    }

    /// The raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The lossless manifest spelling of these bytes.
    ///
    /// The encoding is deliberately tiny and injective:
    ///
    /// - bytes that are valid UTF-8 and contain no `%` spell themselves;
    /// - everything else spells as `%` followed by lowercase hex pairs for
    ///   *every* byte.
    ///
    /// Because literal form can never contain `%`, the two forms cannot be
    /// confused, decoding is total, and ordinary ASCII/UTF-8 paths stay
    /// readable while non-UTF-8 paths stay exact. The choice this settles —
    /// which encoding "an explicitly lossless byte encoding" means — is
    /// recorded in `docs/architecture/artifacts-and-workspaces/artifact-model.md`.
    #[must_use]
    pub fn to_manifest_string(&self) -> String {
        encode_path(&self.0)
    }

    /// The sort key: the raw path bytes.
    ///
    /// Entries are ordered by canonical path bytes, as the contract requires;
    /// encoded-string order would differ for escaped paths and is not used.
    #[must_use]
    pub fn sort_key(&self) -> &[u8] {
        &self.0
    }
}

fn encode_path(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.contains('%') => text.to_string(),
        _ => {
            let mut out = String::with_capacity(1 + bytes.len() * 3);
            out.push('%');
            for byte in bytes {
                out.push_str(&format!("{byte:02x}"));
            }
            out
        }
    }
}

fn decode_manifest_string(text: &str) -> Option<Vec<u8>> {
    if !text.starts_with('%') {
        // Literal form: no `%` is possible, so this is injective.
        return Some(text.as_bytes().to_vec());
    }
    let hex = &text[1..];
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let hex = hex.as_bytes();
    for pair in hex.chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
    }
    Some(bytes)
}

impl fmt::Display for RepositoryPath {
    /// The manifest spelling — the one rendering that is always lossless.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_manifest_string())
    }
}

/// The canonical content kind and mode of one present entry.
///
/// These are the v1 Git-style modes the Artifact contract supports. Empty
/// directories are not tree content in Git and are not represented here;
/// directories exist only as traversal structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Regular,
    Executable,
    Symlink,
}

impl EntryKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Executable => "executable",
            Self::Symlink => "symlink",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "regular" => Self::Regular,
            "executable" => Self::Executable,
            "symlink" => Self::Symlink,
            _ => return None,
        })
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One side (before or after) of a changed path.
///
/// A present state always carries a verified CAS reference: the contract
/// forbids a present state whose semantics depend on payload bytes without
/// immutable content behind them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    Absent,
    Present {
        kind: EntryKind,
        blob: Digest,
        size: u64,
    },
}

impl EntryState {
    fn to_value(&self) -> Value {
        match self {
            Self::Absent => Value::object([("state", Value::string("absent"))]),
            Self::Present { kind, blob, size } => Value::object([
                ("state", Value::string("present")),
                ("mode", Value::string(kind.as_str())),
                ("blob", Value::string(blob.to_string())),
                ("size", Value::Integer(*size as i64)),
            ]),
        }
    }
}

/// What changed at one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Add,
    Modify,
    Delete,
}

impl Operation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }
}

/// One changed path with its authoritative before/after states.
///
/// Constructed only through [`ChangesetEntry::new`], which enforces the
/// operation/state agreement the contract states:
///
/// ```text
/// add    -> before absent,  after present
/// modify -> before present, after present
/// delete -> before present, after absent
/// ```
///
/// Rename is not a distinct operation in v1: it is represented as a delete
/// plus an add, because semantic candidate identity is resulting changed-path
/// state, not a diff heuristic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesetEntry {
    path: RepositoryPath,
    operation: Operation,
    before: EntryState,
    after: EntryState,
}

impl ChangesetEntry {
    /// Builds one entry, enforcing operation/state agreement.
    ///
    /// # Errors
    ///
    /// [`ChangesetError::OperationStateMismatch`] when the states do not
    /// agree with the named operation.
    pub fn new(
        path: RepositoryPath,
        operation: Operation,
        before: EntryState,
        after: EntryState,
    ) -> Result<Self, ChangesetError> {
        let mismatch = |detail: String| Err(ChangesetError::OperationStateMismatch { detail });
        let present = |state: &EntryState| matches!(state, EntryState::Present { .. });
        match operation {
            Operation::Add if !present(&before) && present(&after) => {}
            Operation::Modify if present(&before) && present(&after) => {}
            Operation::Delete if present(&before) && !present(&after) => {}
            other => {
                let describe = |state: &EntryState| match state {
                    EntryState::Absent => "absent".to_string(),
                    EntryState::Present { kind, .. } => format!("present ({kind})"),
                };
                return mismatch(format!(
                    "{} claims before {} and after {}",
                    other.as_str(),
                    describe(&before),
                    describe(&after)
                ));
            }
        }
        Ok(Self {
            path,
            operation,
            before,
            after,
        })
    }

    /// The changed path.
    #[must_use]
    pub fn path(&self) -> &RepositoryPath {
        &self.path
    }

    /// The operation.
    #[must_use]
    pub const fn operation(&self) -> Operation {
        self.operation
    }

    /// The authoritative before state.
    #[must_use]
    pub const fn before(&self) -> &EntryState {
        &self.before
    }

    /// The authoritative after state.
    #[must_use]
    pub const fn after(&self) -> &EntryState {
        &self.after
    }

    fn to_value(&self) -> Value {
        Value::object([
            ("path", Value::string(self.path.to_manifest_string())),
            ("operation", Value::string(self.operation.as_str())),
            ("before", self.before.to_value()),
            ("after", self.after.to_value()),
        ])
    }
}

/// One present entry of a captured final logical state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalEntry {
    pub path: RepositoryPath,
    pub kind: EntryKind,
    pub blob: Digest,
    pub size: u64,
}

/// The exact captured logical state of a Workspace, as an immutable value.
///
/// Constructed from entries and normalized immediately: entries are sorted by
/// canonical path bytes and duplicates are refused, so the state's digest is
/// a function of the semantic content alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionState {
    entries: Vec<FinalEntry>,
    digest: Digest,
}

impl RevisionState {
    /// Normalizes captured entries into an immutable state.
    ///
    /// # Errors
    ///
    /// [`ChangesetError::InvalidPath`] when two entries share one path.
    pub fn new(entries: Vec<FinalEntry>) -> Result<Self, ChangesetError> {
        let mut entries = entries;
        entries.sort_by(|a, b| a.path.sort_key().cmp(b.path.sort_key()));
        let duplicate = entries.windows(2).find_map(|pair| {
            (pair[0].path.sort_key() == pair[1].path.sort_key()).then(|| pair[1].path.clone())
        });
        if let Some(duplicate) = duplicate {
            return Err(ChangesetError::InvalidPath {
                value: duplicate.to_manifest_string(),
                reason: "the captured state names it twice",
            });
        }

        let value = Value::array(entries.iter().map(|entry| {
            Value::object([
                ("path", Value::string(entry.path.to_manifest_string())),
                ("mode", Value::string(entry.kind.as_str())),
                ("blob", Value::string(entry.blob.to_string())),
                ("size", Value::Integer(entry.size as i64)),
            ])
        }));
        let digest = value.digest();
        Ok(Self { entries, digest })
    }

    /// The present entries, ordered by canonical path bytes.
    #[must_use]
    pub fn entries(&self) -> &[FinalEntry] {
        &self.entries
    }

    /// The immutable semantic-state digest.
    ///
    /// Deterministic over the captured logical state alone: it binds neither
    /// base commit nor ownership nor time, so semantically identical captures
    /// converge on one digest wherever they occur.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// The canonical JSON document of the state, for durable storage beside
    /// its digest.
    ///
    /// The document is the same array the digest is taken over, wrapped in an
    /// object naming the schema version so stored state remains legible and
    /// versioned without changing content identity.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        Value::object([
            ("schemaVersion", Value::Integer(CHANGESET_SCHEMA_VERSION)),
            (
                "entries",
                Value::array(self.entries.iter().map(|entry| {
                    Value::object([
                        ("path", Value::string(entry.path.to_manifest_string())),
                        ("mode", Value::string(entry.kind.as_str())),
                        ("blob", Value::string(entry.blob.to_string())),
                        ("size", Value::Integer(entry.size as i64)),
                    ])
                })),
            ),
        ])
        .to_string()
    }

    /// Looks up one path's final state.
    #[must_use]
    pub fn entry(&self, path: &RepositoryPath) -> Option<&FinalEntry> {
        self.entries
            .binary_search_by(|entry| entry.path.sort_key().cmp(path.sort_key()))
            .ok()
            .map(|index| &self.entries[index])
    }
}

/// Builds the canonical `code.changeset` manifest value.
///
/// The manifest binds exactly what the contract requires and nothing
/// incidental: schema/kind identity, the repository's semantic identity, the
/// resolved immutable base commit, the immutable WorkspaceRevision *state*
/// digest, and the ordered changed-path entries. Wall-clock time, row ids and
/// CAS locations are deliberately absent — they live in relational provenance
/// so that producing the same changeset twice yields one content identity.
#[must_use]
pub fn changeset_manifest(
    repository: &str,
    base_commit: &str,
    workspace_revision_state: Digest,
    entries: &[ChangesetEntry],
) -> Value {
    // Entries must already be ordered by canonical path bytes; sorting here
    // would silently repair a caller that violated the contract, and a
    // manifest whose order depended on where it was built would not be
    // canonical.
    let mut ordered: Vec<&ChangesetEntry> = entries.iter().collect();
    ordered.sort_by(|a, b| a.path.sort_key().cmp(b.path.sort_key()));
    debug_assert!(ordered.len() == entries.len(), "entries must be unique");

    Value::object([
        ("schemaVersion", Value::Integer(CHANGESET_SCHEMA_VERSION)),
        ("artifactKind", Value::string(CODE_CHANGESET_KIND)),
        ("repository", Value::string(repository)),
        ("baseCommit", Value::string(base_commit)),
        (
            "workspaceRevision",
            Value::string(workspace_revision_state.to_string()),
        ),
        (
            "entries",
            Value::array(ordered.iter().map(|entry| entry.to_value())),
        ),
    ])
}

#[cfg(test)]
mod tests;
