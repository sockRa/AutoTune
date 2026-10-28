#![cfg(windows)]

use autotune_benchmark_worker::monitor::windows_clock::WindowsClock;

/// Explicit read-only, CPU-only OS observation. No sleep, settings change,
/// GPU discovery, GPU DLL loading, process launch or native workload admission.
#[test]
fn real_windows_clock_capture() {
    // Fixture epoch, not a generated session identity or live-evidence authority.
    let token = [0x43; 16];
    let clock = WindowsClock::new(token).expect("Windows QPC frequency");
    let first = clock.capture().expect("first Windows anchor");
    let second = clock.capture().expect("second Windows anchor");
    assert_eq!(clock.clock_token(), token);
    assert!(clock.qpc_frequency() > 0);
    for anchor in [first, second] {
        assert_eq!(anchor.clock_token, token);
        assert!(anchor.qpc_before <= anchor.qpc_after);
    }
    assert!(first.qpc_after <= second.qpc_before);
    assert!(first.interrupt_100ns <= second.interrupt_100ns);
    assert!(first.unbiased_100ns <= second.unbiased_100ns);
    println!(
        "windows_clock_capture qpc_frequency={} first={first:?} second={second:?}",
        clock.qpc_frequency()
    );
}
