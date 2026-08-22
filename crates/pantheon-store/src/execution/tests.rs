//! Evidence for the durable half of Issue #31: T4 Attempt creation, T4a
//! pre-contact rekey, T4b launch-contact marker, terminalization, retry and
//! Run conclusion — what each authoritative transaction does to stored state.
//!
//! The engine-level orchestration (preparation gates, crash windows,
//! restart inventory over a fake backend) is proven in `pantheon-engine` and
//! `pantheond`. What only a test at this altitude can establish is that the
//! transactions themselves carry the fences: atomicity under injected
//! failure, the one-nonterminal-Attempt rule at the database layer, the
//! rekey boundary's monotonicity across contact and restore generations, and
//! replay semantics that reconcile rather than duplicate lineage.

use pantheon_core::attempt::{LaunchContactState, Observation};
use pantheon_core::config::Digest;
use pantheon_core::config::canonical::Value;
use pantheon_core::context::{CONTEXT_BUILDER_VERSION, guidance_digest};
use pantheon_core::execution::LogicalAgentVersion;
use pantheon_core::scheduling::{ContextSourceSnapshot, ExecutionBinding};
use rusqlite::Connection;

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::execution::{AttemptCreated, AttemptCreation, ObservationUpdate, RunInventoryEntry};
use crate::planning::tests as fixture;
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;

const TASK: &str = "task-1";
const GOAL: &str = "goal-1";
const WORKSPACE: &str = "ws-1";
const RUN: &str = "run-1";

fn command<'a>(epoch: &'a str, id: &'a str, hash: &'a [u8; 32], event: &'a str) -> Command<'a> {
    fixture::command(epoch, id, hash, event)
}

/// A committed Active Run. `attach_plan` decides whether the one-time
/// ContextPlan relation exists, i.e. whether the Run reached LaunchReady.
struct World {
    _dir: TempDir,
    db_path: std::path::PathBuf,
    store: Store,
}

impl World {
    fn store(&self) -> &Store {
        &self.store
    }
}

fn world(label: &str, attach_plan: bool) -> World {
    let (dir, store, sequence) = fixture::store_with_configuration(label);
    let db_path = dir.path().join("pantheon.db");
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
        source_path: "/tmp/pantheon-execution-test-source",
        requested_base: &requested,
        resolved_base: &resolved,
    };
    store
        .open_workspace(
            &command(
                epoch.as_str(),
                "cmd-ws-open",
                &[7u8; 32],
                "workspace.opened",
            ),
            WORKSPACE,
            &binding,
        )
        .expect("open workspace");
    store
        .begin_workspace_materialization(
            &command(
                epoch.as_str(),
                "cmd-ws-begin",
                &[8u8; 32],
                "workspace.materializing",
            ),
            WORKSPACE,
            Revision::new(1),
        )
        .expect("begin materialization");
    store
        .complete_workspace_materialization(
            &command(epoch.as_str(), "cmd-ws-done", &[9u8; 32], "workspace.ready"),
            WORKSPACE,
            Revision::new(2),
            &resolved,
        )
        .expect("complete materialization");

    let active_pointer = store.configuration_pointer().expect("pointer");
    let active = active_pointer.active.clone().expect("active");

    let agent = LogicalAgentVersion {
        name: "builder".to_string(),
        version: 1,
    };
    let agents_json = store
        .revision_agents_component_json(active.activation_sequence)
        .expect("read agents component")
        .expect("agents component stored");
    let value = pantheon_core::config::parse::parse(&agents_json).expect("fixture component");
    let guidance =
        pantheon_core::context::frozen_agent_guidance(&value, &agent).expect("fixture guidance");
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
    let intent = crate::scheduling::RunIntent {
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

    if attach_plan {
        // Attach the one-time ContextPlan: the final LaunchReady gate.
        let plan_canonical = Value::object([
            ("builder", Value::string(CONTEXT_BUILDER_VERSION)),
            ("fixture", Value::string(label)),
        ])
        .to_canonical_bytes();
        let plan_digest = Digest::of(&plan_canonical);
        let plan_json = String::from_utf8(plan_canonical).expect("utf-8");
        store
            .attach_run_context_plan(
                &command(
                    epoch.as_str(),
                    "cmd-t3a",
                    &[21u8; 32],
                    "run.context.attached",
                ),
                &crate::context::ContextPlanAttachment {
                    run_id: RUN,
                    source_snapshot_digest: &snapshot_digest,
                    plan_digest: &plan_digest,
                    builder_version: CONTEXT_BUILDER_VERSION,
                    plan_canonical_json: &plan_json,
                },
            )
            .expect("attachment commits");
    }

    World {
        _dir: dir,
        db_path,
        store,
    }
}

