//! The serialized authoritative write boundary and its revision/CAS
//! primitive.
//!
//! `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
//! requires "one serialized authoritative writer connection", `BEGIN
//! IMMEDIATE` for state-dependent authoritative write transactions, and
//! revisioned mutable rows updated through
//!
//! ```sql
//! ... WHERE id = ? AND revision = ?
//! ```
//!
//! incrementing the revision, with exactly one affected row required.
//!
//! This module is where those three rules become mechanical rather than
//! remembered. [`Writer`] is the only handle from which an authoritative
//! mutation can be issued, it exists only inside a transaction the store
//! opened with write intent, and [`Writer::update_revisioned`] is the only
//! way to write a revisioned row.
//!
//! # Why `Writer` is a newtype and not a `rusqlite::Transaction`
//!
//! `rusqlite::Transaction` dereferences to the whole `Connection`. Handing
//! one to caller code would let that code run `COMMIT` mid-closure — after
//! which the remaining statements execute in autocommit, cannot be rolled
//! back, and the store's own cleanup silently does nothing because
//! rusqlite short-circuits when the connection is already in autocommit.
//! `ATTACH`, `PRAGMA` and nested `BEGIN` are reachable the same way.
//! `Writer` wraps the transaction without `Deref` and exposes only the two
//! operations this mission defines, which closes all of those at once.

use std::cell::Cell;

use rusqlite::{Connection, Transaction, TransactionState};

use crate::error::StoreError;

/// The revision of a mutable authoritative row.
///
/// Revisions are `INTEGER` in SQLite's STRICT tables, hence `i64`. A
/// revision is only ever meaningful for the row it was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[must_use]
pub struct Revision(i64);

impl Revision {
    /// Wraps a revision value read from an authoritative row.
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// The underlying integer.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl std::fmt::Display for Revision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A value bound into an authoritative statement.
///
/// Pantheon defines its own value type rather than re-exporting
/// `rusqlite`'s so that the database driver stays a private implementation
/// detail of this crate. Every other crate names `pantheon-store`'s API,
/// not SQLite's, so a driver upgrade is not a workspace-wide breaking
/// change.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

fn bind(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::Integer(v) => rusqlite::types::Value::Integer(*v),
        Value::Real(v) => rusqlite::types::Value::Real(*v),
        Value::Text(v) => rusqlite::types::Value::Text(v.clone()),
        Value::Blob(v) => rusqlite::types::Value::Blob(v.clone()),
    }
}

/// The column carrying a revisioned row's identity, per the canonical
/// contract's `WHERE id = ? AND revision = ?` shape.
const ID_COLUMN: &str = "id";
/// The column carrying a revisioned row's revision.
const REVISION_COLUMN: &str = "revision";

