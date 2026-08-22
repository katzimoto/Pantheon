//! Executable evidence for the Agent Control persistence boundary: the
//! authentication fences, the `(attempt, request)` idempotency ledger, and
//! the T6 Candidate submission transaction.
//!
//! The fixture composes the same authoritative steps the controllers drive —
//! configuration, Goal, Task, Workspace, T3, T4 — and then exercises the new
//! surface directly against durable state. Refusals are asserted to write
//! nothing; races are established by explicit commit ordering, never sleeps.

use pantheon_core::candidate::CandidateResult;
use pantheon_core::config::Digest;
use rusqlite::Connection;

use crate::agent_control::{
    AgentCredential, AgentOperation, AgentRequestOpened, AgentRequestState, CandidateSubmission,
    SubmissionOutcome,
};
use crate::artifacts::{ProducerProvenance, SealedChangeset};
use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::seal::SealAuthority;
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;

const TASK: &str = "task-1";
const GOAL: &str = "goal-1";
const WORKSPACE: &str = "ws-1";
const RUN: &str = "run-1";
const ATTEMPT: &str = "attempt-1";
const SLOT: &str = "changeset";

fn command<'a>(epoch: &'a str, id: &'a str, hash: &'a [u8; 32], event: &'a str) -> Command<'a> {
    crate::planning::tests::command(epoch, id, hash, event)
}

/// A deterministic verifier standing in for SHA-256(bearer): the store only
/// ever sees verifier bytes.
fn bearer_verifier(seed: u8) -> [u8; 32] {
    *Digest::of(&[seed; 64]).as_bytes()
}

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

/// One committed Active Run with its ContextPlan attached, plus one live
/// Attempt whose session carries `bearer_verifier(seed)`.
fn world(label: &str, seed: u8) -> World {
    let (dir, store, sequence) = crate::planning::tests::store_with_configuration(label);
    let db_path = dir.path().join("pantheon.db");
    let epoch = store.restore_generation().expect("generation");
    crate::planning::tests::create_goal(&store, GOAL, "cmd-goal");
    let op = crate::planning::tests::plan_and_record(&store, GOAL, sequence, "op-1");
    let registry_digest = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .as_ref()
        .expect("active")
        .components
        .evaluator_registry;
    let plan = crate::planning::tests::validated(sequence, registry_digest, "unit-v1");
    crate::planning::tests::materialize(&store, &op, TASK, &plan, "cmd-materialize")
        .expect("materializes");

    let requested = pantheon_core::workspace::RequestedBase::parse("main").expect("ref");
    let resolved = pantheon_core::workspace::ResolvedBase::parse(&"a".repeat(40)).expect("base");
    let binding = crate::workspace::WorkspaceBinding {
        task_id: TASK,
        repository: "repo://project",
        source_path: "/tmp/pantheon-agent-control-test-source",
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

    // Freeze Binding + snapshot + Run + Task activation in T3, exactly as the
    // scheduler would.
    use pantheon_core::context::{CONTEXT_BUILDER_VERSION, guidance_digest};
    use pantheon_core::execution::LogicalAgentVersion;
    use pantheon_core::scheduling::{ContextSourceSnapshot, ExecutionBinding};

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
    let spec_digest = crate::planning::tests::tasks_of(&store, GOAL)[0].spec_digest;

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

    let mut world = World {
        _dir: dir,
        db_path,
        store,
    };
    launch_attempt(&mut world, ATTEMPT, "acs-1", seed);
    world
}

use pantheon_core::config::canonical::Value;

fn run_status_revision(store: &Store) -> Revision {
    let view = store
        .run_execution_view(RUN)
        .expect("read execution view")
        .expect("the Run exists");
    assert_eq!(view.phase, "Active");
    view.revision
}

fn task_revision(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT revision FROM tasks WHERE id = ?1",
        rusqlite::params![TASK],
        |row| row.get(0),
    )
    .expect("task row")
}

/// T4 with fully explicit inputs, so tests own the verifier bytes.
fn launch_attempt(world: &mut World, attempt_id: &str, session_id: &str, seed: u8) {
    let epoch = world.store.restore_generation().expect("generation");
    let revision = run_status_revision(world.store());
    world
        .store
        .create_attempt(
            &command(
                epoch.as_str(),
                &format!("cmd-t4-{attempt_id}"),
                &[31u8; 32],
                "run.attempt.created",
            ),
            &crate::execution::AttemptCreation {
                run_id: RUN,
                attempt_id,
                launch_key: &format!("{seed:02x}").repeat(32),
                session_id,
                credential_verifier: &bearer_verifier(seed),
                expected_run_status_revision: revision,
            },
        )
        .expect("T4 commits");
}

