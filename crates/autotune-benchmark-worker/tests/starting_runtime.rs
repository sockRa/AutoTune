use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{
        manifest::Artifacts,
        starting::{Scope, ScopeDraft},
        starting_control::State,
        starting_runtime::{Fault, Operation, Trace},
    },
};
fn d(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn budget() -> Budget {
    Budget::new(32 * 1024 * 1024).unwrap()
}
#[test]
fn logical_native_groups_are_one_shot_with_separate_raw_cleanup_return() {
    let b = budget();
    let s = scope(&b);
    let state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
    assert!(trace.begin(Operation::Open, Some(a(2000)), &shared));
    assert!(shared.admission().pending());
    trace.returned(Operation::Open, Some(a(3000)), &shared);
    assert!(!shared.admission().pending());
    assert!(trace.begin(Operation::Describe, Some(a(4000)), &shared));
    trace.returned(Operation::Describe, Some(a(5000)), &shared);
    assert!(trace.begin(Operation::Read, Some(a(6000)), &shared));
    assert!(!shared.admission().pending());
    // The real Snapshot collector owns begin/complete and closes the read lane.
    shared.admission().begin().unwrap();
    shared.admission().complete();
    shared.admission().stop();
    trace.returned(Operation::Read, Some(a(7000)), &shared);
    shared.request_stop(Some(a(8000))).unwrap();
    assert!(trace.begin(Operation::Cleanup, Some(a(9000)), &shared));
    trace.cleanup_returned(Some(a(10000)), Some(0), &shared);
    assert_eq!(trace.cleanup_status, Some(0));
    assert!(trace.fault.is_none());
    assert!(!shared.admission().pending());
    assert!(trace.drain(Some(a(11000)), true, true, &shared));
    let original = trace.calls[3].after;
    trace.cleanup_returned(Some(a(12000)), Some(-99), &shared);
    assert_eq!(trace.calls[3].after, original);
    assert_eq!(trace.cleanup_status, Some(0));
    assert_eq!(trace.fault, Some(Fault::Protocol));
}
#[test]
fn late_open_return_or_stop_during_it_has_no_successor_group() {
    let b = budget();
    let s = scope(&b);
    let mut state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
    assert!(trace.begin(Operation::Open, Some(a(2000)), &shared));
    state.tick(Some(a(10_000_000_000)));
    trace.returned(Operation::Open, Some(a(10_000_000_100)), &shared);
    assert!(trace.calls[0].returned);
    assert_eq!(trace.calls[0].after, Some(a(10_000_000_100)));
    assert_eq!(trace.fault, Some(Fault::Deadline));
    assert!(!trace.begin(Operation::Describe, Some(a(10_000_000_200)), &shared));
    assert!(!trace.calls[1].attempted);
    assert!(trace.begin(Operation::Cleanup, Some(a(10_000_000_300)), &shared));
    trace.cleanup_returned(Some(a(10_000_000_400)), Some(0), &shared);
    assert_eq!(trace.fault, Some(Fault::Deadline));
    assert!(shared.admission().stopped());
}
#[test]
fn failed_post_clock_retains_raw_return_and_closes_cleanup() {
    let b = budget();
    let s = scope(&b);
    let state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
    assert!(trace.begin(Operation::Open, Some(a(2000)), &shared));
    trace.returned(Operation::Open, None, &shared);
    assert!(trace.calls[0].returned);
    assert_eq!(trace.calls[0].after, None);
    assert!(trace.history.missing());
    assert_eq!(shared.cooperative_raw(), 0);
    assert!(!trace.begin(Operation::Cleanup, Some(a(3000)), &shared));
    assert!(!trace.calls[3].attempted);
}
#[test]
fn original_cleanup_boundary_drain_and_pending_state_are_not_renewable() {
    let b = budget();
    let s = scope(&b);
    for pending in [false, true] {
        let state = State::new(&s, a(1000), &b).ok().unwrap();
        let shared = state.shared();
        let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
        if pending {
            shared.admission().begin().unwrap();
        }
        shared.request_stop(Some(a(1_000_000_000))).unwrap();
        let before = if pending {
            a(1_000_001_000)
        } else {
            a(2_000_000_000)
        };
        assert!(!trace.begin(Operation::Cleanup, Some(before), &shared));
        assert!(!trace.calls[3].attempted);
        assert!(shared.admission().stopped());
    }
    let state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
    shared.request_stop(Some(a(1_000_000_000))).unwrap();
    assert!(trace.begin(Operation::Cleanup, Some(a(1_000_001_000)), &shared));
    trace.cleanup_returned(Some(a(2_000_000_000)), Some(0), &shared);
    assert_eq!(trace.cleanup_status, Some(0));
    assert_eq!(trace.fault, Some(Fault::Cleanup));
    assert!(!trace.drain(Some(a(2_000_000_100)), true, true, &shared));
    assert_eq!(trace.fault, Some(Fault::Cleanup));
}
#[test]
fn foreign_scope_order_and_capacity_never_admit_a_native_group() {
    let b = budget();
    let s = scope(&b);
    let mut d = s.draft().clone();
    d.capture_nonce = [9; 16];
    let foreign = Scope::freeze(d, &b).unwrap();
    let state = State::new(&foreign, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
    assert!(!trace.begin(Operation::Open, Some(a(2000)), &shared));
    assert!(!trace.calls[0].attempted);
    assert_eq!(trace.fault, Some(Fault::Protocol));
    assert!(matches!(
        Trace::new(&s, a(1000), &Budget::new(1).unwrap()),
        Err(Fault::Capacity)
    ));
    assert!(matches!(
        Trace::new(&s, a(10_000_000_000), &b),
        Err(Fault::Deadline)
    ));
    let state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
    assert!(!trace.begin(Operation::Read, Some(a(2000)), &shared));
    assert!(!trace.calls[2].attempted);
}
#[test]
fn prepared_trace_survives_ordinary_exhaustion_and_rejects_duplicate_open() {
    let b = budget();
    let s = scope(&b);
    let state = State::new(&s, a(1000), &b).ok().unwrap();
    let shared = state.shared();
    let mut trace = Trace::new(&s, a(1000), &b).ok().unwrap();
    let _ordinary = b.reserve(32 * 1024 * 1024 - b.used()).unwrap();
    assert!(trace.begin(Operation::Open, Some(a(2000)), &shared));
    trace.returned(Operation::Open, Some(a(3000)), &shared);
    assert!(!trace.begin(Operation::Open, Some(a(4000)), &shared));
    assert_eq!(trace.calls[0].before, Some(a(2000)));
    assert_eq!(trace.calls[0].after, Some(a(3000)));
}
#[cfg(windows)]
#[test]
fn actual_starting_entry_compiles_without_native_execution() {
    use autotune_benchmark_worker::workload::{
        child_windows::Context,
        starting_bootstrap::Bound,
        starting_entry::{execute, EntryFailure, Outcome},
    };
    let _entry: unsafe fn(Context<Bound>) -> Result<Outcome, EntryFailure> = execute;
    let _retained = Outcome::initialized_session_retained;
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