/// The handle through which authoritative mutations are issued, valid only
/// for the duration of one [`crate::Store::write`] transaction.
#[derive(Debug)]
pub struct Writer<'a> {
    tx: &'a Transaction<'a>,
    /// Set when a mutation failed after SQLite had already applied part of
    /// it, or when a mutation's outcome must not be committed.
    ///
    /// Returning an error is not enough on its own: a closure that ignores
    /// the error and returns `Ok` would otherwise commit a transaction in
    /// which an `UPDATE` had already touched the wrong number of rows. The
    /// commit decision therefore does not rest on caller discipline —
    /// [`run`] refuses to commit once this is set.
    failed: Cell<bool>,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(tx: &'a Transaction<'a>) -> Self {
        Self {
            tx,
            failed: Cell::new(false),
        }
    }

    pub(crate) fn fail<T>(&self, err: StoreError) -> Result<T, StoreError> {
        self.failed.set(true);
        Err(err)
    }

    pub(crate) fn has_failed(&self) -> bool {
        self.failed.get()
    }

    /// The current revision of a revisioned row, read inside this
    /// authoritative transaction.
    ///
    /// Use this, not [`crate::Store::revision_of`], when a decision made
    /// inside the transaction depends on stored state: this reads the
    /// transaction's own snapshot and therefore sees writes this
    /// transaction has already performed, while `Store::revision_of` reads
    /// the separate read-only connection and does not.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvariantViolated`] if `table` is not a plain SQL
    /// identifier; [`StoreError::Sqlite`] if the query fails.
    pub fn revision_of(
        &self,
        table: &'static str,
        id: &str,
    ) -> Result<Option<Revision>, StoreError> {
        if let Err(err) = ensure_plain_identifier("table", table) {
            return self.fail(err);
        }
        Ok(self.current_revision(table, id)?.map(Revision::new))
    }

    /// Mutates exactly the row of `table` identified by `id`, and only if
    /// its current revision is `expected`. Returns the row's new revision.
    ///
    /// The revision comparison and the mutation are one statement inside
    /// the caller's authoritative transaction, so there is no window
    /// between validating the revision and acting on it. The revision is
    /// incremented by that same statement, so it advances exactly once per
    /// successful mutation and cannot advance without the mutation.
    ///
    /// # Errors
    ///
    /// - [`StoreError::RevisionConflict`] when no row matches: either the
    ///   row is at a different revision (`actual: Some(_)`) or it does not
    ///   exist (`actual: None`). This is an expected concurrent outcome.
    ///   Returning it as an error rather than a value is deliberate: the
    ///   caller's `?` aborts the closure, so a stale mutation can never
    ///   commit alongside the other writes that closure already performed.
    /// - [`StoreError::InvariantViolated`] when more than one row matched,
    ///   or when a supplied identifier is not a plain identifier or would
    ///   overwrite the identity or revision column. The transaction is
    ///   rolled back.
    pub fn update_revisioned(
        &self,
        table: &'static str,
        id: &str,
        expected: Revision,
        assignments: &[(&'static str, Value)],
    ) -> Result<Revision, StoreError> {
        if let Err(err) = ensure_plain_identifier("table", table) {
            return self.fail(err);
        }
        for (column, _) in assignments {
            if let Err(err) = ensure_plain_identifier("column", column) {
                return self.fail(err);
            }
            // SQLite accepts an UPDATE that assigns the same column twice,
            // so a payload assignment naming `revision` or `id` would
            // silently compete with the CAS predicate and the increment
            // this statement exists to guarantee. Reject it instead.
            if column.eq_ignore_ascii_case(REVISION_COLUMN)
                || column.eq_ignore_ascii_case(ID_COLUMN)
            {
                return self.fail(StoreError::InvariantViolated(format!(
                    "a revisioned update may not assign the {column} column of {table}"
                )));
            }
        }

        // Parameters are positional: ?1 is the id, ?2 the expected
        // revision, and the payload values follow from ?3.
        let mut set_clauses = Vec::with_capacity(assignments.len() + 1);
        for (index, (column, _)) in assignments.iter().enumerate() {
            set_clauses.push(format!("{column} = ?{}", index + 3));
        }
        set_clauses.push(format!("{REVISION_COLUMN} = {REVISION_COLUMN} + 1"));

        let sql = format!(
            "UPDATE {table} SET {} WHERE {ID_COLUMN} = ?1 AND {REVISION_COLUMN} = ?2",
            set_clauses.join(", ")
        );

        let mut bound = Vec::with_capacity(assignments.len() + 2);
        bound.push(rusqlite::types::Value::Text(id.to_string()));
        bound.push(rusqlite::types::Value::Integer(expected.get()));
        bound.extend(assignments.iter().map(|(_, value)| bind(value)));

        // Only the count returned by this very statement is trustworthy.
        // `Connection::changes()` reports the most recently *completed*
        // statement on the connection, so reading it separately can return
        // a count inherited from an earlier write in the same transaction.
        let affected = match self.tx.execute(&sql, rusqlite::params_from_iter(bound)) {
            Ok(affected) => affected,
            Err(err) => return self.fail(StoreError::Sqlite(err)),
        };

        match affected {
            1 => Ok(Revision::new(expected.get() + 1)),
            0 => {
                // Safe to look up inside the transaction: this transaction
                // already holds write authority, so the answer cannot
                // change under us and no check-then-write window opens.
                let actual = self.current_revision(table, id)?;
                self.fail(StoreError::RevisionConflict {
                    table,
                    id: id.to_string(),
                    expected: expected.get(),
                    actual,
                })
            }
            more => self.fail(StoreError::InvariantViolated(format!(
                "revisioned update of {table} row {id} affected {more} rows, not exactly one"
            ))),
        }
    }

    /// Runs a compiled-in statement inside the same authoritative
    /// transaction, returning the number of rows it affected.
    ///
    /// Crate-private on purpose. Issue #17 defines exactly one *public*
    /// authoritative mutation — the revisioned CAS above. A public
    /// arbitrary-SQL escape hatch would reopen the writer boundary this
    /// module exists to close. The command kernel in [`crate::command`] is
    /// inside that boundary and reaches the transaction only through this
    /// and [`Writer::query_optional`], never through a `Connection`.
    ///
    /// A failure marks the transaction uncommittable, so a kernel statement
    /// whose error is discarded still cannot be committed.
    pub(crate) fn execute(&self, sql: &'static str, params: &[Value]) -> Result<usize, StoreError> {
        let bound: Vec<rusqlite::types::Value> = params.iter().map(bind).collect();
        match self.tx.execute(sql, rusqlite::params_from_iter(bound)) {
            Ok(affected) => Ok(affected),
            Err(err) => self.fail(StoreError::Sqlite(err)),
        }
    }

    /// Reads at most one row inside the same authoritative transaction.
    ///
    /// This exists so the command kernel can consult `system_state` and the
    /// command ledger on the *transaction's* snapshot. Reading either through
    /// [`crate::Store`]'s read-only connection would consult the
    /// pre-transaction snapshot and put the epoch fence outside the
    /// transaction that acts on it.
    pub(crate) fn query_optional<T>(
        &self,
        sql: &'static str,
        params: &[Value],
        map: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>, StoreError> {
        let bound: Vec<rusqlite::types::Value> = params.iter().map(bind).collect();
        match self
            .tx
            .query_row(sql, rusqlite::params_from_iter(bound), map)
        {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => self.fail(StoreError::Sqlite(err)),
        }
    }

    fn current_revision(&self, table: &str, id: &str) -> Result<Option<i64>, StoreError> {
        let sql = format!("SELECT {REVISION_COLUMN} FROM {table} WHERE {ID_COLUMN} = ?1");
        self.tx
            .query_row(&sql, rusqlite::params![id], |row| row.get::<_, i64>(0))
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Sqlite(other)),
            })
    }
}

