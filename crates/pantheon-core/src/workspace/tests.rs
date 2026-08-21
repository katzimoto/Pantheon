use super::*;

#[test]
fn a_resolved_base_accepts_both_object_name_widths() {
    let sha1 = "0".repeat(40);
    let sha256 = "a".repeat(64);
    assert_eq!(ResolvedBase::parse(&sha1).unwrap().as_str(), sha1);
    assert_eq!(ResolvedBase::parse(&sha256).unwrap().as_str(), sha256);
}

#[test]
fn a_resolved_base_refuses_anything_that_is_not_a_canonical_object_name() {
    for refused in [
        // A ref name, not an object name: the whole point of resolution.
        "refs/heads/main",
        // An abbreviation is ambiguous by construction.
        "dc6fcd7",
        // Uppercase would compare unequal to what Git reports.
        &"A".repeat(40),
        // Not hexadecimal.
        &"g".repeat(40),
        // Between the two supported widths.
        &"0".repeat(41),
        "",
    ] {
        assert!(
            matches!(
                ResolvedBase::parse(refused),
                Err(BaseError::NotAnObjectName(_))
            ),
            "{refused:?} was accepted as a resolved base"
        );
    }
}

#[test]
fn a_requested_base_accepts_ordinary_ref_names() {
    for accepted in ["main", "refs/heads/main", "release/2026-08", "v1.0.0-rc1"] {
        assert_eq!(
            RequestedBase::parse(accepted).unwrap().as_str(),
            accepted,
            "{accepted:?} was refused"
        );
    }
}

#[test]
fn a_requested_base_refuses_values_that_would_be_read_as_something_else() {
    // Each entry names a distinct way a string that is not a plain ref name
    // could change what a downstream command does or what the durable
    // requested/resolved pair means.
    for (refused, why) in [
        ("--upload-pack=/bin/sh", "reads as a command-line option"),
        ("-c", "reads as a command-line option"),
        ("main~3", "is a revision expression, not a ref"),
        ("main^", "is a revision expression, not a ref"),
        ("main@{yesterday}", "resolves differently depending on when"),
        ("refs/heads/../../etc", "escapes its namespace"),
        ("refs//heads/main", "has an empty path component"),
        ("refs/heads/main.lock", "collides with Git's lock file"),
        ("refs/.hidden/main", "starts a component with a dot"),
        ("main branch", "contains a space"),
        ("main\nrefs/heads/other", "contains a control character"),
        ("refs/heads/*", "is a pattern, not a name"),
        ("", "is empty"),
    ] {
        assert!(
            matches!(
                RequestedBase::parse(refused),
                Err(BaseError::NotARefName { .. })
            ),
            "{refused:?} was accepted even though it {why}"
        );
    }
    assert!(RequestedBase::parse(&"a".repeat(RequestedBase::MAX_LEN + 1)).is_err());
}

#[test]
fn every_workspace_phase_round_trips_through_its_stored_spelling() {
    for phase in [
        WorkspacePhase::Requested,
        WorkspacePhase::Materializing,
        WorkspacePhase::Ready,
        WorkspacePhase::Frozen,
        WorkspacePhase::Releasing,
        WorkspacePhase::Released,
        WorkspacePhase::Error,
    ] {
        assert_eq!(WorkspacePhase::parse(phase.as_str()), Some(phase));
    }
    assert_eq!(WorkspacePhase::parse("READY"), None);
    assert_eq!(WorkspacePhase::parse("Materialized"), None);
}

#[test]
fn only_phases_reachable_after_ready_count_as_having_been_mutable() {
    // The predicate that decides whether partial external state may be
    // discarded and rebuilt. A phase that has never handed the Workspace to
    // an execution owner must answer false, or a retry would delete work.
    assert!(!WorkspacePhase::Requested.has_been_mutable());
    assert!(!WorkspacePhase::Materializing.has_been_mutable());
    assert!(!WorkspacePhase::Error.has_been_mutable());

    assert!(WorkspacePhase::Ready.has_been_mutable());
    assert!(WorkspacePhase::Frozen.has_been_mutable());
    assert!(WorkspacePhase::Releasing.has_been_mutable());
    assert!(WorkspacePhase::Released.has_been_mutable());
}

#[test]
fn every_materialization_observation_round_trips_through_its_stored_spelling() {
    for observed in [
        Materialization::Present,
        Materialization::Absent,
        Materialization::Unknown,
    ] {
        assert_eq!(Materialization::parse(observed.as_str()), Some(observed));
    }
    assert_eq!(Materialization::parse("Missing"), None);
}
