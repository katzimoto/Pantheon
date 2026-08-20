//! Evidence for the operator operations layer: identity derivation is what
//! makes a retried request idempotent, and readiness reports only what this
//! build can establish.

use pantheon_core::planning::goal::{Deliverable, GoalConstraints, GoalInput, GoalSpec};
use pantheon_core::planning::{GoalPhase, TaskPhase};
use pantheon_store::{Command, Store};

use crate::configuration::{ConfigurationAuthority, SourceSet};
use crate::operator::{
    CommandIdentity, ComponentState, OperatorError, OperatorService, derive_command_id,
    derive_goal_id,
};

fn spec() -> GoalSpec {
    GoalSpec {
        objective: "Fix the checkout timeout with the smallest safe change.".to_string(),
        inputs: vec![GoalInput {
            name: "repository".to_string(),
            reference: "repo://whiskyshop".to_string(),
        }],
        deliverables: vec![Deliverable {
            name: "changeset".to_string(),
            kind: "code.changeset".to_string(),
            required: true,
        }],
        constraints: GoalConstraints {
            permitted_effects: vec![
                "filesystem.read".to_string(),
                "filesystem.write".to_string(),
            ],
            forbidden_effects: vec!["git.push".to_string()],
            permitted_resources: vec!["workspace://src/**".to_string()],
        },
    }
}

struct Fixture {
    _dir: tempdir::TempDir,
    store: Store,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = tempdir::TempDir::new(label);
        let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
        Self { _dir: dir, store }
    }
}

mod tempdir {
    use std::path::{Path, PathBuf};

    /// A directory removed when it drops, so tests leave nothing behind.
    pub(super) struct TempDir(PathBuf);

    impl TempDir {
        pub(super) fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pantheon-engine-operator-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        pub(super) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// The smallest configuration that compiles and resolves the DIRECT
/// planner's evaluator reference, so planning has something real to pin.
fn sources() -> SourceSet {
    SourceSet::single(
        "configuration.json",
        r#"{
  "agents": [{"name":"builder","version":1,"accepts":["code-change"],"competencies":["rust"],
    "routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"]}],
  "routing": {"policies":[{"name":"default","ordering":["featureMatch"],"tieBreak":"backendId"}]},
  "execution": {
    "profiles":[{"name":"strict","isolationClass":"CONTAINER",
      "guarantees":["isolation.control-plane"],"networkMode":"NONE",
      "environmentIdentity":"sha256:image"}],
    "backends":[{"backendId":"fake-local","enabled":true,"selector":"fake"}]},
  "evaluators": {
    "versions":[{"id":"unit-v1","kind":"check","argv":["/bin/check"],"timeoutMs":1000,
      "sandboxProfile":"strict","resultProtocol":"p-v1"}],
    "refs":[{"ref":"check://project/unit-tests","currentVersion":"unit-v1"}]},
  "context": {"schemaVersion":1,"mandatorySections":["task"],"preloadPriority":["task"],
    "memoryLimitTokens":4000,"workspaceOrientationLimitTokens":2000,
    "safetyMarginTokens":512,"optionalDropOrder":["memory"]},
  "authorization": {"schemaVersion":1,"rules":[{"action":"filesystem.read","effect":"permit"}]}
}"#,
    )
}

fn activate(store: &Store, authority: &ConfigurationAuthority<&Store>) {
    let epoch = store.restore_generation().expect("generation");
    authority
        .activate(
            &Command {
                epoch: epoch.as_str(),
                id: "activate",
                request_hash: &[1u8; 32],
                event_type: "configuration.activated",
            },
            &sources(),
        )
        .expect("activation commits");
}

fn command(epoch: &str, id: &str, hash: [u8; 32]) -> CommandIdentity {
    CommandIdentity {
        epoch: epoch.to_string(),
        id: id.to_string(),
        request_hash: hash,
    }
}

