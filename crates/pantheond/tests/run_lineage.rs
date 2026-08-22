//! End-to-end evidence for Issue #31 over a real daemon on a real socket.
//!
//! The sequence is the one the mission asks to be demonstrated:
//!
//! ```text
//! goal via Operator Control -> Ready coding Task          (daemon A)
//! Workspace materialized + T3 committed + plan attached     (in-process,
//!                                                           the way every
//!                                                           pre-Run fixture
//!                                                           composes it)
//! daemon B (--executor fake): preparation reaches LaunchReady,
//!   T4 creates Attempt + LaunchKey + session               -> killed mid-flight,
//!                                                            deterministically
//!                                                            pre-contact
//! daemon C: inventories the nonterminal lineage, rekeys the lost bearer
//!   through T4a, crosses T4b once, reconciles the fake world to a
//!   durable conclusion — with exactly ONE Attempt/LaunchKey ever
//! ```
//!
//! The daemon has never driven Workspace materialization (that supervision is
//! #27's controller loop), so this test composes the pre-Run durable facts
//! itself between incarnations, exactly as `workspace_sealing.rs` composes
//! them. Everything after LaunchReady is the daemon's own work.

mod support;

use std::path::Path;

use pantheon_core::config::Digest;
use pantheon_core::context::{frozen_agent_guidance, guidance_digest};
use pantheon_store::{Command, Revision, RunIntent, Store};

use support::{Installation, get, post};

/// Slow enough that observing one lifecycle Event durably proves which
/// controller passes have run (the next pass is a full tick away); fast
/// enough that the whole flow finishes in seconds.
const TICK_ARGS: &[&str] = &["--executor", "fake", "--tick-millis", "400"];

fn command<'a>(
    epoch: &'a str,
    id: &'a str,
    hash: &'a [u8; 32],
    event: &'static str,
) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: hash,
        event_type: event,
    }
}

/// One event page after `cursor`, decoded. An empty cursor names the head.
async fn events_since(socket: &Path, cursor: &str) -> (Vec<String>, String) {
    let path = if cursor.is_empty() {
        "/api/v1/events?limit=200".to_string()
    } else {
        format!("/api/v1/events?after={cursor}&limit=200")
    };
    let answer = get(socket, &path).await;
    let body = answer.json();
    let types = body["events"]
        .as_array()
        .expect("events array")
        .iter()
        .map(|event| event["eventType"].as_str().expect("event type").to_string())
        .collect();
    let next = body["nextCursor"]
        .as_str()
        .expect("next cursor")
        .to_string();
    (types, next)
}

