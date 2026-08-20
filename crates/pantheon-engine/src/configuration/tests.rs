//! Evidence for the publication half of Issue #23: durable authority and the
//! process-local snapshot never disagree, restart reloads rather than
//! redeploys, and source drift is visible without becoming authority.

use pantheon_store::{Command, Store};

use super::{ConfigurationAuthority, ConfigurationError, ConfigurationStatus, SourceSet};

/// A minimal internally consistent configuration, parameterised so a test can
/// produce a semantically different but still valid candidate.
fn source(memory_limit: i64) -> String {
    format!(
        r#"{{
  "agents": [{{"name":"builder","version":1,"accepts":["code-change"],"competencies":["rust"],
    "routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"]}}],
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
  "context": {{"schemaVersion":1,"mandatorySections":["task"],"preloadPriority":["task"],
    "memoryLimitTokens":{memory_limit},"workspaceOrientationLimitTokens":2000,
    "safetyMarginTokens":512,"optionalDropOrder":["memory"]}},
  "authorization": {{"schemaVersion":1,"rules":[{{"action":"filesystem.read","effect":"permit"}}]}}
}}"#
    )
}

fn sources(memory_limit: i64) -> SourceSet {
    SourceSet::single("pantheon.json", source(memory_limit))
}

/// A dependency-free isolated directory, mirroring the store's own helper.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pantheon-engine-test-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated test directory");
        Self(dir)
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.0.join("pantheon.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command<'a>(epoch: &'a str, id: &'a str, hash: &'a [u8; 32]) -> Command<'a> {
    Command {
        epoch,
        id,
        request_hash: hash,
        event_type: "configuration.activated",
    }
}

#[test]
fn a_fresh_installation_publishes_nothing_and_is_not_ready() {
    let dir = TempDir::new("uninit");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);

    let status = authority.load(&sources(4000)).expect("load");
    assert_eq!(status, ConfigurationStatus::Uninitialized);
    assert!(
        matches!(
            authority.snapshot(),
            Err(ConfigurationError::Unavailable(_))
        ),
        "an uninitialized installation must not serve a snapshot"
    );
}

#[test]
fn activation_publishes_the_same_revision_the_database_says_is_active() {
    // AC11: the durable pointer and the in-memory snapshot identify one
    // revision, so no authority-bearing operation can begin against another.
    let dir = TempDir::new("publish");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    let hash = [1u8; 32];

    authority
        .activate(&command(epoch.as_str(), "cmd-1", &hash), &sources(4000))
        .expect("activation commits");

    let snapshot = authority.snapshot().expect("a snapshot is published");
    let durable = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active");
    assert_eq!(
        *snapshot.active(),
        durable,
        "published snapshot and durable pointer must identify the same revision"
    );
    assert!(
        snapshot.compiled().is_some(),
        "the published snapshot carries the compiled semantics"
    );
}

