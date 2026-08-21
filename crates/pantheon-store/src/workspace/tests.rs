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
use crate::scheduling::tests::dispatch_ready_task;
use crate::seal::SealAuthority;
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;
use crate::workspace::{WorkspaceBinding, WorkspaceRecord};

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
fn a_workspace_that_has_been_mutable_cannot_be_recorded_as_a_materialization_failure() {
    // The rematerialization fence is *derivative*: it asks whether the current
    // phase has ever been mutable, and `Error` reads as never-mutable. So the
    // transition into `Error` is where invariant 20 is actually weakest — a
    // Ready row moved here would lose the only durable evidence protecting it,
    // and the next rebuild would pass every remaining check and delete the
    // worker's work. Nothing shipped calls this against a Ready Workspace; the
    // fence exists so nothing ever can.
    let (_dir, store, task) = store_with_ready_task("workspace-failure-after-ready");
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
        .fail_workspace_materialization(
            &command(epoch.as_str(), "cmd-fail", &[10u8; 32], "workspace.error"),
            "workspace-1",
            ready.revision,
            Materialization::Unknown,
        )
        .expect_err("a Ready workspace cannot be demoted to Error");

    assert!(
        matches!(
            err,
            StoreError::WorkspaceNotRematerializable { phase: "Ready", .. }
        ),
        "unexpected error: {err}"
    );

    // The evidence survived, so the fence that reads it still holds.
    let current = store
        .workspace_for_task(&task)
        .expect("read")
        .expect("exists");
    assert_eq!(current.phase, WorkspacePhase::Ready);
    assert_eq!(current.materialization, Materialization::Present);
    assert_eq!(current.revision, ready.revision, "no revision was burned");

    // And the rebuild path is still refused, which is the property the fence
    // above exists to protect rather than merely to state.
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
        .expect_err("and the workspace is still not rebuildable");
    assert!(
        matches!(
            err,
            StoreError::WorkspaceNotRematerializable { phase: "Ready", .. }
        ),
        "unexpected error: {err}"
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

/// Drives one Workspace all the way to Ready, with a distinct durable
/// command identity per transition.
fn ready(store: &Store, id: &str, command_id: &str) -> WorkspaceRecord {
    let opened = open(store, "task-1", id, &format!("{command_id}:open"));
    let begun = begin(store, id, opened.revision, &format!("{command_id}:begin"));
    executed(
        complete(
            store,
            id,
            begun.revision,
            &resolved(BASE),
            &format!("{command_id}:complete"),
        )
        .expect("Ready"),
    )
}

/// Dispatches the fixture Task through T3 so a seal authority exists. The
/// Workspace must already be Ready: T3 selects only Tasks owning one.
fn dispatch(store: &Store, ws_id: &str) {
    dispatch_ready_task(store, "goal-1", "task-1", ws_id, BASE, RUN, "cmd-t3");
}

fn freeze(
    store: &Store,
    id: &str,
    expected: Revision,
    command_id: &str,
) -> Result<Committed<WorkspaceRecord>, StoreError> {
    let epoch = store.restore_generation().expect("generation");
    store.freeze_workspace(
        &command(epoch.as_str(), command_id, &[11u8; 32], "workspace.frozen"),
        &run_authority(1),
        "task-1",
        "changeset",
        id,
        expected,
    )
}

#[test]
fn freezing_a_ready_workspace_suspends_mutation_authority_and_keeps_presence() {
    let (_dir, store, _task) = store_with_ready_task("freeze-happy");
    ready(&store, "workspace-1", "cmd-ready");
    dispatch(&store, "workspace-1");
    let current = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");

    let frozen =
        executed(freeze(&store, "workspace-1", current.revision, "cmd-freeze").expect("freezes"));
    assert_eq!(frozen.phase, WorkspacePhase::Frozen);
    assert_eq!(
        frozen.materialization,
        Materialization::Present,
        "a freeze is a fence, not an observation"
    );
    assert_eq!(frozen.revision.get(), current.revision.get() + 1);
}

#[test]
fn freezing_requires_the_current_run_not_merely_a_ready_task() {
    // Post-#29 a Ready Task owns zero Runs and no execution has happened:
    // there is nothing settled to seal and no Run relation to authorize it.
    let (_dir, store, _task) = store_with_ready_task("freeze-no-run");
    ready(&store, "workspace-1", "cmd-ready");
    let current = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");

    let err = freeze(&store, "workspace-1", current.revision, "cmd-freeze")
        .expect_err("a Ready Task with no Run cannot authorize a seal");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );

    let current = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    assert_eq!(
        current.phase,
        WorkspacePhase::Ready,
        "the refusal wrote nothing: no freeze exists"
    );
}

#[test]
fn freeze_refuses_stale_or_unauthorized_runs_and_freezes_nothing() {
    enum Fault {
        NonexistentRun,
        StaleRevision,
        WrongTaskRun,
        TerminalRun,
        NotCurrentRun,
        UnknownSlot,
    }

    for (fault, label) in [
        (Fault::NonexistentRun, "nonexistent-run"),
        (Fault::StaleRevision, "stale-revision"),
        (Fault::WrongTaskRun, "wrong-task-run"),
        (Fault::TerminalRun, "terminal-run"),
        (Fault::NotCurrentRun, "not-current-run"),
        (Fault::UnknownSlot, "unknown-slot"),
    ] {
        let (_dir, store, _task) = store_with_ready_task("freeze-fault");
        ready(&store, "workspace-1", "cmd-ready");
        dispatch(&store, "workspace-1");
        let current = store
            .workspace_for_task("task-1")
            .expect("read")
            .expect("current");

        let authority = match fault {
            Fault::NonexistentRun => SealAuthority {
                run_id: "run-absent".to_string(),
                expected_run_revision: Revision::new(1),
            },
            Fault::StaleRevision => run_authority(99),
            Fault::WrongTaskRun => {
                insert_foreign_run(&store);
                SealAuthority {
                    run_id: "run-foreign".to_string(),
                    expected_run_revision: Revision::new(1),
                }
            }

            Fault::TerminalRun => {
                terminalize_current_run(&store);
                run_authority(1)
            }
            // A second Run of this Task exists and is nonterminal, but the
            // Task's pointer still names the terminal first Run: claiming
            // the second must refuse on currentness alone.
            Fault::NotCurrentRun => {
                insert_superseding_run(&store);
                SealAuthority {
                    run_id: "run-2".to_string(),
                    expected_run_revision: Revision::new(1),
                }
            }
            Fault::UnknownSlot => run_authority(1),
        };
        let slot = match fault {
            Fault::UnknownSlot => "no-such-slot",
            _ => "changeset",
        };

        let epoch = store.restore_generation().expect("generation");
        let Err(err) = store.freeze_workspace(
            &command(
                epoch.as_str(),
                "cmd-freeze",
                &[11u8; 32],
                "workspace.frozen",
            ),
            &authority,
            "task-1",
            slot,
            "workspace-1",
            current.revision,
        ) else {
            panic!("{label}: the faulting scenario must refuse the freeze");
        };
        assert!(
            matches!(err, StoreError::SealAuthorityInvalid { .. }),
            "{label}: {err}"
        );

        let after = store
            .workspace_for_task("task-1")
            .expect("read")
            .expect("current");
        assert_eq!(
            after.phase,
            WorkspacePhase::Ready,
            "{label}: nothing was frozen"
        );
        assert_eq!(
            after.revision, current.revision,
            "{label}: no revision burned"
        );
    }
}

/// Inserts a Run owned by another Task (which exists, satisfying the FK),
/// to prove holder identity is checked against the sealing Task. The
/// fixture's own Run is terminalized first so the single global slot can
/// hold the foreign one.
fn insert_foreign_run(store: &Store) {
    terminalize_current_run(store);
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
                 VALUES ('run-foreign', 'task-2', 'Active', NULL, 1, NULL, unixepoch())",
                &[],
            )
        })
        .expect("fixture run inserts");
}