/// The Run's status revision as the controller would read it before acting.
fn run_status_revision(store: &Store) -> Revision {
    let view = store
        .run_execution_view(RUN)
        .expect("read execution view")
        .expect("the Run exists");
    assert_eq!(view.phase, "Active");
    view.revision
}

/// A 64-character launch key whose only varying content is `seed`.
fn launch_key(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

/// One T4/T8 invocation with fully explicit inputs.
#[allow(clippy::too_many_arguments)]
fn t4<'a>(
    world: &'a World,
    command_id: &'a str,
    request_hash: [u8; 32],
    attempt_id: &'a str,
    key_seed: u8,
    session_id: &'a str,
    verifier: &'a [u8; 32],
) -> Result<Committed<AttemptCreated>, StoreError> {
    let revision = run_status_revision(world.store());
    let epoch = world.store().restore_generation().expect("generation");
    let key = launch_key(key_seed);
    world.store.create_attempt(
        &command(
            epoch.as_str(),
            command_id,
            &request_hash,
            "run.attempt.created",
        ),
        &AttemptCreation {
            run_id: RUN,
            attempt_id,
            launch_key: &key,
            session_id,
            credential_verifier: verifier,
            expected_run_status_revision: revision,
        },
    )
}

/// The canonical first-Attempt call most tests start from.
fn create_first_attempt(world: &World) -> Committed<AttemptCreated> {
    match t4(
        world,
        "t4-run1-ord1",
        [0x31u8; 32],
        "attempt-1",
        0xAA,
        "acs-1",
        &[7u8; 32],
    )
    .expect("T4 commits")
    {
        executed @ Committed::Executed { .. } => executed,
        other => panic!("a fresh T4 executes, got {other:?}"),
    }
}

/// The current nonterminal lineage, asserted present.
fn lineage(world: &World) -> crate::execution::AttemptLineageView {
    world
        .store()
        .run_execution_view(RUN)
        .expect("read view")
        .expect("Run exists")
        .attempt
        .expect("a nonterminal Attempt exists")
}

/// A separate raw connection for schema-level assertions and tampering that
/// simulates what a disaster-restore fence does to durable state.
fn raw(world: &World) -> Connection {
    Connection::open(&world.db_path).expect("raw connection")
}

fn event_total(world: &World) -> i64 {
    raw(world)
        .query_row("SELECT COUNT(*) FROM event_journal", [], |row| row.get(0))
        .expect("count events")
}

fn attempt_rows(world: &World) -> i64 {
    raw(world)
        .query_row("SELECT COUNT(*) FROM attempts", [], |row| row.get(0))
        .expect("count attempts")
}

