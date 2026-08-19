//! Evidence for the durable half of Issue #23: a fresh installation has no
//! configuration authority, activation is one atomic transition through the
//! command envelope, and the durable pointer survives reopen unchanged.

use pantheon_core::config::compile::compile;
use pantheon_core::config::{CompiledConfiguration, Digest};

use crate::command::{Command, Committed};
use crate::error::StoreError;
use crate::store::Store;
use crate::test_support::TempDir;
use crate::transaction::Revision;

/// A minimal internally consistent configuration. Store-level evidence cares
/// about identities and transitions, not component content, so this is
/// deliberately the smallest thing that compiles.
fn source(memory_limit: i64) -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"accepts":["code-change"],"competencies":["rust"],
    "routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["workspace.read"]}}],
  "routing": {{"policies":[{{"name":"default","ordering":["featureMatch"],"tieBreak":"backendId"}}]}},
  "execution": {{
    "profiles":[{{"name":"strict","isolationClass":"CONTAINER",
      "guarantees":["isolation.control-plane"],"networkMode":"NONE",
      "environmentIdentity":"sha256:image"}}],
    "backends":[{{"backendId":"fake-local","enabled":true,"selector":"fake"}}]}},
  "evaluators": {{
    "versions":[{{"id":"unit-v1","kind":"check","argv":["/bin/check"],"timeoutMs":1000,
      "sandboxProfile":"strict","resultProtocol":"p-v1"}}],
    "refs":[{{"ref":"check://p/unit","currentVersion":"unit-v1"}}]}},
  "context": {{"schemaVersion":1,"mandatorySections":["task"],"preloadPriority":["task"],
    "memoryLimitTokens":{memory_limit},"workspaceOrientationLimitTokens":2000,
    "safetyMarginTokens":512,"optionalDropOrder":["memory"]}},
  "authorization": {{"schemaVersion":1,"rules":[{{"action":"workspace.read","effect":"permit"}}]}}
}}"#
    )
}

fn compiled(memory_limit: i64) -> CompiledConfiguration {
    compile(&source(memory_limit)).expect("the fixture configuration compiles")
}

fn open(label: &str) -> (TempDir, Store) {
    let dir = TempDir::new(label);
    let store = Store::open(dir.path().join("pantheon.db")).expect("open store");
    (dir, store)
}

fn activate(
    store: &Store,
    id: &str,
    memory_limit: i64,
    expected: Revision,
) -> Result<Committed<super::ActiveConfiguration>, StoreError> {
    let epoch = store.restore_generation().expect("read generation");
    let compiled = compiled(memory_limit);
    store.activate_configuration(
        &Command {
            epoch: epoch.as_str(),
            id,
            request_hash: &[7u8; 32],
            event_type: "configuration.activated",
        },
        &compiled,
        Digest::of(b"source-set"),
        expected,
    )
}

#[test]
fn a_fresh_installation_has_no_configuration_authority() {
    // AC6: a fresh install is not yet ready for authority-bearing work, and
    // that state is durable and inspectable rather than an in-memory default.
    let (_dir, store) = open("cfg-fresh");
    let pointer = store.configuration_pointer().expect("read pointer");
    assert!(
        pointer.active.is_none(),
        "nothing may be active before the first activation"
    );
    assert_eq!(pointer.revision, Revision::new(1));
}

#[test]
fn the_first_activation_establishes_durable_authority() {
    let (_dir, store) = open("cfg-first");
    let committed = activate(&store, "cmd-1", 4000, Revision::new(1)).expect("activation commits");
    assert!(committed.was_executed());

    let pointer = store.configuration_pointer().expect("read pointer");
    let active = pointer.active.expect("a revision is active");
    assert_eq!(active.activation_sequence, 1);
    assert_eq!(active.content_digest, compiled(4000).revision_digest());
    assert_eq!(active.components, compiled(4000).component_digests());
    // The pointer advanced by exactly one revisioned CAS.
    assert_eq!(pointer.revision, Revision::new(2));
}

#[test]
fn reopening_preserves_the_exact_active_revision_and_component_digests() {
    // AC7: restart reloads and verifies the same durable revision.
    let dir = TempDir::new("cfg-reopen");
    let path = dir.path().join("pantheon.db");
    let before = {
        let store = Store::open(&path).expect("open store");
        activate(&store, "cmd-1", 4000, Revision::new(1)).expect("activation commits");
        let pointer = store.configuration_pointer().expect("read pointer");
        store.close().expect("close");
        pointer.active.expect("active")
    };

    let store = Store::open(&path).expect("reopen store");
    let after = store
        .configuration_pointer()
        .expect("read pointer")
        .active
        .expect("still active");

    assert_eq!(after, before, "reopen must reconstruct the same identity");
}

