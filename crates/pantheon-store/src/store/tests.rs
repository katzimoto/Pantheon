use super::*;
use crate::test_support::TempDir;

#[test]
fn open_flags_use_a_private_cache_never_a_shared_one() {
    assert!(WRITER_FLAGS.contains(OpenFlags::SQLITE_OPEN_PRIVATE_CACHE));
    assert!(!WRITER_FLAGS.contains(OpenFlags::SQLITE_OPEN_SHARED_CACHE));
    assert!(READER_FLAGS.contains(OpenFlags::SQLITE_OPEN_PRIVATE_CACHE));
    assert!(!READER_FLAGS.contains(OpenFlags::SQLITE_OPEN_SHARED_CACHE));
}

#[test]
fn the_read_flags_grant_no_write_capability() {
    assert!(READER_FLAGS.contains(OpenFlags::SQLITE_OPEN_READ_ONLY));
    assert!(!READER_FLAGS.contains(OpenFlags::SQLITE_OPEN_READ_WRITE));
    assert!(!READER_FLAGS.contains(OpenFlags::SQLITE_OPEN_CREATE));
}

/// The concurrency evidence this mission requires depends on `Store`
/// crossing thread boundaries; a `Connection` alone cannot.
#[test]
fn store_is_shareable_across_threads() {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
}

#[test]
fn the_read_connection_cannot_mutate_authoritative_state() {
    let dir = crate::test_support::TempDir::new("read-only");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");

    let err = store
        .read(|conn| {
            conn.execute("UPDATE system_state SET restore_generation = 'x'", [])
                .map_err(StoreError::Sqlite)
        })
        .expect_err("the read connection must reject a mutation");

    match err {
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(f, _)) => {
            assert_eq!(f.code, rusqlite::ErrorCode::ReadOnly);
        }
        other => panic!("expected a SQLite read-only failure, got {other}"),
    }

    // And it really did not write.
    let generation = store.restore_generation().expect("read generation");
    assert_ne!(generation.as_str(), "x");
}

#[test]
fn re_entering_the_writer_is_a_typed_error_rather_than_a_deadlock() {
    let dir = crate::test_support::TempDir::new("reentrant");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");

    let err = store
        .write(|_| store.write(|_| Ok(())))
        .expect_err("a nested authoritative write must be rejected");
    assert!(
        matches!(err, StoreError::ConnectionUnavailable(ref d) if d.contains("re-enter")),
        "unexpected error: {err}"
    );

    // The guard released, so the writer is still usable.
    store.write(|_| Ok(())).expect("writer remains usable");
}

#[test]
fn a_read_inside_a_write_does_not_deadlock_but_sees_the_pre_transaction_snapshot() {
    let dir = crate::test_support::TempDir::new("read-in-write");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
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

    store
        .write(|w| {
            let updated = w.update_revisioned(
                "cas_fixture",
                "row-a",
                crate::Revision::new(7),
                &[("value", crate::Value::from("inside"))],
            )?;
            assert_eq!(updated, crate::Revision::new(8));

            // The read path uses a different lock from the writer, so
            // this must not deadlock...
            let through_reader = store.revision_of("cas_fixture", "row-a")?;
            // ...but it is a different connection on a different WAL
            // snapshot, so it cannot see this transaction's own writes.
            // Documented rather than incidental: a controller needing
            // read-your-own-writes must use the writer.
            assert_eq!(through_reader, Some(crate::Revision::new(7)));

            let through_writer = w.revision_of("cas_fixture", "row-a")?;
            assert_eq!(through_writer, Some(crate::Revision::new(8)));
            Ok(())
        })
        .expect("a read inside a write must not deadlock");

    // After commit the reader catches up.
    assert_eq!(
        store.revision_of("cas_fixture", "row-a").unwrap(),
        Some(crate::Revision::new(8))
    );
}

#[test]
fn a_second_store_for_the_same_database_is_refused() {
    let dir = crate::test_support::TempDir::new("single-store");
    let path = dir.path().join("pantheon.db");
    let first = Store::open(&path).expect("first store opens");

    let err = Store::open(&path).expect_err("a second store must be refused");
    assert!(
        matches!(err, StoreError::AlreadyOpen { .. }),
        "unexpected error: {err}"
    );

    // The same database reached by a different spelling of the path is
    // still the same database.
    let indirect = dir.path().join(".").join("pantheon.db");
    assert!(matches!(
        Store::open(indirect).expect_err("an aliased path must also be refused"),
        StoreError::AlreadyOpen { .. }
    ));

    // Closing releases the claim, so an ordinary restart works.
    first.close().expect("close");
    let reopened = Store::open(&path).expect("reopening after close succeeds");
    reopened.close().expect("close again");

    // So does dropping without closing.
    drop(Store::open(&path).expect("open after drop-release"));
    Store::open(&path).expect("claim released on drop");
}

