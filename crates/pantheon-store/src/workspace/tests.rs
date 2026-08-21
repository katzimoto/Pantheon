//! Evidence for the durable half of Issue #27: Task-scoped Workspace
//! ownership, the immutable base binding, and the fences that stop partial
//! materialization from being reported Ready.

use pantheon_core::config::Digest;
use pantheon_core::planning::validate::Materializable;
use pantheon_core::workspace::{Materialization, RequestedBase, ResolvedBase, WorkspacePhase};

use crate::command::Committed;
use crate::error::StoreError;
use crate::planning::tests::{
    command, create_goal, materialize, plan_and_record, store_with_configuration, validated,
};
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;
use crate::workspace::{WorkspaceBinding, WorkspaceRecord};

const BASE: &str = "dc6fcd729d1c3b0426712ab6985f28c19be95d55";
const OTHER_BASE: &str = "3ab5ae51b3728243d6d221857e865ec97189e6e1";

/// A store holding one Ready Task, which is what a Workspace needs a holder
/// to be.
fn store_with_ready_task(label: &str) -> (TempDir, Store, String) {
    let (dir, store, sequence) = store_with_configuration(label);
    create_goal(&store, "goal-1", "cmd-goal");
    let op = plan_and_record(&store, "goal-1", sequence, "cmd-plan");
    let plan: Materializable = validated(sequence, Digest::of(b"registry"), "unit-tests-v1");
    materialize(&store, &op, "task-1", &plan, "cmd-materialize").expect("task materializes");
    (dir, store, "task-1".to_string())
}

fn binding<'a>(
    task_id: &'a str,
    requested: &'a RequestedBase,
    resolved: &'a ResolvedBase,
) -> WorkspaceBinding<'a> {
    WorkspaceBinding {
        task_id,
        repository: "repo://whiskyshop",
        source_path: "/srv/repositories/whiskyshop",
        requested_base: requested,
        resolved_base: resolved,
    }
}

fn requested() -> RequestedBase {
    RequestedBase::parse("refs/heads/main").expect("valid ref name")
}

fn resolved(oid: &str) -> ResolvedBase {
    ResolvedBase::parse(oid).expect("valid object name")
}

fn executed<T>(committed: Committed<T>) -> T {
    match committed {
        Committed::Executed { value, .. } => value,
        Committed::Replayed { .. } => panic!("expected execution, got a replay"),
    }
}

fn open(store: &Store, task_id: &str, id: &str, command_id: &str) -> WorkspaceRecord {
    let epoch = store.restore_generation().expect("generation");
    let requested = requested();
    let resolved = resolved(BASE);
    executed(
        store
            .open_workspace(
                &command(epoch.as_str(), command_id, &[7u8; 32], "workspace.opened"),
                id,
                &binding(task_id, &requested, &resolved),
            )
            .expect("workspace opens"),
    )
}

fn begin(store: &Store, id: &str, expected: Revision, command_id: &str) -> WorkspaceRecord {
    let epoch = store.restore_generation().expect("generation");
    executed(
        store
            .begin_workspace_materialization(
                &command(
                    epoch.as_str(),
                    command_id,
                    &[8u8; 32],
                    "workspace.materializing",
                ),
                id,
                expected,
            )
            .expect("materialization begins"),
    )
}

fn complete(
    store: &Store,
    id: &str,
    expected: Revision,
    base: &ResolvedBase,
    command_id: &str,
) -> Result<Committed<WorkspaceRecord>, StoreError> {
    let epoch = store.restore_generation().expect("generation");
    store.complete_workspace_materialization(
        &command(epoch.as_str(), command_id, &[9u8; 32], "workspace.ready"),
        id,
        expected,
        base,
    )
}

#[test]
fn opening_a_workspace_commits_identity_and_base_before_any_side_effect() {
    let (_dir, store, task) = store_with_ready_task("workspace-open");

    let record = open(&store, &task, "workspace-1", "cmd-open");

    assert_eq!(record.phase, WorkspacePhase::Requested);
    // The whole point of a distinct Requested phase: durable state says no
    // external effect has been attempted, so recovery can conclude nothing
    // exists at the Workspace path.
    assert_eq!(record.materialization, Materialization::Absent);
    assert_eq!(record.task_id, task);
    assert_eq!(record.requested_base.as_str(), "refs/heads/main");
    assert_eq!(record.resolved_base.as_str(), BASE);

    let read_back = store
        .workspace_for_task(&task)
        .expect("read")
        .expect("the task owns a workspace");
    assert_eq!(read_back, record);
}

