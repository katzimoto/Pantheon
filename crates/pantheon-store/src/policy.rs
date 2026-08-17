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

use std::time::Duration;

use rusqlite::Connection;

use crate::error::StoreError;

/// Bounded wait for a conflicting lock before SQLite reports `SQLITE_BUSY`,
/// for the single authoritative connection this crate currently opens.
const BUSY_TIMEOUT: Duration = Duration::from_millis(5_000);

/// Declares Pantheon's SQLite database format via `PRAGMA application_id`,
/// per the contract's "Migrations / backup" section. Value is the ASCII
/// bytes of "PANT" read as a big-endian `i32`.
const APPLICATION_ID: i32 = 0x5041_4e54;

/// Applies Pantheon's v1 authoritative SQLite operating policy to `conn`
/// and reads each setting back to confirm SQLite actually accepted it.
pub(crate) fn apply_and_verify(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let journal_mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    require(journal_mode.eq_ignore_ascii_case("wal"), || {
        format!("journal_mode is {journal_mode}, not wal")
    })?;

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

    conn.pragma_update(None, "application_id", APPLICATION_ID)?;
    let application_id: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    require(application_id == i64::from(APPLICATION_ID), || {
        format!("application_id is {application_id}, not {APPLICATION_ID}")
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

        apply_and_verify(&conn).expect("policy applies and verifies");

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
}