/// Polls until every wanted Event type has been observed.
async fn wait_for(socket: &Path, mut cursor: String, wanted: &[&str]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..600 {
        let (types, next) = events_since(socket, &cursor).await;
        cursor = next;
        seen.extend(types);
        if wanted.iter().all(|want| seen.iter().any(|got| got == want)) {
            return seen;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {wanted:?}; saw {seen:?}");
}

/// Composes the durable pre-Run facts the daemon does not yet supervise:
/// the Task's verified Workspace, the T3 Run intent, and the one-time
/// ContextPlan attachment. The Run is left at LaunchReady with zero Attempts.
fn bridge_to_launch_ready(db_path: &Path) {
    let store = Store::open(db_path).expect("open the installation store");

    // Discover the Ready Task the daemon materialized.
    let goals = store.goal_snapshot().expect("goal snapshot").goals;
    let goal_id = goals.first().expect("the Goal exists").id.clone();
    let detail = store
        .goal_detail(&goal_id)
        .expect("detail")
        .expect("detail");
    let task = &detail.tasks[0];

    // WorkspaceReady: open, materialize, verify — as #27's controller would.
    let requested = pantheon_core::workspace::RequestedBase::parse("main").expect("ref");
    let resolved = pantheon_core::workspace::ResolvedBase::parse(&"a".repeat(40)).expect("base");
    let binding = pantheon_store::WorkspaceBinding {
        task_id: &task.id,
        repository: "repo://whiskyshop",
        source_path: "/tmp/pantheond-run-lineage-source",
        requested_base: &requested,
        resolved_base: &resolved,
    };
    let epoch = store.restore_generation().expect("generation").to_string();
    store
        .open_workspace(
            &command(&epoch, "ws-open", &[7u8; 32], "workspace.opened"),
            "ws-lineage",
            &binding,
        )
        .expect("open workspace");
    store
        .begin_workspace_materialization(
            &command(&epoch, "ws-begin", &[8u8; 32], "workspace.materializing"),
            "ws-lineage",
            Revision::new(1),
        )
        .expect("begin materialization");
    store
        .complete_workspace_materialization(
            &command(&epoch, "ws-done", &[9u8; 32], "workspace.ready"),
            "ws-lineage",
            Revision::new(2),
            &resolved,
        )
        .expect("complete materialization");

    // Freeze the strategy and source universe exactly as the Scheduler would,
    // then commit T3 with fresh expectations read from current state.
    let active = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active configuration");
    let agent = pantheon_core::execution::LogicalAgentVersion {
        name: "builder".to_string(),
        version: 1,
    };
    let agents_json = store
        .revision_agents_component_json(active.activation_sequence)
        .expect("read agents component")
        .expect("agents component stored");
    let value = pantheon_core::config::parse::parse(&agents_json).expect("fixture component");
    let guidance = frozen_agent_guidance(&value, &agent).expect("fixture guidance");

    // The Binding freezes the *real* sandbox profile identity: the content
    // digest of the configured profile under the captured revision.
    let profiles_json = store
        .configuration_component_json(active.components.execution_profile)
        .expect("read execution profiles")
        .expect("execution profiles stored")
        .1;
    let profiles_value =
        pantheon_core::config::parse::parse(&profiles_json).expect("fixture component");
    let profiles = match profiles_value.get("profiles") {
        Some(pantheon_core::config::canonical::Value::Array(items)) => items,
        other => panic!("profiles array expected, got {other:?}"),
    };
    let strict = &profiles[0];
    let sandbox_profile_digest = Digest::of(&strict.to_canonical_bytes());

    let binding_frozen = pantheon_core::scheduling::ExecutionBinding {
        task_id: task.id.clone(),
        agent: agent.clone(),
        request_digest: Digest::of(b"request"),
        offer_digest: Digest::of(b"offer"),
        backend_id: "fake-local".to_string(),
        descriptor_revision: 1,
        descriptor_digest: Digest::of(b"descriptor"),
        execution_profile_digest: active.components.execution_profile,
        sandbox_profile_digest,
        route_policy_digest: active.components.routing,
        configuration_activation_sequence: active.activation_sequence,
        configuration_content_digest: active.content_digest,
        component_digests: active.components,
    };
    let snapshot_frozen = pantheon_core::scheduling::ContextSourceSnapshot {
        task_spec_digest: task.spec_digest,
        goal_id: task.goal_id.clone(),
        goal_revision: 1,
        graph_revision: task.created_graph_revision,
        agent,
        configuration_activation_sequence: active.activation_sequence,
        context_policy_digest: active.components.context_policy,
        agent_soul_digest: guidance_digest(&guidance.soul),
        agent_behavior_digest: guidance_digest(&guidance.behavior),
        workspace_id: "ws-lineage".to_string(),
        workspace_resolved_base: resolved.as_str().to_string(),
    };
    let binding_digest = binding_frozen.digest();
    let snapshot_digest = snapshot_frozen.digest();

    // Re-read the snapshot now that the Workspace is verified: candidacy
    // requires it, so the earlier read could not have seen this Task.
    let snap = store.scheduling_snapshot().expect("scheduling snapshot");
    let candidate = snap.candidates.first().expect("dispatchable Task").clone();
    let run_id = "run-lineage";
    let intent = RunIntent {
        run_id,
        task_id: &candidate.task_id,
        goal_id: &candidate.goal_id,
        expected_task_revision: candidate.task_revision,
        expected_goal_row_revision: candidate.goal_row_revision,
        expected_goal_current_revision: candidate.goal_current_revision,
        expected_graph_revision: candidate.graph_revision,
        expected_workspace_revision: candidate.workspace_revision,
        expected_scheduler_revision: snap.state.revision,
        expected_goal_fairness_revision: None,
        expected_task_scheduling_revision: candidate.scheduling_revision,
        configuration_activation_sequence: active.activation_sequence,
        binding_digest: &binding_digest,
        binding: &binding_frozen,
        snapshot_digest: &snapshot_digest,
        snapshot: &snapshot_frozen,
    };
    match store
        .commit_run_intent(
            &command(&epoch, "cmd-t3", &[11u8; 32], "run.committed"),
            &intent,
        )
        .expect("T3 commits")
    {
        pantheon_store::Committed::Executed { .. } => {}
        other => panic!("a fresh T3 executes, got {other:?}"),
    }

    // ContextReady: the real deterministic preparation path attaches the
    // one-time plan derived from the exact frozen sources.
    pantheon_engine::context::ContextPreparationController::new(&store)
        .prepare_run_context(run_id)
        .expect("prepare run context");

    store.close().expect("close store");
}

#[tokio::test]
async fn a_fake_executor_run_drives_the_full_lineage_across_a_restart() {
    let installation = Installation::new("lineage");

    // Incarnation A creates the Goal through Operator Control only.
    {
        let daemon = installation.start().await;
        let (_, head) = events_since(daemon.socket(), "").await;
        let created = post(
            daemon.socket(),
            "/api/v1/goals",
            "goal-lineage",
            Some(support::GOAL_BODY),
        )
        .await;
        assert_eq!(created.status, hyper::StatusCode::CREATED);
        wait_for(daemon.socket(), head, &["task.materialized"]).await;
    }

    // Between incarnations: compose the pre-Run durable facts in-process.
    bridge_to_launch_ready(&installation.dir.join("pantheon.db"));

    // Incarnation B owns the whole lifecycle start: preparation is already
    // ContextReady, so its first pass establishes the Attempt (T4). With
    // 400ms ticks the launch pass is a full tick later, so stopping right
    // after the creation Event is a deterministic pre-T4b crash window.
    let (_, head_cursor) = {
        let daemon = installation.start_with(TICK_ARGS).await;
        let (_, head) = events_since(daemon.socket(), "").await;
        let seen = wait_for(daemon.socket(), head.clone(), &["run.attempt.created"]).await;
        assert!(
            !seen.contains(&"run.attempt.contact-initiated".to_string()),
            "the kill must land pre-contact; saw {seen:?}"
        );
        ((), head)
    };

    // Incarnation C reconstructs over the same durable store: it must
    // inventory the nonterminal Run/Attempt, rekey the SAME session's lost
    // bearer through T4a, cross the contact boundary exactly once, and
    // reconcile the fake external world to a durable conclusion.
    let daemon = installation.start_with(TICK_ARGS).await;
    let mut cursor = head_cursor;
    let mut after_restart: Vec<String> = Vec::new();
    loop {
        let (types, next) = events_since(daemon.socket(), &cursor).await;
        cursor = next;
        after_restart.extend(types);
        if after_restart.iter().any(|kind| kind == "run.concluded") {
            break;
        }
        assert!(
            after_restart.len() < 64,
            "runaway reconciliation: {after_restart:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // A concluded Run stays concluded: while this incarnation keeps running,
    // no further lifecycle Events appear.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let (quiet, _) = events_since(daemon.socket(), &cursor).await;
    assert!(
        !quiet
            .iter()
            .any(|kind| kind.starts_with("run.attempt") || kind == "run.committed"),
        "a concluded Run stays concluded: {quiet:?}"
    );
    drop(daemon);

    // The complete lineage story. Everything B committed after `head` plus
    // everything C committed is one continuous journal window: exactly one
    // Attempt, one rekey, one contact, one conclusion.
    let count_in = |haystack: &[String], kind: &str| haystack.iter().filter(|k| k == &kind).count();
    assert_eq!(
        count_in(&after_restart, "run.attempt.created"),
        1,
        "exactly one Attempt ever exists across both incarnations"
    );
    assert_eq!(
        count_in(&after_restart, "agent-control.session.rekeyed"),
        1,
        "T4a recovers the lost pre-contact bearer exactly once"
    );
    assert_eq!(
        count_in(&after_restart, "run.attempt.contact-initiated"),
        1,
        "the external boundary is crossed exactly once"
    );
    let position = |kind: &str| {
        after_restart
            .iter()
            .position(|k| k == kind)
            .unwrap_or_else(|| panic!("{kind} must appear"))
    };
    assert!(
        position("run.attempt.created") < position("agent-control.session.rekeyed")
            && position("agent-control.session.rekeyed")
                < position("run.attempt.contact-initiated"),
        "T4 -> T4a -> T4b order holds across the restart: {:?}",
        after_restart
    );
    let tail_at = position("run.attempt.terminal");
    assert!(
        after_restart[tail_at..].contains(&"run.concluded".to_string()),
        "the Run concludes after its Attempt terminalizes"
    );
}