#[test]
fn t4_commits_the_whole_lineage_in_one_transaction() {
    let world = world("t4-atomic", true);
    let events_before = event_total(&world);

    let committed = create_first_attempt(&world);
    let Committed::Executed { value, .. } = committed else {
        panic!("unreachable: create_first_attempt asserts Executed");
    };
    assert_eq!(value.attempt_id, "attempt-1");
    assert_eq!(value.ordinal, 1);
    assert_eq!(value.launch_key, launch_key(0xAA));
    assert_eq!(
        value.restore_generation,
        world
            .store()
            .restore_generation()
            .expect("generation")
            .as_str()
    );

    let view = world
        .store()
        .run_execution_view(RUN)
        .expect("read")
        .unwrap();
    assert_eq!(view.current_attempt_id.as_deref(), Some("attempt-1"));
    assert!(
        view.context_plan_digest.is_some(),
        "LaunchReady evidence intact"
    );
    let live = view.attempt.expect("current lineage");
    assert_eq!(live.attempt.ordinal, 1);
    assert!(!live.terminal);
    assert_eq!(live.observed_execution, Observation::Absent);
    assert_eq!(live.launch_contact_state, LaunchContactState::NotContacted);
    assert_eq!(live.session.id, "acs-1");
    assert_eq!(live.session.credential_revision, 1);
    assert_eq!(live.session.restore_generation, value.restore_generation);

    // The stored credential material is exactly the verifier — 32 bytes,
    // derived from the bearer upstream. Nothing else was persisted.
    let stored_hash: Vec<u8> = raw(&world)
        .query_row(
            "SELECT credential_hash FROM agent_control_sessions WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("session row");
    assert_eq!(stored_hash, vec![7u8; 32]);

    // Exactly one Event recorded the creation.
    assert_eq!(event_total(&world), events_before + 1);
}

#[test]
fn a_failed_t4_leaves_no_partial_lineage() {
    // The failure is injected *late*: attempt and status rows insert first,
    // then the session INSERT collides with an existing primary key. A T4
    // that left the earlier inserts behind would fail exactly here.
    let world = world("t4-rollback", true);
    create_first_attempt(&world);
    let revision_after_first = run_status_revision(world.store());
    let events_after_first = event_total(&world);

    // Definitively end lineage 1 so the one-nonterminal check passes; the
    // collision below is what must abort this transaction.
    world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-R", Revision::new(1), 1)
        .expect("contact marker commits");
    let _ = world
        .store()
        .record_execution_observation(
            "attempt-1",
            Revision::new(2),
            ObservationUpdate::Terminal(Observation::Exited),
        )
        .expect("terminal observation commits");

    let error = t4(
        &world,
        "t4-second",
        [0x42u8; 32],
        "attempt-2",
        0xDD,    // fresh LaunchKey
        "acs-1", // collides with lineage 1's session primary key
        &[9u8; 32],
    )
    .expect_err("a duplicate session identity fails the whole transaction");
    assert!(
        matches!(error, StoreError::Sqlite(_)),
        "the session primary key refuses duplication: {error}"
    );

    // Nothing partial survived: no second Attempt row, no orphaned
    // attempt_status, unchanged pointer and Run revision, no Event. The
    // pointer still names terminal lineage 1; the view reports no *live*
    // lineage.
    assert_eq!(attempt_rows(&world), 1);
    let view = world
        .store()
        .run_execution_view(RUN)
        .expect("read")
        .unwrap();
    assert_eq!(view.current_attempt_id.as_deref(), Some("attempt-1"));
    assert_eq!(view.attempt, None);
    assert_eq!(view.revision, revision_after_first);
    let statuses: i64 = raw(&world)
        .query_row(
            "SELECT COUNT(*) FROM attempt_status WHERE attempt_id = 'attempt-2'",
            [],
            |r| r.get(0),
        )
        .expect("count stray statuses");
    assert_eq!(statuses, 0);
    // Two internal Events (contact marker, terminalization) landed since the
    // first lineage committed; the failed T4 appended none.
    assert_eq!(event_total(&world), events_after_first + 2);

    // And the Run still accepts a well-formed retry afterwards.
    let retry = t4(
        &world,
        "t4-second-retry",
        [0x43u8; 32],
        "attempt-2",
        0xEE,
        "acs-2",
        &[9u8; 32],
    )
    .expect("the corrected retry commits");
    assert!(
        retry.was_executed(),
        "the retry is genuinely new work, not a replay"
    );
}

#[test]
fn attempt_creation_requires_the_attached_context_plan() {
    let world = world("t4-no-plan", false);
    let error = t4(
        &world,
        "t4-unready",
        [0x51u8; 32],
        "attempt-1",
        0xBB,
        "acs-1",
        &[3u8; 32],
    )
    .expect_err("an Attempt before LaunchReady is refused");
    assert!(
        matches!(error, StoreError::AttemptNotLaunchReady { .. }),
        "typed refusal, got {error}"
    );
    assert_eq!(attempt_rows(&world), 0, "nothing was written");
    let view = world
        .store()
        .run_execution_view(RUN)
        .expect("read")
        .unwrap();
    assert_eq!(view.current_attempt_id, None);
    assert!(view.context_plan_digest.is_none());
}

#[test]
fn one_nonterminal_attempt_per_run_is_enforced_by_controller_and_database() {
    let world = world("t4-one-live", true);
    create_first_attempt(&world);

    // Controller layer: a second creation while attempt-1 is nonterminal.
    let error = t4(
        &world,
        "t4-run1-ord2",
        [0x52u8; 32],
        "attempt-2",
        0xCC,
        "acs-2",
        &[4u8; 32],
    )
    .expect_err("a second concurrent lineage is refused");
    assert!(matches!(error, StoreError::AttemptNotLaunchReady { .. }));

    // Database layer: even a caller that bypasses every controller check
    // cannot persist a second nonterminal Attempt for the Run.
    let conn = raw(&world);
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         INSERT INTO attempts (id, run_id, ordinal, launch_key, created_at)
              VALUES ('attempt-sneaky', 'run-1', 2, 'X', unixepoch());
         INSERT INTO attempt_status
              (attempt_id, run_id, observed_execution, terminal, revision,
               launch_contact_state, updated_at)
              VALUES ('attempt-sneaky', 'run-1', 'ABSENT', 0, 1,
                      'NOT_CONTACTED', unixepoch());
         ROLLBACK;",
    )
    .expect_err("the partial unique index rejects the second nonterminal row");
    assert_eq!(attempt_rows(&world), 1);
}

