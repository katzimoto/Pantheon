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

    // The durable state this behaviour needs — and nothing else. No
    // revisioned fixture table, and none of the future domain schema
    // (Attempt, Sandbox, ContextPlan, reservation, budget) that later
    // missions own.
    assert_eq!(
        tables,
        vec![
            "active_configuration".to_string(),
            "commands".to_string(),
            "configuration_components".to_string(),
            "configuration_revisions".to_string(),
            "context_source_snapshots".to_string(),
            "event_journal".to_string(),
            "execution_bindings".to_string(),
            "goal_revisions".to_string(),
            "goal_scheduling_state".to_string(),
            "goals".to_string(),
            "journal_epochs".to_string(),
            "planning_operations".to_string(),
            "planning_records".to_string(),
            "run_status".to_string(),
            "runs".to_string(),
            "scheduler_state".to_string(),
            "schema_migrations".to_string(),
            "system_state".to_string(),
            "task_graph_edges".to_string(),
            "task_graphs".to_string(),
            "task_scheduling_state".to_string(),
            "task_specs".to_string(),
            "tasks".to_string(),
            "workspaces".to_string(),
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

    // The Task carries only the bounded semantic outcome. No Logical Agent,
    // backend, provider, model, runtime, credential, Workspace path,
    // LaunchKey or dependency edge has a column here — the Task contract
    // forbids each by name, and a column-level guard is what keeps a later
    // mission from adding one quietly.
    assert_eq!(
        columns(&dir.db_path(), "tasks"),
        [
            "active_run_id",
            "created_graph_revision",
            "goal_id",
            "id",
            "phase",
            "revision",
            "spec_digest",
            "terminal_reason_json",
            "terminal_target"
        ],
        "the Task row is lifecycle state only"
    );
    assert_eq!(
        columns(&dir.db_path(), "task_specs"),
        [
            "acceptance_digest",
            "canonical_json",
            "configuration_activation_sequence",
            "digest",
            "evaluator_registry_digest",
            "goal_id",
            "goal_revision"
        ]
    );
    // Dependency relationships live on the graph, never on the spec.
    assert_eq!(
        columns(&dir.db_path(), "task_graph_edges"),
        [
            "created_graph_revision",
            "downstream_task_id",
            "goal_id",
            "kind",
            "removed_graph_revision",
            "upstream_task_id"
        ]
    );
    assert_eq!(columns(&dir.db_path(), "task_graphs"), ["id", "revision"]);
    assert_eq!(
        columns(&dir.db_path(), "goals"),
        [
            "created_at",
            "current_revision",
            "id",
            "phase",
            "revision",
            "terminal_target"
        ]
    );
    assert_eq!(
        columns(&dir.db_path(), "goal_revisions"),
        [
            "canonical_json",
            "content_digest",
            "created_at",
            "goal_id",
            "revision"
        ]
    );
    // Local deterministic planning carries no external backend, metering or
    // attempt lineage — those columns arrive with the behaviour that needs
    // them, and `planning_attempts` does not exist at all.
    assert_eq!(
        columns(&dir.db_path(), "scheduler_state"),
        [
            "dispatch_mode",
            "id",
            "next_service_sequence",
            "revision",
            "updated_at"
        ],
        "the scheduler singleton is desired state plus the fairness counter only"
    );
    assert_eq!(
        columns(&dir.db_path(), "goal_scheduling_state"),
        [
            "created_at",
            "goal_id",
            "last_served_sequence",
            "revision",
            "updated_at"
        ]
    );
    assert_eq!(
        columns(&dir.db_path(), "task_scheduling_state"),
        [
            "eligible_since",
            "last_failure_code",
            "last_failure_detail_json",
            "next_attempt_at",
            "revision",
            "task_id",
            "updated_at"
        ],
        "backoff is suppression metadata; it never becomes Task lifecycle"
    );
    assert_eq!(
        columns(&dir.db_path(), "execution_bindings"),
        [
            "canonical_json",
            "configuration_activation_sequence",
            "configuration_content_digest",
            "created_at",
            "digest",
            "task_id"
        ]
    );
    assert_eq!(
        columns(&dir.db_path(), "context_source_snapshots"),
        [
            "agent_name",
            "agent_version",
            "canonical_json",
            "configuration_activation_sequence",
            "context_policy_digest",
            "created_at",
            "digest",
            "goal_id",
            "goal_revision",
            "graph_revision",
            "task_spec_digest",
            "workspace_id",
            "workspace_resolved_base"
        ]
    );
    assert_eq!(
        columns(&dir.db_path(), "runs"),
        [
            "binding_digest",
            "context_source_snapshot_digest",
            "created_at",
            "id",
            "task_id"
        ],
        "the immutable Run row names its frozen strategy and source universe"
    );
    assert_eq!(
        columns(&dir.db_path(), "run_status"),
        [
            "active_slot",
            "phase",
            "revision",
            "run_id",
            "task_id",
            "terminal_target",
            "updated_at"
        ],
        "no Attempt, lease or Candidate column exists before its behaviour does"
    );

    assert_eq!(
        columns(&dir.db_path(), "planning_operations"),
        [
            "configuration_activation_sequence",
            "created_at",
            "expected_graph_revision",
            "goal_id",
            "goal_revision",
            "id",
            "planner_implementation",
            "planner_version",
            "planning_input_digest",
            "revision",
            "state",
            "trigger_kind"
        ]
    );
    assert_eq!(
        columns(&dir.db_path(), "planning_records"),
        [
            "canonical_proposal",
            "created_at",
            "normalization_provenance",
            "planning_operation_id",
            "proposal_digest"
        ]
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

    // The Task-owned Workspace carries durable ownership, the two base
    // identities and lifecycle — and nothing about a Run, a Sandbox, a
    // WorkspaceRevision, a Candidate or a Git worktree. `repositories`,
    // `workspace_revisions` and `integration_intents` are named in the same
    // canonical table family and deliberately do not exist yet.
    assert_eq!(
        columns(&dir.db_path(), "workspaces"),
        [
            "created_at",
            "id",
            "materialization",
            "phase",
            "repository",
            "requested_base",
            "resolved_base",
            "revision",
            "source_path",
            "task_id"
        ],
        "the Workspace row is ownership, base identity and lifecycle only"
    );

    // The cardinality invariant lives in the database, not in controller
    // discipline: a Task owns at most one Workspace that is not Released.
    let index: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'index' AND name = 'workspaces_one_current_per_task'",
            [],
            |row| row.get(0),
        )
        .expect("the one-current-workspace index exists");
    assert!(
        index.contains("UNIQUE") && index.contains("phase != 'Released'"),
        "the one-current-workspace index must stay a partial unique index: {index}"
    );

    // The one-live-Run rule and the v0.1.0 single execution slot are both
    // database constraints, not controller promises.
    let one_run: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'index' AND name = 'one_nonterminal_run_per_task'",
            [],
            |row| row.get(0),
        )
        .expect("the one-nonterminal-run index exists");
    assert!(
        one_run.contains("UNIQUE") && one_run.contains("phase IN ('Active', 'Finalizing')"),
        "one nonterminal Run per Task must stay a partial unique index: {one_run}"
    );
    let slot: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'index' AND name = 'one_active_execution_slot'",
            [],
            |row| row.get(0),
        )
        .expect("the single-slot index exists");
    assert!(
        slot.contains("UNIQUE") && slot.contains("active_slot IS NOT NULL"),
        "the single global execution slot must stay a partial unique index: {slot}"
    );
}