/// Terminalizes the fixture's current Run in place, keeping its revision:
/// exactly what a cancelled or failed execution leaves behind.
fn terminalize_current_run(store: &Store) {
    store
        .write(|writer| {
            writer.execute(
                "UPDATE run_status SET phase = 'Failed', active_slot = NULL
                 WHERE run_id = 'run-1'",
                &[],
            )
        })
        .expect("fixture terminalizes");
}

/// Gives the Task a second, nonterminal Run while its responsible-Run
/// pointer still names the terminal first one — the shape that isolates the
/// not-current rejection from every other arm.
fn insert_superseding_run(store: &Store) {
    terminalize_current_run(store);
    store
        .write(|writer| {
            writer.execute(
                "INSERT INTO runs (id, task_id, binding_digest,
                        context_source_snapshot_digest, created_at)
                 SELECT 'run-2', task_id, binding_digest,
                        context_source_snapshot_digest, unixepoch()
                 FROM runs WHERE id = 'run-1'",
                &[],
            )?;
            writer.execute(
                "INSERT INTO run_status (run_id, task_id, phase, terminal_target,
                        revision, active_slot, updated_at)
                 VALUES ('run-2', 'task-1', 'Active', NULL, 1, 'global', unixepoch())",
                &[],
            )
        })
        .expect("fixture superseding run inserts");
}

#[test]
fn only_a_verified_ready_workspace_can_be_frozen() {
    let (_dir, store, _task) = store_with_ready_task("freeze-refuses");
    // Still Requested: never materialized, nothing to capture.
    open(&store, "task-1", "workspace-1", "cmd-open");
    let current = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    let err = freeze(&store, "workspace-1", current.revision, "cmd-freeze")
        .expect_err("Requested cannot be frozen");
    assert!(
        matches!(err, StoreError::WorkspaceNotFreezable { .. }),
        "{err}"
    );

    // And a row that does not exist at all is a revision conflict, not a
    // freeze failure.
    let err = freeze(&store, "no-such-workspace", Revision::new(1), "cmd-freeze")
        .expect_err("must refuse");
    assert!(matches!(err, StoreError::RevisionConflict { .. }), "{err}");
}