#[test]
fn creating_a_goal_reaches_one_ready_task() {
    let fixture = Fixture::new("create");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);

    let epoch = fixture.store.restore_generation().expect("generation");
    let goal = service
        .create_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), &spec())
        .expect("the whole path commits");

    // Materialization leaves the Goal Active: the Goal is Active once a valid
    // TaskGraph exists, and this path produces one before it returns.
    assert_eq!(goal.phase, GoalPhase::Active);
    assert_eq!(goal.goal_revision, 1);
    assert_eq!(goal.tasks.len(), 1, "DIRECT produces exactly one Task");
    assert_eq!(goal.tasks[0].phase, TaskPhase::Ready);
    assert_eq!(goal.spec, spec(), "the Goal reads back as it was stored");
}

#[test]
fn retrying_a_create_returns_the_same_goal_rather_than_a_second_one() {
    // The command kernel's replay carries no value, so idempotency depends
    // entirely on the resource identity being derivable from the command.
    let fixture = Fixture::new("create-retry");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);
    let epoch = fixture.store.restore_generation().expect("generation");

    let first = service
        .create_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), &spec())
        .expect("first attempt commits");
    let second = service
        .create_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), &spec())
        .expect("the retry reconciles");

    assert_eq!(first, second);
    assert_eq!(
        service.goals().expect("list").goals.len(),
        1,
        "a retry must not create a second Goal"
    );
}

#[test]
fn a_different_command_id_creates_a_different_goal() {
    let fixture = Fixture::new("create-distinct");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);
    let epoch = fixture.store.restore_generation().expect("generation");

    let first = service
        .create_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), &spec())
        .expect("commits");
    let second = service
        .create_goal(&command(epoch.as_str(), "req-2", [3u8; 32]), &spec())
        .expect("commits");

    assert_ne!(first.id, second.id);
    assert_eq!(service.goals().expect("list").goals.len(), 2);
}

#[test]
fn a_stale_command_epoch_is_refused_before_anything_is_created() {
    let fixture = Fixture::new("stale-epoch");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);

    let err = service
        .create_goal(&command(&"0".repeat(32), "req-1", [2u8; 32]), &spec())
        .expect_err("a stale epoch must fail closed");
    assert!(
        matches!(err, OperatorError::StaleCommandEpoch { .. }),
        "unexpected: {err}"
    );
    assert!(service.goals().expect("list").goals.is_empty());
}

#[test]
fn creating_a_goal_without_usable_configuration_is_a_readiness_failure() {
    // Reported as not-ready rather than as the caller's fault: the request
    // was fine, the daemon was not.
    let fixture = Fixture::new("unconfigured");
    let authority = ConfigurationAuthority::new(&fixture.store);
    let service = OperatorService::new(&fixture.store, &authority);
    let epoch = fixture.store.restore_generation().expect("generation");

    let err = service
        .create_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), &spec())
        .expect_err("planning has nothing to plan against");
    assert!(
        matches!(err, OperatorError::NotReady(_)),
        "unexpected: {err}"
    );
}

#[test]
fn cancelling_an_unknown_goal_is_not_found_rather_than_an_internal_failure() {
    let fixture = Fixture::new("cancel-missing");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);
    let epoch = fixture.store.restore_generation().expect("generation");

    let err = service
        .cancel_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), "goal-nope")
        .expect_err("no such goal");
    assert!(
        matches!(
            err,
            OperatorError::NotFound {
                resource: "goal",
                ..
            }
        ),
        "unexpected: {err}"
    );
}

#[test]
fn cancelling_fences_the_goal_and_its_task_without_terminalizing_either() {
    let fixture = Fixture::new("cancel");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);
    let epoch = fixture.store.restore_generation().expect("generation");

    let created = service
        .create_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), &spec())
        .expect("commits");
    let cancelled = service
        .cancel_goal(&command(epoch.as_str(), "req-2", [3u8; 32]), &created.id)
        .expect("cancellation commits");

    assert_eq!(cancelled.phase, GoalPhase::Finalizing);
    assert_eq!(cancelled.tasks[0].phase, TaskPhase::Finalizing);
    assert!(
        cancelled.revision > created.revision,
        "the ETag must change"
    );
    assert_eq!(
        cancelled.goal_revision, created.goal_revision,
        "cancellation is not a semantic Goal revision"
    );
}