#[test]
fn command_replay_reconciles_the_same_committed_result() {
    let world = world("t4-replay", true);
    create_first_attempt(&world);

    // A retry after a lost response derives the same command identity. Even
    // regenerated random material under the same identity replays rather
    // than minting a second lineage.
    let replay = t4(
        &world,
        "t4-run1-ord1",
        [0x31u8; 32],
        "attempt-regenerated",
        0xFF, // different launch key than the committed one
        "acs-regenerated",
        &[8u8; 32],
    )
    .expect("the retry reconciles");
    assert!(
        matches!(replay, Committed::Replayed { .. }),
        "the same identity and hash reconcile as a replay"
    );

    assert_eq!(attempt_rows(&world), 1);
    let live = lineage(&world);
    assert_eq!(live.attempt.id, "attempt-1");
    assert_eq!(live.attempt.launch_key, launch_key(0xAA));
    assert_eq!(live.session.credential_revision, 1);
}

#[test]
fn conflicting_command_reuse_fails_closed() {
    let world = world("t4-conflict", true);
    create_first_attempt(&world);

    // Same command identity, different request hash: a caller defect, not a
    // retry. Nothing executes and nothing changes.
    let error = t4(
        &world,
        "t4-run1-ord1",
        [0x99u8; 32], // different hash under the same identity
        "attempt-1",
        0xAA,
        "acs-1",
        &[7u8; 32],
    )
    .expect_err("identity reuse with a different request fails closed");
    assert!(matches!(error, StoreError::CommandConflict { .. }));
    assert_eq!(attempt_rows(&world), 1);
}

