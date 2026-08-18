use super::*;
use crate::store::Store;
use crate::test_support::TempDir;

/// The revisioned fixture the mission's evidence uses. It is created by
/// tests, never by `migrations::MIGRATIONS`, so no production database
/// can grow it.
const FIXTURE_DDL: &str = "CREATE TABLE cas_fixture (
        id       TEXT    PRIMARY KEY,
        revision INTEGER NOT NULL CHECK (revision > 0),
        value    TEXT    NOT NULL
    ) STRICT;
    INSERT INTO cas_fixture (id, revision, value) VALUES
        ('row-a', 7, 'original'),
        ('row-b', 7, 'sibling');";

fn store_with_fixture(label: &str) -> (TempDir, Store) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    store
        .write(|w| {
            w.execute_batch_for_test(FIXTURE_DDL)?;
            Ok(())
        })
        .expect("create fixture");
    (dir, store)
}

fn row(store: &Store, id: &str) -> Option<(i64, String)> {
    store
        .read_row_for_test("SELECT revision, value FROM cas_fixture WHERE id = ?1", id)
        .expect("read fixture row")
}

#[test]
fn a_successful_revisioned_write_increments_the_revision_exactly_once() {
    let (_dir, store) = store_with_fixture("cas-success");

    let new_revision = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture",
                "row-a",
                Revision::new(7),
                &[("value", Value::from("updated"))],
            )
        })
        .expect("the mutation applies");

    assert_eq!(new_revision, Revision::new(8));
    // Read back independently rather than trusting the returned value.
    assert_eq!(row(&store, "row-a"), Some((8, "updated".to_string())));
    // A missing `WHERE id = ?` would have moved this row too.
    assert_eq!(row(&store, "row-b"), Some((7, "sibling".to_string())));
}

#[test]
fn a_stale_expected_revision_is_a_typed_conflict_and_mutates_nothing() {
    let (_dir, store) = store_with_fixture("cas-stale");
    let first = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture",
                "row-a",
                Revision::new(7),
                &[("value", Value::from("first"))],
            )
        })
        .expect("first mutation applies");
    assert_eq!(first, Revision::new(8));

    let err = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture",
                "row-a",
                Revision::new(7),
                &[("value", Value::from("second"))],
            )
        })
        .expect_err("the stale mutation must fail");

    assert!(
        matches!(
            err,
            StoreError::RevisionConflict {
                table: "cas_fixture",
                expected: 7,
                actual: Some(8),
                ..
            }
        ),
        "unexpected error: {err}"
    );
    assert!(
        !matches!(err, StoreError::Sqlite(_)),
        "a semantic conflict must not be an undifferentiated storage error"
    );
    // Not 9: the failed attempt must not have incremented anything.
    assert_eq!(row(&store, "row-a"), Some((8, "first".to_string())));
}

#[test]
fn a_missing_row_is_a_typed_conflict_not_a_silent_success() {
    let (_dir, store) = store_with_fixture("cas-missing");

    let err = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture",
                "no-such-row",
                Revision::new(7),
                &[("value", Value::from("x"))],
            )
        })
        .expect_err("a missing row must not report success");

    assert!(
        matches!(err, StoreError::RevisionConflict { actual: None, .. }),
        "unexpected error: {err}"
    );
}

#[test]
fn a_mutation_matching_more_than_one_row_fails_closed_and_rolls_back() {
    let dir = TempDir::new("cas-ambiguous");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    store
        .write(|w| {
            // No primary key: deliberately admits two rows sharing an
            // id, so "exactly one affected row" has something to reject.
            w.execute_batch_for_test(
                "CREATE TABLE cas_fixture (
                    id       TEXT    NOT NULL,
                    revision INTEGER NOT NULL,
                    value    TEXT    NOT NULL
                ) STRICT;
                INSERT INTO cas_fixture (id, revision, value) VALUES
                    ('dup', 7, 'one'), ('dup', 7, 'two');",
            )?;
            Ok(())
        })
        .expect("create ambiguous fixture");

    let err = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture",
                "dup",
                Revision::new(7),
                &[("value", Value::from("collapsed"))],
            )
        })
        .expect_err("more than one affected row must fail closed");

    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("affected 2 rows")),
        "unexpected error: {err}"
    );

    let revisions: Vec<i64> = store
        .read_all_for_test("SELECT revision FROM cas_fixture ORDER BY value")
        .expect("read rows");
    assert_eq!(
        revisions,
        vec![7, 7],
        "the rejected mutation must have rolled back"
    );
}

