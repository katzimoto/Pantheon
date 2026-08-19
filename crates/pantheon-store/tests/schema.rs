//! The standing production-schema guard.
//!
//! Kept in its own binary because it is the one test whose job is to fail when
//! anybody adds a table or column: a reviewer should see it move on its own,
//! not buried in an unrelated diff.

mod common;

use common::{TempDir, columns};
use pantheon_store::Store;

#[test]
fn production_schema_contains_only_the_tables_this_behaviour_needs() {
    let dir = TempDir::new("schema-purity");
    Store::open(dir.db_path()).expect("open store");

    let conn = rusqlite::Connection::open(dir.db_path()).expect("raw connection");
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .unwrap();
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    // The durable command ledger, Event Journal and journal epoch/sequence
    // state this behaviour needs — and nothing else. No revisioned fixture
    // table, and none of the future domain schema (Goal, Task, Run,
    // scheduler) that later missions own.
    assert_eq!(
        tables,
        vec![
            "active_configuration".to_string(),
            "commands".to_string(),
            "configuration_components".to_string(),
            "configuration_revisions".to_string(),
            "event_journal".to_string(),
            "journal_epochs".to_string(),
            "schema_migrations".to_string(),
            "system_state".to_string(),
        ],
        "unexpected production table set"
    );

    // The exact column set of every table this mission adds. A table-level
    // guard cannot see a speculative column, and it cannot see a request-body
    // or payload column being added to the command ledger — which is the
    // mission's "persist only non-sensitive request identity and hash data"
    // constraint expressed as something mechanical rather than as prose.
    assert_eq!(
        columns(&dir.db_path(), "commands"),
        [
            "command_epoch",
            "command_id",
            "journal_epoch",
            "recorded_at",
            "request_hash",
            "sequence"
        ],
        "the command ledger must carry identity and hash only, never a request body"
    );
    assert_eq!(
        columns(&dir.db_path(), "event_journal"),
        [
            "command_epoch",
            "command_id",
            "event_id",
            "event_type",
            "journal_epoch",
            "recorded_at",
            "sequence"
        ]
    );
    assert_eq!(
        columns(&dir.db_path(), "journal_epochs"),
        ["epoch", "is_current", "next_sequence"]
    );

    assert_eq!(
        columns(&dir.db_path(), "active_configuration"),
        ["activation_sequence", "id", "revision"],
        "the active pointer is one small row, as the configuration contract requires"
    );
    assert_eq!(
        columns(&dir.db_path(), "configuration_components"),
        ["canonical_json", "digest", "domain"]
    );
    assert_eq!(
        columns(&dir.db_path(), "configuration_revisions"),
        [
            "activation_sequence",
            "agents_digest",
            "authorization_digest",
            "compiler_version",
            "content_digest",
            "context_policy_digest",
            "evaluator_registry_digest",
            "execution_profile_digest",
            "recorded_at",
            "routing_digest",
            "source_set_digest"
        ],
        "each component keeps its own digest column; no ambiguous catch-all hash"
    );

    // AUTOINCREMENT would create `sqlite_sequence` and would be a plausible
    // shortcut that contradicts the contract's singleton next-sequence
    // allocator and its epoch-scoped ordering.
    let autoincrement: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'sqlite_sequence')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !autoincrement,
        "journal sequencing must not use SQLite AUTOINCREMENT"
    );
}
