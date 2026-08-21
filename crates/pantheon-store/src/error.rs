//! Typed failures from opening, migrating, or operating the authoritative
//! store.

use std::fmt;
use std::path::PathBuf;

/// A store-level failure, typed distinctly from an ordinary SQLite error so
/// callers can react to a fail-closed condition rather than a transient one.
#[derive(Debug)]
pub enum StoreError {
    /// An underlying SQLite operation failed.
    Sqlite(rusqlite::Error),
    /// The database's `PRAGMA user_version` names a schema version newer
    /// than any migration this build knows about. Opening it would risk
    /// silently misinterpreting a schema this build cannot understand, so
    /// the store fails closed instead.
    UnsupportedSchemaVersion { found: i64, max_known: i64 },
    /// The database's migration bookkeeping does not agree with the
    /// compiled migration set: a gap, a recorded version this build does
    /// not know, or a checksum mismatch against the compiled SQL.
    InconsistentMigrationState(String),
    /// A migration failed to apply. The transaction attempting it was
    /// rolled back in full, so the database retains neither a partial
    /// schema change from this migration nor an advanced version number.
    MigrationFailed {
        version: i64,
        name: &'static str,
        source: rusqlite::Error,
    },
    /// A connection policy setting did not read back as the value Pantheon
    /// requires after being applied.
    PolicyVerificationFailed(String),
    /// A revisioned mutation did not apply because the row's authoritative
    /// revision is not the one the caller expected.
    ///
    /// This is an ordinary optimistic-concurrency outcome, not a storage
    /// failure: the caller observed a revision, someone else advanced it
    /// first, and the caller must re-read and decide again. It is a
    /// distinct variant precisely so a caller can tell it apart from
    /// [`StoreError::Sqlite`] and from [`StoreError::InvariantViolated`]
    /// without inspecting message text.
    ///
    /// `actual` distinguishes the two ways the expectation can fail:
    /// `Some(revision)` means the row exists at a different revision
    /// (stale), and `None` means no such row exists (missing).
    RevisionConflict {
        table: &'static str,
        id: String,
        expected: i64,
        actual: Option<i64>,
    },
    /// The store observed a condition that must be impossible, and refused
    /// to continue rather than commit through it.
    ///
    /// Distinct from [`StoreError::RevisionConflict`]: a conflict is an
    /// expected concurrent outcome, whereas this means an invariant the
    /// store enforces has already been violated. Any authoritative
    /// transaction in progress is rolled back.
    InvariantViolated(String),
    /// This process already has an open [`crate::Store`] for this database
    /// file.
    ///
    /// Two `Store` values on one path would be two independent
    /// authoritative writer connections, which is exactly what the
    /// serialized-writer boundary exists to prevent. Opening the second one
    /// fails closed rather than silently degrading write serialization to
    /// SQLite's file lock. This is an in-process guard; excluding other
    /// *processes* is the daemon's operating-system installation lock, per
    /// `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`.
    AlreadyOpen { path: PathBuf },
    /// The command carried an epoch that is not the installation's current
    /// RestoreGeneration.
    ///
    /// Fails closed as stale command authority. This is decided *before* the
    /// command ledger is consulted: a restored snapshot may have lost the row
    /// for a command that already produced an effect, so the absence of a
    /// row can never make an old-epoch request new.
    StaleCommandEpoch { supplied: String, current: String },
    /// This command identity was already used in this epoch with a different
    /// non-sensitive request hash.
    ///
    /// A command ID is single-use within its epoch. Reusing one for a
    /// different request is a caller defect, not a retry, so it fails closed
    /// rather than executing or overwriting the durable identity.
    CommandConflict { command_id: String },
    /// The Goal cannot be driven toward `Cancelled` from the phase it is in.
    ///
    /// Typed rather than a generic conflict so a caller can tell it apart from
    /// a stale revision. The Goal lifecycle contract grants cancellation to
    /// every nonterminal phase — including retargeting a `Finalizing` Goal
    /// that is heading for `Succeeded` or `Failed` — so the one refused case
    /// is a terminal Goal: terminal history never reopens, and nothing is
    /// written.
    GoalNotCancellable {
        goal_id: String,
        phase: &'static str,
        terminal_target: Option<String>,
    },
    /// A connection Pantheon needs could not be used: the authoritative
    /// writer is poisoned by an earlier panic, or a caller attempted to
    /// re-enter the serialized authoritative writer it already holds.
    ConnectionUnavailable(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(err) => write!(f, "sqlite error: {err}"),
            Self::UnsupportedSchemaVersion { found, max_known } => write!(
                f,
                "database schema version {found} is newer than the highest \
                 migration this build knows ({max_known})"
            ),
            Self::InconsistentMigrationState(detail) => {
                write!(f, "inconsistent migration bookkeeping: {detail}")
            }
            Self::MigrationFailed {
                version,
                name,
                source,
            } => write!(
                f,
                "migration {version} ({name}) failed and was rolled back: {source}"
            ),
            Self::GoalNotCancellable {
                goal_id,
                phase,
                terminal_target,
            } => match terminal_target {
                Some(target) => write!(
                    f,
                    "goal {goal_id} is {phase} toward {target} and cannot be cancelled"
                ),
                None => write!(f, "goal {goal_id} is {phase} and cannot be cancelled"),
            },
            Self::PolicyVerificationFailed(detail) => {
                write!(f, "connection policy verification failed: {detail}")
            }
            Self::RevisionConflict {
                table,
                id,
                expected,
                actual,
            } => match actual {
                Some(actual) => write!(
                    f,
                    "stale revision for {table} row {id}: expected {expected}, \
                     found {actual}"
                ),
                None => write!(
                    f,
                    "no {table} row {id} to mutate at expected revision {expected}"
                ),
            },
            Self::AlreadyOpen { path } => write!(
                f,
                "this process already has an open store for {}",
                path.display()
            ),
            Self::StaleCommandEpoch { supplied, current } => write!(
                f,
                "stale command epoch {supplied}: the current restore generation \
                 is {current}"
            ),
            Self::CommandConflict { command_id } => write!(
                f,
                "command {command_id} was already used in this epoch with a \
                 different request hash"
            ),
            Self::InvariantViolated(detail) => {
                write!(f, "store invariant violated: {detail}")
            }
            Self::ConnectionUnavailable(detail) => {
                write!(f, "store connection unavailable: {detail}")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(err) => Some(err),
            Self::MigrationFailed { source, .. } => Some(source),
            Self::UnsupportedSchemaVersion { .. }
            | Self::InconsistentMigrationState(_)
            | Self::PolicyVerificationFailed(_)
            | Self::RevisionConflict { .. }
            | Self::AlreadyOpen { .. }
            | Self::StaleCommandEpoch { .. }
            | Self::CommandConflict { .. }
            | Self::GoalNotCancellable { .. }
            | Self::InvariantViolated(_)
            | Self::ConnectionUnavailable(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}
