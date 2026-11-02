use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::{Anchor, Metric, Ticket},
    sampler::{
        budget::Budget,
        child::NativeAdmission,
        framing::{Framer, ReadStep, Source},
        wire::Lane,
    },
    workload::{
        manifest::{Artifacts, PilotDraft, PilotManifest},
        telemetry::{Collector, Error, Output, Receipt},
        transport::{Queue, QueuedBytes, RawReceived, SendFailure, Sender, WorkerReceived},
        wire::{Channel, Direction, Message, WorkerBinding},
    },
};
use autotune_probe::report::{Failure, Field, Reason};
use std::{cell::RefCell, rc::Rc};
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
struct Bytes<'a>(&'a [u8]);
impl Source for Bytes<'_> {
    fn read(&mut self, out: &mut [u8]) -> ReadStep {
        let n = out.len().min(self.0.len());
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        ReadStep {
            reported: n,
            error: None,
        }
    }
}
struct Sink {
    b: Budget,
    frames: Rc<RefCell<Vec<QueuedBytes>>>,
    fail: bool,
}
impl Queue for Sink {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        assert_eq!(lane, Lane::Ordinary);
        if self.fail {
            return Err(SendFailure::Os(109));
        }
        self.frames
            .borrow_mut()
            .push(QueuedBytes::copy(bytes, &self.b)?);
        Ok(())
    }
}
struct Harness {
    b: Budget,
    binding: WorkerBinding,
    collector: Collector,
    tx: Channel,
    rx: Channel,
    out: Channel,
    sender: Sender<Sink>,
    frames: Rc<RefCell<Vec<QueuedBytes>>>,
    gate: NativeAdmission,
}
impl Harness {
    fn new(fail: bool) -> Self {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
        let frames = Rc::new(RefCell::new(Vec::new()));
        Self {
            collector: Collector::new(binding, a(0), b.clone()).unwrap(),
            tx: Channel::sampler(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap(),
            rx: Channel::sampler(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap(),
            out: Channel::sampler(binding, Lane::Ordinary, Direction::FromWorker, b.clone())
                .unwrap(),
            sender: Sender::new(
                Sink {
                    b: b.clone(),
                    frames: frames.clone(),
                    fail,
                },
                binding,
                Direction::FromWorker,
                &b,
            )
            .unwrap(),
            frames,
            gate: NativeAdmission::default(),
            binding,
            b,
        }
    }
    fn ticket(&self, metric: Metric, ns: u64) -> Ticket {
        Ticket {
            binding: self.binding.monitor(),
            metric,
            slot: ns / 250_000_000,
            scheduled_ns: ns / 250_000_000 * 250_000_000,
            start_ns: ns,
        }
    }
    fn request(&mut self, t: Ticket, receipt: Option<Anchor>) -> WorkerReceived {
        let frame = self
            .tx
            .encode(&Message::SamplerQuery {
                ticket: t,
                issued: a(t.start_ns),
            })
            .unwrap();
        decode(frame.as_bytes(), receipt, &mut self.rx, &self.b)
    }
    fn collect(&mut self, t: Ticket, raw: &Field, before: Option<Anchor>, after: Option<Anchor>) {
        let req = self.request(t, Some(a(t.start_ns + 100)));
        let mut clocks = [before, after].into_iter();
        assert!(self
            .collector
            .fixture(
                req,
                raw,
                &self.gate,
                || clocks.next().flatten(),
                Output {
                    channel: &mut self.out,
                    sender: &mut self.sender
                }
            )
            .is_ok());
    }
}
fn decode(bytes: &[u8], receipt: Option<Anchor>, rx: &mut Channel, b: &Budget) -> WorkerReceived {
    RawReceived::new(
        Lane::Ordinary,
        Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes)),
        receipt,
    )
    .decode_worker(rx)
}
#[test]
fn all_six_metrics_use_one_serial_gate_and_preserve_query_and_response_owners() {
    let mut h = Harness::new(false);
    for (i, metric) in Metric::ALL.into_iter().enumerate() {
        let ns = i as u64 * 1_000_000;
        let t = h.ticket(metric, ns);
        let (unit, value) = match metric {
            Metric::Temperature => ("degC", 42),
            Metric::Power => ("mW", 100_000),
            Metric::GraphicsClock | Metric::MemoryClock => ("MHz", 1500),
            _ => ("%", 10),
        };
        h.collect(
            t,
            &Field::available(unit, value),
            Some(a(ns + 200)),
            Some(a(ns + 300)),
        );
    }
    assert_eq!(h.collector.entries().len(), 6);
    assert_eq!(h.frames.borrow().len(), 6);
    assert_eq!(h.collector.fault(), None);
    assert!(!h.gate.pending());
    let mut rx = Channel::sampler(
        h.binding,
        Lane::Ordinary,
        Direction::FromWorker,
        h.b.clone(),
    )
    .unwrap();
    for (i, frame) in h.frames.borrow().iter().enumerate() {
        let received = decode(frame.bytes(), Some(a(10_000_000)), &mut rx, &h.b);
        assert!(matches!(
            received.message(),
            Ok(Message::SamplerRecord {
                native: false,
                raw: Some(_),
                ..
            })
        ));
        let t = h.ticket(Metric::ALL[i], i as u64 * 1_000_000);
        assert_eq!(
            Receipt::assess(h.binding, t, a(t.start_ns), received).record,
            Err(Error::Simulated)
        );
    }
}
#[test]
fn stop_during_getter_return_keeps_raw_value_and_closes_all_successors() {
    let mut h = Harness::new(false);
    let t = h.ticket(Metric::Temperature, 0);
    let req = h.request(t, Some(a(100)));
    let mut calls = 0;
    assert!(h
        .collector
        .fixture(
            req,
            &Field::available("degC", 42),
            &h.gate,
            || {
                calls += 1;
                if calls == 2 {
                    assert!(h.gate.pending());
                    h.gate.stop();
                }
                Some(a(calls * 100 + 100))
            },
            Output {
                channel: &mut h.out,
                sender: &mut h.sender
            }
        )
        .is_ok());
    assert_eq!(h.collector.fault(), Some(Error::Stopped));
    assert!(!h.gate.pending());
    assert!(h.gate.stopped());
    assert!(matches!(
        &h.collector.entries()[0].response,
        Some(Message::SamplerRecord {
            raw: Some(_),
            after: Some(_),
            ..
        })
    ));
    assert!(h.collector.entries()[0].queued);
    let t = h.ticket(Metric::Power, 1_000_000);
    let req = h.request(t, Some(a(1_000_100)));
    let pointer = req.raw().packet().bytes().as_ptr();
    let rejected = h
        .collector
        .fixture(
            req,
            &Field::available("mW", 1),
            &h.gate,
            || panic!("no successor capture"),
            Output {
                channel: &mut h.out,
                sender: &mut h.sender,
            },
        )
        .err()
        .unwrap();
    assert_eq!(rejected.raw().packet().bytes().as_ptr(), pointer);
    assert_eq!(h.collector.entries().len(), 1);
}
#[test]
fn missing_post_clock_retains_raw_return_and_no_fabricated_anchor() {
    let mut h = Harness::new(false);
    let t = h.ticket(Metric::Power, 0);
    let raw = Field::available("mW", 100_000);
    h.collect(t, &raw, Some(a(200)), None);
    assert_eq!(h.collector.fault(), Some(Error::Clock));
    assert!(h.collector.history().missing());
    assert!(
        matches!(&h.collector.entries()[0].response,Some(Message::SamplerRecord{after:None,raw:Some(f),..}) if f==&raw)
    );
    assert!(h.collector.entries()[0].queued);
}
#[test]
fn stale_or_stopped_query_never_enters_a_getter() {
    for stopped in [false, true] {
        let mut h = Harness::new(false);
        if stopped {
            h.gate.stop();
        }
        let t = h.ticket(Metric::Power, 0);
        h.collect(
            t,
            &Field::available("mW", 100_000),
            Some(a(if stopped { 200 } else { 50_000_000 })),
            Some(a(50_000_100)),
        );
        assert!(h.gate.stopped());
        assert!(!h.gate.pending());
        assert!(matches!(
            &h.collector.entries()[0].response,
            Some(Message::SamplerRecord {
                raw: None,
                after: None,
                ..
            })
        ));
    }
}
#[test]
fn replay_cannot_replace_an_earlier_successful_reading() {
    let mut h = Harness::new(false);
    let t = h.ticket(Metric::Temperature, 0);
    h.collect(t, &Field::available("degC", 42), Some(a(200)), Some(a(300)));
    let first = h.collector.entries()[0].response.clone();
    let t = h.ticket(Metric::Temperature, 1_000_000);
    h.collect(
        t,
        &Field::available("degC", 1),
        Some(a(1_000_200)),
        Some(a(1_000_300)),
    );
    assert_eq!(h.collector.fault(), Some(Error::Replay));
    assert_eq!(h.collector.entries()[0].response, first);
    assert_eq!(h.collector.entries()[0].fault, None);
}
#[test]
fn gpu_loss_safety_boundaries_and_fractional_values_keep_raw_failures() {
    for (metric, field, expected) in [
        (
            Metric::MemoryUtilization,
            Field::unavailable("wrong", Failure::vendor(15, "raw getter")),
            Error::DeviceLost,
        ),
        (
            Metric::Temperature,
            Field::available("degC", 75),
            Error::Safety,
        ),
        (
            Metric::Power,
            Field::available("mW", 200_000),
            Error::Safety,
        ),
        (Metric::Power, Field::available("mW", 1.5), Error::Value),
    ] {
        let mut h = Harness::new(false);
        let t = h.ticket(metric, 0);
        h.collect(t, &field, Some(a(200)), Some(a(300)));
        assert_eq!(h.collector.fault(), Some(expected));
        assert!(h.collector.entries()[0].queued);
        assert!(
            matches!(&h.collector.entries()[0].response,Some(Message::SamplerRecord{raw:Some(f),..}) if f==&field)
        );
    }
}
#[test]
fn optional_memory_unavailability_is_retained_without_stopping_other_metrics() {
    let mut h = Harness::new(false);
    let t = h.ticket(Metric::MemoryUtilization, 0);
    h.collect(
        t,
        &Field::unavailable("%", Failure::new(Reason::Unsupported, "optional")),
        Some(a(200)),
        Some(a(300)),
    );
    assert_eq!(h.collector.fault(), None);
    assert!(!h.gate.stopped());
}
#[test]
fn uncertain_send_keeps_exact_response_and_os_cause_without_retry() {
    let mut h = Harness::new(true);
    let t = h.ticket(Metric::Power, 0);
    h.collect(
        t,
        &Field::available("mW", 100_000),
        Some(a(200)),
        Some(a(300)),
    );
    let e = &h.collector.entries()[0];
    assert_eq!(e.send_error, Some(SendFailure::Os(109)));
    assert!(e.unsent.is_some());
    assert!(!e.queued);
    assert!(h.gate.stopped());
    assert!(h.frames.borrow().is_empty());
}
#[test]
fn parent_reader_checks_original_issue_conservative_bracket_age_and_raw_source() {
    for (before, after, receipt, expected) in [
        (Some(a(200)), Some(a(300)), Some(a(400)), None),
        (None, Some(a(300)), Some(a(400)), Some(Error::Clock)),
        (
            Some(a(200)),
            Some(a(50_000_100)),
            Some(a(50_000_200)),
            Some(Error::Deadline),
        ),
        (
            Some(a(200)),
            Some(a(300)),
            Some(a(100_000_400)),
            Some(Error::Deadline),
        ),
        (
            Some(Anchor {
                clock_token: [0; 16],
                ..a(200)
            }),
            Some(a(300)),
            Some(a(400)),
            Some(Error::Clock),
        ),
    ] {
        let h = Harness::new(false);
        let t = h.ticket(Metric::Temperature, 0);
        let message = Message::SamplerRecord {
            ticket: t,
            before,
            after,
            raw: Some(Field::available("degC", 42)),
            native: true,
        };
        let mut tx = Channel::sampler(
            h.binding,
            Lane::Ordinary,
            Direction::FromWorker,
            h.b.clone(),
        )
        .unwrap();
        let mut rx = Channel::sampler(
            h.binding,
            Lane::Ordinary,
            Direction::FromWorker,
            h.b.clone(),
        )
        .unwrap();
        let frame = tx.encode(&message).unwrap();
        let received = decode(frame.as_bytes(), receipt, &mut rx, &h.b);
        let assessed = Receipt::assess(h.binding, t, a(0), received);
        assert_eq!(assessed.record.err(), expected);
        assert_eq!(assessed.raw.raw().packet().bytes(), frame.as_bytes());
    }
}
#[test]
fn full_run_six_metric_history_and_owned_facts_fit_the_session_budget() {
    let mut h = Harness::new(false);
    for tick in 0..4500 {
        let ns = tick * 10_000_000;
        assert!(h.collector.idle(Some(a(ns)), &h.gate));
        if tick % 25 == 0 {
            for (i, m) in Metric::ALL.into_iter().enumerate() {
                let n = ns + i as u64 * 1_000_000;
                let t = h.ticket(m, n);
                let (unit, value) = match m {
                    Metric::Temperature => ("degC", 42),
                    Metric::Power => ("mW", 100_000),
                    Metric::GraphicsClock | Metric::MemoryClock => ("MHz", 1500),
                    _ => ("%", 10),
                };
                h.collect(
                    t,
                    &Field::available(unit, value),
                    Some(a(n + 200)),
                    Some(a(n + 300)),
                );
            }
        }
    }
    assert_eq!(h.collector.entries().len(), 1080);
    assert_eq!(h.frames.borrow().len(), 1080);
    assert_eq!(h.collector.fault(), None);
    assert!(!h.collector.idle(Some(a(45_000_000_000)), &h.gate));
    assert_eq!(h.collector.fault(), Some(Error::Deadline));
    assert!(h.b.used() < 32 * 1024 * 1024);
    assert_eq!(h.collector.entries()[0].fault, None);
}
#[test]
fn full_codec_binding_survives_decode_and_cannot_be_substituted_by_same_monitor_identity() {
    let mut h = Harness::new(false);
    let text = manifest(&h.b)
        .canonical()
        .replace(&digest(9).to_hex(), &digest(77).to_hex());
    let m = PilotManifest::parse(text.as_bytes(), &h.b).unwrap();
    let foreign = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    assert_eq!(foreign.monitor(), h.binding.monitor());
    assert_ne!(foreign, h.binding);
    let t = h.ticket(Metric::Power, 0);
    let mut tx =
        Channel::sampler(foreign, Lane::Ordinary, Direction::ToWorker, h.b.clone()).unwrap();
    let mut rx =
        Channel::sampler(foreign, Lane::Ordinary, Direction::ToWorker, h.b.clone()).unwrap();
    let frame = tx
        .encode(&Message::SamplerQuery {
            ticket: t,
            issued: a(0),
        })
        .unwrap();
    let req = decode(frame.as_bytes(), Some(a(100)), &mut rx, &h.b);
    assert_eq!(req.binding(), Ok(foreign));
    assert!(h
        .collector
        .fixture(
            req,
            &Field::available("mW", 1),
            &h.gate,
            || panic!("wrong manifest must not enter getter lane"),
            Output {
                channel: &mut h.out,
                sender: &mut h.sender
            }
        )
        .is_ok());
    assert_eq!(h.collector.fault(), Some(Error::Protocol));
    let mut tx =
        Channel::sampler(foreign, Lane::Ordinary, Direction::FromWorker, h.b.clone()).unwrap();
    let mut rx =
        Channel::sampler(foreign, Lane::Ordinary, Direction::FromWorker, h.b.clone()).unwrap();
    let frame = tx
        .encode(&Message::SamplerRecord {
            ticket: t,
            before: Some(a(200)),
            after: Some(a(300)),
            raw: Some(Field::available("mW", 1)),
            native: true,
        })
        .unwrap();
    let received = decode(frame.as_bytes(), Some(a(400)), &mut rx, &h.b);
    assert_eq!(
        Receipt::assess(h.binding, t, a(0), received).record,
        Err(Error::Protocol)
    );
}
#[test]
fn metadata_and_staging_pressure_fail_before_getter_without_dropping_the_request() {
    let mut h = Harness::new(false);
    let t = h.ticket(Metric::Power, 0);
    let req = h.request(t, Some(a(100)));
    let pointer = req.raw().packet().bytes().as_ptr();
    let _pressure = h.b.reserve(32 * 1024 * 1024 - h.b.used()).unwrap();
    assert!(h
        .collector
        .fixture(
            req,
            &Field::available("mW", 1),
            &h.gate,
            || panic!("no getter clock after failed reservation"),
            Output {
                channel: &mut h.out,
                sender: &mut h.sender
            }
        )
        .is_ok());
    assert_eq!(h.collector.fault(), Some(Error::Capacity));
    assert_eq!(
        h.collector.entries()[0]
            .request
            .raw()
            .packet()
            .bytes()
            .as_ptr(),
        pointer
    );
    assert!(h.collector.entries()[0].response.is_none());
}
#[test]
fn malformed_scalar_tag_wrong_role_and_invalid_slots_are_rejected_with_raw_bytes_retained() {
    let h = Harness::new(false);
    let t = h.ticket(Metric::Power, 0);
    let message = Message::SamplerRecord {
        ticket: t,
        before: Some(a(200)),
        after: Some(a(300)),
        raw: Some(Field::available("mW", 1)),
        native: true,
    };
    let mut tx = Channel::sampler(
        h.binding,
        Lane::Ordinary,
        Direction::FromWorker,
        h.b.clone(),
    )
    .unwrap();
    let frame = tx.encode(&message).unwrap();
    let text = std::str::from_utf8(&frame.as_bytes()[4..])
        .unwrap()
        .replace("\"kind\":\"integer\"", "\"kind\":\"float_repr\"");
    let len = text.len() + 4;
    let mut bytes = (len as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(text.as_bytes());
    let mut rx = Channel::sampler(
        h.binding,
        Lane::Ordinary,
        Direction::FromWorker,
        h.b.clone(),
    )
    .unwrap();
    let received = decode(&bytes, Some(a(400)), &mut rx, &h.b);
    assert!(received.message().is_err());
    assert_eq!(received.raw().packet().bytes(), bytes);
    let mut worker = Channel::new(
        h.binding,
        Lane::Ordinary,
        Direction::FromWorker,
        h.b.clone(),
    )
    .unwrap();
    assert!(worker.decode(frame.as_bytes()).is_err());
    let mut tx = Channel::sampler(h.binding, Lane::Ordinary, Direction::ToWorker, h.b).unwrap();
    assert!(tx
        .encode(&Message::SamplerQuery {
            ticket: Ticket {
                slot: u64::MAX,
                ..t
            },
            issued: a(0)
        })
        .is_err());
}
#[test]
fn initial_nonzero_bracket_is_recorded_once_and_idle_clock_failure_remains_raw() {
    let h = Harness::new(false);
    let first = Anchor {
        qpc_after: 10,
        ..a(0)
    };
    let mut collector = Collector::new(h.binding, first, h.b).unwrap();
    assert_eq!(collector.history().raw(), [first]);
    assert!(!collector.idle(None, &h.gate));
    assert!(collector.history().missing());
    assert_eq!(collector.fault(), Some(Error::Clock));
    assert!(h.gate.stopped());
}
