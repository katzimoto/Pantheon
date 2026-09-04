use crate::LocalContainerBackend;

#[test]
fn detect_returns_some_on_typical_system() {
    // Most CI and dev machines have either podman or docker.
    // This test documents that detect() does not panic.
    let _ = LocalContainerBackend::detect();
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod integration {
    use super::*;

    #[test]
    fn ensure_and_release_roundtrip() {
        let backend = LocalContainerBackend::detect().expect("no container runtime found");
        let key = SandboxKey::new("pantheon-test-roundtrip").unwrap();
        let plan = test_plan();

        // Clean up any leftover from a previous aborted run
        let _ = backend.release_sandbox(&key);

        let presence = backend.ensure_sandbox(&key, &plan).unwrap();
        assert_eq!(presence, pantheon_core::sandbox::SandboxPresence::Present);

        let verified = backend.verify_sandbox(&key, &plan).unwrap();
        assert!(verified.all_passed());

        backend.release_sandbox(&key).unwrap();

        let after = backend.inspect_sandbox(&key).unwrap();
        assert_eq!(after, pantheon_core::sandbox::SandboxPresence::Absent);
    }
}
