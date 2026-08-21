//! Evidence for the `code.changeset` identity rules: lossless paths,
//! operation/state agreement, and deterministic identity.

use super::*;

fn digest(text: &str) -> Digest {
    Digest::of(text.as_bytes())
}

fn path(bytes: &[u8]) -> RepositoryPath {
    RepositoryPath::from_bytes(bytes).expect("valid fixture path")
}

#[test]
fn ordinary_paths_spell_themselves_in_the_manifest() {
    let p = path(b"src/checkout/main.rs");
    assert_eq!(p.to_manifest_string(), "src/checkout/main.rs");
    assert_eq!(
        RepositoryPath::from_manifest_string("src/checkout/main.rs").expect("round trip"),
        p
    );
}

#[test]
fn non_utf8_paths_survive_losslessly() {
    // Valid UTF-8 except one byte: a Latin-1 filename, the classic case.
    let raw = [b's', b'r', b'c', b'/', 0xE9, b'.', b't', b'x', b't'];
    let p = RepositoryPath::from_bytes(&raw).expect("non-UTF-8 path is representable");
    let spelling = p.to_manifest_string();
    assert_ne!(
        spelling,
        String::from_utf8_lossy(&raw),
        "no lossy conversion"
    );
    assert_eq!(
        RepositoryPath::from_manifest_string(&spelling).expect("decodes"),
        p,
        "the encoding round-trips exactly"
    );
    // The decoded bytes equal the original bytes, not their replacement-char
    // rendering.
    assert_eq!(
        RepositoryPath::from_manifest_string(&spelling)
            .expect("decodes")
            .as_bytes(),
        &raw
    );
}

#[test]
fn percent_containing_paths_cannot_be_confused_with_escapes() {
    // A valid UTF-8 path containing `%` must not decode as an escape form.
    let p = path(b"a%62c.txt");
    let spelling = p.to_manifest_string();
    assert_eq!(
        RepositoryPath::from_manifest_string(&spelling).expect("decodes"),
        p,
        "literal % forces full-hex encoding so decoding stays injective"
    );
}

#[test]
fn structurally_impossible_paths_are_refused() {
    for bad in [
        &b""[..],
        b"/absolute",
        b"trailing/",
        b"double//slash",
        b"dot/./path",
        b"up/../path",
        b"nul\0byte",
    ] {
        let err = RepositoryPath::from_bytes(bad).expect_err("must refuse");
        assert!(matches!(err, ChangesetError::InvalidPath { .. }), "{err}");
    }
    // A long component is refused with its own reason rather than by
    // truncation.
    let long = vec![b'a'; 256];
    assert!(RepositoryPath::from_bytes(&long).is_err());
    assert!(RepositoryPath::from_bytes(&long[..255]).is_ok());
}

#[test]
fn entries_enforce_operation_state_agreement() {
    let present = |kind| EntryState::Present {
        kind,
        blob: digest("bytes"),
        size: 5,
    };
    let absent = EntryState::Absent;
    let p = path(b"f");

    ChangesetEntry::new(
        p.clone(),
        Operation::Add,
        absent.clone(),
        present(EntryKind::Regular),
    )
    .expect("add agrees");
    ChangesetEntry::new(
        p.clone(),
        Operation::Modify,
        present(EntryKind::Regular),
        present(EntryKind::Executable),
    )
    .expect("modify agrees");
    ChangesetEntry::new(
        p.clone(),
        Operation::Delete,
        present(EntryKind::Symlink),
        absent.clone(),
    )
    .expect("delete agrees");

    for (operation, before, after) in [
        (
            Operation::Add,
            present(EntryKind::Regular),
            present(EntryKind::Regular),
        ),
        (Operation::Add, absent.clone(), absent.clone()),
        (
            Operation::Modify,
            absent.clone(),
            present(EntryKind::Regular),
        ),
        (
            Operation::Modify,
            present(EntryKind::Regular),
            absent.clone(),
        ),
        (
            Operation::Delete,
            absent.clone(),
            present(EntryKind::Symlink),
        ),
        (Operation::Delete, absent.clone(), absent),
    ] {
        let err =
            ChangesetEntry::new(p.clone(), operation, before, after).expect_err("must refuse");
        assert!(
            matches!(err, ChangesetError::OperationStateMismatch { .. }),
            "{err}"
        );
    }
}

/// Builds one final state entry from content text.
fn final_entry(bytes: &[u8], kind: EntryKind) -> FinalEntry {
    FinalEntry {
        path: path(bytes),
        kind,
        blob: digest(&format!("{bytes:?}-{kind:?}")),
        size: 3,
    }
}

#[test]
fn revision_state_identity_is_order_independent_and_content_determined() {
    let a = vec![
        final_entry(b"src/a.rs", EntryKind::Regular),
        final_entry(b"src/lib/b.rs", EntryKind::Executable),
        final_entry(b"link", EntryKind::Symlink),
    ];
    let reversed = {
        let mut entries = a.clone();
        entries.reverse();
        entries
    };

    let first = RevisionState::new(a).expect("unique paths");
    let second = RevisionState::new(reversed).expect("unique paths");

    assert_eq!(first.digest(), second.digest());
    // Entries come back sorted by canonical path bytes.
    let keys: Vec<&[u8]> = first.entries().iter().map(|e| e.path.sort_key()).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
}