/// Crosses T4b for the world's Attempt, so terminalization is legal later.
fn contact_attempt(world: &World) {
    let view = world
        .store()
        .run_execution_view(RUN)
        .expect("view")
        .expect("run exists");
    let lineage = view.attempt.expect("live lineage");
    world
        .store()
        .mark_launch_contact(
            RUN,
            &lineage.attempt.id,
            "test-incarnation",
            lineage.status_revision,
            lineage.session.credential_revision,
        )
        .expect("T4b commits");
}

fn credential<'a>(verifier: &'a [u8; 32]) -> AgentCredential<'a> {
    AgentCredential {
        attempt_id: ATTEMPT,
        verifier,
    }
}

/// Seals a minimal-but-real empty changeset through the authoritative
/// publication path, carrying this Attempt as producer. Returns the Artifact
/// digest the submission will reference.
fn seal_for_submission(world: &World, slot: &str, artifact_seed: u8) -> Digest {
    let store = world.store();
    let epoch = store.restore_generation().expect("generation");
    let ws = store
        .workspace_record(WORKSPACE)
        .expect("workspace readable")
        .expect("workspace exists");
    let authority = SealAuthority {
        run_id: RUN.to_string(),
        expected_run_revision: run_status_revision(store),
    };
    store
        .freeze_workspace(
            &command(
                epoch.as_str(),
                &format!("cmd-freeze-{artifact_seed}"),
                &[41u8; 32],
                "workspace.frozen",
            ),
            &authority,
            TASK,
            slot,
            WORKSPACE,
            ws.revision,
        )
        .expect("freeze commits");

    // Two distinct slots must seal distinct content, or the second freeze
    // would converge on the first artifact's identity.
    let manifest =
        format!("{{\"entries\":[],\"schemaVersion\":1,\"variant\":\"{artifact_seed}\"}}");
    let artifact_digest = Digest::of(manifest.as_bytes());
    let outcome = store
        .commit_changeset_seal(
            &command(
                epoch.as_str(),
                &format!("cmd-seal-{artifact_seed}"),
                &[42u8; 32],
                "workspace.sealed",
            ),
            &SealedChangeset {
                workspace_id: WORKSPACE,
                task_id: TASK,
                fence_revision: Revision::new(ws.revision.get() + 1),
                authority: &authority,
                output_slot: slot,
                repository: "repo://project",
                resolved_base: ws.resolved_base.as_str(),
                revision_state_digest: Digest::of(b"state"),
                revision_state_json: "{\"entries\":[],\"schemaVersion\":1}",
                artifact_digest,
                artifact_json: &manifest,
                members: Vec::new(),
                producer: Some(ProducerProvenance {
                    attempt_id: ATTEMPT,
                }),
            },
        )
        .expect("publication commits");
    assert!(!matches!(outcome, Committed::Replayed { .. }));
    artifact_digest
}

fn submission<'a>(
    verifier: &'a [u8; 32],
    request_id: &'a str,
    request_hash: &'a [u8; 32],
    candidate: &'a CandidateResult,
    expected_task_revision: Revision,
) -> CandidateSubmission<'a> {
    CandidateSubmission {
        credential: credential(verifier),
        request_id,
        request_hash,
        candidate,
        expected_task_revision,
    }
}

fn submit(
    world: &World,
    verifier: &[u8; 32],
    request_id: &str,
    request_hash: [u8; 32],
    outputs: &[(String, Digest)],
) -> Result<SubmissionOutcome, StoreError> {
    let conn = Connection::open(&world.db_path).expect("raw conn");
    let revision = task_revision(&conn);
    let candidate =
        CandidateResult::new(TASK, RUN, outputs.iter().cloned()).expect("valid mapping");
    world.store().submit_candidate(&submission(
        verifier,
        request_id,
        &request_hash,
        &candidate,
        Revision::new(revision),
    ))
}

/// The Candidate a submission of `outputs` must produce: identity is the
/// digest of the candidate document, deliberately NOT any output Artifact's
/// content digest.
fn expected_candidate(outputs: &[(String, Digest)]) -> (CandidateResult, Digest) {
    let candidate =
        CandidateResult::new(TASK, RUN, outputs.iter().cloned()).expect("valid mapping");
    let digest = candidate.digest();
    (candidate, digest)
}

fn count(world: &World, sql: &str) -> i64 {
    let conn = Connection::open(&world.db_path).expect("raw conn");
    conn.query_row(sql, [], |row| row.get(0))
        .expect("aggregate")
}

#[test]
fn describe_exposes_only_the_authenticated_lineage() {
    let world = world("describe-happy", 1);
    let verifier = bearer_verifier(1);
    let description = world
        .store()
        .describe_agent_session(credential(&verifier))
        .expect("authorized describe");

    assert_eq!(description.attempt_id, ATTEMPT);
    assert_eq!(description.run_id, RUN);
    assert_eq!(description.task_id, TASK);
    assert_eq!(description.task_phase, "Active");
    assert_eq!(description.outputs.len(), 1);
    assert_eq!(description.outputs[0].name, SLOT);
    assert_eq!(description.outputs[0].kind, "code.changeset");
    assert!(description.outputs[0].required);
}