#[test]
fn a_second_activation_supersedes_the_first_atomically() {
    let (_dir, store) = open("cfg-second");
    activate(&store, "cmd-1", 4000, Revision::new(1)).expect("first activation");
    let pointer = store.configuration_pointer().expect("read pointer");

    activate(&store, "cmd-2", 4096, pointer.revision).expect("second activation");

    let after = store.configuration_pointer().expect("read pointer");
    let active = after.active.expect("active");
    assert_eq!(
        active.activation_sequence, 2,
        "activation history moves forward"
    );
    assert_eq!(active.content_digest, compiled(4096).revision_digest());
    // Every component digest is the new revision's: nothing is left behind
    // from the old one.
    assert_eq!(active.components, compiled(4096).component_digests());
}

#[test]
fn activating_against_a_stale_pointer_observation_loses_deterministically() {
    // Two activations derived from the same observed pointer revision cannot
    // both commit — the same CAS guarantee every revisioned mutation has.
    let (_dir, store) = open("cfg-stale");
    activate(&store, "cmd-1", 4000, Revision::new(1)).expect("first activation");
    let stale = Revision::new(1);

    let err = activate(&store, "cmd-2", 4096, stale).expect_err("a stale activation must fail");
    assert!(
        matches!(err, StoreError::RevisionConflict { .. }),
        "unexpected error: {err}"
    );

    // And the earlier revision is still completely authoritative.
    let active = store
        .configuration_pointer()
        .expect("read pointer")
        .active
        .expect("active");
    assert_eq!(active.activation_sequence, 1);
    assert_eq!(active.content_digest, compiled(4000).revision_digest());
}

#[test]
fn a_failed_activation_leaves_no_revision_and_no_component_behind() {
    // AC12: a failure cannot leave a partially active mixture, and must not
    // advance the pointer to an unusable revision.
    let (_dir, store) = open("cfg-failed");
    activate(&store, "cmd-1", 4000, Revision::new(1)).expect("first activation");

    let epoch = store.restore_generation().expect("generation");
    let pointer = store.configuration_pointer().expect("pointer");
    let candidate = compiled(4096);
    let err = store
        .execute_command(
            &Command {
                epoch: epoch.as_str(),
                id: "cmd-doomed",
                request_hash: &[9u8; 32],
                event_type: "configuration.activated",
            },
            |writer| {
                // The whole activation lands inside this transaction...
                super::write_activation(
                    writer,
                    &candidate,
                    Digest::of(b"source-set"),
                    pointer.revision,
                )?;
                // ...and then fails, after every row has been written.
                Err::<(), StoreError>(StoreError::InvariantViolated("injected".to_string()))
            },
        )
        .expect_err("the injected failure aborts the activation");
    assert!(matches!(err, StoreError::InvariantViolated(ref d) if d == "injected"));

    let active = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active");
    assert_eq!(
        active.activation_sequence, 1,
        "the pointer must not have moved"
    );
    assert_eq!(active.content_digest, compiled(4000).revision_digest());
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM configuration_revisions")
            .expect("count revisions"),
        vec![1],
        "the candidate revision row must not survive"
    );
    // The candidate's distinct component must not survive either: components
    // and the pointer share one transaction, so neither can outlive it.
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM configuration_components")
            .expect("count components"),
        vec![6],
        "only the committed revision's components may exist"
    );
}

#[test]
fn replaying_an_activation_command_does_not_create_a_second_revision() {
    // The configuration integration point with the #18 envelope: a retry
    // reconciles rather than activating again.
    let (_dir, store) = open("cfg-replay");
    activate(&store, "cmd-1", 4000, Revision::new(1)).expect("first activation");

    let replay = activate(&store, "cmd-1", 4000, Revision::new(1)).expect("the retry reconciles");
    assert!(
        !replay.was_executed(),
        "a repeated activation command must not execute again"
    );

    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM configuration_revisions")
            .expect("count"),
        vec![1],
        "replay must not create a second ConfigurationRevision"
    );
    assert_eq!(
        store
            .read_all_for_test("SELECT COUNT(*) FROM event_journal")
            .expect("count"),
        vec![1],
        "replay must not append a duplicate activation Event"
    );
}
