//! Evidence for the durable half of Issue #30: T3's frozen-guidance fence and
//! the one-time T3a ContextPlan attachment.
//!
//! The engine-level composition (reconstruction, deterministic building,
//! restart behavior) is proven in `pantheon-engine`; what only a test at this
//! altitude can establish is what one authoritative transaction does to stored
//! state under success, replay, conflict, wrong-source and corrupted-content
//! claims — and that the relational constraints back every controller check.

use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value;
use pantheon_core::context::{CONTEXT_BUILDER_VERSION, guidance_digest};
use pantheon_core::execution::LogicalAgentVersion;
use pantheon_core::scheduling::{ContextSourceSnapshot, ExecutionBinding};

use crate::command::{Command, Committed};
use crate::configuration::ActiveConfiguration;
use crate::context::{AttachedContextPlan, ContextPlanAttachment, RunContextPlanRecord};
use crate::error::StoreError;
use crate::planning::tests as fixture;
use crate::scheduling::RunIntent;
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::{Revision, Value as RowValue};

const TASK: &str = "task-1";
const GOAL: &str = "goal-1";
const WORKSPACE: &str = "ws-1";
const RUN: &str = "run-1";

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

/// A committed Run with its frozen authority, ready for attachment tests.
struct World {
    _dir: TempDir,
    store: Option<Store>,
    active: ActiveConfiguration,
    snapshot_digest: Digest,
}

impl World {
    fn s(&self) -> &Store {
        self.store.as_ref().expect("store open")
    }

    fn close_store(&mut self) {
        if let Some(store) = self.store.take() {
            store.close().expect("close store");
        }
    }
}

/// The approved guidance of one Agent version under an activated revision.
fn stored_guidance(
    store: &Store,
    active: &ActiveConfiguration,
    agent: &LogicalAgentVersion,
) -> pantheon_core::context::AgentGuidance {
    let agents_json = store
        .revision_agents_component_json(active.activation_sequence)
        .expect("read agents component")
        .expect("agents component stored");
    let value = pantheon_core::config::parse::parse(&agents_json).expect("fixture component");
    pantheon_core::context::frozen_agent_guidance(&value, agent).expect("fixture guidance")
}

fn world(label: &str) -> World {
    let (dir, store, sequence) = fixture::store_with_configuration(label);
    let epoch = store.restore_generation().expect("generation");
    fixture::create_goal(&store, GOAL, "cmd-goal");
    let op = fixture::plan_and_record(&store, GOAL, sequence, "op-1");
    let registry_digest = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .as_ref()
        .expect("active")
        .components
        .evaluator_registry;
    let plan = fixture::validated(sequence, registry_digest, "unit-v1");
    fixture::materialize(&store, &op, TASK, &plan, "cmd-materialize").expect("materializes");

    // Materialize the Task-owned Workspace exactly as #27's controller would.
    let requested = pantheon_core::workspace::RequestedBase::parse("main").expect("ref");
    let resolved = pantheon_core::workspace::ResolvedBase::parse(&"a".repeat(40)).expect("base");
    let binding = crate::workspace::WorkspaceBinding {
        task_id: TASK,
        repository: "repo://project",
        source_path: "/tmp/pantheon-context-test-source",
        requested_base: &requested,
        resolved_base: &resolved,
    };
    let open_id = format!("cmd-ws-open-{WORKSPACE}");
    let begin_id = format!("cmd-ws-begin-{WORKSPACE}");
    let complete_id = format!("cmd-ws-complete-{WORKSPACE}");
    store
        .open_workspace(
            &command(epoch.as_str(), &open_id, &[7u8; 32], "workspace.opened"),
            WORKSPACE,
            &binding,
        )
        .expect("open workspace");
    store
        .begin_workspace_materialization(
            &command(
                epoch.as_str(),
                &begin_id,
                &[8u8; 32],
                "workspace.materializing",
            ),
            WORKSPACE,
            Revision::new(1),
        )
        .expect("begin materialization");
    store
        .complete_workspace_materialization(
            &command(epoch.as_str(), &complete_id, &[9u8; 32], "workspace.ready"),
            WORKSPACE,
            Revision::new(2),
            &resolved,
        )
        .expect("complete materialization");

    let active_pointer = store.configuration_pointer().expect("pointer");
    let active = active_pointer.active.clone().expect("active");

    // Freeze the strategy and source universe, then commit T3 with fresh
    // expectations read from current state.
    let agent = LogicalAgentVersion {
        name: "builder".to_string(),
        version: 1,
    };
    let guidance = stored_guidance(&store, &active, &agent);
    let spec_digest = fixture::tasks_of(&store, GOAL)[0].spec_digest;
    let binding_frozen = ExecutionBinding {
        task_id: TASK.to_string(),
        agent: agent.clone(),
        request_digest: Digest::of(b"request"),
        offer_digest: Digest::of(b"offer"),
        backend_id: "fake-local".to_string(),
        descriptor_revision: 3,
        descriptor_digest: Digest::of(b"descriptor"),
        execution_profile_digest: active.components.execution_profile,
        sandbox_profile_digest: Digest::of(b"sandbox-profile"),
        route_policy_digest: active.components.routing,
        configuration_activation_sequence: active.activation_sequence,
        configuration_content_digest: active.content_digest,
        component_digests: active.components,
    };
    let snapshot_frozen = ContextSourceSnapshot {
        task_spec_digest: spec_digest,
        goal_id: GOAL.to_string(),
        goal_revision: 1,
        graph_revision: 1,
        agent,
        configuration_activation_sequence: active.activation_sequence,
        context_policy_digest: active.components.context_policy,
        agent_soul_digest: guidance_digest(&guidance.soul),
        agent_behavior_digest: guidance_digest(&guidance.behavior),
        workspace_id: WORKSPACE.to_string(),
        workspace_resolved_base: resolved.as_str().to_string(),
    };
    let binding_digest = binding_frozen.digest();
    let snapshot_digest = snapshot_frozen.digest();

    let snap = store.scheduling_snapshot().expect("snapshot");
    let candidate = snap.candidates.first().expect("dispatchable Task");
    let intent = RunIntent {
        run_id: RUN,
        task_id: candidate.task_id.as_str(),
        goal_id: candidate.goal_id.as_str(),
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
            &command(epoch.as_str(), "cmd-t3", &[11u8; 32], "run.committed"),
            &intent,
        )
        .expect("T3 commits")
    {
        Committed::Executed { .. } => {}
        other => panic!("a fresh T3 executes, got {other:?}"),
    }

    World {
        _dir: dir,
        store: Some(store),
        active,
        snapshot_digest,
    }
}