#[test]
fn duplicate_paths_are_refused_rather_than_silently_merged() {
    let entries = vec![
        final_entry(b"same", EntryKind::Regular),
        final_entry(b"same", EntryKind::Executable),
    ];
    assert!(RevisionState::new(entries).is_err());
}

#[test]
fn semantically_identical_states_produce_one_digest_across_incidental_variation() {
    // The same logical file under different sizes-of-noise would be a
    // different state; what must NOT change the digest is anything outside
    // the semantic content. There is no field here that could carry it —
    // which is the point. Build two states differing only in construction
    // order and confirm equality, then confirm real differences move it.
    let base = RevisionState::new(vec![final_entry(b"one", EntryKind::Regular)]).expect("ok");
    let same = RevisionState::new(vec![final_entry(b"one", EntryKind::Regular)]).expect("ok");
    assert_eq!(base.digest(), same.digest());

    let other_kind =
        RevisionState::new(vec![final_entry(b"one", EntryKind::Executable)]).expect("ok");
    assert_ne!(base.digest(), other_kind.digest());

    let other_content = FinalEntry {
        blob: digest("different"),
        ..final_entry(b"one", EntryKind::Regular)
    };
    let other = RevisionState::new(vec![other_content]).expect("ok");
    assert_ne!(base.digest(), other.digest());
}

#[test]
fn manifest_binds_contract_fields_and_nothing_incidental() {
    let state = RevisionState::new(vec![
        final_entry(b"src/new.rs", EntryKind::Regular),
        final_entry(b"src/gone.rs", EntryKind::Regular),
    ])
    .expect("ok");

    let add = ChangesetEntry::new(
        path(b"src/new.rs"),
        Operation::Add,
        EntryState::Absent,
        EntryState::Present {
            kind: EntryKind::Regular,
            blob: digest("after-new"),
            size: 9,
        },
    )
    .expect("agrees");
    let delete = ChangesetEntry::new(
        path(b"src/gone.rs"),
        Operation::Delete,
        EntryState::Present {
            kind: EntryKind::Regular,
            blob: digest("before-gone"),
            size: 4,
        },
        EntryState::Absent,
    )
    .expect("agrees");

    let manifest = changeset_manifest(
        "repo://project",
        "1111111111111111111111111111111111111111",
        state.digest(),
        &[add, delete],
    );

    let json = manifest.to_string();
    assert!(
        json.contains(r#""artifactKind":"code.changeset""#),
        "{json}"
    );
    assert!(json.contains(r#""schemaVersion":1"#), "{json}");
    assert!(json.contains(r#""repository":"repo://project""#), "{json}");
    assert!(json.contains("\"workspaceRevision\":\"sha256:"), "{json}");
    // No incidental provenance may appear: time, row ids, CAS locations,
    // command ids are relational facts, never manifest content.
    assert!(!json.contains("created_at"), "{json}");
    assert!(!json.contains("command"), "{json}");

    // Entries render ordered by canonical path bytes regardless of input
    // order: src/gone.rs sorts before src/new.rs.
    let gone = json.find("src/gone.rs").expect("delete present");
    let new = json.find("src/new.rs").expect("add present");
    assert!(gone < new, "entries must be canonically ordered: {json}");

    // Identity depends on every binding: change any of them and the digest
    // moves.
    let full = changeset_manifest(
        "repo://project",
        "1111111111111111111111111111111111111111",
        state.digest(),
        &[],
    );
    let other_repo = changeset_manifest(
        "repo://other",
        "1111111111111111111111111111111111111111",
        state.digest(),
        &[],
    );
    let other_base = changeset_manifest(
        "repo://project",
        "2222222222222222222222222222222222222222",
        state.digest(),
        &[],
    );
    let other_state = changeset_manifest(
        "repo://project",
        "1111111111111111111111111111111111111111",
        digest("other"),
        &[],
    );
    assert_eq!(
        full.digest(),
        changeset_manifest(
            "repo://project",
            "1111111111111111111111111111111111111111",
            state.digest(),
            &[]
        )
        .digest()
    );
    assert_ne!(full.digest(), other_repo.digest());
    assert_ne!(full.digest(), other_base.digest());
    assert_ne!(full.digest(), other_state.digest());
}

#[test]
fn empty_changesets_have_a_well_defined_identity() {
    let state = RevisionState::new(Vec::new()).expect("empty is a state");
    let a = changeset_manifest(
        "repo://p",
        "1111111111111111111111111111111111111111",
        state.digest(),
        &[],
    );
    let b = changeset_manifest(
        "repo://p",
        "1111111111111111111111111111111111111111",
        state.digest(),
        &[],
    );
    assert_eq!(a.digest(), b.digest(), "the deterministic empty result");
    assert!(a.to_string().contains(r#""entries":[]"#));
}
