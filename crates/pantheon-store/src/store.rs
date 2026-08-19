//! `Store`: Pantheon's authoritative local SQLite database.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use rusqlite::{Connection, OpenFlags};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::transaction::{Revision, Writer};
use crate::{command, migrations, policy, transaction};

/// The `OpenFlags` the authoritative writer connection uses. Extracted as a
/// constant so a unit test can assert `SQLITE_OPEN_PRIVATE_CACHE` is set and
/// `SQLITE_OPEN_SHARED_CACHE` is not, since SQLite exposes no per-connection
/// `PRAGMA` that reflects shared-cache status after the fact — see
/// `crate::policy`.
const WRITER_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_WRITE
    .union(OpenFlags::SQLITE_OPEN_CREATE)
    .union(OpenFlags::SQLITE_OPEN_NO_MUTEX)
    .union(OpenFlags::SQLITE_OPEN_PRIVATE_CACHE);

/// The `OpenFlags` the read connection uses. `SQLITE_OPEN_READ_WRITE` and
/// `SQLITE_OPEN_CREATE` are absent on purpose: read/write separation is a
/// property SQLite enforces on this handle, not a convention Pantheon
/// remembers.
const READER_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_ONLY
    .union(OpenFlags::SQLITE_OPEN_NO_MUTEX)
    .union(OpenFlags::SQLITE_OPEN_PRIVATE_CACHE);

/// The database files this process currently has a [`Store`] open for.
///
/// Two `Store` values on one file would be two independent authoritative
/// writer connections, and the serialized-writer boundary would degrade
/// silently to SQLite's file lock. Opening the second fails closed instead.
///
/// This is deliberately only the in-process half of the guarantee. Keeping
/// *other processes* off the database is the daemon's operating-system
/// installation lock, which the recovery contract places at daemon scope;
/// this is not that lock and does not pretend to be.
fn open_stores() -> &'static Mutex<HashSet<PathBuf>> {
    static OPEN_STORES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    OPEN_STORES.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Holds this process's claim on one database file for the lifetime of the
/// `Store` that owns it. Releasing is `Drop`'s job, so an early return, a
/// panic, or an ordinary drop all release it.
#[derive(Debug)]
struct PathClaim(PathBuf);

impl PathClaim {
    /// Claims `path` for this process, keyed by its fully canonical form.
    ///
    /// The *whole* path is canonicalized, not just its parent, so a
    /// symlinked database file resolves to the same key as its target and
    /// the two spellings collide as they should. That requires the file to
    /// already exist, which is why [`Store::open`] claims only after
    /// opening the connection: opening creates the database file but
    /// applies no connection policy, runs no migration and performs no
    /// authoritative write, so a refused second open has mutated nothing.
    fn acquire(path: &Path) -> Result<Self, StoreError> {
        let key = path.canonicalize().map_err(|err| {
            StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("cannot resolve {}: {err}", path.display())),
            ))
        })?;

        let mut open = open_stores()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !open.insert(key.clone()) {
            return Err(StoreError::AlreadyOpen { path: key });
        }
        Ok(Self(key))
    }
}

impl Drop for PathClaim {
    fn drop(&mut self) {
        let mut open = open_stores()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        open.remove(&self.0);
    }
}

/// Distinguishes stores so the re-entrancy guard can tell "re-entering the
/// store I already hold" from "using a second, unrelated store".
static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The stores this thread is currently inside [`Store::write`] for.
    ///
    /// `std::sync::Mutex` is not reentrant, so re-entering the *same*
    /// store's writer from inside its own transaction would deadlock the
    /// control plane. This turns that into a typed error. It records
    /// identities rather than a single flag so that holding a transaction
    /// on one store while writing to a *different* one — which shares no
    /// lock and cannot deadlock — stays legal.
    static WRITES_HELD: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

#[must_use = "dropping the guard immediately would disable re-entrancy protection"]
struct WriteReentrancyGuard(u64);

impl WriteReentrancyGuard {
    fn enter(store_id: u64) -> Result<Self, StoreError> {
        WRITES_HELD.with(|held| {
            let mut held = held.borrow_mut();
            if held.contains(&store_id) {
                return Err(StoreError::ConnectionUnavailable(
                    "cannot re-enter the serialized authoritative writer of a store \
                     from inside that store's own authoritative transaction"
                        .to_string(),
                ));
            }
            held.push(store_id);
            Ok(Self(store_id))
        })
    }
}

impl Drop for WriteReentrancyGuard {
    fn drop(&mut self) {
        // `try_with`: during thread-local destruction the registry may
        // already be gone, and a failure to unwind-clean is not worth a
        // panic inside `Drop`.
        let _ = WRITES_HELD.try_with(|held| {
            let mut held = held.borrow_mut();
            if let Some(index) = held.iter().rposition(|id| *id == self.0) {
                held.remove(index);
            }
        });
    }
}

