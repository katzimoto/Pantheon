//! The v1 authoritative SQLite connection policy.
//!
//! `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
//! ("SQLite operating rules") requires every authoritative connection to
//! apply WAL mode, `synchronous=FULL`, `foreign_keys=ON`,
//! `trusted_schema=OFF`, a configured bounded `busy_timeout`, and no
//! shared-cache mode. The contract also requires *verifying* the policy,
//! not merely issuing `PRAGMA` statements and assuming SQLite accepted
//! them, so every setting here is read back and checked.
//!
//! No-shared-cache is not one of those readable settings: SQLite exposes no
//! per-connection `PRAGMA` that reflects shared-cache status. It is instead
//! enforced structurally by the `OpenFlags` [`crate::store`] passes to
//! `Connection::open_with_flags` (`SQLITE_OPEN_PRIVATE_CACHE`, not
//! `SQLITE_OPEN_SHARED_CACHE`), which a unit test on that module asserts
//! directly.
//!
//! The same rules name "one serialized authoritative writer connection +
//! small bounded read pool", so the policy has two entry points rather than
//! one. They differ only in what a read-only connection is permitted to do:
//! `journal_mode` and `application_id` are properties of the *database
//! file*, so setting them is a write. A read-only handle therefore verifies
//! them by reading them back and never re-applies them.
//!
//! That distinction is not cosmetic. On a read-only connection,
//! `PRAGMA journal_mode = WAL` succeeds and changes nothing when the
//! database is *already* WAL, and fails with `SQLITE_READONLY` when it is
//! not. Pantheon's databases are always already WAL by the time a reader
//! opens, so applying it there would always report success while proving
//! nothing. Only the read-back is evidence. Both halves of that behaviour
//! are pinned by a test below, because a future edit that collapsed the two
//! entry points into one would otherwise still pass every other test.

use std::time::Duration;

use rusqlite::Connection;

use crate::error::StoreError;

/// Bounded wait for a conflicting lock before SQLite reports `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_millis(5_000);

/// Declares Pantheon's SQLite database format via `PRAGMA application_id`,
/// per the contract's "Migrations / backup" section. Value is the ASCII
/// bytes of "PANT" read as a big-endian `i32`.
const APPLICATION_ID: i32 = 0x5041_4e54;

/// Applies Pantheon's v1 authoritative SQLite operating policy to the
/// serialized authoritative writer connection and reads each setting back
/// to confirm SQLite actually accepted it.
pub(crate) fn apply_and_verify_writer(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let journal_mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    require(journal_mode.eq_ignore_ascii_case("wal"), || {
        format!("journal_mode is {journal_mode}, not wal")
    })?;

    apply_and_verify_connection_settings(conn)?;

    conn.pragma_update(None, "application_id", APPLICATION_ID)?;
    let application_id: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    require(application_id == i64::from(APPLICATION_ID), || {
        format!("application_id is {application_id}, not {APPLICATION_ID}")
    })?;

    Ok(())
}

/// Applies the connection-scoped part of the same policy to a read-only
/// connection, verifies the database-scoped part by reading it back, and
/// confirms SQLite itself considers the connection read-only.
///
/// The final check is what makes read/write separation a structural fact
/// rather than a naming convention: it asks SQLite, not Pantheon, whether
/// this handle can mutate the database.
pub(crate) fn apply_and_verify_reader(conn: &Connection) -> Result<(), StoreError> {
    let journal_mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    require(journal_mode.eq_ignore_ascii_case("wal"), || {
        format!("journal_mode is {journal_mode}, not wal")
    })?;

    apply_and_verify_connection_settings(conn)?;

    let application_id: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    require(application_id == i64::from(APPLICATION_ID), || {
        format!("application_id is {application_id}, not {APPLICATION_ID}")
    })?;

    let read_only = conn.is_readonly(rusqlite::MAIN_DB)?;
    require(read_only, || {
        "the read connection is not read-only at the SQLite level".to_string()
    })?;

    Ok(())
}

/// The settings that are scoped to a connection rather than to the database
/// file, and so apply identically to the writer and to a read-only handle.
fn apply_and_verify_connection_settings(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "synchronous", "FULL")?;
    let synchronous: i64 = conn.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    require(synchronous == 2, || {
        format!("synchronous is {synchronous}, not FULL (2)")
    })?;

    conn.pragma_update(None, "foreign_keys", "ON")?;
    let foreign_keys: i64 = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    require(foreign_keys == 1, || {
        format!("foreign_keys is {foreign_keys}, not ON")
    })?;

    conn.pragma_update(None, "trusted_schema", "OFF")?;
    let trusted_schema: i64 = conn.pragma_query_value(None, "trusted_schema", |row| row.get(0))?;
    require(trusted_schema == 0, || {
        format!("trusted_schema is {trusted_schema}, not OFF")
    })?;

    conn.busy_timeout(BUSY_TIMEOUT)?;
    let busy_timeout: i64 = conn.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
    let expected_busy_timeout = i64::try_from(BUSY_TIMEOUT.as_millis()).unwrap_or(i64::MAX);
    require(busy_timeout == expected_busy_timeout, || {
        format!("busy_timeout is {busy_timeout}ms, not {expected_busy_timeout}ms")
    })?;

    Ok(())
}