#[test]
fn freezing_twice_from_the_same_observation_is_one_durable_transition() {
    let (_dir, store, _task) = store_with_ready_task("freeze-replay");
    ready(&store, "workspace-1", "cmd-ready");
    dispatch(&store, "workspace-1");
    let current = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");

    let first = freeze(&store, "workspace-1", current.revision, "same-command").expect("freezes");
    // Same identity + same request hash: replayed, not re-executed.
    let second = freeze(&store, "workspace-1", current.revision, "same-command").expect("replays");
    assert!(!second.was_executed());
    assert_eq!(
        executed(first).revision,
        store
            .workspace_for_task("task-1")
            .expect("read")
            .expect("current")
            .revision
    );
}

#[test]
fn capture_failure_evidence_requires_the_fence_it_claims() {
    let (_dir, store, _task) = store_with_ready_task("capture-failure");
    ready(&store, "workspace-1", "cmd-ready");
    dispatch(&store, "workspace-1");
    let epoch = store.restore_generation().expect("generation");
    let record_failure = |expected: Revision| {
        store.record_capture_failure(
            &command(
                epoch.as_str(),
                "cmd-failed",
                &[12u8; 32],
                "workspace.capture-failed",
            ),
            "workspace-1",
            expected,
        )
    };

    // Not frozen yet: recording failure would claim a fence that does not
    // hold.
    let current = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    let err = record_failure(current.revision).expect_err("must refuse while not frozen");
    assert!(matches!(err, StoreError::InvariantViolated(_)), "{err}");

    // Frozen at the fenced revision: the evidence records without changing
    // any lifecycle fact.
    freeze(&store, "workspace-1", current.revision, "cmd-freeze").expect("freezes");
    let frozen = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    let recorded = executed(record_failure(frozen.revision).expect("evidence records"));
    assert_eq!(recorded.phase, WorkspacePhase::Frozen);
    assert_eq!(recorded.materialization, Materialization::Present);
}

#[test]
fn an_already_frozen_retry_revalidates_current_run_authority() {
    // The Workspace fence surviving an earlier attempt must not stand in
    // for authorization: the retry boundary re-proves the Run relation
    // before capture, and refuses without writing anything when it is gone.
    let (_dir, store, _task) = store_with_ready_task("frozen-revalidate");
    ready(&store, "workspace-1", "cmd-ready");
    dispatch(&store, "workspace-1");
    let pre_freeze = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current")
        .revision;
    let frozen_record =
        executed(freeze(&store, "workspace-1", pre_freeze, "cmd-freeze").expect("freezes"));
    let fence = frozen_record.revision;

    let revalidate = |authority: &SealAuthority, expected: Revision| {
        let epoch = store.restore_generation().expect("generation");
        store.validate_seal_authority_command(
            &command(
                epoch.as_str(),
                "cmd-revalidate",
                &[13u8; 32],
                "workspace.seal-authority.validated",
            ),
            authority,
            "task-1",
            "changeset",
            "workspace-1",
            expected,
        )
    };

    // A stale claimed revision is refused against current durable state.
    let err = revalidate(&run_authority(99), fence).expect_err("stale run authority must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );

    // A Run that went terminal after the fence is refused too.
    terminalize_current_run(&store);
    let err = revalidate(&run_authority(1), fence).expect_err("terminal run authority must refuse");
    assert!(
        matches!(err, StoreError::SealAuthorityInvalid { .. }),
        "{err}"
    );

    // And a moved fence is a conflict before any authority question.
    dispatch_superseding_pointer(&store);
    let err = revalidate(&run_authority(1), Revision::new(fence.get() + 1))
        .expect_err("a moved fence must refuse as a conflict");
    assert!(
        matches!(
            err,
            StoreError::RevisionConflict {
                table: "workspaces",
                ..
            }
        ),
        "{err}"
    );

    // None of the refusals thawed the Workspace or burned a revision.
    let after = store
        .workspace_for_task("task-1")
        .expect("read")
        .expect("current");
    assert_eq!(after.phase, WorkspacePhase::Frozen);
    assert_eq!(after.revision, fence);
}

/// Repairs the fixture's responsibility pointer to the superseding Run so
/// later fixture work stays coherent (used only after deliberate corruption
/// above).
fn dispatch_superseding_pointer(store: &Store) {
    insert_superseding_run(store);
    store
        .write(|writer| {
            writer.execute(
                "UPDATE tasks SET active_run_id = 'run-2' WHERE id = 'task-1'",
                &[],
            )
        })
        .expect("fixture pointer moves");
}