/// A symlinked database file is the same database. Canonicalizing only the
/// parent directory would miss this, because the differing final component
/// would produce two different keys for one file.
#[cfg(unix)]
#[test]
fn a_second_store_reaching_the_same_file_through_a_symlink_is_refused() {
    let dir = crate::test_support::TempDir::new("symlink-alias");
    let real = dir.path().join("pantheon.db");
    let alias = dir.path().join("alias.db");

    let first = Store::open(&real).expect("first store opens");
    std::os::unix::fs::symlink(&real, &alias).expect("create symlink");

    let err =
        Store::open(&alias).expect_err("the same file reached through a symlink must be refused");
    assert!(
        matches!(err, StoreError::AlreadyOpen { .. }),
        "unexpected error: {err}"
    );

    // And the claim still releases, so the alias is usable afterwards.
    first.close().expect("close");
    Store::open(&alias).expect("opening through the symlink after close succeeds");
}

#[test]
fn holding_one_store_does_not_block_writing_to_a_different_one() {
    let dir_a = crate::test_support::TempDir::new("two-stores-a");
    let dir_b = crate::test_support::TempDir::new("two-stores-b");
    let store_a = Store::open(dir_a.path().join("pantheon.db")).expect("open a");
    let store_b = Store::open(dir_b.path().join("pantheon.db")).expect("open b");

    // Different files share no lock, so this cannot deadlock and must
    // not be mistaken for re-entering the same writer.
    store_a
        .write(|_| store_b.write(|_| Ok(())))
        .expect("writing a second, unrelated store is legitimate");
}

/// A write committed while a plain [`Store::read`] closure is open becomes
/// visible to the next statement in that same closure.
///
/// This is not a defect in `read` — it is what autocommit means, and most
/// reads are single statements that do not care. It is recorded here because
/// it is precisely the hazard [`Store::read_snapshot`] exists to remove, and
/// a future change that made `read` transactional would make the snapshot
/// variant look redundant without this to say otherwise.
#[test]
fn a_plain_read_gives_each_statement_its_own_view() {
    let dir = TempDir::new("read-autocommit");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    seed_counter(&store);

    let (before, after) = store
        .read(|conn| {
            let before = counter(conn);
            bump_counter(&store);
            Ok((before, counter(conn)))
        })
        .expect("read");

    assert_eq!(before, 0);
    assert_eq!(after, 1, "a plain read sees a write committed during it");
}

/// A read snapshot does not.
///
/// The reads inside one snapshot agree with each other, which is the whole
/// property the Operator surface's gap-free list-then-watch depends on.
#[test]
fn a_read_snapshot_holds_one_view_across_every_statement_in_it() {
    let dir = TempDir::new("read-snapshot");
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    seed_counter(&store);

    let (before, after) = store
        .read_snapshot(|conn| {
            let before = counter(conn);
            // Commits on the authoritative writer connection, which is a
            // different connection and a different lock, so this really does
            // land in the database while the snapshot is open.
            bump_counter(&store);
            Ok((before, counter(conn)))
        })
        .expect("read");

    assert_eq!(before, 0);
    assert_eq!(
        after, before,
        "a snapshot read must not see a write committed during it"
    );

    // And the write did commit — the assertion above is isolation, not a
    // write that silently failed.
    let settled = store
        .read(|conn| {
            Ok(conn
                .query_row("SELECT value FROM probe_counter WHERE id = 1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("value"))
        })
        .expect("read");
    assert_eq!(settled, 1);
}

fn seed_counter(store: &Store) {
    store
        .write(|writer| {
            writer.execute_batch_for_test(
                "CREATE TABLE probe_counter (
                     id       INTEGER PRIMARY KEY CHECK (id = 1),
                     value    INTEGER NOT NULL,
                     revision INTEGER NOT NULL
                 ) STRICT;
                 INSERT INTO probe_counter (id, value, revision) VALUES (1, 0, 1);",
            )
        })
        .expect("fixture commits");
}

fn bump_counter(store: &Store) {
    store
        .write(|writer| {
            let affected = writer.execute(
                "UPDATE probe_counter SET value = value + 1 WHERE id = 1",
                &[],
            )?;
            assert_eq!(affected, 1);
            Ok(())
        })
        .expect("the write commits");
}

fn counter(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT value FROM probe_counter WHERE id = 1", [], |row| {
        row.get::<_, i64>(0)
    })
    .expect("value")
}