/// A minimal valid plan claim over the world's frozen snapshot.
fn plan_claim(value: u8) -> (Digest, String) {
    let canonical = Value::object([
        ("builder", Value::string(CONTEXT_BUILDER_VERSION)),
        ("fixture", Value::Integer(i64::from(value))),
    ])
    .to_canonical_bytes();
    let digest = Digest::of(&canonical);
    (digest, String::from_utf8(canonical).expect("utf-8"))
}

fn attachment<'a>(
    run_id: &'a str,
    snapshot_digest: &'a Digest,
    plan_digest: &'a Digest,
    plan_json: &'a str,
) -> ContextPlanAttachment<'a> {
    ContextPlanAttachment {
        run_id,
        source_snapshot_digest: snapshot_digest,
        plan_digest,
        builder_version: CONTEXT_BUILDER_VERSION,
        plan_canonical_json: plan_json,
    }
}

#[test]
fn t3a_attaches_once_reconciles_the_same_plan_and_appends_one_event() {
    // One-time attachment invariant: same Run + same source + same plan is
    // idempotent under both command replay and a fresh command identity, and
    // the Event lands atomically with the original attachment only.
    let mut world = world("t3a-idempotent");
    let store = world.s();
    let (plan_digest, plan_json) = plan_claim(1);
    let snapshot_digest = world.snapshot_digest;

    let epoch = store.restore_generation().expect("generation");
    let first = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-1", &[21u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &plan_digest, &plan_json),
        )
        .expect("first attach commits");
    let Committed::Executed { value, .. } = first else {
        panic!("a fresh attachment executes, got {first:?}");
    };
    assert_eq!(
        value,
        AttachedContextPlan {
            run_id: RUN.to_string(),
            context_plan_digest: plan_digest
        }
    );

    // Same command identity: durable replay, no second Event, no mutation.
    let replayed = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-1", &[21u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &plan_digest, &plan_json),
        )
        .expect("replay reconciles");
    assert!(matches!(replayed, Committed::Replayed { .. }));
    let events = attachment_events(store);
    assert_eq!(events, 1, "command replay appends nothing");

    // A different command identity with the same content reconciles against
    // the existing attachment instead of writing a second row. The attempt
    // itself is auditable history, so it appends its own Event — the durable
    // attachment, not the Event count, is the one-time fact.
    let reconciled = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-2", &[22u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &plan_digest, &plan_json),
        )
        .expect("same-plan retry reconciles");
    let Committed::Executed { value, .. } = reconciled else {
        panic!("a fresh reconcile attempt executes, got {reconciled:?}");
    };
    assert_eq!(value.context_plan_digest, plan_digest);

    // Exactly one attachment row naming exactly this plan.
    let record = store
        .run_context_plan(RUN)
        .expect("read")
        .expect("attached");
    assert_eq!(
        record,
        RunContextPlanRecord {
            run_id: RUN.to_string(),
            context_source_snapshot_digest: snapshot_digest,
            context_plan_digest: plan_digest,
        }
    );
    assert_eq!(
        attachment_events(store),
        2,
        "the original attachment and the audited reconcile attempt"
    );

    world.close_store();
}

