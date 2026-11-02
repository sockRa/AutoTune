use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, child::NativeAdmission},
    workload::{
        manifest::{Artifacts, PilotDraft, PilotManifest},
        sampler_runtime::{route, Fault, Operation, Route, Trace},
        wire::WorkerBinding,
    },
};
fn digest(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn manifest(b: &Budget) -> PilotManifest {
    PilotManifest::freeze(
        PilotDraft {
            session: [1; 16],
            runs: [[2; 16], [3; 16], [4; 16]],
            clock_token: [5; 16],
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: digest(6),
                lockfile: digest(7),
                controller: digest(8),
                worker: digest(9),
                sampler: digest(10),
                ptx: digest(11),
                protocol: digest(12),
            },
            starting_evidence: digest(13),
            environment_evidence: digest(14),
            approval_ref: "not execution authority".into(),
            output_root: "D:\\capture\\pilot".into(),
        },
        b,
    )
    .unwrap()
}
fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [5; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn setup(entry: u64, first: u64) -> (Budget, Trace, NativeAdmission) {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let trace = Trace::new(binding, a(entry), a(first), &b).unwrap();
    (b, trace, NativeAdmission::default())
}
#[test]
fn open_and_description_keep_actual_brackets_and_one_pending_operation() {
    let (_, mut trace, gate) = setup(0, 100);
    assert!(trace.begin(Operation::Open, Some(a(200)), u64::MAX, &gate));
    assert!(gate.pending());
    trace.returned(Operation::Open, Some(a(300)), &gate);
    assert!(!gate.pending());
    assert!(trace.begin(Operation::Describe, Some(a(400)), u64::MAX, &gate));
    trace.returned(Operation::Describe, Some(a(500)), &gate);
    assert_eq!(trace.fault, None);
    assert_eq!(trace.calls[0].before, Some(a(200)));
    assert_eq!(trace.calls[0].after, Some(a(300)));
    assert!(trace.calls[0].attempted && trace.calls[0].returned);
}
#[test]
fn late_return_cannot_renew_parent_setup_or_admit_description() {
    let (_, mut trace, gate) = setup(100, 5_000_000_000);
    assert!(trace.begin(Operation::Open, Some(a(9_000_000_000)), u64::MAX, &gate));
    trace.returned(Operation::Open, Some(a(10_000_000_100)), &gate);
    assert_eq!(trace.fault, Some(Fault::Deadline));
    assert!(gate.stopped());
    assert!(!trace.begin(
        Operation::Describe,
        Some(a(10_000_000_200)),
        u64::MAX,
        &gate
    ));
    assert!(!trace.calls[1].attempted);
    assert_eq!(trace.calls[0].after, Some(a(10_000_000_100)));
}
#[test]
fn description_return_at_original_setup_equality_is_failed_even_inside_cumulative_boundary() {
    let (_, mut trace, gate) = setup(0, 100);
    assert!(trace.begin(Operation::Describe, Some(a(9_000_000_000)), u64::MAX, &gate));
    trace.returned(Operation::Describe, Some(a(10_000_000_000)), &gate);
    assert_eq!(trace.fault, Some(Fault::Deadline));
    assert!(trace.calls[1].returned);
    assert!(gate.stopped());
}
#[test]
fn post_call_clock_failure_keeps_return_and_does_not_manufacture_a_time() {
    let (_, mut trace, gate) = setup(0, 100);
    assert!(trace.begin(Operation::Open, Some(a(200)), u64::MAX, &gate));
    trace.returned(Operation::Open, None, &gate);
    assert!(trace.calls[0].returned);
    assert_eq!(trace.calls[0].after, None);
    assert!(trace.history.missing());
    assert_eq!(trace.fault, Some(Fault::Clock));
    assert!(gate.stopped());
    assert!(!gate.pending());
}
#[test]
fn stop_during_native_return_never_reopens_admission() {
    let (_, mut trace, gate) = setup(0, 100);
    assert!(trace.begin(Operation::Open, Some(a(200)), u64::MAX, &gate));
    gate.stop();
    trace.returned(Operation::Open, Some(a(300)), &gate);
    assert!(gate.stopped());
    assert!(!gate.pending());
    assert!(!trace.begin(Operation::Describe, Some(a(400)), u64::MAX, &gate));
    assert!(!trace.calls[1].attempted);
}
#[test]
fn cleanup_needs_closed_idle_lane_and_original_grace_before_any_attempt() {
    for pending in [false, true] {
        let (_, mut trace, gate) = setup(0, 100);
        if pending {
            gate.begin().unwrap();
            gate.stop();
        }
        assert!(!trace.begin(Operation::Cleanup, Some(a(200)), 10, &gate));
        assert!(!trace.calls[2].attempted);
    }
    let (_, mut trace, gate) = setup(0, 100);
    gate.stop();
    assert!(!trace.begin(Operation::Cleanup, Some(a(1000)), 10, &gate));
    assert!(!trace.calls[2].attempted);
}
#[test]
fn cleanup_keeps_signed_raw_status_once_and_cannot_retry() {
    for status in [Some(0), Some(-1), Some(999), None] {
        let (_, mut trace, gate) = setup(0, 100);
        gate.stop();
        assert!(trace.begin(Operation::Cleanup, Some(a(200)), 10, &gate));
        trace.cleanup_returned(Some(a(300)), status, 10, &gate);
        assert_eq!(trace.cleanup_status, status);
        assert!(trace.calls[2].returned);
        assert!(!gate.pending());
        assert!(gate.stopped());
        assert_eq!(
            trace.fault,
            if status == Some(0) {
                None
            } else {
                Some(Fault::Cleanup)
            }
        );
        assert!(!trace.begin(Operation::Cleanup, Some(a(400)), 20, &gate));
    }
}
#[test]
fn late_cleanup_return_and_missing_clock_never_become_timely_success() {
    for after in [Some(a(1000)), None] {
        let (_, mut trace, gate) = setup(0, 100);
        gate.stop();
        assert!(trace.begin(Operation::Cleanup, Some(a(200)), 10, &gate));
        trace.cleanup_returned(after, Some(0), 10, &gate);
        assert_eq!(trace.cleanup_status, Some(0));
        assert!(trace.calls[2].returned);
        assert_eq!(trace.calls[2].after, after);
        assert!(trace.fault.is_some());
    }
}
#[test]
fn output_and_control_must_both_be_observed_before_original_deadline() {
    let (_, mut trace, gate) = setup(0, 100);
    gate.stop();
    assert!(!trace.drain(Some(a(200)), true, false, 10, &gate));
    assert_eq!(trace.fault, None);
    assert!(!trace.drain(Some(a(300)), false, true, 10, &gate));
    assert!(trace.drain(Some(a(400)), true, true, 10, &gate));
    assert!(!trace.drain(Some(a(1000)), true, true, 10, &gate));
    assert_eq!(trace.fault, Some(Fault::Drain));
    assert!(trace.output_drained && trace.control_finished);
}
#[test]
fn later_drain_does_not_erase_the_first_runtime_failure() {
    let (_, mut trace, gate) = setup(0, 100);
    trace.fail(Fault::Runtime, &gate);
    assert!(trace.drain(Some(a(200)), true, true, 10, &gate));
    assert_eq!(trace.fault, Some(Fault::Runtime));
}
fn frame(text: &str) -> Vec<u8> {
    let mut out = ((text.len() + 4) as u32).to_le_bytes().to_vec();
    out.extend_from_slice(text.as_bytes());
    out
}
#[test]
fn route_only_selects_a_parser_and_cannot_validate_or_authorize_a_request() {
    let b = Budget::new(2 * 1024 * 1024).unwrap();
    for (role, expected) in [
        ("pilot_sampler", Ok(Route::Telemetry)),
        ("pilot_settings_sampler", Ok(Route::Settings)),
        ("sampler", Err(Fault::Protocol)),
        ("worker", Err(Fault::Protocol)),
    ] {
        let bytes = frame(&format!("{{\"version\":\"1\",\"role\":\"{role}\"}}"));
        assert_eq!(route(&bytes, &b), expected);
    }
    assert_eq!(
        route(
            &frame("{\"version\":\"1\",\"version\":\"1\",\"role\":\"pilot_sampler\"}"),
            &b
        ),
        Err(Fault::Protocol)
    );
    let bytes = frame("{\"version\":\"1\",\"role\":\"pilot_sampler\"}");
    let _pressure = b.reserve(2 * 1024 * 1024).unwrap();
    assert_eq!(route(&bytes, &b), Err(Fault::Capacity));
}
#[test]
fn whole_run_poll_history_has_a_fixed_budget_and_stops_at_run_equality() {
    let (b, mut trace, gate) = setup(0, 0);
    let before = b.used();
    for tick in 1..4500 {
        for offset in [0, 100, 200] {
            assert!(trace
                .observe(Some(a(tick * 10_000_000 + offset)), &gate)
                .is_ok());
        }
    }
    assert_eq!(b.used(), before);
    assert_eq!(
        trace.observe(Some(a(45_000_000_000)), &gate),
        Err(Fault::Deadline)
    );
    assert!(gate.stopped());
}
#[test]
fn failed_begin_and_duplicate_cleanup_cannot_overwrite_original_evidence() {
    let (_, mut trace, gate) = setup(0, 100);
    assert!(!trace.begin(Operation::Open, None, u64::MAX, &gate));
    assert!(!trace.begin(Operation::Open, Some(a(200)), u64::MAX, &gate));
    assert_eq!(trace.calls[0].before, None);
    assert!(trace.calls[0].requested && !trace.calls[0].attempted);
    let (_, mut trace, gate) = setup(0, 100);
    gate.stop();
    assert!(trace.begin(Operation::Cleanup, Some(a(200)), 10, &gate));
    trace.cleanup_returned(Some(a(300)), Some(999), 10, &gate);
    trace.cleanup_returned(Some(a(400)), Some(0), 10, &gate);
    assert_eq!(trace.cleanup_status, Some(999));
    assert_eq!(trace.calls[2].after, Some(a(300)));
    assert_eq!(trace.fault, Some(Fault::Cleanup));
}