/// Pantheon's authoritative local SQLite store.
///
/// Owns the connection topology the canonical contract's "SQLite operating
/// rules" require: one serialized authoritative writer connection, plus
/// read access that is separated from it. Every authoritative mutation goes
/// through [`Store::write`], which is the only place a transaction is
/// opened and the only way to obtain a [`Writer`]; the connections
/// themselves are never handed out. Reads use a connection SQLite itself
/// treats as read-only, so the read path cannot mutate control-plane state
/// even by mistake.
///
/// The contract also names a "small bounded read pool". Pantheon has no
/// concurrent readers to pool yet, and both this mission and the one that
/// created the store exclude a speculative connection-pool abstraction, so
/// read access is one read-only connection today. Widening it is a later
/// mission's work and does not change this boundary.
#[derive(Debug)]
pub struct Store {
    /// The single authoritative writer. `rusqlite::Connection` is `Send`
    /// but not `Sync` — it is opened `SQLITE_OPEN_NO_MUTEX`, so SQLite's
    /// own handle mutex is off — and the workspace denies `unsafe_code`.
    /// The `Mutex` is therefore both what makes `Store` shareable across
    /// threads and what serializes write authority.
    writer: Mutex<Connection>,
    /// Read access, physically incapable of writing.
    reader: Mutex<Connection>,
    /// This process's exclusive claim on the database file. Released on
    /// drop, so `Store` itself needs no `Drop` and can still be
    /// destructured by [`Store::close`].
    claim: PathClaim,
    /// Identifies this store to the re-entrancy guard.
    id: u64,
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
        let path = path.as_ref();
        let mut writer = Connection::open_with_flags(path, WRITER_FLAGS)?;
        // Claimed as soon as the file exists — which opening guarantees —
        // and before any policy, migration or statement runs, so a refused
        // second open closes its connection having written nothing.
        let claim = PathClaim::acquire(path)?;
        policy::apply_and_verify_writer(&writer)?;
        migrations::run(&mut writer)?;

        // Opened after the writer, deliberately. A read-only connection
        // cannot create the `-wal`/`-shm` files a WAL database needs, so
        // the writer must have established them first.
        let reader = Connection::open_with_flags(path, READER_FLAGS)?;
        policy::apply_and_verify_reader(&reader)?;