#[test]
fn the_goal_list_cursor_covers_everything_the_list_already_shows() {
    let fixture = Fixture::new("list-cursor");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);
    let epoch = fixture.store.restore_generation().expect("generation");
    service
        .create_goal(&command(epoch.as_str(), "req-1", [2u8; 32]), &spec())
        .expect("commits");

    let page = service.goals().expect("list");
    assert_eq!(page.goals.len(), 1);
    let after = service
        .events_after(&page.snapshot_cursor, 64)
        .expect("watch from the snapshot");
    assert!(after.events.is_empty());
    assert_eq!(
        after.next, page.snapshot_cursor,
        "an empty page must not skip"
    );

    service
        .create_goal(&command(epoch.as_str(), "req-2", [3u8; 32]), &spec())
        .expect("commits");
    let next = service
        .events_after(&page.snapshot_cursor, 64)
        .expect("watch");
    assert!(
        next.events
            .iter()
            .any(|event| event.event_type == "goal.created"),
        "the second Goal's Events must be reachable from the first snapshot"
    );
}

#[test]
fn readiness_reports_what_this_build_cannot_establish_instead_of_asserting_it() {
    let fixture = Fixture::new("readiness");
    let authority = ConfigurationAuthority::new(&fixture.store);
    let service = OperatorService::new(&fixture.store, &authority);

    let before = service.readiness();
    assert!(!before.ready, "nothing is published yet");
    let configuration = before
        .components
        .iter()
        .find(|component| component.name == "active-configuration")
        .expect("the configuration component is reported");
    assert_eq!(configuration.state, ComponentState::Unsatisfied);

    assert!(
        before
            .components
            .iter()
            .any(|component| component.name == "recovery-barrier"
                && component.state == ComponentState::Unimplemented),
        "the missing recovery barrier must be visible, not silently satisfied"
    );

    activate(&fixture.store, &authority);
    let after = service.readiness();
    assert!(
        after.ready,
        "an activated configuration makes the daemon ready"
    );
}

#[test]
fn the_system_view_separates_the_command_epoch_from_the_journal_epoch() {
    // The two rotate for different reasons during disaster restore. A view
    // that reported one for both would make that distinction unobservable.
    let fixture = Fixture::new("system");
    let authority = ConfigurationAuthority::new(&fixture.store);
    activate(&fixture.store, &authority);
    let service = OperatorService::new(&fixture.store, &authority);

    let view = service.system().expect("system view");
    assert_ne!(view.command_epoch, view.journal.epoch);
    assert_eq!(
        view.command_epoch,
        fixture
            .store
            .restore_generation()
            .expect("generation")
            .as_str()
    );
    assert_eq!(view.api_versions, ["v1"]);
    assert!(view.schema_version > 0);
    assert!(view.journal.latest_sequence.is_some());
    assert!(view.active_configuration.is_some());
}

#[test]
fn derived_identities_are_stable_and_do_not_collide_across_steps_or_epochs() {
    // Every idempotency property in this module rests on these being a
    // function of the command identity alone.
    let goal = derive_goal_id("epoch-a", "req-1");
    assert_eq!(goal, derive_goal_id("epoch-a", "req-1"));
    assert_ne!(goal, derive_goal_id("epoch-b", "req-1"));
    assert_ne!(goal, derive_goal_id("epoch-a", "req-2"));

    let plan = derive_command_id("epoch-a", "req-1", "goal-plan");
    assert_eq!(plan, derive_command_id("epoch-a", "req-1", "goal-plan"));
    assert_ne!(plan, derive_command_id("epoch-a", "req-1", "goal-create"));
    assert_ne!(plan, derive_command_id("epoch-b", "req-1", "goal-plan"));

    // Canonical encoding, not concatenation: no split of the parts can be
    // read two ways.
    assert_ne!(
        derive_command_id("a", "bc", "step"),
        derive_command_id("ab", "c", "step")
    );
}
