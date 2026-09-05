use std::sync::atomic::{AtomicU64, Ordering};

use pantheon_core::config::Digest;
use pantheon_core::sandbox::{
    SandboxKey, SandboxMount, SandboxNetworkMode, SandboxPhase, SandboxPlan, SandboxPresence,
};

use crate::{Command, Committed, Store, StoreError};

use super::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_store() -> (Store, std::path::PathBuf) {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "pantheon-store-sandbox-test-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("store.db");
    let store = Store::open(&path).expect("open");
    (store, dir)
}

fn command<'a>(epoch: &'a str, id: &'a str) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: &[0u8; 32],
        event_type: "test",
    }
}

fn test_plan() -> SandboxPlan {
    SandboxPlan {
        sandbox_profile_digest: Digest::of(b"profile"),
        environment_identity: "env-1".to_string(),
        mounts: vec![SandboxMount {
            source: "/tmp/src".to_string(),
            destination: "/dst".to_string(),
            read_only: true,
        }],
        network_mode: SandboxNetworkMode::None,
        cpu_limit_millicores: Some(500),
        memory_limit_mb: Some(512),
    }
}

#[test]
fn create_sandbox_establishes_identity() {
    let (store, _dir) = temp_store();
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");
    let key = SandboxKey::new("sandbox-a").unwrap();
    let plan = test_plan();
    let digest = plan.digest();
    let binding = SandboxBinding {
        run_id: "run-1",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };

    let committed = store.create_sandbox(&cmd, &key, &binding).unwrap();
    let record = match committed {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };

    assert_eq!(record.id, "sandbox-a");
    assert_eq!(record.run_id, "run-1");
    assert_eq!(record.phase, SandboxPhase::Requested);
    assert_eq!(record.observed_presence, SandboxPresence::Absent);
    assert_eq!(record.revision.get(), 1);
}

#[test]
fn create_sandbox_is_idempotent() {
    let (store, _dir) = temp_store();
    let epoch = store.restore_generation().expect("generation");
    let cmd1 = command(epoch.as_str(), "cmd-1");
    let cmd2 = command(epoch.as_str(), "cmd-1");
    let key = SandboxKey::new("sandbox-b").unwrap();
    let plan = test_plan();
    let digest = plan.digest();
    let binding = SandboxBinding {
        run_id: "run-1",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };

    let first = store.create_sandbox(&cmd1, &key, &binding).unwrap();
    let second = store.create_sandbox(&cmd2, &key, &binding).unwrap();

    assert!(matches!(first, Committed::Executed { .. }));
    assert!(matches!(second, Committed::Replayed { .. }));
}

#[test]
fn lifecycle_transitions_work() {
    let (store, _dir) = temp_store();
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");
    let key = SandboxKey::new("sandbox-c").unwrap();
    let plan = test_plan();
    let digest = plan.digest();
    let binding = SandboxBinding {
        run_id: "run-1",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };

    let created = store.create_sandbox(&cmd, &key, &binding).unwrap();
    let record = match created {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };

    let cmd2 = command(epoch.as_str(), "cmd-2");
    let begun = store
        .begin_sandbox_preparation(&cmd2, &key, record.revision)
        .unwrap();
    let record = match begun {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    assert_eq!(record.phase, SandboxPhase::Preparing);

    let cmd3 = command(epoch.as_str(), "cmd-3");
    let completed = store
        .complete_sandbox_preparation(&cmd3, &key, record.revision)
        .unwrap();
    let record = match completed {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    assert_eq!(record.phase, SandboxPhase::Ready);

    let cmd4 = command(epoch.as_str(), "cmd-4");
    let releasing = store
        .begin_sandbox_release(&cmd4, &key, record.revision)
        .unwrap();
    let record = match releasing {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    assert_eq!(record.phase, SandboxPhase::Releasing);

    let cmd5 = command(epoch.as_str(), "cmd-5");
    let released = store
        .complete_sandbox_release(&cmd5, &key, record.revision, SandboxPresence::Absent)
        .unwrap();
    let record = match released {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    assert_eq!(record.phase, SandboxPhase::Released);
}

#[test]
fn revision_conflict_on_stale_update() {
    let (store, _dir) = temp_store();
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");
    let key = SandboxKey::new("sandbox-d").unwrap();
    let plan = test_plan();
    let digest = plan.digest();
    let binding = SandboxBinding {
        run_id: "run-1",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };

    let created = store.create_sandbox(&cmd, &key, &binding).unwrap();
    let record = match created {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };

    let cmd2 = command(epoch.as_str(), "cmd-2");
    store
        .begin_sandbox_preparation(&cmd2, &key, record.revision)
        .unwrap();

    let cmd3 = command(epoch.as_str(), "cmd-3");
    let err = store
        .begin_sandbox_preparation(&cmd3, &key, record.revision)
        .unwrap_err();
    assert!(matches!(err, StoreError::RevisionConflict { .. }));
}

#[test]
fn sandbox_for_run_returns_current() {
    let (store, _dir) = temp_store();
    let epoch = store.restore_generation().expect("generation");
    let cmd = command(epoch.as_str(), "cmd-1");
    let key = SandboxKey::new("sandbox-e").unwrap();
    let plan = test_plan();
    let digest = plan.digest();
    let binding = SandboxBinding {
        run_id: "run-2",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };

    store.create_sandbox(&cmd, &key, &binding).unwrap();

    let found = store.sandbox_for_run("run-2").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, "sandbox-e");

    let missing = store.sandbox_for_run("run-3").unwrap();
    assert!(missing.is_none());
}

#[test]
fn nonreleased_inventory_excludes_released() {
    let (store, _dir) = temp_store();
    let epoch = store.restore_generation().expect("generation");
    let plan = test_plan();

    // Create two sandboxes
    let cmd1 = command(epoch.as_str(), "cmd-1");
    let key1 = SandboxKey::new("sandbox-f1").unwrap();
    let digest = plan.digest();
    let binding1 = SandboxBinding {
        run_id: "run-a",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };
    store.create_sandbox(&cmd1, &key1, &binding1).unwrap();

    let cmd2 = command(epoch.as_str(), "cmd-2");
    let key2 = SandboxKey::new("sandbox-f2").unwrap();
    let binding2 = SandboxBinding {
        run_id: "run-b",
        sandbox_plan_digest: digest.as_bytes(),
        environment_identity: &plan.environment_identity,
    };
    let created = store.create_sandbox(&cmd2, &key2, &binding2).unwrap();
    let record = match created {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };

    // Transition the second one to Ready, then release it
    let cmd3 = command(epoch.as_str(), "cmd-3");
    let preparing = store
        .begin_sandbox_preparation(&cmd3, &key2, record.revision)
        .unwrap();
    let record = match preparing {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    let cmd4 = command(epoch.as_str(), "cmd-4");
    let ready = store
        .complete_sandbox_preparation(&cmd4, &key2, record.revision)
        .unwrap();
    let record = match ready {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    let cmd5 = command(epoch.as_str(), "cmd-5");
    let releasing = store
        .begin_sandbox_release(&cmd5, &key2, record.revision)
        .unwrap();
    let record = match releasing {
        Committed::Executed { value, .. } => value,
        _ => panic!("expected Executed"),
    };
    let cmd6 = command(epoch.as_str(), "cmd-6");
    store
        .complete_sandbox_release(&cmd6, &key2, record.revision, SandboxPresence::Absent)
        .unwrap();

    let inventory = store.nonreleased_sandbox_inventory().unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].id, "sandbox-f1");
}
