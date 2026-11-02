use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{
        manifest::{Artifacts, PilotDraft, PilotManifest},
        sampler_dispatch::Dispatch,
        settings::{Block, Capture, Draft, Observation},
        settings_transport::{Child, Parent},
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
use autotune_probe::{nvml::SettingMetric, report::Field};
fn draft() -> Draft {
    Draft {
        raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
        raw_pci: "0000:01:02.0".into(),
        observations: SettingMetric::ALL.map(|kind| Observation {
            kind,
            before: Some(a(1000)),
            after: Some(a(1000)),
            raw: Some(Field::available(
                kind.unit(),
                if kind == SettingMetric::PowerLimit {
                    215000
                } else {
                    0
                },
            )),
        }),
        halt: None,
    }
}
use autotune_benchmark_worker::{
    evidence::Reason,
    sampler::{
        framing::{Framer, ReadStep, Source},
        wire::{Lane, WireError},
    },
    workload::{
        transport::{Queue, QueuedBytes, RawReceived, SendFailure, Sender},
        wire::{Channel, Direction, Message, WorkerBinding},
    },
};
use std::{cell::RefCell, rc::Rc};
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
struct MemoryQueue {
    b: Budget,
    frames: Rc<RefCell<Vec<(Lane, QueuedBytes)>>>,
    reject_ordinary: bool,
}
impl Queue for MemoryQueue {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        if self.reject_ordinary && lane == Lane::Ordinary {
            return Err(SendFailure::Queue);
        }
        self.frames
            .borrow_mut()
            .push((lane, QueuedBytes::copy(bytes, &self.b)?));
        Ok(())
    }
}
#[test]
fn owned_worker_handoff_passes_real_framer_and_decoder_and_keeps_receipt() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    let baseline = b.used();
    {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let mut sender = Sender::new(
            MemoryQueue {
                b: b.clone(),
                frames: frames.clone(),
                reject_ordinary: false,
            },
            binding,
            Direction::ToWorker,
            &b,
        )
        .unwrap();
        let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let frame = tx.encode(&Message::Start { issued: a(1000) }).unwrap();
        let expected = frame.as_bytes().to_vec();
        assert!(sender.send_worker(frame).is_ok());
        let (lane, copy) = frames.borrow_mut().pop().unwrap();
        assert_eq!(copy.bytes(), expected);
        let packet = Framer::new(b.clone())
            .unwrap()
            .read(&mut Bytes(copy.bytes()));
        let received = RawReceived::new(lane, packet, Some(a(2000))).decode_worker(&mut rx);
        assert_eq!(received.raw().receipt(), Some(a(2000)));
        assert_eq!(received.raw().packet().bytes(), expected);
        assert!(matches!(received.message(), Ok(Message::Start { issued }) if *issued == a(1000)));
    }
    assert_eq!(b.used(), baseline);
}
#[test]
fn ordinary_failure_returns_exact_frame_and_preserves_control_capacity() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let frames = Rc::new(RefCell::new(Vec::new()));
    let mut sender = Sender::new(
        MemoryQueue {
            b: b.clone(),
            frames: frames.clone(),
            reject_ordinary: true,
        },
        binding,
        Direction::ToWorker,
        &b,
    )
    .unwrap();
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    let frame = tx.encode(&Message::Start { issued: a(1000) }).unwrap();
    let pointer = frame.as_bytes().as_ptr();
    let returned = sender.send_worker(frame).err().unwrap();
    assert_eq!(returned.as_bytes().as_ptr(), pointer);
    assert_eq!(sender.fault(Lane::Ordinary), Some(SendFailure::Queue));
    assert!(sender.send_worker(returned).is_err());
    assert!(frames.borrow().is_empty());
    let mut control = Channel::new(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let stop = control
        .encode(&Message::Stop {
            reason: Reason::new("fixture-stop").unwrap(),
            trigger_ns: 1000,
            cooperative_ns: 1_000_001_000,
            exit_ns: 3_000_001_000,
        })
        .unwrap();
    assert!(sender.send_worker(stop).is_ok());
    assert_eq!(frames.borrow()[0].0, Lane::Control);
}
#[test]
fn different_run_or_direction_never_enters_the_pipe_queue() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    for wrong_direction in [false, true] {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let mut sender = Sender::new(
            MemoryQueue {
                b: b.clone(),
                frames: frames.clone(),
                reject_ordinary: false,
            },
            binding,
            Direction::ToWorker,
            &b,
        )
        .unwrap();
        let other = WorkerBinding::new(&m.bind_run(1, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
        let mut tx = Channel::new(
            if wrong_direction { binding } else { other },
            Lane::Ordinary,
            if wrong_direction {
                Direction::FromWorker
            } else {
                Direction::ToWorker
            },
            b.clone(),
        )
        .unwrap();
        let message = if wrong_direction {
            Message::PhaseEntered {
                phase: autotune_benchmark_worker::workload::dispatch::Phase::Measured,
                anchor: a(1000),
            }
        } else {
            Message::Start { issued: a(1000) }
        };
        assert!(sender.send_worker(tx.encode(&message).unwrap()).is_err());
        assert_eq!(sender.fault(Lane::Ordinary), Some(SendFailure::Binding));
        assert!(frames.borrow().is_empty());
    }
}
#[test]
fn framing_rejections_wrong_lane_and_missing_clock_keep_bytes_and_close_decoder() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    for case in 0..3 {
        let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let frame = tx.encode(&Message::Start { issued: a(1000) }).unwrap();
        let bytes = if case == 0 {
            &frame.as_bytes()[..17]
        } else {
            frame.as_bytes()
        };
        let packet = Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes));
        let r = RawReceived::new(
            if case == 1 {
                Lane::Control
            } else {
                Lane::Ordinary
            },
            packet,
            if case == 2 { None } else { Some(a(2000)) },
        )
        .decode_worker(&mut rx);
        assert_eq!(r.raw().packet().bytes(), bytes);
        if case == 2 {
            assert!(r.message().is_ok());
            assert_eq!(r.raw().receipt(), None);
        } else {
            assert!(r.message().is_err());
        }
        assert!(matches!(
            rx.decode(frame.as_bytes()),
            Err(WireError::Poisoned)
        ));
    }
}
#[test]
fn settings_pipe_packet_roundtrips_and_transport_rejection_cannot_be_retried() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    for rejected in [false, true] {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        let request = parent.issue(a(500)).unwrap();
        let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
        let capture = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
        let response = child
            .accept(request.as_bytes())
            .unwrap()
            .respond(&capture)
            .unwrap();
        let mut sender = Sender::new(
            MemoryQueue {
                b: b.clone(),
                frames: frames.clone(),
                reject_ordinary: false,
            },
            binding,
            Direction::FromWorker,
            &b,
        )
        .unwrap();
        assert!(sender.send_settings(response).is_ok());
        let (lane, bytes) = frames.borrow_mut().pop().unwrap();
        let raw = if rejected {
            &bytes.bytes()[..20]
        } else {
            bytes.bytes()
        };
        let packet = Framer::new(b.clone()).unwrap().read(&mut Bytes(raw));
        let received =
            RawReceived::new(lane, packet, Some(a(2000))).decode_settings(&mut parent, &run);
        assert_eq!(received.raw().packet().bytes(), raw);
        if rejected {
            assert_eq!(received.result().err(), Some(Block::Scope));
            assert!(parent.receive(&run, bytes.bytes(), a(3000)).is_err());
        } else {
            assert_eq!(
                received.result().unwrap().capture().digest(),
                capture.digest()
            );
            assert_eq!(received.result().unwrap().fault(), None);
        }
    }
}
#[test]
fn destination_capacity_is_reserved_before_copy_and_released_on_drop() {
    let b = Budget::new(16).unwrap();
    let bytes = vec![7; 1000];
    assert!(matches!(
        QueuedBytes::copy(&bytes, &b),
        Err(SendFailure::Capacity)
    ));
    assert_eq!(b.used(), 0);
    let b = Budget::new(4096).unwrap();
    let copy = QueuedBytes::copy(&bytes, &b).unwrap();
    assert!(b.used() >= bytes.len());
    assert_eq!(copy.bytes(), bytes);
    drop(copy);
    assert_eq!(b.used(), 0);
    assert!(QueuedBytes::copy(&vec![0; 65537], &b).is_err());
    assert_eq!(b.used(), 0);
}