        Ok(Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            claim,
            id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
        })
    }

    /// Runs `f` inside the one serialized authoritative write transaction.
    ///
    /// The transaction is opened `BEGIN IMMEDIATE`, so it holds write
    /// authority before `f` reads anything and no other writer can
    /// invalidate a decision `f` makes. It commits only if `f` returns
    /// `Ok`; on any error, including one raised part-way through, every
    /// write the transaction performed is rolled back.
    ///
    /// Write authority is serialized for the process: concurrent callers
    /// queue on the one authoritative writer rather than proceeding on
    /// independent connections, and [`Store::open`] refuses a second store
    /// for the same database file so no second writer connection exists to
    /// proceed on. Keeping other *processes* off the database is the
    /// daemon's operating-system installation lock, not this boundary.
    ///
    /// A panic inside `f` rolls the transaction back, but poisons the
    /// writer: every later `write` on this store returns
    /// [`StoreError::ConnectionUnavailable`]. That is deliberate — the
    /// rollback's own result was discarded while unwinding, so the
    /// connection is not known to be clean — but it is permanent for the
    /// life of the store.
    ///
    /// # Errors
    ///
    /// Returns whatever `f` returns on failure, or a [`StoreError`] if the
    /// transaction could not be opened, committed, or rolled back. Returns
    /// [`StoreError::ConnectionUnavailable`] if the writer was poisoned by
    /// an earlier panic, or if called from inside another `write`.
    pub fn write<T>(
        &self,
        f: impl FnOnce(&Writer<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let _guard = WriteReentrancyGuard::enter(self.id)?;
        let mut conn = self.writer.lock().map_err(|_| {
            // Never recovered with `into_inner`: the panicking transaction's
            // rollback error was discarded while unwinding, so the
            // connection's state is not known to be clean.
            StoreError::ConnectionUnavailable(
                "the authoritative writer is poisoned by an earlier panic".to_string(),
            )
        })?;
        transaction::run(&mut conn, f)
    }

    /// Runs `mutation` under durable command identity.
    ///
    /// The whole envelope — the epoch fence, the command-ledger decision,
    /// `mutation` itself, the journal sequence allocation, the Event append
    /// and the durable command outcome — runs inside one call to
    /// [`Store::write`], and therefore inside one authoritative
    /// `BEGIN IMMEDIATE` transaction. They commit together or not at all.
    ///
    /// `mutation` runs only when the command is genuinely new for its epoch.
    /// A replay does not invoke it, and neither does a conflict or a stale
    /// epoch.
    ///
    /// # Errors
    ///
    /// - [`StoreError::StaleCommandEpoch`] when `command.epoch` is not the
    ///   installation's current RestoreGeneration. Decided before the
    ///   command ledger is consulted, so a missing row can never make a
    ///   stale request look new.
    /// - [`StoreError::CommandConflict`] when this identity was already used
    ///   in this epoch with a different request hash.
    /// - whatever `mutation` returns on failure, with the transaction rolled
    ///   back in full.
    pub fn execute_command<T>(
        &self,
        command: &Command<'_>,
        mutation: impl FnOnce(&Writer<'_>) -> Result<T, StoreError>,
    ) -> Result<Committed<T>, StoreError> {
        self.write(|writer| command::execute(writer, command, mutation))
    }

    /// Returns the current revision of a revisioned authoritative row, or
    /// `None` if no such row exists.
    ///
    /// This is the read half of an optimistic update: observe a revision
    /// here, then supply it to [`Writer::update_revisioned`], which
    /// revalidates it inside the authoritative transaction. Observing a
    /// revision outside a transaction is safe precisely because it is
    /// never trusted outside one.
    ///
    /// This reads the separate read-only connection, so calling it from
    /// inside [`Store::write`] returns the state as of *before* that
    /// transaction began — it will not show writes the open transaction has
    /// already made. Use [`Writer::revision_of`] for a decision that
    /// depends on the transaction's own snapshot.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] if `table` is not a plain SQL
    /// identifier, or [`StoreError::Sqlite`] if the query fails.
    pub fn revision_of(
        &self,
        table: &'static str,
        id: &str,
    ) -> Result<Option<Revision>, StoreError> {
        transaction::ensure_revisioned_table(table)?;
        let sql = format!("SELECT revision FROM {table} WHERE id = ?1");
        self.read(|conn| {
            conn.query_row(&sql, rusqlite::params![id], |row| row.get::<_, i64>(0))
                .map(|value| Some(Revision::new(value)))
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StoreError::Sqlite(other)),
                })
        })
    }

    /// Returns the installation's current RestoreGeneration.
    pub fn restore_generation(&self) -> Result<RestoreGeneration, StoreError> {
        self.read(|conn| {
            let value: String = conn.query_row(
                "SELECT restore_generation FROM system_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            Ok(RestoreGeneration(value))
        })
    }

    /// Runs `f` against the read-only connection.
    ///
    /// Separate from the writer's lock, so a read never waits on an
    /// authoritative transaction and can never be the reason one is held
    /// open.
    pub(crate) fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let conn = self.reader.lock().map_err(|_| {
            StoreError::ConnectionUnavailable(
                "the read connection is poisoned by an earlier panic".to_string(),
            )
        })?;
        f(&conn)
    }

    /// Closes the store.
    ///
    /// Returns any error SQLite reports while finalizing outstanding
    /// resources rather than silently discarding it, which a bare `drop`
    /// cannot do.
    pub fn close(self) -> Result<(), StoreError> {
        // Destructured rather than dropped field by field: `claim` releases
        // this process's hold on the path when it drops at the end of this
        // function, whether or not closing succeeded.
        let Self {
            writer,
            reader,
            claim: _claim,
            id: _id,
        } = self;

        let writer = writer.into_inner().map_err(|_| {
            StoreError::ConnectionUnavailable(
                "the authoritative writer is poisoned by an earlier panic".to_string(),
            )
        })?;
        let reader = reader.into_inner().map_err(|_| {
            StoreError::ConnectionUnavailable(
                "the read connection is poisoned by an earlier panic".to_string(),
            )
        })?;

        // Close the reader first: it holds no write authority, and closing
        // the writer last keeps the WAL files alive until every handle is
        // finished with them. Both are always closed — returning early on
        // the reader's error would discard whatever SQLite had to say about
        // finalizing the authoritative handle, which is the error this
        // method exists to surface.
        let reader_result = reader.close().map_err(|(_, err)| StoreError::Sqlite(err));
        let writer_result = writer.close().map_err(|(_, err)| StoreError::Sqlite(err));
        writer_result.and(reader_result)
    }
}

#[cfg(test)]
impl Store {
    /// Runs compiled-in test fixture DDL/DML inside the authoritative
    /// transaction. Test-only: production callers get the revisioned CAS.
    pub(crate) fn read_row_for_test(
        &self,
        sql: &'static str,
        id: &str,
    ) -> Result<Option<(i64, String)>, StoreError> {
        self.read(|conn| {
            conn.query_row(sql, rusqlite::params![id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Sqlite(other)),
            })
        })
    }

    pub(crate) fn read_all_for_test(&self, sql: &'static str) -> Result<Vec<i64>, StoreError> {
        self.read(|conn| {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests;
