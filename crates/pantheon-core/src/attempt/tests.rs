use super::{LaunchContactState, Observation};

#[test]
fn observations_round_trip_through_their_canonical_spelling() {
    for observation in [
        Observation::Absent,
        Observation::Starting,
        Observation::Running,
        Observation::Exited,
        Observation::Unknown,
    ] {
        assert_eq!(
            Observation::parse(observation.as_str()),
            Some(observation),
            "{} must round-trip",
            observation.as_str()
        );
    }
}

#[test]
fn launch_contact_states_round_trip_and_stay_distinct() {
    for state in [
        LaunchContactState::NotContacted,
        LaunchContactState::ContactMayHaveOccurred,
    ] {
        assert_eq!(LaunchContactState::parse(state.as_str()), Some(state));
    }
    assert_ne!(
        LaunchContactState::NotContacted.as_str(),
        LaunchContactState::ContactMayHaveOccurred.as_str()
    );
}

#[test]
fn unparsable_text_is_refused() {
    assert_eq!(Observation::parse("absent"), None);
    assert_eq!(Observation::parse(""), None);
    assert_eq!(LaunchContactState::parse("CONTACTED"), None);
}