fn request_packet(b: &Budget, bytes: &[u8], receipt: Option<Anchor>) -> RawReceived {
    RawReceived::new(
        Lane::Ordinary,
        Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes)),
        receipt,
    )
}
#[test]
fn selected_dispatch_retains_the_request_capture_and_full_parent_response_chain() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    let baseline = b.used();
    {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let mut sender = Sender::new(
            MemoryQueue {
                b: b.clone(),
                frames: frames.clone(),
                reject_ordinary: false,
            },
            binding,
            Direction::FromWorker,
            &b,
        )
        .unwrap();
        let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        let request = parent.issue(a(500)).unwrap();
        let mut dispatch = Dispatch::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        assert!(dispatch
            .fixture(
                &run,
                request_packet(&b, request.as_bytes(), Some(a(800))),
                &draft(),
                &mut sender
            )
            .is_ok());
        assert_eq!(dispatch.fault(), None);
        assert!(dispatch.response_queued());
        assert_eq!(dispatch.raw().unwrap().packet().bytes(), request.as_bytes());
        assert_eq!(dispatch.raw().unwrap().receipt(), Some(a(800)));
        assert_eq!(
            dispatch.capture().unwrap().source(),
            autotune_benchmark_worker::workload::settings::Source::SimulatedFixture
        );
        let (lane, response) = frames.borrow_mut().pop().unwrap();
        let received = RawReceived::new(
            lane,
            Framer::new(b.clone())
                .unwrap()
                .read(&mut Bytes(response.bytes())),
            Some(a(2000)),
        )
        .decode_settings(&mut parent, &run);
        assert_eq!(received.result().unwrap().fault(), None);
        assert_eq!(
            received.result().unwrap().capture().digest(),
            dispatch.capture().unwrap().digest()
        );
        let duplicate = request_packet(&b, request.as_bytes(), Some(a(3000)));
        let pointer = duplicate.packet().bytes().as_ptr();
        let rejected = dispatch
            .fixture(&run, duplicate, &draft(), &mut sender)
            .err()
            .unwrap();
        assert_eq!(rejected.packet().bytes().as_ptr(), pointer);
        assert_eq!(dispatch.raw().unwrap().receipt(), Some(a(800)));
        assert_eq!(dispatch.fault(), Some(Block::Scope));
        assert!(frames.borrow().is_empty());
    }
    assert_eq!(b.used(), baseline);
}
#[test]
fn dispatcher_rejects_bad_request_timing_scope_and_partial_bytes_before_collection() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    for case in 0..5 {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let mut sender = Sender::new(
            MemoryQueue {
                b: b.clone(),
                frames: frames.clone(),
                reject_ordinary: false,
            },
            binding,
            Direction::FromWorker,
            &b,
        )
        .unwrap();
        let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        let request = parent.issue(a(500)).unwrap();
        let mut dispatch = Dispatch::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        let raw = request_packet(
            &b,
            if case == 0 {
                &request.as_bytes()[..20]
            } else {
                request.as_bytes()
            },
            match case {
                1 => None,
                2 => Some(a(400)),
                3 => Some(a(10_000_000_000)),
                _ => Some(a(800)),
            },
        );
        let other = m.bind_run(1, 0).unwrap();
        assert!(dispatch
            .fixture(
                if case == 4 { &other } else { &run },
                raw,
                &draft(),
                &mut sender
            )
            .is_ok());
        assert!(dispatch.raw().is_some());
        assert!(dispatch.capture().is_none());
        assert!(dispatch.fault().is_some());
        assert!(!dispatch.response_queued());
        assert!(frames.borrow().is_empty());
    }
}
#[test]
fn dispatch_failure_preserves_capture_original_frame_and_exact_send_reason() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    let frames = Rc::new(RefCell::new(Vec::new()));
    let mut sender = Sender::new(
        MemoryQueue {
            b: b.clone(),
            frames: frames.clone(),
            reject_ordinary: true,
        },
        binding,
        Direction::FromWorker,
        &b,
    )
    .unwrap();
    let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    let request = parent.issue(a(500)).unwrap();
    let mut dispatch = Dispatch::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    assert!(dispatch
        .fixture(
            &run,
            request_packet(&b, request.as_bytes(), Some(a(800))),
            &draft(),
            &mut sender
        )
        .is_ok());
    assert_eq!(dispatch.send_failure(), Some(SendFailure::Queue));
    assert_eq!(dispatch.fault(), Some(Block::Stopped));
    assert!(dispatch.capture().unwrap().current().is_ok());
    let digest = dispatch.capture().unwrap().digest();
    let retained = dispatch.unsent().unwrap().as_bytes().as_ptr();
    assert!(dispatch
        .fixture(
            &run,
            request_packet(&b, request.as_bytes(), Some(a(2000))),
            &draft(),
            &mut sender
        )
        .is_err());
    assert_eq!(dispatch.capture().unwrap().digest(), digest);
    assert_eq!(dispatch.unsent().unwrap().as_bytes().as_ptr(), retained);
    assert!(frames.borrow().is_empty());
    assert!(!dispatch.response_queued());
}
#[test]
fn child_setup_window_and_receipt_cannot_be_rebased_by_a_later_parent_request() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    for case in 0..3 {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let mut sender = Sender::new(
            MemoryQueue {
                b: b.clone(),
                frames: frames.clone(),
                reject_ordinary: false,
            },
            binding,
            Direction::FromWorker,
            &b,
        )
        .unwrap();
        let mut parent =
            Parent::new(&run, 1_000_000_000, a(0), a(1_000_000_000), b.clone()).unwrap();
        let request = parent.issue(a(1_000_000_000)).unwrap();
        let mut dispatch = Dispatch::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        let mut d = draft();
        let observed = if case == 0 {
            10_000_000_000
        } else if case == 1 {
            1_000_000_000
        } else {
            2_000_000_000
        };
        for o in &mut d.observations {
            o.before = Some(a(observed));
            o.after = Some(a(observed));
        }
        if case == 2 {
            assert_eq!(dispatch.tick(None), Err(Block::Clock));
        }
        assert!(dispatch
            .fixture(
                &run,
                request_packet(&b, request.as_bytes(), Some(a(1_000_001_000))),
                &d,
                &mut sender
            )
            .is_ok());
        assert!(dispatch.fault().is_some());
        assert_eq!(dispatch.deadline_ns(), 10_000_000_000);
        assert!(!dispatch.response_queued());
        assert!(frames.borrow().is_empty());
        if case != 2 {
            assert!(dispatch.capture().is_some());
        }
    }
}
#[test]
fn failed_raw_facts_still_reach_parent_without_becoming_readable_values() {
    use autotune_probe::report::{Failure, Reason};
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    let frames = Rc::new(RefCell::new(Vec::new()));
    let mut sender = Sender::new(
        MemoryQueue {
            b: b.clone(),
            frames: frames.clone(),
            reject_ordinary: false,
        },
        binding,
        Direction::FromWorker,
        &b,
    )
    .unwrap();
    let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    let request = parent.issue(a(500)).unwrap();
    let mut dispatch = Dispatch::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    let mut d = draft();
    d.halt = Some(Block::GpuLost);
    d.observations[0].raw = Some(Field::unavailable(
        "mW",
        Failure {
            reason: Reason::GpuLost,
            vendor_status: Some(15),
            detail: "original getter fault".into(),
        },
    ));
    assert!(dispatch
        .fixture(
            &run,
            request_packet(&b, request.as_bytes(), Some(a(800))),
            &d,
            &mut sender
        )
        .is_ok());
    assert_eq!(dispatch.fault(), Some(Block::GpuLost));
    assert!(dispatch.response_queued());
    let (_, response) = frames.borrow_mut().pop().unwrap();
    let received = parent.receive(&run, response.bytes(), a(2000)).unwrap();
    assert_eq!(received.fault(), Some(Block::GpuLost));
    assert!(received
        .capture()
        .canonical()
        .contains("original getter fault"));
}

#[test]
fn polling_after_collection_keeps_the_last_getter_clock_instead_of_receipt() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    let frames = Rc::new(RefCell::new(Vec::new()));
    let mut sender = Sender::new(
        MemoryQueue {
            b: b.clone(),
            frames,
            reject_ordinary: false,
        },
        binding,
        Direction::FromWorker,
        &b,
    )
    .unwrap();
    let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    let request = parent.issue(a(500)).unwrap();
    let mut dispatch = Dispatch::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    assert!(dispatch
        .fixture(
            &run,
            request_packet(&b, request.as_bytes(), Some(a(800))),
            &draft(),
            &mut sender
        )
        .is_ok());
    assert_eq!(dispatch.fault(), None);
    assert_eq!(dispatch.tick(Some(a(900))), Err(Block::Clock));
    assert_eq!(dispatch.tick(Some(a(2000))), Err(Block::Clock));
    assert!(dispatch.capture().unwrap().current().is_ok());
    assert_eq!(dispatch.raw().unwrap().receipt(), Some(a(800)));
}
