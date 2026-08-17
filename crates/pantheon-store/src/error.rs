//! Typed failures from opening, migrating, or operating the authoritative
//! store.

use std::fmt;

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
            Self::PolicyVerificationFailed(detail) => {
                write!(f, "connection policy verification failed: {detail}")
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
            | Self::PolicyVerificationFailed(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}