#[test]
fn a_wrong_bearer_is_refused_and_creates_no_request_row() {
    let world = world("describe-wrong-bearer", 1);
    let wrong = bearer_verifier(2);
    let error = world
        .store()
        .describe_agent_session(credential(&wrong))
        .expect_err("wrong bearer fails closed");
    assert!(
        matches!(error, StoreError::AgentControlUnauthorized { .. }),
        "got {error:?}"
    );

    let opened = world.store().open_agent_request(
        credential(&wrong),
        AgentOperation::SealArtifact,
        "req-1",
        &[9u8; 32],
    );
    assert!(matches!(
        opened,
        Err(StoreError::AgentControlUnauthorized { .. })
    ));
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);
}

#[test]
fn an_old_generation_session_is_fenced_even_with_its_own_bearer() {
    let world = world("describe-old-generation", 1);
    let verifier = bearer_verifier(1);
    let conn = Connection::open(&world.db_path).expect("raw conn");
    conn.execute(
        "UPDATE system_state SET restore_generation = 'ffffffffffffffffffffffffffffffff'",
        [],
    )
    .expect("rotate generation");

    let error = world
        .store()
        .describe_agent_session(credential(&verifier))
        .expect_err("old-generation sessions are fenced");
    assert!(matches!(error, StoreError::AgentControlUnauthorized { .. }));

    // The fence precedes request lookup/creation: no row may appear even for
    // a bearer that is itself still correct for the historical verifier.
    let opened = world.store().open_agent_request(
        credential(&verifier),
        AgentOperation::SealArtifact,
        "req-restore",
        &[9u8; 32],
    );
    assert!(opened.is_err());
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);
}

#[test]
fn a_revoked_session_fails_closed_everywhere() {
    let world = world("revoked-session", 1);
    // Terminalize the Attempt: terminalization revokes its session.
    contact_attempt(&world);
    let status = world
        .store()
        .run_execution_view(RUN)
        .expect("view")
        .unwrap();
    let lineage = status.attempt.expect("live lineage");
    let _advanced = world
        .store()
        .record_execution_observation(
            &lineage.attempt.id,
            lineage.status_revision,
            crate::execution::ObservationUpdate::Terminal(
                pantheon_core::attempt::Observation::Exited,
            ),
        )
        .expect("terminalizes");
    let verifier = bearer_verifier(1);
    assert!(
        world
            .store()
            .describe_agent_session(credential(&verifier))
            .is_err()
    );
    let opened = world.store().open_agent_request(
        credential(&verifier),
        AgentOperation::SubmitResult,
        "req-late",
        &[5u8; 32],
    );
    assert!(opened.is_err());
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);
}

#[test]
fn open_records_started_before_any_effect_and_reconciles_the_same_hash() {
    let world = world("open-reconcile", 1);
    let verifier = bearer_verifier(1);

    let opened = world
        .store()
        .open_agent_request(
            credential(&verifier),
            AgentOperation::SealArtifact,
            "req-seal",
            &[7u8; 32],
        )
        .expect("opens");
    assert!(matches!(opened, AgentRequestOpened::Started));

    let again = world
        .store()
        .open_agent_request(
            credential(&verifier),
            AgentOperation::SealArtifact,
            "req-seal",
            &[7u8; 32],
        )
        .expect("reconciles");
    assert_eq!(
        again,
        AgentRequestOpened::Reconciled(AgentRequestState::Started)
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 1);

    // Same identity under different semantics fails closed and preserves the
    // stored identity untouched.
    let conflicting = world.store().open_agent_request(
        credential(&verifier),
        AgentOperation::SealArtifact,
        "req-seal",
        &[8u8; 32],
    );
    assert!(matches!(
        conflicting,
        Err(StoreError::AgentRequestConflict { .. })
    ));
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 1);
}

