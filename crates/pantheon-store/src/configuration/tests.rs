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
pub(crate) fn source(memory_limit: i64) -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"accepts":["code-change"],"competencies":["rust"],
    "routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"],"soul":"Careful coding agent identity.","behavior":"Plan first; keep changes minimal."}}],
  "routing": {{"policies":[{{"name":"default","ordering":["contextCapacity"],"tieBreak":"backendId"}}]}},
  "execution": {{
    "profiles":[{{"name":"strict","isolationClass":"CONTAINER",
      "guarantees":["isolation.control-plane"],"networkMode":"NONE",
      "environmentIdentity":"sha256:image"}}],
    "backends":[{{"backendId":"fake-local","enabled":true,"selector":"fake"}}]}},
  "evaluators": {{
    "versions":[{{"id":"unit-v1","kind":"check","argv":["/bin/check"],"timeoutMs":1000,
      "sandboxProfile":"strict","resultProtocol":"p-v1"}}],
    "refs":[{{"ref":"check://p/unit","currentVersion":"unit-v1"}}]}},
  "context": {{"schemaVersion":1,"mandatorySections":["task-contract","goal-contract","agent-soul","agent-behavior"],"preloadPriority":["workspace-orientation"],
    "memoryLimitTokens":{memory_limit},"workspaceOrientationLimitTokens":2000,
    "safetyMarginTokens":512,"optionalDropOrder":["workspace-orientation"]}},
  "authorization": {{"schemaVersion":1,"rules":[{{"action":"filesystem.read","effect":"permit"}}]}}
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

#[test]
fn a_tampered_component_payload_fails_closed_on_reload() {
    // The configuration contract requires startup to *verify* the component
    // hashes, not merely read them. A payload that no longer hashes to the
    // identity the active revision names must not be served as authority.
    let dir = TempDir::new("cfg-tampered");
    let path = dir.path().join("pantheon.db");
    {
        let store = Store::open(&path).expect("open store");
        activate(&store, "cmd-1", 4000, Revision::new(1)).expect("activation");
        store.close().expect("close");
    }

    {
        let conn = rusqlite::Connection::open(&path).expect("raw connection");
        let changed = conn
            .execute(
                "UPDATE configuration_components
                 SET canonical_json = '{\"tampered\":true}'
                 WHERE domain = 'context'",
                [],
            )
            .expect("tamper with a stored component");
        assert_eq!(
            changed, 1,
            "the tamper must have landed, or this proves nothing"
        );
    }

    let store = Store::open(&path).expect("reopen");
    let err = store
        .configuration_pointer()
        .expect_err("a tampered component must fail closed");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("hashes to")),
        "unexpected error: {err}"
    );
}

#[test]
fn a_component_referenced_by_a_revision_cannot_be_deleted() {
    // The first layer of the same protection, and the one that actually
    // fires: the revision's foreign keys make a referenced component
    // undeletable, so "the active revision names a component that is not
    // stored" is unreachable through ordinary deletion rather than merely
    // detected afterwards.
    let dir = TempDir::new("cfg-missing-component");
    let path = dir.path().join("pantheon.db");
    {
        let store = Store::open(&path).expect("open store");
        activate(&store, "cmd-1", 4000, Revision::new(1)).expect("activation");
        store.close().expect("close");
    }

    let conn = rusqlite::Connection::open(&path).expect("raw connection");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("enforce foreign keys");
    let err = conn
        .execute(
            "DELETE FROM configuration_components WHERE domain = 'routing'",
            [],
        )
        .expect_err("a referenced component must not be deletable");
    assert!(
        matches!(
            err,
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
        ),
        "unexpected error: {err}"
    );

    // And the store still loads the revision it always had.
    let store = Store::open(&path).expect("reopen");
    let active = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active");
    assert_eq!(active.activation_sequence, 1);
}

#[test]
fn a_component_stored_under_the_wrong_domain_fails_closed() {
    // Content addressing alone does not prove a component is being used as
    // what it was compiled as.
    let dir = TempDir::new("cfg-wrong-domain");
    let path = dir.path().join("pantheon.db");
    {
        let store = Store::open(&path).expect("open store");
        activate(&store, "cmd-1", 4000, Revision::new(1)).expect("activation");
        store.close().expect("close");
    }

    {
        let conn = rusqlite::Connection::open(&path).expect("raw connection");
        let changed = conn
            .execute(
                "UPDATE configuration_components SET domain = 'evaluators' WHERE domain = 'context'",
                [],
            )
            .expect("relabel a stored component");
        assert_eq!(changed, 1);
    }

    let store = Store::open(&path).expect("reopen");
    let err = store
        .configuration_pointer()
        .expect_err("a relabelled component must fail closed");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("stored as")),
        "unexpected error: {err}"
    );
}

#[test]
fn a_revision_whose_content_digest_disagrees_with_its_components_fails_closed() {
    // Component payloads can each be intact while the manifest as a whole is
    // not the revision it claims to be. Without recomputing the revision
    // identity from the component digests, a swapped `content_digest` would let
    // a caller recover a different revision's semantics while the durable
    // component bindings still belong to this one.
    let dir = TempDir::new("cfg-tampered-manifest");
    let path = dir.path().join("pantheon.db");
    {
        let store = Store::open(&path).expect("open store");
        activate(&store, "cmd-1", 4000, Revision::new(1)).expect("activation");
        store.close().expect("close");
    }

    {
        let conn = rusqlite::Connection::open(&path).expect("raw connection");
        // A different, well-formed 32-byte digest: every component row stays
        // valid and individually verifiable.
        let changed = conn
            .execute(
                "UPDATE configuration_revisions SET content_digest = ?1 WHERE activation_sequence = 1",
                rusqlite::params![vec![0x5au8; 32]],
            )
            .expect("tamper with the revision manifest");
        assert_eq!(
            changed, 1,
            "the tamper must have landed, or this proves nothing"
        );
    }

    let store = Store::open(&path).expect("reopen");
    let err = store
        .configuration_pointer()
        .expect_err("a manifest that disagrees with its components must fail closed");
    assert!(
        matches!(err, StoreError::InvariantViolated(ref d) if d.contains("produce")),
        "unexpected error: {err}"
    );
}
