use autotune_recovery_windows::*;

#[test]
fn missing_audited_ports_always_block_arm() {
    assert!(require_arm_support().is_err());
}

#[cfg(windows)]
#[test]
fn cng_matches_portable_hmac_and_rng_has_fresh_outputs() {
    let a = random().unwrap();
    let b = random().unwrap();
    assert_ne!(a, b);
    assert_ne!(a, [0; 32]);
    for bytes in [b"".as_slice(), b"canonical transcript", &[0; 65536]] {
        assert_eq!(
            hmac(&a, bytes).unwrap(),
            autotune_recovery_contract::mac(&a, bytes)
        );
    }
}

#[cfg(windows)]
#[test]
fn actual_clock_samples_are_monotonic_without_host_changes() {
    let a = clock().unwrap();
    let b = clock().unwrap();
    assert!(b.interrupt_100ns >= a.interrupt_100ns);
    assert!(b.unbiased_100ns >= a.unbiased_100ns);
    assert!(a.bracket_100ns <= 20_000);
}

#[cfg(not(windows))]
#[test]
fn unsupported_platform_has_no_fallback() {
    assert_eq!(random(), Err(PlatformError::Unsupported));
    assert_eq!(verify_isolation(), Err(PlatformError::Unsupported));
}
