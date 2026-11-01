use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, wire::Lane},
    workload::{
        assessment::Audit,
        dispatch::{FailureVector, Operation},
        gate::{Gate, StartObservation, StopWindow},
        manifest::{Artifacts, PilotDraft, PilotManifest},
        producer::{Driver, End, Fault, FrameSink, Inputs, Producer},
        trace::Observer,
        wire::{Channel, Direction, EncodedFrame, Message, WorkerBinding},
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
use autotune_benchmark_worker::worker::{Fault as NativeFault, Identity, Provenance, Query};
use autotune_fma_reference::input::Shape;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
#[derive(Default)]
struct Stats {
    submits: AtomicUsize,
    queries: AtomicUsize,
    closes: AtomicUsize,
}
struct Fake {
    identity: Identity,
    shape: Shape,
    provenance: Provenance,
    output: Vec<f32>,
    stats: Arc<Stats>,
    pending: usize,
    pending_left: usize,
    stale: bool,
    partial: bool,
    fail: bool,
}
impl Driver for Fake {
    fn provenance(&self) -> Provenance {
        self.provenance
    }
    fn identity(&self) -> Identity {
        self.identity
    }
    fn shape(&self) -> Shape {
        self.shape
    }
    fn submit(&mut self, input: Inputs<'_>, o: &mut impl Observer) -> Result<(), NativeFault> {
        self.stats.submits.fetch_add(1, Ordering::SeqCst);
        self.pending_left = self.pending;
        use Operation::*;
        for op in [
            UploadA,
            UploadB,
            UploadX,
            Sentinel,
            UploadCompletion,
            Launch,
        ] {
            o.before(op)?;
            if self.fail && op == UploadB {
                o.after(op, 700)?;
                return Err(NativeFault {
                    operation: "fixture upload",
                    raw_status: 700,
                });
            }
            if op == Launch && (!self.stale || self.stats.submits.load(Ordering::SeqCst) == 1) {
                for (i, out) in self.output.iter_mut().enumerate() {
                    *out = (input.x[i] as i32
                        + input.steps as i32 * input.a[i] as i32 * input.b[i] as i32)
                        as f32;
                }
            }
            o.after(op, 0)?;
        }
        Ok(())
    }
    fn query(&mut self, o: &mut impl Observer) -> Result<Query, NativeFault> {
        o.before(Operation::Query)?;
        self.stats.queries.fetch_add(1, Ordering::SeqCst);
        let pending = self.pending_left > 0;
        if pending {
            self.pending_left -= 1;
        }
        o.after(Operation::Query, if pending { 600 } else { 0 })?;
        Ok(if pending {
            Query::Pending
        } else {
            Query::Complete
        })
    }
    fn readback(&mut self, out: &mut [f32], o: &mut impl Observer) -> Result<usize, NativeFault> {
        o.before(Operation::Readback)?;
        let n = out.len() - usize::from(self.partial);
        out[..n].copy_from_slice(&self.output[..n]);
        o.after(Operation::Readback, 0)?;
        Ok(out.len())
    }
    fn close(&mut self) -> Result<(), NativeFault> {
        self.stats.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[derive(Default)]
struct Sink {
    frames: Vec<EncodedFrame>,
    reject: bool,
    jump: Option<(usize, Arc<AtomicU64>, u64)>,
}
impl FrameSink for Sink {
    fn send(&mut self, frame: EncodedFrame) -> Result<(), EncodedFrame> {
        if self.reject {
            Err(frame)
        } else {
            self.frames.push(frame);
            if let Some((count, time, value)) = &self.jump {
                if self.frames.len() == *count {
                    time.store(*value, Ordering::SeqCst);
                }
            }
            Ok(())
        }
    }
}
type TestProducer =
    Producer<Fake, Box<dyn FnMut() -> Option<Anchor>>, Box<dyn FnMut() -> Option<StopWindow>>>;
struct Fixture {
    producer: TestProducer,
    stats: Arc<Stats>,
    time: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    bad: Arc<AtomicBool>,
    future: Arc<AtomicBool>,
    binding: WorkerBinding,
}
fn start(m: &PilotManifest, binding: WorkerBinding) -> StartObservation {
    let mut gate = Gate::new(binding).unwrap();
    gate.accept(
        &Message::Bootstrap {
            manifest: m.canonical().into(),
        },
        Some(a(100)),
    )
    .unwrap();
    let lanes = (0..3)
        .map(|i| m.bind_run(i, 0).unwrap())
        .find(|r| r.run() == binding.monitor().run)
        .unwrap()
        .plan()
        .shape
        .lanes();
    gate.ready(&Message::Ready {
        anchor: a(1000),
        identity: binding.monitor().device,
        context: binding.monitor().device,
        worker_digest: digest(9),
        driver_version: 12090,
        compute_major: 7,
        compute_minor: 5,
        prepared_lanes: lanes,
    })
    .unwrap();
    gate.accept(&Message::Start { issued: a(2000) }, Some(a(3000)))
        .unwrap();
    let observed = gate.take_start().unwrap();
    assert!(gate.take_start().is_none());
    observed
}
fn fixture(b: &Budget, m: &PilotManifest, mode: u8) -> Fixture {
    fixture_run(b, m, mode, 0)
}
fn fixture_run(b: &Budget, m: &PilotManifest, mode: u8, index: usize) -> Fixture {
    let run = m.bind_run(index, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    let stats = Arc::new(Stats::default());
    let time = Arc::new(AtomicU64::new(10_000));
    let stop = Arc::new(AtomicBool::new(false));
    let bad = Arc::new(AtomicBool::new(false));
    let future = Arc::new(AtomicBool::new(false));
    let driver = Fake {
        identity: binding.monitor().device,
        shape: run.plan().shape,
        provenance: if mode == 5 {
            Provenance::Native
        } else {
            Provenance::SimulatedFixture
        },
        output: vec![f32::NAN; run.plan().shape.lanes() as usize],
        stats: stats.clone(),
        pending: usize::from(mode == 3) * 2,
        pending_left: 0,
        stale: mode == 1,
        partial: mode == 2,
        fail: mode == 4,
    };
    let tick = time.clone();
    let wrong = bad.clone();
    let signal = stop.clone();
    let clock: Box<dyn FnMut() -> Option<Anchor>> = Box::new(move || {
        if wrong.load(Ordering::SeqCst) {
            Some(a(0))
        } else {
            Some(a(tick.fetch_add(1000, Ordering::SeqCst)))
        }
    });
    let invalid = future.clone();
    let control: Box<dyn FnMut() -> Option<StopWindow>> = Box::new(move || {
        if invalid.load(Ordering::SeqCst) {
            return Some(StopWindow {
                trigger_ns: 40_000_000_000,
                cooperative_ns: 41_000_000_000,
                exit_ns: 43_000_000_000,
            });
        }
        signal.load(Ordering::SeqCst).then_some(StopWindow {
            trigger_ns: 10_000,
            cooperative_ns: 1_000_010_000,
            exit_ns: 3_000_010_000,
        })
    });
    let producer = Producer::prepare(binding, driver, clock, control, b).unwrap();
    Fixture {
        producer,
        stats,
        time,
        stop,
        bad,
        future,
        binding,
    }
}
#[test]
fn complete_prepared_plan_flows_through_actual_codec_and_parent_audit() {
    let b = Budget::new(24 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 0);
    let mut sink = Sink::default();
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    for _ in 0..800 {
        f.producer.advance(&mut sink);
        if f.producer.end().is_some() {
            break;
        }
    }
    assert_eq!(f.producer.end(), Some(End::ReportsComplete));
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 384);
    assert_eq!(f.producer.reports_sent(), 384);
    assert_eq!(sink.frames.len(), 385);
    let mut rx = Channel::new(f.binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let mut audit = Audit::new(f.binding, a(2000), &b).unwrap();
    for frame in &sink.frames {
        let decoded = rx.decode(frame.as_bytes()).unwrap();
        let end = match decoded.message() {
            Message::DispatchRecord(r) => r.calls.last().unwrap().after.unwrap(),
            Message::PhaseEntered { anchor, .. } => *anchor,
            _ => panic!("unexpected frame"),
        };
        audit
            .observe(decoded.message(), Some(a(end.qpc_after + 100)))
            .unwrap();
    }
    assert!(audit.reports_complete());
    assert_eq!(audit.fault(), None);
    f.producer.cleanup();
    f.producer.cleanup();
    assert_eq!(f.stats.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.producer.cleanup_observation().unwrap().result,
        Some(Ok(()))
    );
}
#[test]
fn stop_during_inflight_drains_original_dispatch_without_a_successor() {
    let b = Budget::new(16 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 3);
    let mut sink = Sink::default();
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    f.producer.advance(&mut sink);
    f.stop.store(true, Ordering::SeqCst);
    for _ in 0..8 {
        f.producer.advance(&mut sink);
    }
    assert_eq!(f.producer.end(), Some(End::Stopped));
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 1);
    assert_eq!(f.stats.queries.load(Ordering::SeqCst), 3);
    let record = f.producer.last_record().unwrap().record();
    assert!(!record.pending);
    assert_eq!(record.checked_lanes, 65536);
    let trigger = f.producer.stop_observation().unwrap();
    assert!(trigger.elapsed_ns > trigger.window.trigger_ns);
    assert_eq!(f.producer.exit_deadline_ns(), Some(trigger.window.exit_ns));
    f.producer.advance(&mut sink);
    assert_eq!(f.producer.stop_observation(), Some(trigger));
    f.producer.cleanup();
    assert_eq!(f.stats.closes.load(Ordering::SeqCst), 1);
}
#[test]
fn stale_second_dispatch_retains_and_exports_full_first_failure_without_retry() {
    let b = Budget::new(24 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 1);
    let mut sink = Sink::default();
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    for _ in 0..5 {
        f.producer.advance(&mut sink);
    }
    assert_eq!(f.producer.end(), Some(End::Fault(Fault::Incorrect)));
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 2);
    let record = f.producer.last_record().unwrap().record();
    assert_eq!(record.dispatch, 1);
    assert_eq!(record.mismatches, 65536);
    assert_eq!(record.verified_fma, 0);
    let mut vector = FailureVector::new(f.binding, record, &b).unwrap();
    for _ in 0..32 {
        f.producer.export_failure_step(&mut sink).unwrap();
    }
    assert!(f.producer.failure_export_complete());
    let mut rx = Channel::new(f.binding, Lane::Ordinary, Direction::FromWorker, b).unwrap();
    for frame in &sink.frames {
        let decoded = rx.decode(frame.as_bytes()).unwrap();
        if let Message::FailureChunk(chunk) = decoded.message() {
            vector.accept(chunk).unwrap();
        }
    }
    assert!(vector.complete());
    assert!(vector.bits().iter().copied().eq(f
        .producer
        .retained_output()
        .iter()
        .map(|v| v.to_bits())));
    f.producer.advance(&mut sink);
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 2);
}
#[test]
fn partial_device_output_and_native_error_keep_raw_failures_and_no_successor() {
    for mode in [2, 4] {
        let b = Budget::new(16 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let mut f = fixture(&b, &m, mode);
        let mut sink = Sink::default();
        f.producer.start_fixture(start(&m, f.binding)).unwrap();
        for _ in 0..4 {
            f.producer.advance(&mut sink);
        }
        assert_eq!(f.stats.submits.load(Ordering::SeqCst), 1);
        let record = f.producer.last_record().unwrap().record();
        if mode == 2 {
            assert_eq!(f.producer.end(), Some(End::Fault(Fault::Incorrect)));
            assert_eq!(record.nonfinite, 1);
            assert_eq!(record.first_failure.unwrap().lane, 65535);
            assert!(f.producer.retained_output()[65535].is_nan());
        } else {
            assert!(matches!(
                f.producer.end(),
                Some(End::Fault(Fault::Native(_)))
            ));
            assert_eq!(record.calls.last().unwrap().status, Some(700));
            assert!(record.pending);
            f.producer.cleanup();
            assert_eq!(f.stats.closes.load(Ordering::SeqCst), 0);
        }
    }
}
#[test]
fn native_fixture_bypass_and_unstarted_cleanup_cannot_execute_any_driver_call() {
    let b = Budget::new(16 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 5);
    assert_eq!(
        f.producer.start_fixture(start(&m, f.binding)),
        Err(Fault::Admission)
    );
    f.producer.cleanup();
    f.producer.advance(&mut Sink::default());
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 0);
    assert_eq!(f.stats.closes.load(Ordering::SeqCst), 0);
    assert_eq!(f.producer.end(), Some(End::Fault(Fault::Admission)));
}
#[test]
fn uncertain_send_and_cross_dispatch_clock_failure_preserve_evidence_without_retry() {
    for mode in 0..2 {
        let b = Budget::new(16 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let mut f = fixture(&b, &m, 0);
        let mut sink = Sink {
            reject: mode == 0,
            ..Default::default()
        };
        f.producer.start_fixture(start(&m, f.binding)).unwrap();
        f.producer.advance(&mut sink);
        f.producer.advance(&mut sink);
        if mode == 0 {
            assert_eq!(f.producer.end(), Some(End::Fault(Fault::Transport)));
            assert!(f.producer.failed_frame().is_some());
            assert!(f.producer.last_record().is_some());
        } else {
            f.bad.store(true, Ordering::SeqCst);
            f.producer.advance(&mut sink);
            assert_eq!(f.producer.end(), Some(End::Fault(Fault::Clock)));
            assert_eq!(f.producer.clock_failure().unwrap().anchor, Some(a(0)));
        }
        for _ in 0..3 {
            f.producer.advance(&mut sink);
        }
        assert_eq!(f.stats.submits.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn completed_warmup_does_not_reapply_its_old_deadline_to_measurement_entry() {
    let b = Budget::new(24 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 0);
    let mut sink = Sink::default();
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    for _ in 0..256 {
        f.producer.advance(&mut sink);
    }
    assert_eq!(f.producer.reports_sent(), 128);
    assert_eq!(f.producer.end(), None);
    f.time.store(6_000_000_000, Ordering::SeqCst);
    f.producer.advance(&mut sink);
    assert_eq!(f.producer.end(), None);
    assert!(matches!(
        f.producer.phase_entry(),
        Some(Message::PhaseEntered { .. })
    ));
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 129);
}
#[test]
fn original_dispatch_timeout_stops_pending_query_and_cleanup_without_retry() {
    let b = Budget::new(16 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 3);
    let mut sink = Sink::default();
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    f.producer.advance(&mut sink);
    f.time.store(500_000_000, Ordering::SeqCst);
    f.producer.advance(&mut sink);
    assert_eq!(f.producer.end(), Some(End::Fault(Fault::Deadline)));
    assert!(f.producer.last_record().unwrap().record().pending);
    for _ in 0..3 {
        f.producer.advance(&mut sink);
    }
    f.producer.cleanup();
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 1);
    assert_eq!(f.stats.closes.load(Ordering::SeqCst), 0);
}
#[test]
fn delayed_original_stop_window_cannot_renew_cleanup_or_terminal_deadline() {
    let b = Budget::new(16 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 0);
    let mut sink = Sink::default();
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    f.stop.store(true, Ordering::SeqCst);
    f.time.store(1_100_000_000, Ordering::SeqCst);
    f.producer.advance(&mut sink);
    assert_eq!(f.producer.end(), Some(End::Stopped));
    assert_eq!(f.producer.exit_deadline_ns(), Some(3_000_010_000));
    f.producer.cleanup();
    assert!(!f.producer.cleanup_observation().unwrap().attempted);
    assert_eq!(f.stats.closes.load(Ordering::SeqCst), 0);
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 0);
}
#[test]
fn future_control_window_is_retained_and_never_grants_native_entry() {
    let b = Budget::new(16 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture(&b, &m, 0);
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    f.future.store(true, Ordering::SeqCst);
    f.producer.advance(&mut Sink::default());
    assert_eq!(f.producer.end(), Some(End::Fault(Fault::Protocol)));
    assert_eq!(
        f.producer.control_failure().unwrap().trigger_ns,
        40_000_000_000
    );
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 0);
}
#[test]
fn largest_plan_first_failure_is_owned_and_uncertain_export_preserves_first_fault() {
    let b = Budget::new(24 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let mut f = fixture_run(&b, &m, 2, 2);
    let mut sink = Sink::default();
    f.producer.start_fixture(start(&m, f.binding)).unwrap();
    f.producer.advance(&mut sink);
    f.producer.advance(&mut sink);
    assert_eq!(f.producer.end(), Some(End::Fault(Fault::Incorrect)));
    assert_eq!(f.producer.retained_output().len(), 262144);
    let record = f.producer.last_record().unwrap().record();
    assert_eq!(record.first_failure.unwrap().lane, 262143);
    assert_eq!(record.nonfinite, 1);
    sink.reject = true;
    assert_eq!(
        f.producer.export_failure_step(&mut sink),
        Err(Fault::Transport)
    );
    assert!(f.producer.failed_frame().is_some());
    assert_eq!(f.producer.end(), Some(End::Fault(Fault::Incorrect)));
    assert!(!f.producer.failure_export_complete());
    assert_eq!(
        f.producer.export_failure_step(&mut sink),
        Err(Fault::Transport)
    );
    assert_eq!(f.stats.submits.load(Ordering::SeqCst), 1);
}
#[test]
fn failure_export_capacity_and_late_last_handoff_cannot_be_retried_into_success() {
    for mode in 0..2 {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let mut f = fixture(&b, &m, 2);
        let mut sink = Sink::default();
        f.producer.start_fixture(start(&m, f.binding)).unwrap();
        f.producer.advance(&mut sink);
        f.producer.advance(&mut sink);
        if mode == 0 {
            let full = b.reserve(24 * 1024 * 1024 - b.used()).unwrap();
            assert_eq!(
                f.producer.export_failure_step(&mut sink),
                Err(Fault::Capacity)
            );
            drop(full);
            assert_eq!(f.producer.export_fault(), Some(Fault::Capacity));
            assert_eq!(
                f.producer.export_failure_step(&mut sink),
                Err(Fault::Capacity)
            );
            assert_eq!(f.producer.failure_words_handed_off(), 0);
        } else {
            sink.jump = Some((
                33,
                f.time.clone(),
                f.producer.exit_deadline_ns().unwrap() + 1000,
            ));
            for _ in 0..31 {
                assert_eq!(f.producer.export_failure_step(&mut sink), Ok(false));
            }
            assert_eq!(
                f.producer.export_failure_step(&mut sink),
                Err(Fault::Deadline)
            );
            assert_eq!(f.producer.failure_words_handed_off(), 65536);
            assert_eq!(f.producer.export_fault(), Some(Fault::Deadline));
        }
        assert!(!f.producer.failure_export_complete());
        assert_eq!(f.producer.retained_output().len(), 65536);
        assert_eq!(f.producer.end(), Some(End::Fault(Fault::Incorrect)));
    }
}
