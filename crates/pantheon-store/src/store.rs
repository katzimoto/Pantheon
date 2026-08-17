//! `Store`: Pantheon's authoritative local SQLite database.

use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::error::StoreError;
use crate::{migrations, policy};

/// The `OpenFlags` `Store::open` uses. Extracted as a constant so a unit
/// test can assert `SQLITE_OPEN_PRIVATE_CACHE` is set and
/// `SQLITE_OPEN_SHARED_CACHE` is not, since SQLite exposes no per-connection
/// `PRAGMA` that reflects shared-cache status after the fact — see
/// `crate::policy`.
const OPEN_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_WRITE
    .union(OpenFlags::SQLITE_OPEN_CREATE)
    .union(OpenFlags::SQLITE_OPEN_NO_MUTEX)
    .union(OpenFlags::SQLITE_OPEN_PRIVATE_CACHE);

/// Pantheon's authoritative local SQLite store.
///
/// Owns exactly one connection: the small bounded connection/write
/// mechanics this mission requires to create, open, migrate, validate,
/// close, and reopen the control-plane database. The reusable
/// state-dependent authoritative write/CAS transaction mechanism (a
/// serialized writer plus a bounded read pool) belongs to a later mission;
/// this crate does not build that abstraction speculatively.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

/// The installation's RestoreGeneration.
///
/// A fresh unpredictable value, established once at first installation and
/// preserved across ordinary daemon restart. Rotating it is the exclusive
/// responsibility of the disaster-restore authority fence (T0), which this
/// mission does not implement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreGeneration(String);

impl RestoreGeneration {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RestoreGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Store {
    /// Opens (creating if necessary) the authoritative SQLite database at
    /// `path`, applies and verifies the v1 connection policy, and brings
    /// the schema up to date through the ordered migration set.
    ///
    /// Fails closed — returning a typed [`StoreError`] rather than opening
    /// a store in a state Pantheon cannot trust — when the connection
    /// policy cannot be verified, or the database's migration state is
    /// unsupported or inconsistent.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut conn = Connection::open_with_flags(path, OPEN_FLAGS)?;
        policy::apply_and_verify(&conn)?;
        migrations::run(&mut conn)?;

        Ok(Self { conn })
    }

    /// Returns the installation's current RestoreGeneration.
    pub fn restore_generation(&self) -> Result<RestoreGeneration, StoreError> {
        let value: String = self.conn.query_row(
            "SELECT restore_generation FROM system_state WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(RestoreGeneration(value))
    }

    /// Closes the store.
    ///
    /// Returns any error SQLite reports while finalizing outstanding
    /// resources rather than silently discarding it, which a bare `drop`
    /// cannot do.
    pub fn close(self) -> Result<(), StoreError> {
        self.conn
            .close()
            .map_err(|(_, err)| StoreError::Sqlite(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_flags_use_a_private_cache_never_a_shared_one() {
        assert!(OPEN_FLAGS.contains(OpenFlags::SQLITE_OPEN_PRIVATE_CACHE));
        assert!(!OPEN_FLAGS.contains(OpenFlags::SQLITE_OPEN_SHARED_CACHE));
    }
}
