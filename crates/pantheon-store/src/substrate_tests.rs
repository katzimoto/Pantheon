//! Assumptions about SQLite that Pantheon's correctness rests on.
//!
//! Every test here pins a fact about the substrate, not about Pantheon. They
//! exist because a comment asserting substrate behaviour is not evidence, and
//! Pantheon has been wrong about this in a way that reached `main`: Issue #26
//! shipped a helper documented as reading "one implicit read transaction",
//! which SQLite does not provide, and the gap-free guarantee that rested on it
//! did not hold. A ten-line probe settled it in a minute — and then the
//! finding evaporated with the session.
//!
//! A test does not evaporate. When a rusqlite or SQLite upgrade changes one of
//! these behaviours, the design that depended on it fails loudly here instead
//! of becoming quietly wrong somewhere else.
//!
//! Keep this module to facts Pantheon actually depends on. It is not a place
//! to characterise SQLite in general.

use crate::store::Store;
use crate::test_support::TempDir;

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