#[test]
fn an_injected_error_after_intermediate_writes_leaves_no_partial_state() {
    let (_dir, store) = store_with_fixture("rollback");

    let err = store
        .write(|w| {
            // Two real, distinct writes land before the failure, so the
            // test proves *every* write in the transaction rolls back.
            let intermediate = w.update_revisioned(
                "cas_fixture",
                "row-a",
                Revision::new(7),
                &[("value", Value::from("intermediate"))],
            )?;
            // The write really happened inside the transaction; what
            // follows proves it did not survive the failure.
            assert_eq!(intermediate, Revision::new(8));
            w.execute(
                "INSERT INTO cas_fixture (id, revision, value) VALUES ('row-c', 1, 'new')",
                &[],
            )?;
            Err::<(), StoreError>(StoreError::InvariantViolated("injected".to_string()))
        })
        .expect_err("the injected error must abort the transaction");
    assert!(matches!(err, StoreError::InvariantViolated(ref d) if d == "injected"));

    assert_eq!(
        row(&store, "row-a"),
        Some((7, "original".to_string())),
        "the intermediate CAS must have rolled back, revision included"
    );
    assert_eq!(row(&store, "row-c"), None, "the inserted row must be gone");

    // The store is still usable: a left-open transaction would make the
    // next BEGIN IMMEDIATE fail.
    let after = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture",
                "row-a",
                Revision::new(7),
                &[("value", Value::from("after"))],
            )
        })
        .expect("the store remains usable after a rolled-back transaction");
    assert_eq!(after, Revision::new(8));
}

#[test]
fn a_sql_failure_mid_transaction_also_rolls_back_every_write() {
    let (_dir, store) = store_with_fixture("rollback-sql");

    let err = store
        .write(|w| {
            let intermediate = w.update_revisioned(
                "cas_fixture",
                "row-a",
                Revision::new(7),
                &[("value", Value::from("intermediate"))],
            )?;
            // The write really happened inside the transaction; what
            // follows proves it did not survive the failure.
            assert_eq!(intermediate, Revision::new(8));
            // A primary-key collision: this fails while stepping, with
            // real work already done in the transaction.
            w.execute(
                "INSERT INTO cas_fixture (id, revision, value) VALUES ('row-b', 1, 'clash')",
                &[],
            )?;
            Ok(())
        })
        .expect_err("the constraint violation must abort the transaction");
    assert!(matches!(err, StoreError::Sqlite(_)), "unexpected: {err}");

    assert_eq!(row(&store, "row-a"), Some((7, "original".to_string())));
    assert_eq!(row(&store, "row-b"), Some((7, "sibling".to_string())));
}

#[test]
fn the_authoritative_transaction_has_write_intent_before_any_statement() {
    let (_dir, store) = store_with_fixture("immediate-state");

    // Observed from inside the transaction, before it has run anything.
    // `BEGIN DEFERRED` would report `None` here.
    store
        .write(|w| {
            assert_eq!(w.transaction_state_for_test()?, TransactionState::Write);
            Ok(())
        })
        .expect("transaction opens with write intent");
}

#[test]
fn a_payload_assignment_may_not_overwrite_the_revision_or_identity() {
    let (_dir, store) = store_with_fixture("cas-guard");

    for column in ["revision", "REVISION", "id"] {
        let err = store
            .write(|w| {
                w.update_revisioned(
                    "cas_fixture",
                    "row-a",
                    Revision::new(7),
                    &[(column, Value::Integer(99))],
                )
            })
            .expect_err("assigning the revision or identity column must be rejected");
        assert!(
            matches!(err, StoreError::InvariantViolated(_)),
            "unexpected error for {column}: {err}"
        );
    }

    assert_eq!(row(&store, "row-a"), Some((7, "original".to_string())));
}