#[test]
fn an_invalid_candidate_changes_neither_durable_nor_published_authority() {
    // AC9: the prior valid revision remains authoritative and the rejection is
    // a typed configuration-level failure.
    let dir = TempDir::new("invalid");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    let first = [1u8; 32];
    authority
        .activate(&command(epoch.as_str(), "cmd-1", &first), &sources(4000))
        .expect("first activation");
    let before = authority.snapshot().expect("snapshot");

    // Syntactically valid, internally inconsistent: the Agent now names a
    // route policy nothing declares.
    let broken = SourceSet::single(
        "pantheon.json",
        source(4096).replacen(r#""routePolicy":"default""#, r#""routePolicy":"ghost""#, 1),
    );
    let second = [2u8; 32];
    let err = authority
        .activate(&command(epoch.as_str(), "cmd-2", &second), &broken)
        .expect_err("an inconsistent candidate is rejected");
    assert!(
        matches!(err, ConfigurationError::Invalid(_)),
        "unexpected error: {err}"
    );

    assert_eq!(
        authority.snapshot().expect("snapshot").active().clone(),
        before.active,
        "the published snapshot must not move"
    );
    let durable = store
        .configuration_pointer()
        .expect("pointer")
        .active
        .expect("active");
    assert_eq!(
        durable.activation_sequence, 1,
        "no new revision was created"
    );
}

#[test]
fn restart_reloads_the_durable_revision_rather_than_the_source() {
    let dir = TempDir::new("restart");
    let path = dir.db_path();
    let established = {
        let store = Store::open(&path).expect("open store");
        let authority = ConfigurationAuthority::new(&store);
        let epoch = store.restore_generation().expect("generation");
        let hash = [1u8; 32];
        authority
            .activate(&command(epoch.as_str(), "cmd-1", &hash), &sources(4000))
            .expect("activation");
        let snapshot = authority.snapshot().expect("snapshot");
        store.close().expect("close");
        snapshot.active().clone()
    };

    let store = Store::open(&path).expect("reopen");
    let authority = ConfigurationAuthority::new(&store);
    let status = authority.load(&sources(4000)).expect("load");

    assert_eq!(status.active(), Some(&established));
    assert!(!status.is_drifted(), "unchanged source is not drift");
    let snapshot = authority.snapshot().expect("snapshot");
    assert_eq!(
        *snapshot.active(),
        established,
        "identity preserved exactly"
    );
    assert_eq!(
        snapshot.active.components, established.components,
        "component digests preserved exactly"
    );
}

#[test]
fn changed_source_is_visible_as_drift_and_is_not_activated_by_restart() {
    // The mission's central distinction: ordinary restart is not deployment.
    let dir = TempDir::new("drift");
    let path = dir.db_path();
    let r1 = {
        let store = Store::open(&path).expect("open store");
        let authority = ConfigurationAuthority::new(&store);
        let epoch = store.restore_generation().expect("generation");
        let hash = [1u8; 32];
        authority
            .activate(&command(epoch.as_str(), "cmd-1", &hash), &sources(4000))
            .expect("activation");
        let active = authority.snapshot().expect("snapshot").active().clone();
        store.close().expect("close");
        active
    };

    // The operator edits the source and restarts without applying it.
    let candidate = sources(4096);
    let store = Store::open(&path).expect("reopen");
    let authority = ConfigurationAuthority::new(&store);
    let status = authority.load(&candidate).expect("load");

    assert!(
        status.is_drifted(),
        "the changed source must be reported as drift"
    );
    assert_eq!(
        status.active(),
        Some(&r1),
        "the durable revision still governs"
    );
    // And the candidate is emphatically not authority.
    let candidate_digest = candidate
        .compile()
        .expect("candidate compiles")
        .revision_digest();
    assert_ne!(
        r1.content_digest, candidate_digest,
        "the fixture must actually differ, or this test proves nothing"
    );
    assert_eq!(
        authority
            .snapshot()
            .expect("snapshot")
            .active
            .content_digest,
        r1.content_digest,
        "the published snapshot must be the durable revision, not the source"
    );
}

#[test]
fn an_explicit_activation_is_what_moves_authority_to_the_new_source() {
    // The other half of the drift rule: applying is deliberate and works.
    let dir = TempDir::new("apply");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    let first = [1u8; 32];
    let second = [2u8; 32];

    authority
        .activate(&command(epoch.as_str(), "cmd-1", &first), &sources(4000))
        .expect("first activation");
    let r1 = authority.snapshot().expect("snapshot").active().clone();

    authority
        .activate(&command(epoch.as_str(), "cmd-2", &second), &sources(4096))
        .expect("second activation");
    let r2 = authority.snapshot().expect("snapshot").active().clone();

    assert_eq!(r2.activation_sequence, 2);
    assert_ne!(r1.content_digest, r2.content_digest);
    assert_eq!(
        store
            .configuration_pointer()
            .expect("pointer")
            .active
            .expect("active"),
        r2,
        "durable and published agree after the transition"
    );
    // No mixed generation: every component digest belongs to R2.
    let expected = sources(4096)
        .compile()
        .expect("compiles")
        .component_digests();
    assert_eq!(r2.components, expected);
}

#[test]
fn a_replayed_activation_leaves_the_published_snapshot_where_it_was() {
    let dir = TempDir::new("replay");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    let hash = [1u8; 32];

    authority
        .activate(&command(epoch.as_str(), "cmd-1", &hash), &sources(4000))
        .expect("first activation");
    let before = authority.snapshot().expect("snapshot").active().clone();

    let replay = authority
        .activate(&command(epoch.as_str(), "cmd-1", &hash), &sources(4000))
        .expect("the retry reconciles");
    assert!(!replay.was_executed());
    assert_eq!(
        authority.snapshot().expect("snapshot").active().clone(),
        before,
        "a replay must not move published authority"
    );
}

#[test]
fn published_semantics_always_describe_the_published_identity() {
    // The mixed-generation hazard, closed structurally: a snapshot cannot pair
    // one revision's identity with another's compiled configuration, so a
    // publication path that wrote them out of order would fail rather than
    // serve a blended view.
    let dir = TempDir::new("paired");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    let first = [1u8; 32];
    let second = [2u8; 32];

    authority
        .activate(&command(epoch.as_str(), "cmd-1", &first), &sources(4000))
        .expect("first activation");
    authority
        .activate(&command(epoch.as_str(), "cmd-2", &second), &sources(4096))
        .expect("second activation");

    let snapshot = authority.snapshot().expect("snapshot");
    let compiled = snapshot.compiled().expect("semantics are published");
    assert_eq!(
        compiled.revision_digest(),
        snapshot.active().content_digest,
        "published semantics and published identity must be the same revision"
    );
    assert_eq!(
        compiled.component_digests(),
        snapshot.active().components,
        "and every component digest must agree"
    );
}

#[test]
fn an_activation_rejected_by_the_store_leaves_the_published_snapshot_whole() {
    // The failure window the publication barrier actually has: the candidate
    // compiles, the lock is taken, and the durable activation is then rejected
    // — here by reusing a command identity with a different request hash.
    // Both halves of the published snapshot must still describe R1.
    let dir = TempDir::new("rejected");
    let store = Store::open(dir.db_path()).expect("open store");
    let authority = ConfigurationAuthority::new(&store);
    let epoch = store.restore_generation().expect("generation");
    let first = [1u8; 32];
    let conflicting = [2u8; 32];

    authority
        .activate(&command(epoch.as_str(), "cmd-1", &first), &sources(4000))
        .expect("first activation");
    let before = authority.snapshot().expect("snapshot");
    let before_identity = before.active().clone();
    let before_semantics = before
        .compiled()
        .expect("semantics published")
        .revision_digest();

    // Same command id, different request hash, and a genuinely different
    // valid candidate — so a premature publication would be observable.
    let err = authority
        .activate(
            &command(epoch.as_str(), "cmd-1", &conflicting),
            &sources(4096),
        )
        .expect_err("a conflicting command identity is rejected");
    assert!(
        matches!(err, ConfigurationError::Store(_)),
        "unexpected error: {err}"
    );

    let after = authority.snapshot().expect("snapshot");
    assert_eq!(
        *after.active(),
        before_identity,
        "the published identity must not move"
    );
    assert_eq!(
        after.compiled().expect("semantics").revision_digest(),
        before_semantics,
        "the published semantics must not move either"
    );
    assert_eq!(
        store
            .configuration_pointer()
            .expect("pointer")
            .active
            .expect("active")
            .activation_sequence,
        1,
        "and no revision was created"
    );
}