#[test]
fn a_task_cannot_acquire_a_second_current_workspace() {
    let (_dir, store, task) = store_with_ready_task("workspace-cardinality");
    open(&store, &task, "workspace-1", "cmd-open");

    let epoch = store.restore_generation().expect("generation");
    let requested = requested();
    let other = resolved(OTHER_BASE);
    let err = store
        .open_workspace(
            // A different command identity, so the command ledger cannot be
            // what refuses this: the cardinality invariant has to.
            &command(epoch.as_str(), "cmd-open-2", &[7u8; 32], "workspace.opened"),
            "workspace-2",
            &binding(&task, &requested, &other),
        )
        .expect_err("a second current workspace is refused");

    assert!(
        matches!(
            err,
            StoreError::WorkspaceAlreadyCurrent { ref workspace_id, .. }
                if workspace_id == "workspace-1"
        ),
        "unexpected error: {err}"
    );
    // The refusal left the first Workspace exactly as it was.
    let current = store
        .workspace_for_task(&task)
        .expect("read")
        .expect("still owns one");
    assert_eq!(current.id, "workspace-1");
    assert_eq!(current.resolved_base.as_str(), BASE);
}

#[test]
fn a_workspace_requires_a_task_that_may_own_one() {
    let (_dir, store, _task) = store_with_ready_task("workspace-holder");
    let epoch = store.restore_generation().expect("generation");
    let requested = requested();
    let base = resolved(BASE);

    let err = store
        .open_workspace(
            &command(epoch.as_str(), "cmd-open", &[7u8; 32], "workspace.opened"),
            "workspace-1",
            &binding("task-that-does-not-exist", &requested, &base),
        )
        .expect_err("an absent holder is refused");

    assert!(
        matches!(
            err,
            StoreError::WorkspaceHolderIneligible {
                phase: "Absent",
                ..
            }
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn the_ready_transition_requires_the_base_that_was_durably_bound() {
    let (_dir, store, task) = store_with_ready_task("workspace-base-fence");
    let opened = open(&store, &task, "workspace-1", "cmd-open");
    let materializing = begin(&store, "workspace-1", opened.revision, "cmd-begin");

    let err = complete(
        &store,
        "workspace-1",
        materializing.revision,
        &resolved(OTHER_BASE),
        "cmd-ready",
    )
    .expect_err("a materialization at another base is refused");

    assert!(
        matches!(err, StoreError::WorkspaceBaseMismatch { ref bound, .. } if bound == BASE),
        "unexpected error: {err}"
    );
    // Still Materializing, and still bound to the base it resolved.
    let current = store
        .workspace_for_task(&task)
        .expect("read")
        .expect("exists");
    assert_eq!(current.phase, WorkspacePhase::Materializing);
    assert_eq!(current.resolved_base.as_str(), BASE);
}

#[test]
fn the_full_materialization_path_reaches_ready_with_present_materialization() {
    let (_dir, store, task) = store_with_ready_task("workspace-ready");
    let opened = open(&store, &task, "workspace-1", "cmd-open");
    let materializing = begin(&store, "workspace-1", opened.revision, "cmd-begin");

    // Crossing into Materializing gives up the "nothing exists" conclusion,
    // and does so durably before the caller performs any side effect.
    assert_eq!(materializing.phase, WorkspacePhase::Materializing);
    assert_eq!(materializing.materialization, Materialization::Unknown);

    let ready = executed(
        complete(
            &store,
            "workspace-1",
            materializing.revision,
            &resolved(BASE),
            "cmd-ready",
        )
        .expect("ready commits"),
    );
    assert_eq!(ready.phase, WorkspacePhase::Ready);
    assert_eq!(ready.materialization, Materialization::Present);
    assert_eq!(ready.resolved_base.as_str(), BASE);
}

#[test]
fn a_failed_materialization_records_error_without_claiming_external_absence() {
    let (_dir, store, task) = store_with_ready_task("workspace-failure");
    let opened = open(&store, &task, "workspace-1", "cmd-open");
    let materializing = begin(&store, "workspace-1", opened.revision, "cmd-begin");

    let epoch = store.restore_generation().expect("generation");
    let failed = executed(
        store
            .fail_workspace_materialization(
                &command(epoch.as_str(), "cmd-fail", &[10u8; 32], "workspace.error"),
                "workspace-1",
                materializing.revision,
                Materialization::Unknown,
            )
            .expect("failure commits"),
    );

    assert_eq!(failed.phase, WorkspacePhase::Error);
    // The mission's rule, in one assertion: an error is not proof that the
    // partially created filesystem state is gone.
    assert_eq!(failed.materialization, Materialization::Unknown);

    // And it is still the Task's one current Workspace, bound to the same
    // base, so a retry reconciles rather than creating a second authority.
    let current = store
        .workspace_for_task(&task)
        .expect("read")
        .expect("exists");
    assert_eq!(current.id, "workspace-1");
    assert_eq!(current.resolved_base.as_str(), BASE);
}

#[test]
fn a_failure_path_cannot_claim_verified_materialization() {
    let (_dir, store, task) = store_with_ready_task("workspace-failure-present");
    let opened = open(&store, &task, "workspace-1", "cmd-open");
    let materializing = begin(&store, "workspace-1", opened.revision, "cmd-begin");

    let epoch = store.restore_generation().expect("generation");
    let err = store
        .fail_workspace_materialization(
            &command(epoch.as_str(), "cmd-fail", &[10u8; 32], "workspace.error"),
            "workspace-1",
            materializing.revision,
            Materialization::Present,
        )
        .expect_err("Present on a failure path is refused");
    assert!(matches!(err, StoreError::InvariantViolated(_)), "{err}");

    let _ = task;
}

#[test]
fn a_workspace_that_has_been_mutable_is_never_rematerialized() {
    let (_dir, store, task) = store_with_ready_task("workspace-no-rematerialize");
    let opened = open(&store, &task, "workspace-1", "cmd-open");
    let materializing = begin(&store, "workspace-1", opened.revision, "cmd-begin");
    let ready = executed(
        complete(
            &store,
            "workspace-1",
            materializing.revision,
            &resolved(BASE),
            "cmd-ready",
        )
        .expect("ready commits"),
    );

    let epoch = store.restore_generation().expect("generation");
    let err = store
        .begin_workspace_materialization(
            &command(
                epoch.as_str(),
                "cmd-begin-2",
                &[8u8; 32],
                "workspace.materializing",
            ),
            "workspace-1",
            ready.revision,
        )
        .expect_err("a Ready workspace is not rebuilt underneath its owner");

    assert!(
        matches!(
            err,
            StoreError::WorkspaceNotRematerializable { phase: "Ready", .. }
        ),
        "unexpected error: {err}"
    );
    assert_eq!(
        store
            .workspace_for_task(&task)
            .expect("read")
            .expect("exists")
            .phase,
        WorkspacePhase::Ready
    );
}

#[test]
fn a_workspace_that_failed_before_ever_being_ready_may_be_rebuilt_at_the_same_base() {
    let (_dir, store, task) = store_with_ready_task("workspace-retry");
    let opened = open(&store, &task, "workspace-1", "cmd-open");
    let materializing = begin(&store, "workspace-1", opened.revision, "cmd-begin");

    let epoch = store.restore_generation().expect("generation");
    let failed = executed(
        store
            .fail_workspace_materialization(
                &command(epoch.as_str(), "cmd-fail", &[10u8; 32], "workspace.error"),
                "workspace-1",
                materializing.revision,
                Materialization::Unknown,
            )
            .expect("failure commits"),
    );

    let retried = begin(&store, "workspace-1", failed.revision, "cmd-begin-retry");
    assert_eq!(retried.phase, WorkspacePhase::Materializing);
    assert_eq!(retried.id, "workspace-1", "the same Workspace identity");
    assert_eq!(
        retried.resolved_base.as_str(),
        BASE,
        "and the same immutable base"
    );
}

#[test]
fn replaying_the_opening_command_does_not_create_a_second_workspace() {
    let (_dir, store, task) = store_with_ready_task("workspace-replay");
    open(&store, &task, "workspace-1", "cmd-open");

    let epoch = store.restore_generation().expect("generation");
    let requested = requested();
    let base = resolved(BASE);
    let replay = store
        .open_workspace(
            // Byte-identical identity and request: a retry, not a new command.
            &command(epoch.as_str(), "cmd-open", &[7u8; 32], "workspace.opened"),
            "workspace-1",
            &binding(&task, &requested, &base),
        )
        .expect("the retry reconciles");

    assert!(
        matches!(replay, Committed::Replayed { .. }),
        "a retried command must reconcile rather than execute again"
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT count(*) FROM workspaces")
            .expect("count"),
        vec![1],
        "exactly one durable Workspace exists"
    );
}

#[test]
fn a_partially_materialized_workspace_cannot_be_stored_as_ready() {
    // The controller paths above cannot reach this state, which is the
    // point: the constraint has to hold for any statement, including one a
    // future mission writes. So this drives the table directly.
    let (_dir, store, task) = store_with_ready_task("workspace-check");
    let opened = open(&store, &task, "workspace-1", "cmd-open");
    begin(&store, "workspace-1", opened.revision, "cmd-begin");

    let outcome = store.write(|writer| {
        writer.execute(
            "UPDATE workspaces SET phase = 'Ready' WHERE id = 'workspace-1'",
            &[],
        )
    });

    assert!(
        matches!(outcome, Err(StoreError::Sqlite(_))),
        "the database must refuse Ready without Present materialization: {outcome:?}"
    );
    assert_eq!(
        store
            .workspace_for_task(&task)
            .expect("read")
            .expect("exists")
            .phase,
        WorkspacePhase::Materializing
    );
}