#[test]
fn a_table_name_that_is_not_an_identifier_is_rejected() {
    let (_dir, store) = store_with_fixture("cas-identifier");

    let err = store
        .write(|w| {
            w.update_revisioned(
                "cas_fixture; DROP TABLE system_state",
                "row-a",
                Revision::new(7),
                &[("value", Value::from("x"))],
            )
        })
        .expect_err("a non-identifier table name must be rejected");
    assert!(matches!(err, StoreError::InvariantViolated(_)));
}

#[test]
fn a_discarded_mutation_error_still_rolls_the_transaction_back() {
    let dir = TempDir::new("swallowed");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    store
        .write(|w| {
            w.execute_batch_for_test(
                "CREATE TABLE cas_fixture (
                    id TEXT NOT NULL, revision INTEGER NOT NULL, value TEXT NOT NULL
                ) STRICT;
                INSERT INTO cas_fixture VALUES ('dup', 7, 'one'), ('dup', 7, 'two');",
            )?;
            Ok(())
        })
        .expect("create ambiguous fixture");

    // The closure ignores the failed mutation and reports success. By
    // this point SQLite has already applied the offending UPDATE to two
    // rows, so honouring the `Ok` would commit exactly the cardinality
    // violation the store exists to reject.
    let err = store
        .write(|w| {
            let _ = w.update_revisioned(
                "cas_fixture",
                "dup",
                Revision::new(7),
                &[("value", Value::from("clobbered"))],
            );
            Ok(())
        })
        .expect_err("a discarded mutation error must not commit");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("discarded")),
        "unexpected error: {err}"
    );

    let revisions = store
        .read_all_for_test("SELECT revision FROM cas_fixture ORDER BY value")
        .expect("read rows");
    assert_eq!(
        revisions,
        vec![7, 7],
        "neither row may survive the rolled-back multi-row update"
    );
}

#[test]
fn a_panic_inside_the_transaction_leaves_no_partial_state() {
    let dir = TempDir::new("panic-rollback");
    let path = dir.path().join("pantheon.db");
    {
        let store = Store::open(&path).expect("open store");
        store
            .write(|w| {
                w.execute_batch_for_test(
                    "CREATE TABLE cas_fixture (
                        id TEXT PRIMARY KEY, revision INTEGER NOT NULL, value TEXT NOT NULL
                    ) STRICT;
                    INSERT INTO cas_fixture VALUES ('row-a', 7, 'original');",
                )?;
                Ok(())
            })
            .expect("fixture");

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.write(|w| {
                let doomed = w.update_revisioned(
                    "cas_fixture",
                    "row-a",
                    Revision::new(7),
                    &[("value", Value::from("doomed"))],
                )?;
                assert_eq!(doomed, Revision::new(8));
                panic!("injected panic after a real write");
                #[allow(unreachable_code)]
                Ok::<(), StoreError>(())
            })
        }));
        assert!(panicked.is_err(), "the closure must have panicked");

        // The writer is now poisoned, permanently and by design: the
        // rollback's own result was discarded while unwinding, so the
        // connection is not known to be clean.
        let err = store
            .write(|_| Ok(()))
            .expect_err("a poisoned writer must fail closed");
        assert!(matches!(err, StoreError::ConnectionUnavailable(_)));
    }

    // A fresh store proves the panicking transaction committed nothing.
    let reopened = Store::open(&path).expect("reopen after panic");
    let survivor = reopened
        .read_row_for_test(
            "SELECT revision, value FROM cas_fixture WHERE id = ?1",
            "row-a",
        )
        .expect("read after reopen");
    assert_eq!(survivor, Some((7, "original".to_string())));
}