/// The number of recorded attachment Events.
fn attachment_events(store: &Store) -> i64 {
    store
        .write(|writer| {
            writer.query_optional(
                "SELECT COUNT(*) FROM event_journal WHERE event_type = 'run.context.attached'",
                &[],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("count events")
        .expect("a count exists")
}

#[test]
fn a_second_different_plan_cannot_replace_the_attached_plan() {
    // Immutability invariant: once attached, the initial plan cannot be
    // replaced by a different one — not by any command identity, ever.
    let mut world = world("t3a-replace");
    let store = world.s();
    let (first_digest, first_json) = plan_claim(1);
    let (second_digest, second_json) = plan_claim(2);
    let snapshot_digest = world.snapshot_digest;

    let epoch = store.restore_generation().expect("generation");
    store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-1", &[21u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &first_digest, &first_json),
        )
        .expect("first attach commits");

    let err = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-2", &[22u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &second_digest, &second_json),
        )
        .expect_err("a second different plan must fail closed");
    match err {
        StoreError::RunContextPlanConflict {
            ref run_id,
            ref attached_plan,
            ref proposed_plan,
        } => {
            assert_eq!(run_id, RUN);
            assert_eq!(*attached_plan, first_digest.to_string());
            assert_eq!(*proposed_plan, second_digest.to_string());
        }
        other => panic!("expected RunContextPlanConflict, got {other:?}"),
    }

    // The original attachment stands untouched.
    let record = store
        .run_context_plan(RUN)
        .expect("read")
        .expect("attached");
    assert_eq!(record.context_plan_digest, first_digest);
    world.close_store();
}

#[test]
fn a_plan_built_from_another_snapshot_cannot_attach() {
    // Wrong-source invariant: an attachment claiming a different frozen
    // source universe fails closed before anything durable happens. The
    // composite foreign key is the database-level backstop for the same rule.
    let mut world = world("t3a-wrong-source");
    let store = world.s();
    let (plan_digest, plan_json) = plan_claim(1);
    let other_snapshot = Digest::of(b"a-different-source-universe");

    let epoch = store.restore_generation().expect("generation");
    let err = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-1", &[21u8; 32], "run.context.attached"),
            &attachment(RUN, &other_snapshot, &plan_digest, &plan_json),
        )
        .expect_err("wrong-source attachment must fail closed");
    match err {
        StoreError::ContextSourceMismatch {
            ref run_id,
            ref frozen,
            ref proposed,
        } => {
            assert_eq!(run_id, RUN);
            assert_eq!(*frozen, world.snapshot_digest.to_string());
            assert_eq!(*proposed, other_snapshot.to_string());
        }
        other => panic!("expected ContextSourceMismatch, got {other:?}"),
    }
    assert!(store.run_context_plan(RUN).expect("read").is_none());

    // The database refuses the same swap even if the typed check were
    // bypassed: the composite FK needs the exact (run, snapshot) pair.
    let fk_refused = store.write(|writer| {
        writer.execute(
            "INSERT INTO run_context_plans
                 (run_id, context_source_snapshot_digest, context_plan_digest, attached_at)
             VALUES (?1, ?2, ?3, unixepoch())",
            &[
                RowValue::from(RUN),
                RowValue::Blob(other_snapshot.as_bytes().to_vec()),
                RowValue::Blob(plan_digest.as_bytes().to_vec()),
            ],
        )
    });
    assert!(
        matches!(fk_refused, Err(StoreError::Sqlite(_))),
        "the composite FK must refuse a mismatched pair: {fk_refused:?}"
    );
    world.close_store();
}

