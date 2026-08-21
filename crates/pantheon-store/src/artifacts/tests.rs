//! Evidence for the durable half of Issue #32's sealing order and Issue
//! #76's Run authority: the publication transaction revalidates every
//! authority it depends on — including that the claimed Run is still the
//! Task's current responsible Run — reuses verified immutable rows instead
//! of duplicating them, and writes nothing when any precondition fails.

use pantheon_core::config::Digest;
use pantheon_core::planning::validate::Materializable;
use pantheon_core::workspace::{RequestedBase, ResolvedBase};

use crate::artifacts::{SealOutcome, SealedChangeset};
use crate::command::Committed;
use crate::error::StoreError;
use crate::planning::tests::{
    command, create_goal, materialize, plan_and_record, store_with_configuration, validated,
};
use crate::scheduling::tests::dispatch_ready_task;
use crate::seal::SealAuthority;
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;

const BASE: &str = "dc6fcd729d1c3b0426712ab6985f28c19be95d55";
const OTHER_BASE: &str = "3ab5ae51b3728243d6d221857e865ec97189e6e1";
/// The Run the fixture dispatches; its status starts at revision 1.
const RUN: &str = "run-1";

fn run_authority(expected_run_revision: i64) -> SealAuthority {
    SealAuthority {
        run_id: RUN.to_string(),
        expected_run_revision: Revision::new(expected_run_revision),
    }
}

/// A store holding one Task executing under [`RUN`], whose Ready Workspace
/// is frozen at the returned revision under that Run's authority.
#[allow(clippy::too_many_lines)]
fn frozen_workspace(label: &str) -> (TempDir, Store, Revision) {
    let (dir, store, sequence) = store_with_configuration(label);
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", sequence, "cmd-plan");
    let plan: Materializable = validated(sequence, Digest::of(b"registry"), "unit-tests-v1");
    materialize(&store, &op, "task-1", &plan, "cmd-materialize").expect("task materializes");

    let epoch = store.restore_generation().expect("generation");
    let requested = RequestedBase::parse("refs/heads/main").expect("ref");
    let base = ResolvedBase::parse(BASE).expect("base");
    let opened = executed(
        store
            .open_workspace(
                &command(epoch.as_str(), "cmd-open", &[7u8; 32], "workspace.opened"),
                "workspace-1",
                &crate::workspace::WorkspaceBinding {
                    task_id: "task-1",
                    repository: "repo://project",
                    source_path: "/trusted/source",
                    requested_base: &requested,
                    resolved_base: &base,
                },
            )
            .expect("opens"),
    );
    let begun = executed(
        store
            .begin_workspace_materialization(
                &command(
                    epoch.as_str(),
                    "cmd-begin",
                    &[8u8; 32],
                    "workspace.materializing",
                ),
                "workspace-1",
                opened.revision,
            )
            .expect("begins"),
    );
    let ready = executed(
        store
            .complete_workspace_materialization(
                &command(epoch.as_str(), "cmd-ready", &[9u8; 32], "workspace.ready"),
                "workspace-1",
                begun.revision,
                &base,
            )
            .expect("ready"),
    );

    // The Task is dispatched before anything freezes: sealing runs under a
    // current Run, never under a bare Ready Task.
    dispatch_ready_task(
        &store,
        "goal-1",
        "task-1",
        "workspace-1",
        BASE,
        RUN,
        "cmd-t3",
    );
    assert_run_current(&store);

    let frozen = executed(
        store
            .freeze_workspace(
                &command(
                    epoch.as_str(),
                    "cmd-freeze",
                    &[11u8; 32],
                    "workspace.frozen",
                ),
                &run_authority(1),
                "task-1",
                "changeset",
                "workspace-1",
                ready.revision,
            )
            .expect("freezes"),
    );
    (dir, store, frozen.revision)
}

/// Asserts the fixture's durable shape: `task-1` Active with [`RUN`]
/// nonterminal as its current responsible Run.
fn assert_run_current(store: &Store) {
    let tasks = crate::planning::tests::tasks_of(store, "goal-1");
    assert_eq!(tasks[0].phase, pantheon_core::planning::TaskPhase::Active);
    assert_eq!(tasks[0].active_run_id.as_deref(), Some(RUN));
}

