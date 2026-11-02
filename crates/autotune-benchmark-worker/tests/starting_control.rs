use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{
        manifest::Artifacts,
        starting::{Scope, ScopeDraft},
        starting_control::{Completion, Fault, State},
    },
};
fn d(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn budget() -> Budget {
    Budget::new(32 * 1024 * 1024).unwrap()
}
fn completion(ns: u64, status: Option<i32>) -> Completion {
    Completion {
        anchor: a(ns),
        initialized_session: true,
        cleanup_attempted: true,
        cleanup_status: status,
    }
}
#[test]
fn orderly_report_keeps_original_scope_and_first_cleanup_facts() {
    let b = budget();
    let s = scope(&b);
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    assert_eq!(shared.scope_digest(), s.digest());
    assert!(state.tick(Some(a(2000))));
    shared.admission().begin().unwrap();
    shared.admission().complete();
    let w = shared.request_stop(Some(a(2_000_000_000))).unwrap();
    assert_eq!(w.trigger_ns, 2_000_000_000);
    assert_eq!(w.cooperative_ns, 3_000_000_000);
    assert_eq!(w.exit_ns, 5_000_000_000);
    assert!(!shared.cleanup_allowed(a(1_999_999_900)));
    assert!(shared.cleanup_allowed(a(2_000_000_100)));
    let done = completion(2_000_001_000, Some(0));
    assert!(state.tick(Some(a(2_000_001_500))));
    state.complete(done, Some(a(2_000_002_000))).unwrap();
    assert!(state.history().raw().contains(&a(2_000_001_500)));
    assert!(state.finished());
    assert_eq!(state.completion(), Some(done));
    assert_eq!(state.completion_receipt(), Some(a(2_000_002_000)));
    assert!(shared.admission().stopped());
    assert_eq!(
        state.complete(completion(2_000_003_000, Some(7)), Some(a(2_000_004_000))),
        Err(Fault::Protocol)
    );
    assert_eq!(state.completion(), Some(done));
    assert_eq!(shared.fault(), Some(Fault::Protocol));
}
#[test]
fn blocked_call_closes_at_original_preflight_and_return_cannot_reopen() {
    let b = budget();
    let s = scope(&b);
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    shared.admission().begin().unwrap();
    assert!(state.tick(Some(a(10_000_000_000))));
    let w = shared.window().unwrap();
    assert_eq!(w.trigger_ns, 10_000_000_000);
    assert_eq!(w.cooperative_ns, 11_000_000_000);
    assert_eq!(w.exit_ns, 13_000_000_000);
    assert_eq!(shared.trigger(), None);
    assert!(shared.admission().pending());
    assert!(shared.admission().stopped());
    assert!(!shared.cleanup_allowed(a(10_000_000_100)));
    shared.admission().complete();
    assert!(shared.admission().begin().is_err());
    assert!(shared.cleanup_allowed(a(10_000_000_200)));
    assert!(!shared.cleanup_allowed(a(11_000_000_000)));
    let done = completion(10_500_000_000, Some(0));
    assert_eq!(
        state.complete(done, Some(a(10_500_001_000))),
        Err(Fault::Deadline)
    );
    assert!(!state.finished());
    assert_eq!(state.completion(), Some(done));
    assert_eq!(shared.window(), Some(w));
}
#[test]
fn delayed_event_or_controller_tick_does_not_renew_the_original_deadline() {
    let b = budget();
    let s = scope(&b);
    let state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let w = shared.request_stop(Some(a(12_000_000_000))).unwrap();
    assert_eq!(w.trigger_ns, 10_000_000_000);
    assert_eq!(w.cooperative_ns, 11_000_000_000);
    assert_eq!(shared.fault(), Some(Fault::Deadline));
    assert_eq!(shared.trigger(), Some(a(12_000_000_000)));
    assert!(!shared.cleanup_allowed(a(12_000_001_000)));
    assert_eq!(shared.request_stop(Some(a(12_500_000_000))).unwrap(), w);
    assert_eq!(shared.trigger(), Some(a(12_000_000_000)));
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    assert!(!state.tick(Some(a(14_000_000_000))));
    assert_eq!(shared.window().unwrap().exit_ns, 13_000_000_000);
}
#[test]
fn explicit_stop_freezes_original_window_and_missing_clock_blocks_cleanup() {
    let b = budget();
    let s = scope(&b);
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let w = shared.request_stop(Some(a(1_000_000_000))).unwrap();
    assert_eq!(shared.request_stop(Some(a(1_500_000_000))).unwrap(), w);
    assert!(shared.cleanup_allowed(a(1_999_999_900)));
    assert!(!shared.cleanup_allowed(a(2_000_000_000)));
    assert!(!state.tick(None));
    assert_eq!(shared.fault(), Some(Fault::Clock));
    assert!(state.history().missing());
    assert_eq!(shared.cooperative_raw(), 0);
    assert!(!shared.cleanup_allowed(a(1_500_000_000)));
    assert_eq!(shared.window(), Some(w));
}
#[test]
fn cleanup_failures_pending_completion_and_future_clocks_remain_raw() {
    let b = budget();
    let s = scope(&b);
    for (done, pending, expected) in [
        (completion(2000, Some(-9)), false, Fault::Cleanup),
        (completion(2000, None), false, Fault::Cleanup),
        (completion(2000, Some(0)), true, Fault::Protocol),
        (completion(4000, Some(0)), false, Fault::Clock),
    ] {
        let mut state = State::new(&s, a(1000), &b).ok().unwrap();
        let shared = state.shared();
        if pending {
            shared.admission().begin().unwrap();
        }
        assert_eq!(state.complete(done, Some(a(3000))), Err(expected));
        assert_eq!(state.completion(), Some(done));
        assert!(!state.finished());
    }
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    assert_eq!(
        state.complete(completion(10_000_000_000, Some(0)), Some(a(10_000_000_100))),
        Err(Fault::Deadline)
    );
    assert_eq!(
        state.shared().window().unwrap().cooperative_ns,
        11_000_000_000
    );
}
#[test]
fn bounded_full_preflight_and_shutdown_history_survives_ordinary_exhaustion() {
    let b = budget();
    let s = scope(&b);
    let mut state = State::new(&s, a(0), &b).ok().unwrap();
    let shared = state.shared();
    let _ordinary = b.reserve(32 * 1024 * 1024 - b.used()).unwrap();
    for tick in 1..1300 {
        assert!(state.tick(Some(a(tick * 10_000_000))));
    }
    assert!(!state.tick(Some(a(13_000_000_000))));
    assert_eq!(state.history().raw().len(), 1301);
    assert!(state.history().overflow().is_none());
    assert_eq!(shared.fault(), Some(Fault::Deadline));
    assert_eq!(shared.cooperative_raw(), 0);
}
#[test]
fn initialized_session_cannot_claim_unattempted_cleanup_as_orderly() {
    let b = budget();
    let s = scope(&b);
    let mut done = completion(2000, None);
    done.cleanup_attempted = false;
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    assert_eq!(state.complete(done, Some(a(3000))), Err(Fault::Cleanup));
    done.initialized_session = false;
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    state.complete(done, Some(a(3000))).unwrap();
    assert!(state.finished());
    done.cleanup_status = Some(0);
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    assert_eq!(state.complete(done, Some(a(3000))), Err(Fault::Cleanup));
}
#[test]
fn history_overflow_preserves_first_extra_clock_and_closes_cleanup() {
    let b = budget();
    let s = scope(&b);
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut rejected = None;
    for i in 1..4000 {
        let anchor = a(1000 + i * 100);
        if !state.tick(Some(anchor)) {
            rejected = Some(anchor);
            break;
        }
    }
    assert_eq!(shared.fault(), Some(Fault::Capacity));
    assert_eq!(state.history().overflow(), rejected);
    assert!(rejected.is_some());
    assert!(shared.admission().stopped());
    assert_eq!(shared.cooperative_raw(), 0);
}
#[test]
fn original_epoch_offset_and_raw_shutdown_end_overflow_are_checked() {
    let b = budget();
    let s = scope(&b);
    let mut d = s.draft().clone();
    d.epoch = a(7_000_000_000);
    let s = Scope::freeze(d, &b).unwrap();
    let mut state = State::new(&s, a(7_000_001_000), &b).ok().unwrap();
    let shared = state.shared();
    assert!(state.tick(Some(a(17_000_000_000))));
    assert_eq!(shared.window().unwrap().cooperative_ns, 11_000_000_000);
    assert_eq!(shared.cooperative_raw(), 180_000_000);
    assert!(!shared.cleanup_allowed(a(18_000_000_000)));
    let mut d = s.draft().clone();
    d.epoch.interrupt_100ns = u64::MAX - 1;
    d.epoch.unbiased_100ns = u64::MAX - 1;
    let s = Scope::freeze(d, &b).unwrap();
    assert!(matches!(
        State::new(&s, s.draft().epoch, &b),
        Err(Fault::Deadline)
    ));
}
#[test]
fn capacity_and_shared_owner_lifetime_are_accounted() {
    let b = budget();
    let s = scope(&b);
    let baseline = b.used();
    assert!(matches!(
        State::new(&s, a(1000), &Budget::new(1).unwrap()),
        Err(Fault::Capacity)
    ));
    assert!(matches!(
        State::new(&s, a(10_000_000_000), &b),
        Err(Fault::Deadline)
    ));
    let state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    drop(state);
    assert!(b.used() > baseline);
    drop(shared);
    assert_eq!(b.used(), baseline);
}
#[cfg(windows)]
#[test]
fn actual_watcher_api_compiles_without_running_threads_or_reading_events() {
    use autotune_benchmark_worker::workload::{
        starting_control_windows::Handle, windows_transport::ControlTransport,
    };
    use std::os::windows::io::OwnedHandle;
    type Start =
        fn(ControlTransport, OwnedHandle, &Scope, Anchor, &Budget) -> Result<Handle, Fault>;
    let _start: Start = Handle::start;
    let _ready = Handle::ready;
    let _stop = Handle::request_stop;
    let _complete = Handle::complete;
    let _take = Handle::take_report;
}
fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [3; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn scope(b: &Budget) -> Scope {
    Scope::freeze(
        ScopeDraft {
            session: [1; 16],
            capture_nonce: [2; 16],
            clock_token: [3; 16],
            frequency: 1_000_000_000,
            epoch: a(0),
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: d(4),
                lockfile: d(5),
                controller: d(6),
                worker: d(7),
                sampler: d(8),
                ptx: d(9),
                protocol: d(10),
            },
            read_approval_ref: "read declaration; not compute consent".into(),
        },
        b,
    )
    .unwrap()
}