#[test]
fn an_existing_plan_digest_with_different_bytes_is_a_content_conflict() {
    // Content-addressing invariant: a digest names exactly one canonical
    // byte sequence. A row that reached durable state under a digest its own
    // bytes no longer produce — external tampering being the realistic route
    // — must be refused at attachment time instead of served as authority,
    // even when the incoming claim is internally consistent.
    let mut world = world("t3a-content-conflict");
    let store = world.s();
    let (plan_digest, plan_json) = plan_claim(1);
    let snapshot_digest = world.snapshot_digest;

    // Plant a corrupted row under the honest digest: same identity column,
    // bytes that hash somewhere else entirely.
    let corrupt_json = plan_json.replace("1", "2");
    assert_ne!(
        Digest::of(corrupt_json.as_bytes()),
        plan_digest,
        "the planted row must be genuinely divergent"
    );
    store
        .write(|writer| {
            writer.execute(
                "INSERT INTO context_plans
                     (digest, source_snapshot_digest, builder_version, canonical_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, unixepoch())",
                &[
                    crate::transaction::Value::Blob(plan_digest.as_bytes().to_vec()),
                    crate::transaction::Value::Blob(snapshot_digest.as_bytes().to_vec()),
                    crate::transaction::Value::from(CONTEXT_BUILDER_VERSION),
                    crate::transaction::Value::from(corrupt_json.as_str()),
                ],
            )
        })
        .expect("the corrupted fixture row applies");

    // The incoming claim is internally consistent (bytes hash to the
    // digest), so the earlier hash fence passes — the stored divergence is
    // what must stop it.
    let epoch = store.restore_generation().expect("generation");
    let err = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-2", &[22u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &plan_digest, &plan_json),
        )
        .expect_err("divergent stored bytes under one digest must fail closed");
    assert!(
        matches!(err, StoreError::ContentIdentityConflict { .. }),
        "expected ContentIdentityConflict, got {err:?}"
    );
    assert!(store.run_context_plan(RUN).expect("read").is_none());
    world.close_store();
}

#[test]
fn plan_bytes_must_hash_to_their_claimed_digest_before_anything_is_written() {
    // The attachment verifies the claim `bytes → digest` where the row is
    // written, so an inconsistent claim never becomes durable state.
    let mut world = world("t3a-hash-fence");
    let store = world.s();
    let (_real_digest, real_json) = plan_claim(1);
    let snapshot_digest = world.snapshot_digest;
    let claimed = Digest::of(b"not-these-bytes");

    let epoch = store.restore_generation().expect("generation");
    let err = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-1", &[21u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &claimed, &real_json),
        )
        .expect_err("bytes must produce their claimed digest");
    assert!(
        matches!(err, StoreError::ContentIdentityConflict { .. }),
        "expected ContentIdentityConflict, got {err:?}"
    );
    assert!(store.run_context_plan(RUN).expect("read").is_none());
    let orphan_plans = store
        .write(|writer| {
            writer.query_optional("SELECT COUNT(*) FROM context_plans", &[], |row| {
                row.get::<_, i64>(0)
            })
        })
        .expect("count plans");
    assert_eq!(orphan_plans, Some(0), "nothing was written");
    world.close_store();
}

#[test]
fn attaching_to_a_missing_run_fails_closed() {
    let mut world = world("t3a-missing-run");
    let store = world.s();
    let (plan_digest, plan_json) = plan_claim(1);
    let snapshot_digest = world.snapshot_digest;

    let epoch = store.restore_generation().expect("generation");
    let err = store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-1", &[21u8; 32], "run.context.attached"),
            &attachment("run-absent", &snapshot_digest, &plan_digest, &plan_json),
        )
        .expect_err("an unknown Run cannot be prepared");
    assert!(
        matches!(err, StoreError::RunNotFound { ref run_id } if run_id == "run-absent"),
        "expected RunNotFound, got {err:?}"
    );
    world.close_store();
}