/// The fixture's Run authority as a shared static claim, so seal builders
/// can borrow it for the `'static` SealedChangeset.
fn current_run_authority() -> &'static SealAuthority {
    use std::sync::OnceLock;
    static AUTHORITY: OnceLock<SealAuthority> = OnceLock::new();
    AUTHORITY.get_or_init(|| run_authority(1))
}

/// A variant claim that must outlive its builder, for tests presenting
/// deliberately different authority. Test-local and bounded.
fn leaked(authority: SealAuthority) -> &'static SealAuthority {
    Box::leak(Box::new(authority))
}

fn current_authority_at(revision: i64) -> &'static SealAuthority {
    leaked(run_authority(revision))
}

/// A well-formed seal claim for the fixture workspace under [`RUN`]'s
/// authority.
fn seal(fence: Revision, artifact_json: &'static str) -> SealedChangeset<'static> {
    SealedChangeset {
        workspace_id: "workspace-1",
        task_id: "task-1",
        fence_revision: fence,
        authority: current_run_authority(),
        output_slot: "changeset",
        repository: "repo://project",
        resolved_base: BASE,
        revision_state_digest: Digest::of(b"state"),
        revision_state_json: r#"{"entries":[],"schemaVersion":1}"#,
        artifact_digest: Digest::of(b"manifest"),
        artifact_json,
        members: vec![(Digest::of(b"bytes"), 5)],
    }
}

fn commit(
    store: &Store,
    id: &str,
    s: &SealedChangeset<'_>,
) -> Result<Committed<SealOutcome>, StoreError> {
    let epoch = store.restore_generation().expect("generation");
    let epoch = epoch.as_str();
    store.commit_changeset_seal(&command(epoch, id, &[13u8; 32], "workspace.sealed"), s)
}

fn count(store: &Store, sql: &'static str) -> i64 {
    store.read_all_for_test(sql).expect("count query")[0]
}

#[test]
fn a_seal_commits_every_immutable_row_in_one_transaction() {
    let (_dir, store, fence) = frozen_workspace("seal-happy");
    let committed = commit(&store, "cmd-seal", &seal(fence, "{}")).expect("seals");
    assert!(committed.was_executed());
    let Committed::Executed { value, .. } = committed else {
        panic!("expected execution");
    };
    assert!(!value.artifact_reused);

    let artifact = store
        .artifact(Digest::of(b"manifest"))
        .expect("read")
        .expect("artifact row exists");
    assert_eq!(artifact.kind, "code.changeset");
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM blobs WHERE size = 5"),
        1,
        "the published member is a durable Blob row"
    );
}

#[test]
fn replaying_the_seal_command_does_not_duplicate_anything() {
    let (_dir, store, fence) = frozen_workspace("seal-replay");
    let first = commit(&store, "cmd-seal", &seal(fence, "{}")).expect("first");
    let second = commit(&store, "cmd-seal", &seal(fence, "{}")).expect("second");
    assert!(first.was_executed() && !second.was_executed(), "replay");
    let artifact = store
        .artifact(Digest::of(b"manifest"))
        .expect("read")
        .expect("exists");
    assert_eq!(artifact.canonical_json, "{}");
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM artifacts"),
        1,
        "one content identity, however many retries"
    );
}