#[cfg(test)]
impl Writer<'_> {
    /// Creates and seeds test fixture schema inside the authoritative
    /// transaction. Test-only, so no fixture DDL path exists in a release
    /// build and none of it can reach `migrations::MIGRATIONS`.
    pub(crate) fn execute_batch_for_test(&self, sql: &'static str) -> Result<(), StoreError> {
        self.tx.execute_batch(sql).map_err(StoreError::Sqlite)
    }

    /// The transaction's SQLite transaction state, for proving write intent.
    pub(crate) fn transaction_state_for_test(&self) -> Result<TransactionState, StoreError> {
        self.tx
            .transaction_state(None::<&str>)
            .map_err(StoreError::Sqlite)
    }
}

/// Rejects anything that is not a bare SQL identifier.
///
/// SQLite cannot bind a table or column name as a parameter, so these are
/// interpolated into the statement text. Requiring `&'static str` at the
/// call site already means an identifier is compiled in rather than
/// caller-supplied data; this check is the second layer, so a future
/// controller cannot reach an injection through a leaked string.
fn ensure_plain_identifier(kind: &str, name: &str) -> Result<(), StoreError> {
    let valid = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvariantViolated(format!(
            "{kind} name {name:?} is not a plain SQL identifier"
        )))
    }
}

/// Rejects a table name that is not a plain SQL identifier, for the read
/// half of an optimistic update.
pub(crate) fn ensure_revisioned_table(table: &str) -> Result<(), StoreError> {
    ensure_plain_identifier("table", table)
}

/// Opens the authoritative transaction, runs `f` inside it, and commits
/// only if `f` succeeded.
///
/// The transaction is always `BEGIN IMMEDIATE`: write authority is
/// acquired before any state is read, so a decision made inside the
/// transaction cannot be invalidated by another writer before it commits.
/// The mode is asserted rather than assumed, because a deferred
/// transaction behaves identically in a single-writer process and the
/// mistake would otherwise survive indefinitely.
pub(crate) fn run<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Writer<'_>) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    // A previous transaction whose rollback failed would leave the
    // connection mid-transaction. rusqlite discards the error from a
    // rollback performed while unwinding, so this is where that condition
    // is detected rather than surfacing later as a confusing "cannot start
    // a transaction within a transaction".
    if !conn.is_autocommit() {
        return Err(StoreError::InvariantViolated(
            "the authoritative writer connection is already inside a transaction".to_string(),
        ));
    }

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    // `BEGIN IMMEDIATE` acquires the write lock at BEGIN, so the
    // transaction is in the write state before any statement runs;
    // `BEGIN DEFERRED` would report `TransactionState::None` here.
    // `is_autocommit` cannot tell them apart — it is false for both.
    let state = tx.transaction_state(None::<&str>)?;
    if state != TransactionState::Write {
        return Err(StoreError::InvariantViolated(format!(
            "authoritative transaction opened in state {state:?}, not Write; \
             it does not have immediate write intent"
        )));
    }

    // Bound rather than `?`: the closure's result decides commit versus
    // rollback, and a commit failure must reach the caller as a failure
    // instead of being reported as the success the closure returned.
    let writer = Writer::new(&tx);
    let outcome = f(&writer);

    match outcome {
        // A closure that ignored a failed mutation and returned `Ok` must
        // not commit it: by this point SQLite has already applied the
        // offending statement inside this transaction.
        Ok(_) if writer.has_failed() => {
            drop(tx.rollback());
            Err(StoreError::InvariantViolated(
                "an authoritative mutation failed inside this transaction and its error \
                 was discarded; the transaction was rolled back rather than committed"
                    .to_string(),
            ))
        }
        Ok(value) => {
            tx.commit()?;
            Ok(value)
        }
        Err(err) => {
            // The caller's error is the interesting one. A rollback that
            // itself fails leaves the connection mid-transaction, which
            // the autocommit check above catches on the next write.
            drop(tx.rollback());
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests;
