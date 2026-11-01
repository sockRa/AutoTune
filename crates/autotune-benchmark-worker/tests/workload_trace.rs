use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, wire::Lane},
    workload::{
        assessment::Audit,
        dispatch::{Operation, Phase, Record},
        manifest::{Artifacts, PilotDraft, PilotManifest},
        trace::{CallTrace, Fault, Observer},
        wire::{Channel, Direction, Message, WorkerBinding},
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
fn binding(m: &PilotManifest) -> WorkerBinding {
    WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap()
}
fn summary() -> Record {
    Record {
        dispatch: 0,
        phase: Phase::Warmup,
        seed: 0,
        lanes: 65536,
        steps: 4096,
        calls: Vec::new(),
        pending: false,
        readback_lanes: Some(65536),
        checked_lanes: 65536,
        mismatches: 0,
        nonfinite: 0,
        first_failure: None,
        verified_fma: 268435456,
        input_sha256: Some(digest(20)),
        output_sha256: Some(digest(21)),
    }
}
#[test]
fn observed_stages_bind_to_owned_real_frame_and_pass_report_audit() {
    use Operation::*;
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut ns = 0;
    let mut trace = CallTrace::new(
        binding,
        0,
        Phase::Warmup,
        a(0),
        || {
            ns += 1000;
            Some(a(ns))
        },
        || false,
        &b,
    )
    .unwrap();
    trace.before_host(Input).unwrap();
    trace.after_host(Input).unwrap();
    for op in [
        UploadA,
        UploadB,
        UploadX,
        Sentinel,
        UploadCompletion,
        Launch,
        Query,
        Readback,
    ] {
        trace.before(op).unwrap();
        trace.after(op, 0).unwrap();
    }
    for op in [Check, Hash] {
        trace.before_host(op).unwrap();
        trace.after_host(op).unwrap();
    }
    assert!(!trace.pending());
    assert_eq!(trace.fault(), None);
    let charged = b.used();
    let owned = match trace.bind(summary()) {
        Ok(o) => o,
        Err(_) => panic!("unexpected bind rejection"),
    };
    assert_eq!(b.used(), charged);
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let frame = tx.encode_traced(&owned).unwrap();
    let decoded = rx.decode(frame.as_bytes()).unwrap();
    assert!(matches!(decoded.message(), Message::DispatchRecord(r) if r == owned.record()));
    let mut audit = Audit::new(binding, a(0), &b).unwrap();
    assert_eq!(
        audit
            .observe(decoded.message(), Some(a(100_000)))
            .unwrap()
            .warmup,
        1
    );
    drop(owned);
    assert!(b.used() < charged + 16384);
}
#[test]
fn raw_native_error_survives_missing_post_clock_and_blocks_successors() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut reads = 0;
    let mut trace = CallTrace::new(
        binding,
        0,
        Phase::Warmup,
        a(0),
        || {
            reads += 1;
            if reads == 1 {
                Some(a(1000))
            } else {
                None
            }
        },
        || false,
        &b,
    )
    .unwrap();
    trace.before(Operation::UploadA).unwrap();
    let error = trace.after(Operation::UploadA, 700).unwrap_err();
    assert_eq!(error.raw_status, 700);
    assert_eq!(trace.calls()[0].status, Some(700));
    assert_eq!(trace.calls()[0].after, None);
    assert_eq!(
        trace.fault(),
        Some(Fault::Native {
            operation: Operation::UploadA,
            status: 700
        })
    );
    assert!(trace.pending());
    assert!(trace.before(Operation::Launch).is_err());
    assert_eq!(trace.calls().len(), 1);
}
#[test]
fn stop_missing_pre_clock_and_original_phase_equality_prevent_native_entry() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    for kind in 0..3 {
        let mut trace = CallTrace::new(
            binding,
            0,
            Phase::Warmup,
            a(0),
            || match kind {
                0 => Some(a(1000)),
                1 => None,
                _ => Some(a(5_000_000_000)),
            },
            || kind == 0,
            &b,
        )
        .unwrap();
        assert!(trace.before(Operation::UploadA).is_err());
        assert!(!trace.pending());
        assert_eq!(
            trace.fault(),
            Some(match kind {
                0 => Fault::Stopped,
                1 => Fault::Clock,
                _ => Fault::Deadline,
            })
        );
        assert_eq!(trace.calls()[0].status, None);
        assert_eq!(trace.calls()[0].after, None);
    }
}
#[test]
fn stream_budget_covers_upload_completion_and_pending_query_does_not_restart_it() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut ticks = [0, 1_000_000, 2_000_000, 3_000_000, 4_000_000, 100_000_000].into_iter();
    let mut trace = CallTrace::new(
        binding,
        0,
        Phase::Warmup,
        a(0),
        || ticks.next().map(a),
        || false,
        &b,
    )
    .unwrap();
    trace.before(Operation::UploadA).unwrap();
    trace.after(Operation::UploadA, 0).unwrap();
    trace.before(Operation::Query).unwrap();
    trace.after(Operation::Query, 600).unwrap();
    trace.before(Operation::Query).unwrap();
    assert!(trace.after(Operation::Query, 0).is_err());
    assert_eq!(trace.fault(), Some(Fault::Deadline));
    assert_eq!(trace.calls()[2].status, Some(0));
    assert!(!trace.pending());
}
#[test]
fn completed_stream_does_not_apply_100ms_limit_to_full_cycle_check_hash() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut ticks = [
        0,
        1_000_000,
        2_000_000,
        3_000_000,
        4_000_000,
        5_000_000,
        200_000_000,
        201_000_000,
    ]
    .into_iter();
    let mut trace = CallTrace::new(
        binding,
        0,
        Phase::Warmup,
        a(0),
        || ticks.next().map(a),
        || false,
        &b,
    )
    .unwrap();
    trace.before_host(Operation::Input).unwrap();
    trace.after_host(Operation::Input).unwrap();
    trace.before(Operation::UploadA).unwrap();
    trace.after(Operation::UploadA, 0).unwrap();
    trace.before(Operation::Query).unwrap();
    trace.after(Operation::Query, 0).unwrap();
    trace.before_host(Operation::Hash).unwrap();
    trace.after_host(Operation::Hash).unwrap();
    assert_eq!(trace.fault(), None);
}
#[test]
fn capacity_and_unmatched_return_retain_first_raw_data_and_allocation() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let before = b.used();
    let mut ns = 0;
    let mut trace = CallTrace::new(
        binding,
        0,
        Phase::Warmup,
        a(0),
        || {
            ns += 1000;
            Some(a(ns))
        },
        || false,
        &b,
    )
    .unwrap();
    for _ in 0..64 {
        trace.before(Operation::Query).unwrap();
        trace.after(Operation::Query, 600).unwrap();
    }
    assert!(trace.before(Operation::Query).is_err());
    assert_eq!(trace.calls().len(), 64);
    assert_eq!(trace.fault(), Some(Fault::Capacity));
    assert!(trace.after(Operation::Launch, -123).is_err());
    assert_eq!(trace.unmatched().unwrap().status, Some(-123));
    assert_eq!(trace.fault(), Some(Fault::Capacity));
    drop(trace);
    assert_eq!(b.used(), before);
    assert!(matches!(
        CallTrace::new(
            binding,
            0,
            Phase::Warmup,
            a(0),
            || Some(a(0)),
            || false,
            &Budget::new(1).unwrap()
        ),
        Err(Fault::Capacity)
    ));
}
#[test]
fn rejected_summary_binding_preserves_both_raw_owners_and_exact_charge() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let before = b.used();
    let mut ns = 0;
    let mut trace = CallTrace::new(
        binding,
        0,
        Phase::Warmup,
        a(0),
        || {
            ns += 1000;
            Some(a(ns))
        },
        || false,
        &b,
    )
    .unwrap();
    trace.before(Operation::Query).unwrap();
    trace.after(Operation::Query, 600).unwrap();
    let mut record = summary();
    record.calls = trace.calls().to_vec();
    let charged = b.used();
    let rejected = match trace.bind(record) {
        Ok(_) => panic!("nonempty caller trace accepted"),
        Err(e) => e,
    };
    assert_eq!(b.used(), charged);
    assert_eq!(rejected.0.calls(), rejected.1.calls);
    assert_eq!(rejected.0.calls()[0].status, Some(600));
    drop(rejected);
    assert_eq!(b.used(), before);
}
#[test]
fn trace_identity_phase_and_invocation_cannot_be_relabelled_at_bind_or_encode() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let original = binding(&m);
    let other_manifest = PilotManifest::parse(
        m.canonical()
            .replace(&"a".repeat(40), &"b".repeat(40))
            .as_bytes(),
        &b,
    )
    .unwrap();
    let other = binding(&other_manifest);
    assert_ne!(original, other);
    let make = || {
        CallTrace::new(
            original,
            0,
            Phase::Warmup,
            a(0),
            || Some(a(0)),
            || false,
            &b,
        )
        .unwrap()
    };
    let mut wrong = summary();
    wrong.seed = 1;
    assert!(make().bind(wrong).is_err());
    let mut wrong = summary();
    wrong.dispatch = 1;
    assert!(make().bind(wrong).is_err());
    let mut wrong = summary();
    wrong.phase = Phase::Measured;
    wrong.dispatch = 128;
    assert!(make().bind(wrong).is_err());
    assert!(matches!(
        CallTrace::new(
            original,
            128,
            Phase::Warmup,
            a(0),
            || Some(a(0)),
            || false,
            &b
        ),
        Err(Fault::Binding)
    ));
    let owned = match make().bind(summary()) {
        Ok(o) => o,
        Err(_) => panic!("valid binding rejected"),
    };
    let mut foreign =
        Channel::new(other, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    assert!(foreign.encode_traced(&owned).is_err());
    let mut correct = Channel::new(original, Lane::Ordinary, Direction::FromWorker, b).unwrap();
    assert!(correct.encode_traced(&owned).is_ok());
    // Failed foreign encoding retains the original immutable raw owner.
    assert_eq!(owned.record().dispatch, 0);
}