#[test]
fn t6_commits_candidate_outputs_lifecycle_request_and_event_atomically() {
    let world = world("t6-happy", 1);
    let artifact = seal_for_submission(&world, SLOT, 1);
    let events_before = count(&world, "SELECT COUNT(*) FROM event_journal");

    let outcome = submit(
        &world,
        &bearer_verifier(1),
        "req-submit",
        [3u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect("submission commits");

    assert!(!outcome.reconciled);
    let (_candidate, candidate_digest) = expected_candidate(&[(SLOT.to_string(), artifact)]);
    assert_eq!(
        outcome.committed.candidate_digest,
        candidate_digest.to_string()
    );

    let conn = Connection::open(&world.db_path).expect("raw conn");
    let (run_phase, target, candidate_on_run, slot_held): (
        String,
        String,
        Vec<u8>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT phase, terminal_target, candidate_digest, active_slot
             FROM run_status WHERE run_id = ?1",
            rusqlite::params![RUN],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("run status");
    assert_eq!(run_phase, "Finalizing");
    assert_eq!(target, "Completed");
    assert_eq!(candidate_on_run, candidate_digest.as_bytes().to_vec());
    // Finalizing keeps occupying the unique nonterminal Run slot.
    assert_eq!(slot_held.as_deref(), Some("global"));

    let task_phase: String = conn
        .query_row(
            "SELECT phase FROM tasks WHERE id = ?1",
            rusqlite::params![TASK],
            |row| row.get(0),
        )
        .expect("task row");
    assert_eq!(task_phase, "Evaluating");

    let stored_json: String = conn
        .query_row(
            "SELECT canonical_json FROM candidates WHERE digest = ?1",
            rusqlite::params![candidate_digest.as_bytes()],
            |row| row.get(0),
        )
        .expect("candidate row keyed by the candidate document digest");
    assert_eq!(stored_json, _candidate.to_canonical_json());

    let output_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM candidate_outputs WHERE production_run_id = ?1",
            rusqlite::params![RUN],
            |row| row.get(0),
        )
        .expect("outputs counted");
    assert_eq!(output_rows, 1);
    let bound_artifact: Vec<u8> = conn
        .query_row(
            "SELECT artifact_digest FROM candidate_outputs
             WHERE production_run_id = ?1 AND output_slot = ?2",
            rusqlite::params![RUN, SLOT],
            |row| row.get(0),
        )
        .expect("the slot binds the sealed artifact itself");
    assert_eq!(bound_artifact, artifact.as_bytes().to_vec());

    let (state, result_ref): (String, String) = conn
        .query_row(
            "SELECT state, result_ref FROM agent_requests
             WHERE attempt_id = ?1 AND request_id = 'req-submit'",
            rusqlite::params![ATTEMPT],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("request row");
    assert_eq!(state, "SUCCEEDED");
    assert_eq!(result_ref, candidate_digest.to_string());

    assert_eq!(
        count(&world, "SELECT COUNT(*) FROM event_journal"),
        events_before + 1,
        "exactly one Event commits with the mutation"
    );
}

#[test]
fn an_exact_replay_reconciles_the_same_candidate() {
    let world = world("t6-replay", 1);
    let artifact = seal_for_submission(&world, SLOT, 1);
    let first = submit(
        &world,
        &bearer_verifier(1),
        "req-submit",
        [3u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect("first submission commits");
    let candidates_before = count(&world, "SELECT COUNT(*) FROM candidates");
    let events_before = count(&world, "SELECT COUNT(*) FROM event_journal");

    let second = submit(
        &world,
        &bearer_verifier(1),
        "req-submit",
        [3u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect("replay reconciles");

    assert!(second.reconciled);
    assert_eq!(
        second.committed.candidate_digest,
        first.committed.candidate_digest
    );
    assert_eq!(
        count(&world, "SELECT COUNT(*) FROM candidates"),
        candidates_before
    );
    // A reconciled replay is not a new authoritative mutation: no Event.
    assert_eq!(
        count(&world, "SELECT COUNT(*) FROM event_journal"),
        events_before
    );
}

#[test]
fn the_same_request_identity_with_different_semantics_fails_closed_in_t6() {
    let world = world("t6-hash-conflict", 1);
    let artifact = seal_for_submission(&world, SLOT, 1);
    submit(
        &world,
        &bearer_verifier(1),
        "req-submit",
        [3u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect("first submission commits");

    // Same (attempt, request) identity, different canonical request hash —
    // caller misuse, never a retry. The stored outcome is untouched.
    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-submit",
        [4u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect_err("identity reuse with different semantics fails closed");
    assert!(
        matches!(error, StoreError::AgentRequestConflict { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 1);
}

#[test]
fn a_new_request_id_after_a_committed_submission_cannot_mint_authority() {
    let world = world("t6-new-request-after-success", 1);
    let artifact = seal_for_submission(&world, SLOT, 1);
    submit(
        &world,
        &bearer_verifier(1),
        "req-a",
        [3u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect("first commits");

    // A different request identity is a NEW request, not a replay: it must
    // pass full current authority, which a Finalizing Run no longer offers.
    // No second Candidate authority can be minted under any identity.
    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-b",
        [4u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect_err("the Run left Active with its Candidate");
    assert!(
        matches!(error, StoreError::SubmissionStaleAuthority { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 1);
}

#[test]
fn one_candidate_per_run_is_a_database_backstop_not_only_controller_logic() {
    let world = world("t6-one-per-run-sql", 1);
    let artifact = seal_for_submission(&world, SLOT, 1);
    let (candidate, digest) = expected_candidate(&[(SLOT.to_string(), artifact)]);
    submit_with_hash(
        &world,
        ATTEMPT,
        &bearer_verifier(1),
        "req-a",
        [3u8; 32],
        &candidate,
    )
    .expect("commits");

    let conn = Connection::open(&world.db_path).expect("raw conn");
    let forged = Digest::of(b"a-second-different-candidate-document");
    let result = conn.execute(
        "INSERT INTO candidates (digest, task_id, run_id, canonical_json, created_at)
         VALUES (?1, ?2, ?3, '{}', unixepoch())",
        rusqlite::params![forged.as_bytes(), TASK, RUN],
    );
    let error = result.expect_err("the UNIQUE(run_id) backstop refuses a second Candidate");
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    );
    drop(conn);
    let _ = digest;
}

/// Submits an already-built Candidate under an explicit Attempt identity.
fn submit_with_hash(
    world: &World,
    attempt_id: &str,
    verifier: &[u8; 32],
    request_id: &str,
    request_hash: [u8; 32],
    candidate: &CandidateResult,
) -> Result<SubmissionOutcome, StoreError> {
    let revision = {
        let conn = Connection::open(&world.db_path).expect("raw conn");
        conn.query_row(
            "SELECT revision FROM tasks WHERE id = ?1",
            rusqlite::params![TASK],
            |row| row.get(0),
        )
        .expect("task row")
    };
    world.store().submit_candidate(&submission_for(
        attempt_id,
        verifier,
        request_id,
        &request_hash,
        candidate,
        Revision::new(revision),
    ))
}

fn submission_for<'a>(
    attempt_id: &'a str,
    verifier: &'a [u8; 32],
    request_id: &'a str,
    request_hash: &'a [u8; 32],
    candidate: &'a CandidateResult,
    expected_task_revision: Revision,
) -> CandidateSubmission<'a> {
    CandidateSubmission {
        credential: AgentCredential {
            attempt_id,
            verifier,
        },
        request_id,
        request_hash,
        candidate,
        expected_task_revision,
    }
}

#[test]
fn a_second_different_submission_conflicts_deterministically() {
    let world = world("t6-second-different", 1);
    let artifact = seal_for_submission(&world, SLOT, 1);
    submit(
        &world,
        &bearer_verifier(1),
        "req-a",
        [3u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect("first commits");

    // A semantically different payload for the same Run. The committing Run
    // left Active with its Candidate in the same transaction, so this loses
    // deterministically on the lifecycle read — cancellation/finalization
    // precedence generalized: nothing after the commit can mint authority.
    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-b",
        [4u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect_err("the Run already carries its one Candidate");
    assert!(
        matches!(error, StoreError::SubmissionStaleAuthority { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 1);
}

#[test]
fn a_stale_task_revision_conflicts_without_partial_writes() {
    let world = world("t6-stale-revision", 1);
    let digest = seal_for_submission(&world, SLOT, 1);

    let conn = Connection::open(&world.db_path).expect("raw conn");
    let revision = task_revision(&conn);
    drop(conn);
    let candidate =
        CandidateResult::new(TASK, RUN, [(SLOT.to_string(), digest)]).expect("valid mapping");
    let error = world
        .store()
        .submit_candidate(&submission(
            &bearer_verifier(1),
            "req-stale",
            &[6u8; 32],
            &candidate,
            Revision::new(revision + 7),
        ))
        .expect_err("the CAS expectation cannot be met");
    assert!(
        matches!(error, StoreError::RevisionConflict { .. }),
        "{error:?}"
    );

    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);
    let conn = Connection::open(&world.db_path).expect("raw conn");
    let (phase, target): (String, Option<String>) = conn
        .query_row(
            "SELECT phase, terminal_target FROM run_status WHERE run_id = ?1",
            rusqlite::params![RUN],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("run row");
    assert_eq!(phase, "Active");
    assert_eq!(target, None);
}

#[test]
fn a_stale_revision_conflicts_before_content_is_examined() {
    let world = world("t6-stale-before-content", 1);
    // A doubly-invalid submission: the Task revision expectation is stale
    // AND the referenced Artifact does not exist. The canonical ordering is
    // that the revision gate decides first — a caller with a stale view is
    // told to re-read, not handed a verdict about content it cannot know.
    let conn = Connection::open(&world.db_path).expect("raw conn");
    let revision = task_revision(&conn);
    drop(conn);
    let candidate = CandidateResult::new(TASK, RUN, [(SLOT.to_string(), Digest::of(b"ghost"))])
        .expect("valid mapping");
    let error = world
        .store()
        .submit_candidate(&submission(
            &bearer_verifier(1),
            "req-stale-order",
            &[6u8; 32],
            &candidate,
            Revision::new(revision + 7),
        ))
        .expect_err("the stale view loses first");
    assert!(
        matches!(error, StoreError::RevisionConflict { .. }),
        "expected the revision gate to decide first, got {error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
}

#[test]
fn a_terminal_attempt_cannot_submit_and_late_requests_cannot_act() {
    let mut world = world("t6-terminal-attempt", 1);
    contact_attempt(&world);
    let status = world
        .store()
        .run_execution_view(RUN)
        .expect("view")
        .unwrap();
    let lineage = status.attempt.expect("live lineage");
    let _advanced = world
        .store()
        .record_execution_observation(
            &lineage.attempt.id,
            lineage.status_revision,
            crate::execution::ObservationUpdate::Terminal(
                pantheon_core::attempt::Observation::Exited,
            ),
        )
        .expect("terminalizes");
    let artifact = seal_for_submission(&world, SLOT, 1);
    // Terminalization revokes the session, so the old credential fails at
    // the authentication fence — before any request row could appear.
    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-late",
        [5u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect_err("a terminal Attempt has no authority left");
    assert!(matches!(error, StoreError::AgentControlUnauthorized { .. }));
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);

    // A replacement Attempt becomes current under Recovery Policy; the
    // superseded session still cannot act on the newer lineage.
    launch_attempt(&mut world, "attempt-2", "acs-2", 2);
    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-stale-session",
        [5u8; 32],
        &[(SLOT.to_string(), artifact)],
    )
    .expect_err("the superseded session cannot act on the current Attempt");
    assert!(matches!(error, StoreError::AgentControlUnauthorized { .. }));

    // And the current Attempt's own submission proceeds.
    let (candidate, candidate_digest) = expected_candidate(&[(SLOT.to_string(), artifact)]);
    let outcome = submit_with_hash(
        &world,
        "attempt-2",
        &bearer_verifier(2),
        "req-current",
        [5u8; 32],
        &candidate,
    )
    .expect("the current Attempt submits");
    assert_eq!(
        outcome.committed.candidate_digest,
        candidate_digest.to_string()
    );
}

#[test]
fn cancellation_committed_first_defeats_submission() {
    let world = world("race-fence-first", 1);
    let digest = seal_for_submission(&world, SLOT, 1);
    let conn = Connection::open(&world.db_path).expect("raw conn");
    // The cancellation/supersession fence, as the future cancellation
    // transaction will commit it: Task leaves Active with a durable target.
    // No operator cancellation API exists yet (#33 adds none), so the test
    // establishes the exact competing authoritative state directly.
    conn.execute(
        "UPDATE tasks SET phase = 'Finalizing', terminal_target = 'Cancelled',
                revision = revision + 1
         WHERE id = ?1",
        rusqlite::params![TASK],
    )
    .expect("fence commits");
    let revision_after_fence = task_revision(&conn);
    drop(conn);

    let candidate =
        CandidateResult::new(TASK, RUN, [(SLOT.to_string(), digest)]).expect("valid mapping");
    let error = world
        .store()
        .submit_candidate(&submission(
            &bearer_verifier(1),
            "req-race",
            &[8u8; 32],
            &candidate,
            Revision::new(revision_after_fence - 1),
        ))
        .expect_err("cancellation won the race");
    assert!(matches!(error, StoreError::SubmissionStaleAuthority { .. }));

    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);
}

#[test]
fn submission_committed_first_keeps_the_candidate_as_history() {
    let world = world("race-submit-first", 1);
    let artifact = seal_for_submission(&world, SLOT, 1);
    let (candidate, candidate_digest) = expected_candidate(&[(SLOT.to_string(), artifact)]);
    submit_with_hash(
        &world,
        ATTEMPT,
        &bearer_verifier(1),
        "req-first",
        [3u8; 32],
        &candidate,
    )
    .expect("submission wins the race");

    let conn = Connection::open(&world.db_path).expect("raw conn");
    conn.execute(
        "UPDATE tasks SET phase = 'Finalizing', terminal_target = 'Cancelled',
                revision = revision + 1
         WHERE id = ?1",
        rusqlite::params![TASK],
    )
    .expect("later cancellation commits");

    // The immutable Candidate remains historical truth, and the producing Run
    // keeps its Finalizing/Completed target untouched by that cancellation.
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 1);
    let (run_phase, run_target): (String, String) = conn
        .query_row(
            "SELECT phase, terminal_target FROM run_status WHERE run_id = ?1",
            rusqlite::params![RUN],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("run row");
    assert_eq!(run_phase, "Finalizing");
    assert_eq!(run_target, "Completed");

    // And a replay of the winning request still reconciles to it.
    let replay = submit_with_hash(
        &world,
        ATTEMPT,
        &bearer_verifier(1),
        "req-first",
        [3u8; 32],
        &candidate,
    )
    .expect("replay reconciles");
    assert!(replay.reconciled);
    assert_eq!(
        replay.committed.candidate_digest,
        candidate_digest.to_string()
    );
}

#[test]
fn missing_required_output_refuses_clean() {
    let world = world("t6-missing-output", 1);
    let revision = task_revision(&Connection::open(&world.db_path).expect("raw conn"));

    // The spec declares exactly one required slot; submitting nothing omits
    // it. Build the Candidate through the same constructor the gateway uses
    // so the refusal is the spec validation inside T6.
    let candidate =
        CandidateResult::new(TASK, RUN, [] as [(String, Digest); 0]).expect("constructible");
    let error = world
        .store()
        .submit_candidate(&submission(
            &bearer_verifier(1),
            "req-empty",
            &[6u8; 32],
            &candidate,
            Revision::new(revision),
        ))
        .expect_err("the required output is missing");
    assert!(
        matches!(error, StoreError::CandidateInvalid { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);
}

#[test]
fn undeclared_output_refuses_clean() {
    let world = world("t6-undeclared-output", 1);
    let digest = seal_for_submission(&world, SLOT, 1);

    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-extra",
        [6u8; 32],
        &[(SLOT.to_string(), digest), ("bonus".to_string(), digest)],
    )
    .expect_err("no such slot exists in the specification");
    assert!(
        matches!(error, StoreError::CandidateInvalid { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
}

#[test]
fn wrong_artifact_kind_refuses() {
    let world = world("t6-wrong-kind", 1);
    let digest = seal_for_submission(&world, SLOT, 1);
    let conn = Connection::open(&world.db_path).expect("raw conn");
    // The Artifact exists but its stored kind does not permit the slot.
    conn.execute(
        "UPDATE artifacts SET artifact_kind = 'research.report' WHERE digest = ?1",
        rusqlite::params![digest.as_bytes()],
    )
    .expect("kind tampered");
    drop(conn);

    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-kind",
        [6u8; 32],
        &[(SLOT.to_string(), digest)],
    )
    .expect_err("the kind ceiling refuses the artifact");
    assert!(
        matches!(error, StoreError::CandidateInvalid { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
}

#[test]
fn foreign_production_cannot_be_submitted_even_though_the_content_exists() {
    let world = world("t6-foreign-content", 1);
    // The Artifact exists as content, but the Run's ProductionRecord names
    // different content for this slot — exactly what content reuse across
    // lineages looks like to the submitting Run.
    let digest = seal_for_submission(&world, SLOT, 1);
    let foreign = Digest::of(b"content-some-other-lineage-produced");
    let conn = Connection::open(&world.db_path).expect("raw conn");
    conn.execute(
        "INSERT INTO artifacts (digest, artifact_kind, canonical_json, created_at)
         VALUES (?1, 'code.changeset', '{\"entries\":[],\"schemaVersion\":1}', unixepoch())",
        rusqlite::params![foreign.as_bytes()],
    )
    .expect("the foreign content exists");
    conn.execute(
        "UPDATE production_records SET artifact_digest = ?1
         WHERE run_id = ?2 AND output_slot = ?3",
        rusqlite::params![foreign.as_bytes(), RUN, SLOT],
    )
    .expect("this run/slot is recorded as having produced it");
    drop(conn);

    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-foreign",
        [6u8; 32],
        &[(SLOT.to_string(), digest)],
    )
    .expect_err("content without this lineage's provenance cannot be submitted");
    assert!(
        matches!(error, StoreError::CandidateProvenanceInvalid { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
    assert_eq!(count(&world, "SELECT COUNT(*) FROM agent_requests"), 0);
}

#[test]
fn incomplete_artifact_refuses() {
    let world = world("t6-incomplete-artifact", 1);
    let digest = seal_for_submission(&world, SLOT, 1);
    let manifest = format!(
        "{{\"entries\":[{{\"after\":{{\"blob\":\"{}\",\"state\":\"present\"}},\
         \"operation\":\"add\",\"path\":\"src/main.rs\"}}],\
         \"schemaVersion\":1,\"variant\":\"1\"}}",
        Digest::of(b"payload")
    );
    let conn = Connection::open(&world.db_path).expect("raw conn");
    conn.execute(
        "UPDATE artifacts SET canonical_json = ?1 WHERE digest = ?2",
        rusqlite::params![manifest, digest.as_bytes()],
    )
    .expect("manifest now names payload its members lack");
    drop(conn);

    let error = submit(
        &world,
        &bearer_verifier(1),
        "req-incomplete",
        [6u8; 32],
        &[(SLOT.to_string(), digest)],
    )
    .expect_err("the manifest references payload no member retains");
    assert!(
        matches!(error, StoreError::CandidateInvalid { .. }),
        "{error:?}"
    );
    assert_eq!(count(&world, "SELECT COUNT(*) FROM candidates"), 0);
}

#[test]
fn a_retrying_seal_never_overwrites_recorded_provenance() {
    let world = world("provenance-drift", 4);
    let artifact = seal_for_submission(&world, SLOT, 1);

    // Corrupt the recorded provenance to name foreign content.
    let foreign = Digest::of(b"content-some-other-lineage-produced");
    let conn = Connection::open(&world.db_path).expect("raw conn");
    conn.execute(
        "INSERT INTO artifacts (digest, artifact_kind, canonical_json, created_at)
         VALUES (?1, 'code.changeset', '{}', unixepoch())",
        rusqlite::params![foreign.as_bytes()],
    )
    .expect("foreign content row");
    conn.execute(
        "UPDATE production_records SET artifact_digest = ?1
         WHERE run_id = ?2 AND output_slot = ?3",
        rusqlite::params![foreign.as_bytes(), RUN, SLOT],
    )
    .expect("drift injected");
    drop(conn);

    // A retry of the seal (new command identity over the same frozen state)
    // recomputes the same content claim — and must refuse rather than
    // overwrite the recorded provenance.
    let error = seal_for_submission_err(&world, SLOT);
    assert!(
        matches!(error, StoreError::ContentIdentityConflict { .. }),
        "{error:?}"
    );
}

/// Re-drives publication for an already-frozen Workspace and returns the
/// typed refusal instead of panicking.
fn seal_for_submission_err(world: &World, slot: &str) -> StoreError {
    let store = world.store();
    let epoch = store.restore_generation().expect("generation");
    let ws = store
        .workspace_record(WORKSPACE)
        .expect("workspace readable")
        .expect("workspace exists");
    let authority = SealAuthority {
        run_id: RUN.to_string(),
        expected_run_revision: run_status_revision(store),
    };
    // The Workspace is already Frozen; the revalidation command re-proves
    // authority without a second freeze.
    store
        .validate_seal_authority_command(
            &command(epoch.as_str(), "cmd-revalidate", &[43u8; 32], "x"),
            &authority,
            TASK,
            slot,
            WORKSPACE,
            ws.revision,
        )
        .expect("revalidation holds");

    let manifest = "{\"entries\":[],\"schemaVersion\":1,\"variant\":\"1\"}";
    let artifact_digest = Digest::of(manifest.as_bytes());
    match store.commit_changeset_seal(
        &command(
            epoch.as_str(),
            "cmd-reseal",
            &[45u8; 32],
            "workspace.sealed",
        ),
        &SealedChangeset {
            workspace_id: WORKSPACE,
            task_id: TASK,
            fence_revision: Revision::new(ws.revision.get()),
            authority: &authority,
            output_slot: slot,
            repository: "repo://project",
            resolved_base: ws.resolved_base.as_str(),
            revision_state_digest: Digest::of(b"state"),
            revision_state_json: manifest,
            artifact_digest,
            artifact_json: manifest,
            members: Vec::new(),
            producer: Some(ProducerProvenance {
                attempt_id: ATTEMPT,
            }),
        },
    ) {
        Err(error) => error,
        Ok(_) => panic!("drifted provenance must be refused"),
    }
}

#[test]
fn complete_refuses_after_midflight_revocation_and_leaves_the_row_inert() {
    let world = world("midflight-revocation", 1);
    let verifier = bearer_verifier(1);
    let opened = world
        .store()
        .open_agent_request(
            credential(&verifier),
            AgentOperation::SealArtifact,
            "req-mid",
            &[7u8; 32],
        )
        .expect("opens before capture");
    assert!(matches!(opened, AgentRequestOpened::Started));

    // The session is revoked while the external effect would be running.
    contact_attempt(&world);
    let status = world
        .store()
        .run_execution_view(RUN)
        .expect("view")
        .unwrap();
    let lineage = status.attempt.expect("live lineage");
    let _advanced = world
        .store()
        .record_execution_observation(
            &lineage.attempt.id,
            lineage.status_revision,
            crate::execution::ObservationUpdate::Terminal(
                pantheon_core::attempt::Observation::Exited,
            ),
        )
        .expect("terminalizes");

    let error = world.store().complete_agent_request(
        credential(&verifier),
        "req-mid",
        "artifact://never-committed",
    );
    assert!(matches!(
        error,
        Err(StoreError::AgentControlUnauthorized { .. })
    ));

    // The row stays STARTED inert history; nothing grants it authority later.
    let state: String = Connection::open(&world.db_path)
        .expect("raw conn")
        .query_row(
            "SELECT state FROM agent_requests WHERE attempt_id = ?1 AND request_id = 'req-mid'",
            rusqlite::params![ATTEMPT],
            |row| row.get(0),
        )
        .expect("row present");
    assert_eq!(state, "STARTED");
}
