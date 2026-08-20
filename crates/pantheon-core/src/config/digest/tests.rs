use super::Digest;

/// The published SHA-256 vectors. Hand-checked constants are what make this a
/// test of SHA-256 rather than a test that the code agrees with itself.
#[test]
fn matches_the_published_sha256_vectors() {
    assert_eq!(
        Digest::of(b"").to_hex(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        Digest::of(b"abc").to_hex(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn renders_with_the_algorithm_prefix() {
    assert_eq!(
        Digest::of(b"abc").to_string(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn round_trips_through_storage_bytes() {
    let digest = Digest::of(b"pantheon");
    assert_eq!(Digest::from_bytes(*digest.as_bytes()), digest);
}

#[test]
fn round_trips_through_the_display_form() {
    let digest = Digest::of(b"routing");
    assert_eq!(Digest::from_display(&digest.to_string()), Some(digest));
    assert!(Digest::from_display("sha256:not-a-digest").is_none());
}

#[test]
fn different_input_gives_a_different_identity() {
    assert_ne!(Digest::of(b"a"), Digest::of(b"b"));
}
