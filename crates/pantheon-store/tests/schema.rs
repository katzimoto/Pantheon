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
            "agent_control_sessions".to_string(),
            "agent_requests".to_string(),
            "artifact_members".to_string(),
            "artifacts".to_string(),
            "attempt_status".to_string(),
            "attempts".to_string(),
            "blobs".to_string(),
            "candidate_outputs".to_string(),
            "candidates".to_string(),
            "commands".to_string(),
            "configuration_components".to_string(),
            "configuration_revisions".to_string(),
            "context_plans".to_string(),
            "context_source_snapshots".to_string(),
            "event_journal".to_string(),
            "execution_bindings".to_string(),
            "goal_revisions".to_string(),
            "goal_scheduling_state".to_string(),
            "goals".to_string(),
            "journal_epochs".to_string(),
            "planning_operations".to_string(),
            "planning_records".to_string(),
            "production_records".to_string(),
            "run_context_plans".to_string(),
            "run_status".to_string(),
            "runs".to_string(),
            "sandbox_instances".to_string(),
            "scheduler_state".to_string(),
            "schema_migrations".to_string(),
            "system_state".to_string(),
            "task_graph_edges".to_string(),
            "task_graphs".to_string(),
            "task_scheduling_state".to_string(),
            "task_specs".to_string(),
            "tasks".to_string(),
            "workspace_revisions".to_string(),
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
            "agent_behavior_digest",
            "agent_name",
            "agent_soul_digest",
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

    // The context-plan families carry exactly the immutable identities the
    // attachment contract needs: a content-addressed plan bound to one source
    // snapshot, and a one-time Run attachment. No mutable status, no provider
    // representation, no authorization material has a column here.
    assert_eq!(
        columns(&dir.db_path(), "context_plans"),
        [
            "builder_version",
            "canonical_json",
            "created_at",
            "digest",
            "source_snapshot_digest"
        ],
        "a plan is its frozen selected semantics plus the snapshot it came from"
    );
    assert_eq!(
        columns(&dir.db_path(), "run_context_plans"),
        [
            "attached_at",
            "context_plan_digest",
            "context_source_snapshot_digest",
            "run_id"
        ],
        "the Run attachment is exactly the composite identity T3a proves"
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
            "candidate_digest",
            "current_attempt_id",
            "phase",
            "revision",
            "run_id",
            "task_id",
            "terminal_target",
            "updated_at"
        ],
        "Run lifecycle, the holder-safe current-Attempt pointer, and the \
         Candidate fact a Completed Run must carry — and nothing else"
    );
    assert_eq!(
        columns(&dir.db_path(), "attempts"),
        ["created_at", "id", "launch_key", "ordinal", "run_id"],
        "immutable Attempt identity: one ordinal per Run, one global LaunchKey"
    );
    assert_eq!(
        columns(&dir.db_path(), "attempt_status"),
        [
            "attempt_id",
            "finished_at",
            "launch_contact_epoch",
            "launch_contact_initiated_at",
            "launch_contact_state",
            "observed_execution",
            "revision",
            "run_id",
            "started_at",
            "terminal",
            "updated_at"
        ],
        "mutable Attempt status carries the durable contact boundary; no bearer column exists"
    );
    assert_eq!(
        columns(&dir.db_path(), "agent_control_sessions"),
        [
            "attempt_id",
            "created_at",
            "credential_hash",
            "credential_rekeyed_at",
            "credential_revision",
            "id",
            "restore_generation",
            "revocation_reason",
            "revoked_at",
            "state"
        ],
        "only the one-way verifier is persisted; the raw bearer has no column at all"
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

    // The composite parent key the run_context_plans foreign keys require.
    // SQLite cannot add a UNIQUE constraint to `runs` without rebuilding it,
    // so the parent key arrives as a unique index over exactly the two
    // columns the attachment proves against.
    let runs_parent: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'index' AND name = 'runs_id_and_source_snapshot'",
            [],
            |row| row.get(0),
        )
        .expect("the runs composite parent-key index exists");
    assert!(
        runs_parent.contains("UNIQUE")
            && runs_parent.contains("id")
            && runs_parent.contains("context_source_snapshot_digest"),
        "the Run/snapshot composite parent key must be unique: {runs_parent}"
    );

    // The two holder-safe foreign keys on the attachment, as the canonical
    // persistence contract specifies them: together they make attaching a
    // plan built from any other snapshot a database-level impossibility.
    let mut stmt = conn
        .prepare(
            "SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list('run_context_plans')",
        )
        .unwrap();
    let mut fks: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    fks.sort();
    assert_eq!(
        fks,
        vec![
            (
                "context_plan_digest".to_string(),
                "context_plans".to_string(),
                Some("digest".to_string())
            ),
            (
                "context_source_snapshot_digest".to_string(),
                "context_plans".to_string(),
                Some("source_snapshot_digest".to_string())
            ),
            (
                "context_source_snapshot_digest".to_string(),
                "runs".to_string(),
                Some("context_source_snapshot_digest".to_string())
            ),
            (
                "run_id".to_string(),
                "runs".to_string(),
                Some("id".to_string())
            ),
        ],
        "run_context_plans must prove Run→frozen snapshot and plan→same snapshot"
    );

    // The one-nonterminal-Attempt rule is a real partial unique index on the
    // status table where terminality lives, exactly as the persistence
    // contract specifies — not controller discipline.
    let one_attempt: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'index' AND name = 'one_nonterminal_attempt_per_run'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        one_attempt.contains("UNIQUE") && one_attempt.contains("WHERE terminal = 0"),
        "one nonterminal Attempt per Run must stay a partial unique index: {one_attempt}"
    );

    // The LaunchKey is globally unique across every Run and Attempt.
    // Declared inline as UNIQUE, so it is an auto-index; the guard pins it
    // through pragma introspection rather than a mutable internal name.
    let mut stmt = conn
        .prepare("SELECT name, \"unique\", origin FROM pragma_index_list('attempts')")
        .unwrap();
    let mut attempt_indexes: Vec<(String, bool)> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? == 1))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    attempt_indexes.sort();
    let launch_key_unique = attempt_indexes.iter().any(|(name, unique)| {
        *unique && {
            let mut stmt = conn
                .prepare(&format!("SELECT name FROM pragma_index_info('{name}')"))
                .unwrap();
            let columns: Vec<String> = stmt
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            columns == ["launch_key"]
        }
    });
    assert!(
        launch_key_unique,
        "the LaunchKey must stay globally unique via its own unique index"
    );

    // The Attempt families' holder-safe foreign keys, as the contract
    // specifies them: attempt_status cannot drift off its immutable Attempt,
    // the session cannot outlive-or-migrate its Attempt, and the Run's
    // current-Attempt pointer can only name an Attempt of that same Run.
    let attempt_fks = |table: &str| -> Vec<(String, String, Option<String>)> {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list('{table}')"
            ))
            .unwrap();
        let mut fks: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        fks.sort();
        fks
    };
    assert_eq!(
        attempt_fks("attempt_status"),
        vec![
            (
                "attempt_id".to_string(),
                "attempts".to_string(),
                Some("id".to_string())
            ),
            (
                "run_id".to_string(),
                "attempts".to_string(),
                Some("run_id".to_string())
            ),
        ],
        "attempt_status holder identity must be composite-constrained to its own Attempt"
    );
    assert_eq!(
        attempt_fks("agent_control_sessions"),
        vec![(
            "attempt_id".to_string(),
            "attempts".to_string(),
            Some("id".to_string())
        )],
        "each AgentControlSession belongs to exactly one Attempt"
    );
    assert_eq!(
        attempt_fks("run_status"),
        vec![
            (
                "current_attempt_id".to_string(),
                "attempts".to_string(),
                Some("id".to_string())
            ),
            (
                "run_id".to_string(),
                "attempts".to_string(),
                Some("run_id".to_string())
            ),
            (
                "run_id".to_string(),
                "runs".to_string(),
                Some("id".to_string())
            ),
            (
                "task_id".to_string(),
                "runs".to_string(),
                Some("task_id".to_string())
            ),
        ],
        "run_status.current_attempt_id must be holder-safe: this Run's Attempt only"
    );
}