#[test]
fn preparation_leaves_no_attempt_surface_behind() {
    // Lifecycle invariant: context readiness is not execution. No Attempt
    // family exists in this schema generation at all, and preparation creates
    // none of the later lifecycle state.
    let mut world = world("t3a-no-attempt");
    let store = world.s();
    let db_path = world._dir.path().join("pantheon.db");
    let (plan_digest, plan_json) = plan_claim(1);
    let snapshot_digest = world.snapshot_digest;

    let epoch = store.restore_generation().expect("generation");
    store
        .attach_run_context_plan(
            &command(epoch.as_str(), "t3a-1", &[21u8; 32], "run.context.attached"),
            &attachment(RUN, &snapshot_digest, &plan_digest, &plan_json),
        )
        .expect("attachment commits");

    world.close_store();
    let conn = rusqlite::Connection::open(db_path).expect("raw connection");
    for table in ["attempts", "attempt_status", "agent_control_sessions"] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists, "{table} must not exist before its behaviour does");
    }
    let phase: String = conn
        .query_row(
            "SELECT phase FROM run_status WHERE run_id = ?1",
            rusqlite::params![RUN],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        phase, "Active",
        "the Run stays Active with nothing to launch"
    );
}

#[test]
fn t3_refuses_a_snapshot_pairing_guidance_from_another_generation() {
    // Frozen-source invariant at commit time: the guidance digests on a
    // source snapshot must belong to the captured ConfigurationRevision's
    // immutable agents component. A snapshot pairing one revision's facts
    // with another generation's guidance cannot commit.
    let mut world = world("t3-guidance-fence");
    let store = world.s();

    // Build a second Goal/Task/Workspace so a second T3 can be attempted
    // against the same installation while the slot is free... it is not:
    // the first Run holds it. Instead, prove the fence directly by
    // re-committing the SAME intent with mutated guidance digests under a
    // stale scheduler revision — the guidance check runs before the slot
    // check, so the typed refusal is reachable.
    let agent = LogicalAgentVersion {
        name: "builder".to_string(),
        version: 1,
    };
    let guidance = stored_guidance(store, &world.active, &agent);
    let spec_digest = fixture::tasks_of(store, GOAL)[0].spec_digest;
    let binding_frozen = ExecutionBinding {
        task_id: TASK.to_string(),
        agent: agent.clone(),
        request_digest: Digest::of(b"request"),
        offer_digest: Digest::of(b"offer"),
        backend_id: "fake-local".to_string(),
        descriptor_revision: 3,
        descriptor_digest: Digest::of(b"descriptor"),
        execution_profile_digest: world.active.components.execution_profile,
        sandbox_profile_digest: Digest::of(b"sandbox-profile"),
        route_policy_digest: world.active.components.routing,
        configuration_activation_sequence: world.active.activation_sequence,
        configuration_content_digest: world.active.content_digest,
        component_digests: world.active.components,
    };
    let mut tampered = ContextSourceSnapshot {
        task_spec_digest: spec_digest,
        goal_id: GOAL.to_string(),
        goal_revision: 1,
        graph_revision: 1,
        agent,
        configuration_activation_sequence: world.active.activation_sequence,
        context_policy_digest: world.active.components.context_policy,
        agent_soul_digest: guidance_digest(&guidance.soul),
        agent_behavior_digest: guidance_digest(&guidance.behavior),
        workspace_id: WORKSPACE.to_string(),
        workspace_resolved_base: "a".repeat(40),
    };
    tampered.agent_soul_digest = Digest::of(b"guidance-from-somewhere-else");
    let binding_digest = binding_frozen.digest();
    let snapshot_digest = tampered.digest();

    let snap = store.scheduling_snapshot().expect("snapshot");
    // The Task went Active at T3, so no dispatchable candidate exists; drive
    // the transaction with the recorded expectations from the fixture world
    // instead. The guidance fence fires before Task currency is consulted.
    let intent = RunIntent {
        run_id: "run-2",
        task_id: TASK,
        goal_id: GOAL,
        expected_task_revision: Revision::new(2),
        expected_goal_row_revision: Revision::new(1),
        expected_goal_current_revision: 1,
        expected_graph_revision: 1,
        expected_workspace_revision: Revision::new(2),
        expected_scheduler_revision: snap.state.revision,
        expected_goal_fairness_revision: None,
        expected_task_scheduling_revision: Revision::new(1),
        configuration_activation_sequence: world.active.activation_sequence,
        binding_digest: &binding_digest,
        binding: &binding_frozen,
        snapshot_digest: &snapshot_digest,
        snapshot: &tampered,
    };
    let epoch = store.restore_generation().expect("generation");
    let err = store
        .commit_run_intent(
            &command(epoch.as_str(), "cmd-t3-bad", &[12u8; 32], "run.committed"),
            &intent,
        )
        .expect_err("mismatched frozen guidance must fail closed");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref detail)
            if detail.contains("guidance")),
        "expected the guidance currency fence, got {err:?}"
    );
    world.close_store();
}