#[test]
fn a_different_command_capturing_identical_state_converges_on_prior_content() {
    let (_dir, store, fence) = frozen_workspace("seal-converge");
    let first = commit(&store, "cmd-a", &seal(fence, "{}")).expect("first seals");
    let second = commit(&store, "cmd-b", &seal(fence, "{}")).expect("second seals");
    assert!(first.was_executed() && second.was_executed());
    let Committed::Executed { value, .. } = second else {
        panic!("expected execution");
    };
    assert!(value.artifact_reused, "converged on prior content");
    // The checkpoint row was reused too: one Workspace cannot hold two rows
    // for one state digest.
    assert_eq!(
        match first {
            Committed::Executed { value, .. } => value.workspace_revision_id,
            Committed::Replayed { .. } => panic!("first must execute"),
        },
        value.workspace_revision_id
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM workspace_revisions"), 1);
}

#[test]
fn a_stale_fence_writes_nothing() {
    let (_dir, store, fence) = frozen_workspace("seal-stale");
    let stale = seal(Revision::new(fence.get() + 5), "{}");
    let err = commit(&store, "cmd-seal", &stale).expect_err("stale fence must fail");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );
    assert!(
        store
            .artifact(Digest::of(b"manifest"))
            .expect("read")
            .is_none()
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM blobs"), 0);
}

#[test]
fn an_owner_or_task_authority_mismatch_fails_closed() {
    let (_dir, store, fence) = frozen_workspace("seal-owner");
    let mut wrong_task = seal(fence, "{}");
    wrong_task.task_id = "task-other";
    let err = commit(&store, "cmd-wrong-owner", &wrong_task).expect_err("owner mismatch");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );

    // The Task leaving Ready between capture and publication also voids the
    // seal: cancelling the Goal fences its Tasks to Cancelled.
    let epoch = store.restore_generation().expect("generation");
    store
        .cancel_goal(
            &command(epoch.as_str(), "cmd-cancel", &[14u8; 32], "goal.cancelled"),
            "goal-1",
        )
        .expect("cancels");
    let err = commit(&store, "cmd-post-cancel", &seal(fence, "{}")).expect_err("authority is gone");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );
    assert!(
        store
            .artifact(Digest::of(b"manifest"))
            .expect("read")
            .is_none()
    );
}

/// Asserts a rejected publication left the world exactly as it was: no
/// Artifact claim, no Blob rows, no WorkspaceRevision, and the freeze
/// intact at the fence.
fn assert_nothing_was_published(store: &Store) {
    assert!(
        store
            .artifact(Digest::of(b"manifest"))
            .expect("read")
            .is_none(),
        "no complete Artifact claim may exist"
    );
    assert_eq!(
        count(store, "SELECT COUNT(*) FROM blobs"),
        0,
        "no payload reached the durable graph"
    );
    assert_eq!(
        count(store, "SELECT COUNT(*) FROM workspace_revisions"),
        0,
        "no WorkspaceRevision was published for the rejected seal"
    );
    let workspace = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    assert_eq!(
        workspace.phase,
        pantheon_core::workspace::WorkspacePhase::Frozen,
        "the fence outlives the failed seal"
    );
}