fn require(condition: bool, detail: impl FnOnce() -> String) -> Result<(), StoreError> {
    if condition {
        Ok(())
    } else {
        Err(StoreError::PolicyVerificationFailed(detail()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_and_verifies_the_v1_policy() {
        let dir = crate::test_support::TempDir::new("policy-apply");
        let conn = Connection::open(dir.path().join("pantheon.db")).expect("open connection");

        apply_and_verify_writer(&conn).expect("policy applies and verifies");

        let journal_mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert!(journal_mode.eq_ignore_ascii_case("wal"));

        let synchronous: i64 = conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        assert_eq!(synchronous, 2);

        let foreign_keys: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);

        let trusted_schema: i64 = conn
            .pragma_query_value(None, "trusted_schema", |row| row.get(0))
            .unwrap();
        assert_eq!(trusted_schema, 0);

        let busy_timeout: i64 = conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(
            busy_timeout,
            i64::try_from(BUSY_TIMEOUT.as_millis()).unwrap()
        );

        let application_id: i64 = conn
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        assert_eq!(application_id, i64::from(APPLICATION_ID));
    }

    #[test]
    fn a_read_only_connection_verifies_the_policy_the_writer_established() {
        let dir = crate::test_support::TempDir::new("policy-reader");
        let path = dir.path().join("pantheon.db");
        let writer = Connection::open(&path).expect("open writer");
        apply_and_verify_writer(&writer).expect("writer policy applies");

        let reader = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                .union(rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .union(rusqlite::OpenFlags::SQLITE_OPEN_PRIVATE_CACHE),
        )
        .expect("open reader");

        apply_and_verify_reader(&reader).expect("reader policy verifies");
        assert!(reader.is_readonly(rusqlite::MAIN_DB).unwrap());
    }

    #[test]
    fn the_reader_policy_rejects_a_read_write_connection() {
        let dir = crate::test_support::TempDir::new("policy-reader-rejects");
        let path = dir.path().join("pantheon.db");
        let writer = Connection::open(&path).expect("open writer");
        apply_and_verify_writer(&writer).expect("writer policy applies");

        // The same database, opened read-write. Every database-scoped
        // setting the reader policy verifies is already correct, so the
        // only thing that can reject this connection is the read-only
        // check itself.
        let not_a_reader = Connection::open(&path).expect("open second read-write connection");
        let err = apply_and_verify_reader(&not_a_reader)
            .expect_err("a read-write connection must not pass the reader policy");
        assert!(
            matches!(err, StoreError::PolicyVerificationFailed(ref detail) if detail.contains("read-only")),
            "unexpected error: {err}"
        );
    }

    /// Pins the SQLite behaviour `apply_and_verify_reader` is built around:
    /// on a read-only connection, setting `journal_mode` can never change
    /// the database, so a successful `pragma_update` there is not evidence
    /// of anything. If the reader policy were ever collapsed into the
    /// writer policy for tidiness, the WAL read-back would still say "wal"
    /// on an already-WAL database and every other test would still pass —
    /// this is what makes that regression visible.
    #[test]
    fn a_read_only_connection_can_never_set_the_journal_mode() {
        let read_only = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            .union(rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .union(rusqlite::OpenFlags::SQLITE_OPEN_PRIVATE_CACHE);

        // Case 1: the mode already matches. SQLite reports success and
        // nothing happened — so success carries no information. This is
        // exactly Pantheon's situation, since the writer sets WAL first.
        let wal_dir = crate::test_support::TempDir::new("policy-noop-wal");
        let wal_path = wal_dir.path().join("pantheon.db");
        {
            let writer = Connection::open(&wal_path).expect("open writer");
            apply_and_verify_writer(&writer).expect("writer policy applies");
        }
        let wal_reader = Connection::open_with_flags(&wal_path, read_only).expect("open reader");
        wal_reader
            .pragma_update(None, "journal_mode", "WAL")
            .expect("setting WAL on an already-WAL database reports success");
        let journal_mode: String = wal_reader
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert!(journal_mode.eq_ignore_ascii_case("wal"));

        // Case 2: the mode differs. SQLite refuses outright, proving the
        // read-only handle genuinely cannot rewrite the database's own
        // properties.
        let delete_dir = crate::test_support::TempDir::new("policy-noop-delete");
        let delete_path = delete_dir.path().join("pantheon.db");
        {
            let writer = Connection::open(&delete_path).expect("open writer");
            writer
                .pragma_update(None, "journal_mode", "DELETE")
                .expect("set delete journal mode");
            writer
                .execute_batch("CREATE TABLE marker (id INTEGER PRIMARY KEY) STRICT;")
                .expect("materialize the database on disk");
        }
        let delete_reader =
            Connection::open_with_flags(&delete_path, read_only).expect("open reader");
        let err = delete_reader
            .pragma_update(None, "journal_mode", "WAL")
            .expect_err("a read-only connection must not be able to change the journal mode");
        assert!(
            matches!(
                err,
                rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ReadOnly
            ),
            "unexpected error: {err}"
        );
        let journal_mode: String = delete_reader
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert!(journal_mode.eq_ignore_ascii_case("delete"));
    }
}
