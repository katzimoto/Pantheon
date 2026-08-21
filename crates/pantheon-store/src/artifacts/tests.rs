//! Evidence for the durable half of Issue #32's sealing order: the
//! publication transaction revalidates every authority it depends on,
//! reuses verified immutable rows instead of duplicating them, and writes
//! nothing when any precondition fails.

use pantheon_core::config::Digest;
use pantheon_core::planning::validate::Materializable;
use pantheon_core::workspace::{RequestedBase, ResolvedBase};

use crate::artifacts::{SealOutcome, SealedChangeset};
use crate::command::Committed;
use crate::error::StoreError;
use crate::planning::tests::{
    command, create_goal, materialize, plan_and_record, store_with_configuration, validated,
};
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;

const BASE: &str = "dc6fcd729d1c3b0426712ab6985f28c19be95d55";

/// A store holding one Ready Task with a Ready Workspace frozen at the
/// returned revision.
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
    let frozen = executed(
        store
            .freeze_workspace(
                &command(
                    epoch.as_str(),
                    "cmd-freeze",
                    &[11u8; 32],
                    "workspace.frozen",
                ),
                "workspace-1",
                ready.revision,
            )
            .expect("freezes"),
    );
    (dir, store, frozen.revision)
}

/// A well-formed seal claim for the fixture workspace.
fn seal(fence: Revision, artifact_json: &'static str) -> SealedChangeset<'static> {
    SealedChangeset {
        workspace_id: "workspace-1",
        task_id: "task-1",
        fence_revision: fence,
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