#[test]
fn retry_after_terminal_attempt_creates_a_new_distinct_lineage() {
    let world = world("t4-retry", true);
    create_first_attempt(&world);

    // Cross the contact boundary, then prove the lineage definitively ended.
    world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-A", Revision::new(1), 1)
        .expect("contact marker commits");
    let _ = world
        .store()
        .record_execution_observation(
            "attempt-1",
            Revision::new(2),
            ObservationUpdate::Terminal(Observation::Exited),
        )
        .expect("terminal observation commits");

    // Recovery Policy's RETRY_ATTEMPT creates a new lineage under the same
    // Run: new Attempt id, new LaunchKey, new session, next ordinal.
    let second = t4(
        &world,
        "t4-run1-ord2",
        [0x61u8; 32],
        "attempt-2",
        0xDD,
        "acs-2",
        &[5u8; 32],
    )
    .expect("the retry commits");
    let Committed::Executed { value, .. } = second else {
        panic!("a retry after terminal is genuinely new work");
    };
    assert_eq!(
        value.ordinal, 2,
        "the ordinal is strictly Run-local history"
    );

    let view = world
        .store()
        .run_execution_view(RUN)
        .expect("read")
        .unwrap();
    assert_eq!(view.current_attempt_id.as_deref(), Some("attempt-2"));
    let live = view.attempt.expect("the new lineage is current");
    assert_eq!(live.attempt.launch_key, launch_key(0xDD));
    assert_eq!(live.session.id, "acs-2");
    assert_ne!(
        live.session.id, "acs-1",
        "a fresh Attempt never inherits the prior session"
    );

    // The old lineage remains history, terminal with its own frozen facts.
    let (old_terminal, old_observed): (i64, String) = raw(&world)
        .query_row(
            "SELECT terminal, observed_execution FROM attempt_status
             WHERE attempt_id = 'attempt-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("old status row");
    assert_eq!(old_terminal, 1);
    assert_eq!(old_observed, "EXITED");
}

#[test]
fn rekey_rotates_only_the_session_credential_while_not_contacted() {
    let world = world("t4a-ok", true);
    create_first_attempt(&world);
    let generation = world.store().restore_generation().expect("generation");
    let events_before = event_total(&world);

    let new_verifier = [0xE1u8; 32];
    let revision = world
        .store()
        .rekey_agent_control_session("attempt-1", &new_verifier, 1)
        .expect("the pre-contact rekey rotates in place");
    assert_eq!(revision, 2);

    let live = lineage(&world);
    assert_eq!(live.session.credential_revision, 2);
    assert_eq!(live.session.restore_generation, generation.as_str());
    assert_eq!(
        live.attempt.launch_key,
        launch_key(0xAA),
        "LaunchKey immutable"
    );
    assert_eq!(live.attempt.id, "attempt-1", "same Attempt identity");
    assert_eq!(live.launch_contact_state, LaunchContactState::NotContacted);

    // Non-secret provenance Event appended; the verifier replaced.
    assert_eq!(event_total(&world), events_before + 1);
    let stored: Vec<u8> = raw(&world)
        .query_row(
            "SELECT credential_hash FROM agent_control_sessions WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("session row");
    assert_eq!(stored, vec![0xE1u8; 32]);
    let rekeyed_at: i64 = raw(&world)
        .query_row(
            "SELECT credential_rekeyed_at IS NOT NULL FROM agent_control_sessions
             WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("provenance stamp");
    assert_eq!(rekeyed_at, 1);
}

#[test]
fn rekey_is_frozen_once_contact_may_have_occurred() {
    let world = world("t4a-frozen", true);
    create_first_attempt(&world);
    world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-B", Revision::new(1), 1)
        .expect("contact marker commits");

    let error = world
        .store()
        .rekey_agent_control_session("attempt-1", &[0xE2u8; 32], 1)
        .expect_err("a contacted session never rekeys");
    assert!(
        matches!(error, StoreError::AgentControlRekeyForbidden { .. }),
        "typed freeze refusal, got {error}"
    );

    let stored: Vec<u8> = raw(&world)
        .query_row(
            "SELECT credential_hash FROM agent_control_sessions WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("session row");
    assert_eq!(stored, vec![7u8; 32], "the verifier is untouched");
    let revision: i64 = raw(&world)
        .query_row(
            "SELECT credential_revision FROM agent_control_sessions
             WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("revision");
    assert_eq!(revision, 1, "the credential revision is frozen");
}

#[test]
fn rekey_refuses_an_old_generation_session() {
    let world = world("t4a-generation", true);
    create_first_attempt(&world);

    // Simulate the disaster-restore authority fence: the installation's
    // RestoreGeneration rotates underneath the existing session.
    raw(&world)
        .execute(
            "UPDATE system_state SET restore_generation = ?1 WHERE id = 1",
            rusqlite::params![format!("{:032x}", 0xABCDE)],
        )
        .expect("rotate generation");

    let error = world
        .store()
        .rekey_agent_control_session("attempt-1", &[0xE3u8; 32], 1)
        .expect_err("an old-generation session cannot promote itself by rekeying");
    assert!(matches!(
        error,
        StoreError::AgentControlRekeyForbidden { .. }
    ));

    let bound_generation: String = raw(&world)
        .query_row(
            "SELECT restore_generation FROM agent_control_sessions
             WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("session generation");
    assert_ne!(
        bound_generation,
        format!("{:032x}", 0xABCDE),
        "the session's own generation is never rewritten"
    );
}

#[test]
fn rekey_refuses_a_stale_expected_revision() {
    let world = world("t4a-stale", true);
    create_first_attempt(&world);
    world
        .store()
        .rekey_agent_control_session("attempt-1", &[0xE4u8; 32], 1)
        .expect("first rekey");

    // A second controller still believes revision 1 is current.
    let error = world
        .store()
        .rekey_agent_control_session("attempt-1", &[0xE5u8; 32], 1)
        .expect_err("stale expectation loses deterministically");
    assert!(matches!(error, StoreError::RevisionConflict { .. }));
}

#[test]
fn the_contact_marker_commits_monotonically_and_binds_the_current_revision() {
    let world = world("t4b-ordering", true);
    create_first_attempt(&world);
    let events_before = event_total(&world);

    // Wrong credential revision: the launch package does not match the
    // session's current verifier, so the boundary refuses.
    let error = world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-C", Revision::new(1), 2)
        .expect_err("T4b binds the exact current credential revision");
    assert!(matches!(
        error,
        StoreError::LaunchContactStaleAuthority { .. }
    ));

    // Correct authority: the boundary commits durably, before any contact.
    world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-C", Revision::new(1), 1)
        .expect("contact marker commits");
    let (state, initiated, epoch): (String, i64, String) = raw(&world)
        .query_row(
            "SELECT launch_contact_state, launch_contact_initiated_at IS NOT NULL,
                    launch_contact_epoch
             FROM attempt_status WHERE attempt_id = 'attempt-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("status row");
    assert_eq!(state, "CONTACT_MAY_HAVE_OCCURRED");
    assert_eq!(initiated, 1);
    assert_eq!(epoch, "epoch-C");
    assert_eq!(event_total(&world), events_before + 1);

    // Lost-response retry: finding the boundary already crossed reconciles.
    world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-D", Revision::new(2), 1)
        .expect("the retry reconciles without duplicating");
    let provenance: String = raw(&world)
        .query_row(
            "SELECT launch_contact_epoch FROM attempt_status
             WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("status row");
    assert_eq!(
        provenance, "epoch-C",
        "the first crossing owns the provenance"
    );
}

#[test]
fn terminalization_requires_durable_contact_and_revokes_the_session() {
    let world = world("terminal-fence", true);
    create_first_attempt(&world);

    // A lineage Pantheon provably never launched cannot have ended.
    let error = world
        .store()
        .record_execution_observation(
            "attempt-1",
            Revision::new(1),
            ObservationUpdate::Terminal(Observation::Exited),
        )
        .expect_err("terminal without contact is impossible");
    assert!(matches!(error, StoreError::InvariantViolated(_)));

    // Contact happens; observations then land factually.
    world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-E", Revision::new(1), 1)
        .expect("contact marker commits");
    let _ = world
        .store()
        .record_execution_observation(
            "attempt-1",
            Revision::new(2),
            ObservationUpdate::Observe(Observation::Running),
        )
        .expect("running observation commits");

    let live = lineage(&world);
    assert_eq!(live.observed_execution, Observation::Running);
    assert!(!live.terminal);

    // Definitive end: terminal row, finished_at, revoked session, one Event.
    let events_before = event_total(&world);
    let _ = world
        .store()
        .record_execution_observation(
            "attempt-1",
            Revision::new(3),
            ObservationUpdate::Terminal(Observation::Exited),
        )
        .expect("terminal observation commits");
    assert_eq!(event_total(&world), events_before + 1);

    let (terminal, finished): (i64, i64) = raw(&world)
        .query_row(
            "SELECT terminal, finished_at IS NOT NULL FROM attempt_status
             WHERE attempt_id = 'attempt-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("status row");
    assert_eq!(terminal, 1);
    assert_eq!(finished, 1);
    let session: String = raw(&world)
        .query_row(
            "SELECT state FROM agent_control_sessions WHERE attempt_id = 'attempt-1'",
            [],
            |row| row.get(0),
        )
        .expect("session row");
    assert_eq!(session, "REVOKED", "worker authority dies with the Attempt");
}

#[test]
fn unknown_observation_keeps_the_attempt_nonterminal_and_unreplaced() {
    let world = world("unknown-fence", true);
    create_first_attempt(&world);
    world
        .store()
        .mark_launch_contact(RUN, "attempt-1", "epoch-F", Revision::new(1), 1)
        .expect("contact marker commits");

    let _ = world
        .store()
        .record_execution_observation(
            "attempt-1",
            Revision::new(2),
            ObservationUpdate::Observe(Observation::Unknown),
        )
        .expect("UNKNOWN records factually");

    let live = lineage(&world);
    assert_eq!(live.observed_execution, Observation::Unknown);
    assert!(!live.terminal, "UNKNOWN never terminalizes");
    assert_eq!(
        world
            .store()
            .nonterminal_run_inventory()
            .expect("inventory")
            .len(),
        1,
        "the Run stays responsible"
    );
    assert_eq!(attempt_rows(&world), 1, "no replacement lineage exists");

    // And the controller layer agrees: a replacement Attempt is refused.
    let error = t4(
        &world,
        "t4-run1-replace",
        [0x71u8; 32],
        "attempt-2",
        0xEE,
        "acs-2",
        &[6u8; 32],
    )
    .expect_err("UNKNOWN authorizes no replacement");
    assert!(matches!(error, StoreError::AttemptNotLaunchReady { .. }));
}

#[test]
fn concluding_the_run_records_why_and_releases_the_slot() {
    let world = world("conclude", true);
    let revision_before = run_status_revision(world.store());

    world
        .store()
        .conclude_run(RUN, "Failed", revision_before)
        .expect("the Run concludes");

    let (phase, target, slot): (String, Option<String>, Option<String>) = raw(&world)
        .query_row(
            "SELECT phase, terminal_target, active_slot FROM run_status WHERE run_id = ?1",
            rusqlite::params![RUN],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("status row");
    assert_eq!(phase, "Failed");
    assert_eq!(target.as_deref(), Some("Failed"), "the why is durable");
    assert_eq!(slot, None, "the single global slot is released");

    assert!(
        world
            .store()
            .nonterminal_run_inventory()
            .expect("inventory")
            .is_empty(),
        "a concluded Run leaves the nonterminal inventory"
    );

    // Stale conclusions lose deterministically.
    let error = world
        .store()
        .conclude_run(RUN, "Failed", revision_before)
        .expect_err("the revision moved");
    assert!(matches!(error, StoreError::RevisionConflict { .. }));
}

#[test]
fn view_and_inventory_agree_about_the_same_world() {
    let world = world("inventory", true);
    create_first_attempt(&world);

    let entries: Vec<RunInventoryEntry> = world
        .store()
        .nonterminal_run_inventory()
        .expect("inventory");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].run_id, RUN);
    assert_eq!(entries[0].phase, "Active");
    assert_eq!(entries[0].current_attempt_id.as_deref(), Some("attempt-1"));

    let view = world
        .store()
        .run_execution_view(RUN)
        .expect("read")
        .unwrap();
    assert_eq!(view.run_id, RUN);
    assert_eq!(view.phase, entries[0].phase);
    assert_eq!(view.current_attempt_id, entries[0].current_attempt_id);
    assert!(view.attempt.is_some());

    // A Run nobody knows disappears from both consistently.
    assert!(
        world
            .store()
            .run_execution_view("run-absent")
            .expect("read")
            .is_none()
    );
}