#[test]
fn a_stale_run_revision_is_refused_at_publication() {
    let (_dir, store, fence) = frozen_workspace("pub-stale-run");
    let mut stale = seal(fence, "{}");
    stale.authority = current_authority_at(99);
    let err = commit(&store, "cmd-stale-run", &stale).expect_err("must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );
    assert_nothing_was_published(&store);
}

#[test]
fn a_terminal_run_cannot_authorize_publication() {
    let (_dir, store, fence) = frozen_workspace("pub-terminal-run");
    terminalize_run(&store, RUN);
    let err = commit(&store, "cmd-terminal", &seal(fence, "{}")).expect_err("must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );
    assert_nothing_was_published(&store);
}

#[test]
fn a_superseded_run_that_is_not_the_current_responsible_run_cannot_authorize() {
    let (_dir, store, fence) = frozen_workspace("pub-not-current");
    // The Task gains a second nonterminal Run while its pointer still names
    // the terminal first one; claiming the new Run must refuse on
    // currentness alone — it exists, belongs to the Task, and is fresh.
    terminalize_run(&store, RUN);
    insert_superseding_run(&store, "run-2");
    let mut superseded_pointer_claim = seal(fence, "{}");
    superseded_pointer_claim.authority = leaked(SealAuthority {
        run_id: "run-2".to_string(),
        expected_run_revision: Revision::new(1),
    });
    let err = commit(&store, "cmd-not-current", &superseded_pointer_claim)
        .expect_err("a Run the pointer does not name must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );
    assert_nothing_was_published(&store);
}

#[test]
fn a_run_owned_by_another_task_cannot_authorize_a_seal() {
    let (_dir, store, fence) = frozen_workspace("pub-wrong-owner");
    terminalize_run(&store, RUN);
    insert_foreign_task_and_run(&store);
    let mut foreign = seal(fence, "{}");
    foreign.authority = leaked(SealAuthority {
        run_id: "run-foreign".to_string(),
        expected_run_revision: Revision::new(1),
    });
    let err = commit(&store, "cmd-foreign", &foreign).expect_err("must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );
    assert_nothing_was_published(&store);
}

#[test]
fn drift_between_what_the_run_froze_and_current_state_refuses_publication() {
    enum Drift {
        SpecDigest,
        WorkspaceIdentity,
        ResolvedBase,
    }

    for (drift, label) in [
        (Drift::SpecDigest, "spec-digest"),
        (Drift::WorkspaceIdentity, "workspace-identity"),
        (Drift::ResolvedBase, "resolved-base"),
    ] {
        let (_dir, store, fence) = frozen_workspace("pub-drift");
        match drift {
            Drift::SpecDigest => {
                alter_snapshot_spec(&store, Digest::of(b"another-task").as_bytes())
            }
            Drift::ResolvedBase => {
                alter_snapshot_text(&store, "workspace_resolved_base", OTHER_BASE)
            }
            Drift::WorkspaceIdentity => {
                open_second_workspace(&store);
                alter_snapshot_text(&store, "workspace_id", "workspace-2");
            }
        }
        let Err(err) = commit(&store, "cmd-drift", &seal(fence, "{}")) else {
            panic!("{label} must refuse publication");
        };
        drop(err);
        assert_nothing_was_published(&store);
    }
}

#[test]
fn an_output_ceiling_outside_the_frozen_specification_refuses_publication() {
    let (_dir, store, fence) = frozen_workspace("pub-ceiling");

    // A slot the frozen specification never declared.
    let mut unknown_slot = seal(fence, "{}");
    unknown_slot.output_slot = "no-such-slot";
    let err = commit(&store, "cmd-slot-a", &unknown_slot).expect_err("must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );

    // A declared slot whose kind no longer permits code.changeset — the
    // stored specification was doctored, which is exactly the corruption
    // the transactional re-check exists to catch.
    rewrite_stored_spec_kind(&store, "code.changeset", "diagnostic.report");
    let err = commit(&store, "cmd-slot-b", &seal(fence, "{}")).expect_err("must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );
    assert_nothing_was_published(&store);
}

#[test]
fn content_that_collides_with_different_stored_content_is_refused() {
    let (_dir, store, fence) = frozen_workspace("seal-conflict");
    commit(&store, "cmd-first", &seal(fence, "{}")).expect("first seals");

    // Same manifest digest, different manifest: corruption or a broken hash,
    // never overwritten.
    let err = commit(&store, "cmd-second", &seal(fence, r#"{"tampered":true}"#))
        .expect_err("must refuse");
    assert!(
        matches!(err, StoreError::ContentIdentityConflict { .. }),
        "{err}"
    );

    // Same blob digest, different size: the same rule at the payload layer.
    let mut tampered = seal(fence, "{}");
    tampered.members = vec![(Digest::of(b"bytes"), 6)];
    let err = commit(&store, "cmd-third", &tampered).expect_err("must refuse");
    assert!(
        matches!(err, StoreError::ContentIdentityConflict { .. }),
        "{err}"
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM blobs"),
        1,
        "the original blob is untouched"
    );
}

#[test]
fn a_failed_publication_leaves_no_partial_rows_behind() {
    let (_dir, store, fence) = frozen_workspace("seal-atomic");
    // Publish one member first so the conflict below fails *after* this
    // transaction has already inserted a fresh blob row — proving the
    // rollback covers mid-transaction writes.
    let mut earlier = seal(fence, "{}");
    earlier.members = vec![(Digest::of(b"earlier"), 1)];
    commit(&store, "cmd-partial", &earlier).expect("partial seals");

    let mut conflicting = seal(fence, r#"{"other":1}"#);
    conflicting.members = vec![(Digest::of(b"new-bytes"), 2)];
    assert!(
        commit(&store, "cmd-conflict", &conflicting).is_err(),
        "the tampered manifest is refused"
    );

    // Nothing the failed transaction wrote survived: same totals as after
    // the successful partial seal.
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM workspace_revisions"),
        1,
        "exactly the row the successful seal committed"
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM blobs WHERE size = 2"),
        0,
        "the mid-transaction blob must not survive its own failed transaction"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM blobs"), 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM artifacts"), 1);
}

#[test]
fn every_member_of_the_manifest_is_recorded_for_reachability() {
    let (_dir, store, fence) = frozen_workspace("seal-members");
    let mut s = seal(fence, "{}");
    s.members = vec![(Digest::of(b"m1"), 1), (Digest::of(b"m2"), 2)];
    let committed = commit(&store, "cmd-members", &s).expect("seals");
    assert!(committed.was_executed());
    // GC reachability is relational: both members are linked to the
    // Artifact, independent of manifest JSON.
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM artifact_members"),
        2,
        "every required payload member must be reachable"
    );
}

// ---- local helpers -------------------------------------------------------

fn executed<T>(committed: Committed<T>) -> T {
    match committed {
        Committed::Executed { value, .. } => value,
        Committed::Replayed { .. } => panic!("expected an executed command"),
    }
}

// ---- deliberate failure injection ---------------------------------------
//
// The rejection arms these helpers enable have no public mutation path yet —
// Run finalization and supersession arrive with their own lifecycle
// missions. Corrupting one durable fact at a time through direct SQL is the
// deterministic hook that isolates each arm of the authority revalidation.

/// Terminalizes a Run in place, keeping its revision: exactly what a failed
/// or cancelled execution leaves behind.
fn terminalize_run(store: &Store, run_id: &str) {
    store
        .write(|writer| {
            writer.execute(
                "UPDATE run_status SET phase = 'Failed', active_slot = NULL WHERE run_id = ?1",
                &[crate::transaction::Value::from(run_id)],
            )
        })
        .expect("fixture terminalizes");
}

/// Copies `RUN`'s immutable identity into a second nonterminal Run for the
/// same Task. The Task's responsible-Run pointer deliberately still names
/// the old Run.
fn insert_superseding_run(store: &Store, new_run_id: &str) {
    store
        .write(|writer| {
            writer.execute(
                "INSERT INTO runs (id, task_id, binding_digest,
                        context_source_snapshot_digest, created_at)
                 SELECT ?1, task_id, binding_digest,
                        context_source_snapshot_digest, unixepoch()
                 FROM runs WHERE id = ?2",
                &[
                    crate::transaction::Value::from(new_run_id),
                    crate::transaction::Value::from(RUN),
                ],
            )?;
            writer.execute(
                "INSERT INTO run_status (run_id, task_id, phase, terminal_target,
                        revision, active_slot, updated_at)
                 SELECT ?1, task_id, 'Active', NULL, 1, 'global', unixepoch()
                 FROM run_status WHERE run_id = ?2",
                &[
                    crate::transaction::Value::from(new_run_id),
                    crate::transaction::Value::from(RUN),
                ],
            )
        })
        .expect("fixture superseding run inserts");
}

/// Creates a second Task and inserts a Run owned by it, then points the
/// sealing Task's responsible-Run pointer at that foreign Run — exactly the
/// corruption the deferred `tasks.active_run_id` composite FK cannot yet
/// refuse, and which the transactional holder check must. The caller must
/// have freed the global slot first.
fn insert_foreign_task_and_run(store: &Store) {
    create_goal(store, "goal-2", "cmd-goal-2");
    let sequence = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .as_ref()
        .expect("active")
        .activation_sequence;
    let op = plan_and_record(store, "goal-2", sequence, "cmd-plan-2");
    let registry = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .as_ref()
        .expect("active")
        .components
        .evaluator_registry;
    let plan: Materializable =
        crate::planning::tests::validated_for("goal-2", sequence, registry, "unit-tests-v1");
    materialize(store, &op, "task-2", &plan, "cmd-materialize-2").expect("second task");
    store
        .write(|writer| {
            writer.execute(
                "INSERT INTO runs (id, task_id, binding_digest,
                        context_source_snapshot_digest, created_at)
                 SELECT 'run-foreign', 'task-2', binding_digest,
                        context_source_snapshot_digest, unixepoch()
                 FROM runs WHERE id = 'run-1'",
                &[],
            )?;
            writer.execute(
                "INSERT INTO run_status (run_id, task_id, phase, terminal_target,
                        revision, active_slot, updated_at)
                 VALUES ('run-foreign', 'task-2', 'Active', NULL, 1, 'global', unixepoch())",
                &[],
            )?;
            writer.execute(
                "UPDATE tasks SET active_run_id = 'run-foreign' WHERE id = 'task-1'",
                &[],
            )
        })
        .expect("fixture foreign run inserts");
}

/// Opens a second Workspace for the second Task (existence is all the FK
/// needs; identity drift is what the test proves).
fn open_second_workspace(store: &Store) {
    create_goal(store, "goal-2", "cmd-goal-2");
    let sequence = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .as_ref()
        .expect("active")
        .activation_sequence;
    let op = plan_and_record(store, "goal-2", sequence, "cmd-plan-2b");
    let registry = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .as_ref()
        .expect("active")
        .components
        .evaluator_registry;
    let plan: Materializable =
        crate::planning::tests::validated_for("goal-2", sequence, registry, "unit-tests-v1");
    materialize(store, &op, "task-2", &plan, "cmd-materialize-2b").expect("second task");

    let epoch = store.restore_generation().expect("generation");
    let requested = RequestedBase::parse("refs/heads/main").expect("ref");
    let base = ResolvedBase::parse(BASE).expect("base");
    store
        .open_workspace(
            &command(
                epoch.as_str(),
                "cmd-open-2",
                &[21u8; 32],
                "workspace.opened",
            ),
            "workspace-2",
            &crate::workspace::WorkspaceBinding {
                task_id: "task-2",
                repository: "repo://project",
                source_path: "/trusted/source",
                requested_base: &requested,
                resolved_base: &base,
            },
        )
        .expect("second workspace opens");
}

/// Points the Run's frozen snapshot at a different spec digest.
fn alter_snapshot_spec(store: &Store, digest: &[u8]) {
    store
        .write(|writer| {
            writer.execute(
                "UPDATE context_source_snapshots SET task_spec_digest = ?1
                 WHERE digest = (SELECT context_source_snapshot_digest FROM runs WHERE id = ?2)",
                &[
                    crate::transaction::Value::Blob(digest.to_vec()),
                    crate::transaction::Value::from(RUN),
                ],
            )
        })
        .expect("fixture snapshot alters");
}

/// Points the Run's frozen snapshot at a different Workspace identity.
fn alter_snapshot_text(store: &Store, column: &'static str, value: &str) {
    let sql = match column {
        "workspace_resolved_base" => {
            "UPDATE context_source_snapshots SET workspace_resolved_base = ?1
             WHERE digest = (SELECT context_source_snapshot_digest FROM runs WHERE id = ?2)"
        }
        "workspace_id" => {
            "UPDATE context_source_snapshots SET workspace_id = ?1
             WHERE digest = (SELECT context_source_snapshot_digest FROM runs WHERE id = ?2)"
        }
        other => panic!("unknown fixture column {other}"),
    };
    store
        .write(|writer| {
            writer.execute(
                sql,
                &[
                    crate::transaction::Value::from(value),
                    crate::transaction::Value::from(RUN),
                ],
            )
        })
        .expect("fixture snapshot alters");
}

/// Rewrites one kind spelling inside the stored TaskSpec document.
fn rewrite_stored_spec_kind(store: &Store, from: &str, to: &str) {
    store
        .write(|writer| {
            writer.execute(
                "UPDATE task_specs SET canonical_json = replace(canonical_json, ?1, ?2)
                 WHERE digest = (SELECT spec_digest FROM tasks WHERE id = 'task-1')",
                &[
                    crate::transaction::Value::from(from),
                    crate::transaction::Value::from(to),
                ],
            )
        })
        .expect("fixture spec rewrites");
}
