use autotune_benchmark_worker::{
    evidence::{Digest, Domain},
    monitor::{Anchor, Metric, Ticket},
    sampler::{
        budget::{Budget, Reservation},
        framing::{Framer, ReadStep, Source},
        wire::Lane,
    },
    workload::{
        manifest::{Artifacts, PilotDraft, PilotManifest},
        parent_sampler::{Fault, ParentSampler},
        settings::{Capture, Draft, Observation},
        settings_transport::Child,
        transport::{Queue, QueuedBytes, RawReceived, SendFailure, Sender},
        wire::{Channel, Direction, Message, WorkerBinding},
    },
};
use autotune_probe::{
    nvml::SettingMetric,
    report::{Field, Runtime},
};
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
fn received(bytes: &[u8], at: Option<Anchor>, lane: Lane, b: &Budget) -> RawReceived {
    RawReceived::new(
        lane,
        Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes)),
        at,
    )
}
struct Sink {
    root: Budget,
    control: Budget,
    frames: Rc<RefCell<Vec<(Lane, QueuedBytes)>>>,
    fail: bool,
}
impl Queue for Sink {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        if lane == Lane::Ordinary && self.fail {
            return Err(SendFailure::Os(232));
        }
        self.frames.borrow_mut().push((
            lane,
            QueuedBytes::copy(
                bytes,
                if lane == Lane::Control {
                    &self.control
                } else {
                    &self.root
                },
            )?,
        ));
        Ok(())
    }
}
struct Harness {
    b: Budget,
    m: PilotManifest,
    index: usize,
    origin: u64,
    binding: WorkerBinding,
    parent: ParentSampler,
    child: Channel,
    query: Channel,
    sender: Sender<Sink>,
    frames: Rc<RefCell<Vec<(Lane, QueuedBytes)>>>,
    _reserved: Reservation,
}
impl Harness {
    fn new(index: usize, origin: u64, delay: u64, fail: bool) -> Self {
        Self::with_starting(index, origin, delay, fail, None)
    }
    fn with_starting(
        index: usize,
        origin: u64,
        delay: u64,
        fail: bool,
        starting: Option<Digest>,
    ) -> Self {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let reserved = b.reserve(12 * 1024 * 1024).unwrap();
        let control = b.partition(2 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let m = if let Some(starting) = starting {
            PilotManifest::parse(
                m.canonical()
                    .replace(&digest(13).to_hex(), &starting.to_hex())
                    .as_bytes(),
                &b,
            )
            .unwrap()
        } else {
            m
        };
        let run = m.bind_run(index, origin).unwrap();
        let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
        let parent = ParentSampler::new(
            &run,
            1_000_000_000,
            a(0),
            a(origin),
            a(origin + delay),
            b.clone(),
            control.clone(),
        )
        .unwrap();
        let frames = Rc::new(RefCell::new(Vec::new()));
        let sender = Sender::new(
            Sink {
                root: b.clone(),
                control,
                frames: frames.clone(),
                fail,
            },
            binding,
            Direction::ToWorker,
            &b,
        )
        .unwrap();
        Self {
            child: Channel::sampler(binding, Lane::Ordinary, Direction::FromWorker, b.clone())
                .unwrap(),
            query: Channel::sampler(binding, Lane::Ordinary, Direction::ToWorker, b.clone())
                .unwrap(),
            b,
            m,
            index,
            origin,
            binding,
            parent,
            sender,
            frames,
            _reserved: reserved,
        }
    }
    fn message(&mut self, message: Message, ns: u64) {
        let frame = self.child.encode(&message).unwrap();
        let run = self.m.bind_run(self.index, self.origin).unwrap();
        assert!(self
            .parent
            .receive(
                &run,
                received(frame.as_bytes(), Some(a(ns)), Lane::Ordinary, &self.b)
            )
            .is_ok());
    }
    fn ready(&mut self) {
        self.message(
            Message::SamplerReady {
                before: a(self.origin + 200),
                after: a(self.origin + 300),
                raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
                raw_pci: "0000:01:02.0".into(),
                sampler_digest: digest(10),
                runtime: serde_json::to_string(&Runtime {
                    library_path: Some("C:\\Windows\\System32\\nvml.dll".into()),
                    driver_version: Field::available("text", "fixture-driver"),
                    nvml_version: Field::available("text", "fixture-nvml"),
                    cuda_driver_version: Field::available("CUDA encoded integer", 12090),
                    header_api_version: 13,
                })
                .unwrap(),
            },
            self.origin + 400,
        );
    }
    /// Explicit adversarial fixture constructing a *reported* native claim.
    /// It can exercise receipt policy, never authenticate a native execution.
    fn settings(&mut self, native_claim: bool) {
        self.parent
            .issue_settings(a(self.origin + 1000), &mut self.sender)
            .unwrap();
        let bytes = self.frames.borrow().last().unwrap().1.bytes().to_vec();
        let run = self.m.bind_run(self.index, self.origin).unwrap();
        let mut child = Child::new(&run, 1_000_000_000, a(0), self.b.clone()).unwrap();
        let mut request = child.accept(&bytes).unwrap();
        let draft = Draft {
            raw_uuid: run.raw_uuid().into(),
            raw_pci: run.raw_pci().into(),
            observations: SettingMetric::ALL.map(|kind| Observation {
                kind,
                before: Some(a(self.origin + 1500 + kind as u64 * 200)),
                after: Some(a(self.origin + 1600 + kind as u64 * 200)),
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
        };
        let capture = Capture::fixture(&run, 1_000_000_000, a(0), &draft, &self.b).unwrap();
        let frame = request.respond(&capture).unwrap();
        let mut text = std::str::from_utf8(&frame.as_bytes()[4..])
            .unwrap()
            .to_owned();
        if native_claim {
            let canonical = capture
                .canonical()
                .replace("simulated_fixture", "native_getter_path");
            let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
            text = text
                .replace(&capture.digest().to_hex(), &digest.to_hex())
                .replace("simulated_fixture", "native_getter_path");
        }
        let mut bytes = ((text.len() + 4) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(text.as_bytes());
        assert!(self
            .parent
            .receive(
                &run,
                received(&bytes, Some(a(self.origin + 3000)), Lane::Ordinary, &self.b)
            )
            .is_ok());
    }
    fn sample(&mut self, ns: u64, temperature: i64, gpu: i64) -> Ticket {
        assert_eq!(self.parent.next_query(a(ns), &mut self.sender), Ok(true));
        let bytes = self.frames.borrow().last().unwrap().1.bytes().to_vec();
        let decoded = self.query.decode(&bytes).unwrap();
        let ticket = match decoded.message() {
            Message::SamplerQuery { ticket, .. } => *ticket,
            _ => panic!("query expected"),
        };
        let (unit, value) = match ticket.metric {
            Metric::Temperature => ("degC", temperature),
            Metric::Power => ("mW", 100000),
            Metric::GraphicsClock | Metric::MemoryClock => ("MHz", 1500),
            Metric::GpuUtilization => ("%", gpu),
            Metric::MemoryUtilization => ("%", 10),
        };
        self.message(
            Message::SamplerRecord {
                ticket,
                before: Some(a(ns + 100)),
                after: Some(a(ns + 200)),
                raw: Some(Field::available(unit, value)),
                native: true,
            },
            ns + 300,
        );
        ticket
    }
}
#[test]
fn parent_composes_ready_settings_and_four_reentry_pairs_without_issuing_native_permission() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    h.settings(true);
    assert_eq!(h.parent.fault(), None);
    for slot in 0..4 {
        for i in 0..6 {
            h.sample(slot * 250_000_000 + i * 10_000_000 + 5000, 42, 10);
        }
    }
    assert_eq!(h.parent.reentry().consecutive(), 4);
    assert!(h.parent.reported_screen(Some(a(850_000_000))));
    assert!(!h.parent.native_admission_available());
    assert!(!h.parent.report().native_admission_available());
    assert_eq!(h.parent.records().len(), 24);
    h.sample(1_000_005_000, 60, 10);
    assert_eq!(h.parent.reentry().consecutive(), 0);
    assert!(!h.parent.reported_screen(Some(a(1_001_000_000))));
}
#[test]
fn later_run_preserves_epoch_absolute_slots_and_session_ceiling_without_false_prior_gaps() {
    let mut h = Harness::new(1, 45_000_000_000, 100, false);
    assert_eq!(h.parent.fault(), None);
    h.ready();
    h.settings(true);
    let ticket = h.sample(45_000_005_000, 42, 10);
    assert_eq!(ticket.slot, 180);
    assert_eq!(ticket.scheduled_ns, 45_000_000_000);
    assert_eq!(h.parent.report().now_ns(), 45_000_005_300);
    assert_eq!(h.parent.report().run_origin_ns(), 45_000_000_000);
    assert_eq!(h.parent.report().session_deadline_ns(), 180_000_000_000);
    assert!(h
        .parent
        .report()
        .events()
        .all(|e| e.missed.iter().flatten().all(|m| m.first >= 180)));
}
#[test]
fn late_controller_creation_keeps_original_initial_safety_grace() {
    let h = Harness::new(1, 45_000_000_000, 750_000_000, false);
    assert_eq!(h.parent.fault(), Some(Fault::Monitor));
    assert!(h.parent.report().stop().is_some());
    assert_eq!(h.parent.report().run_origin_ns(), 45_000_000_000);
}
#[test]
fn simulated_current_setting_claims_remain_raw_and_cannot_pass_the_reported_screen() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    h.settings(false);
    assert_eq!(h.parent.fault(), Some(Fault::Settings));
    assert!(h.parent.current().as_ref().unwrap().result().is_ok());
    assert!(!h.parent.reported_screen(Some(a(5000))));
    assert!(!h.parent.native_admission_available());
}
#[test]
fn parent_stops_a_missing_query_response_at_original_combined_timing_bound() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    assert_eq!(h.parent.next_query(a(5000), &mut h.sender), Ok(true));
    h.parent.tick(Some(a(150_005_000)));
    assert_eq!(h.parent.fault(), None);
    h.parent.tick(Some(a(150_005_100)));
    assert_eq!(h.parent.fault(), Some(Fault::Telemetry));
    assert_eq!(h.parent.stop().unwrap().trigger_ns, 150_005_000);
    assert!(h.parent.pending().is_some());
    assert_eq!(
        h.parent.next_query(a(150_005_200), &mut h.sender),
        Err(Fault::Telemetry)
    );
}
#[test]
fn uncertain_query_send_retains_frame_and_original_pending_ticket_without_retry() {
    let mut h = Harness::new(0, 0, 100, true);
    h.ready();
    assert_eq!(
        h.parent.next_query(a(5000), &mut h.sender),
        Err(Fault::Transport)
    );
    assert_eq!(h.parent.send_error(), Some(SendFailure::Os(232)));
    assert!(h.parent.unsent().is_some());
    assert!(h.parent.pending().is_some());
    let bytes = h.parent.unsent().as_ref().unwrap().as_bytes().to_vec();
    assert!(h.parent.next_query(a(6000), &mut h.sender).is_err());
    assert_eq!(h.parent.unsent().as_ref().unwrap().as_bytes(), bytes);
}
#[test]
fn stop_encoding_and_handoff_survive_exhausted_ordinary_memory() {
    let mut h = Harness::new(0, 0, 100, false);
    h.parent.tick(None);
    let _pressure = h.b.reserve(32 * 1024 * 1024 - h.b.used()).unwrap();
    h.parent.queue_stop(&mut h.sender);
    let frames = h.frames.borrow();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].0, Lane::Control);
    assert!(h.parent.unsent_stop().is_none());
}
#[test]
fn root_pool_cannot_be_substituted_for_reserved_control_capacity() {
    let b = Budget::new(32 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    assert!(matches!(
        ParentSampler::new(&run, 1_000_000_000, a(0), a(0), a(0), b.clone(), b),
        Err(Fault::Binding)
    ));
}
#[test]
fn normalized_identity_cannot_hide_a_different_raw_pci_function() {
    let mut h = Harness::new(0, 0, 100, false);
    h.message(
        Message::SamplerReady {
            before: a(200),
            after: a(300),
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.1".into(),
            sampler_digest: digest(10),
            runtime: "{}".into(),
        },
        400,
    );
    assert_eq!(h.parent.fault(), Some(Fault::Binding));
    assert_eq!(h.parent.ready().as_ref().unwrap().binding(), Ok(h.binding));
}
#[test]
fn planned_shutdown_keeps_ack_cleanup_and_completion_separate_from_process_exit() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    h.settings(true);
    h.parent.finish(Some(a(5000)), &mut h.sender).unwrap();
    let original = h.parent.stop().unwrap();
    let mut tx =
        Channel::sampler(h.binding, Lane::Control, Direction::FromWorker, h.b.clone()).unwrap();
    for (message, ns) in [
        (
            Message::StopAck {
                anchor: a(6000),
                pending: false,
            },
            7000,
        ),
        (
            Message::RoleFinished {
                anchor: a(8000),
                pending: false,
                cleanup_attempted: true,
                cleanup_status: Some(0),
            },
            9000,
        ),
    ] {
        let frame = tx.encode(&message).unwrap();
        let run = h.m.bind_run(0, 0).unwrap();
        assert!(h
            .parent
            .receive(
                &run,
                received(frame.as_bytes(), Some(a(ns)), Lane::Control, &h.b)
            )
            .is_ok());
    }
    assert!(h.parent.reported_finished());
    assert_eq!(h.parent.fault(), None);
    assert!(!h.parent.reported_screen(Some(a(10000))));
    assert_eq!(h.parent.next_query(a(11000), &mut h.sender), Ok(false));
    h.parent.finish(Some(a(12000)), &mut h.sender).unwrap();
    assert_eq!(h.parent.stop(), Some(original));
    assert!(!h.parent.native_admission_available());
    assert_eq!(h.parent.controls().len(), 2);
}
#[test]
fn late_cleanup_report_cannot_clear_an_original_shutdown_timeout() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    h.settings(true);
    h.parent.finish(Some(a(5000)), &mut h.sender).unwrap();
    let deadline = h.parent.stop().unwrap().cooperative_ns;
    let mut tx =
        Channel::sampler(h.binding, Lane::Control, Direction::FromWorker, h.b.clone()).unwrap();
    let frame = tx
        .encode(&Message::RoleFinished {
            anchor: a(deadline - 100),
            pending: false,
            cleanup_attempted: true,
            cleanup_status: Some(0),
        })
        .unwrap();
    let run = h.m.bind_run(0, 0).unwrap();
    assert!(h
        .parent
        .receive(
            &run,
            received(frame.as_bytes(), Some(a(deadline)), Lane::Control, &h.b)
        )
        .is_ok());
    assert_eq!(h.parent.fault(), Some(Fault::ShutdownDeadline));
    assert!(!h.parent.reported_finished());
    assert_eq!(h.parent.controls().len(), 1);
}
#[test]
fn late_metric_reply_is_retained_without_replacing_pending_or_reopening() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    h.parent.next_query(a(5000), &mut h.sender).unwrap();
    let ticket = h.parent.pending().unwrap().0;
    h.parent.tick(Some(a(150_005_100)));
    let original = h.parent.stop();
    h.message(
        Message::SamplerRecord {
            ticket,
            before: Some(a(5100)),
            after: Some(a(5200)),
            raw: Some(Field::available("degC", 42)),
            native: true,
        },
        150_005_200,
    );
    assert_eq!(h.parent.fault(), Some(Fault::Telemetry));
    assert_eq!(h.parent.records().len(), 1);
    assert!(h.parent.records()[0].record.is_err());
    assert!(h.parent.pending().is_some());
    assert_eq!(h.parent.stop(), original);
}
#[test]
fn complete_parent_run_with_reserved_writer_failure_pools_fits_the_original_budget() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    h.settings(true);
    for tick in 0..4500 {
        let base = tick * 10_000_000;
        h.parent.tick(Some(a(base + 4000)));
        if tick % 25 < 6 {
            h.sample(base + 5000, 42, 10);
        }
        assert_eq!(h.parent.fault(), None);
    }
    assert_eq!(h.parent.records().len(), 1080);
    assert!(h.b.used() < 32 * 1024 * 1024);
    assert!(h.parent.report().capacity().allocated_bytes < 5 * 1024 * 1024);
    h.parent.tick(Some(a(45_000_000_000)));
    assert_eq!(h.parent.fault(), Some(Fault::RunDeadline));
    assert_eq!(h.parent.report().session_deadline_ns(), 180_000_000_000);
}
#[test]
fn a_missed_reentry_slot_clears_the_old_four_pairs_before_a_new_pair_completes() {
    let mut h = Harness::new(0, 0, 100, false);
    h.ready();
    h.settings(true);
    for slot in 0..4 {
        for i in 0..6 {
            h.sample(slot * 250_000_000 + i * 10_000_000 + 5000, 42, 10);
        }
    }
    assert!(h.parent.reported_screen(Some(a(850_000_000))));
    h.parent.tick(Some(a(1_250_000_000)));
    assert_eq!(h.parent.fault(), None);
    assert_eq!(h.parent.reentry().consecutive(), 0);
    assert!(!h.parent.reported_screen(Some(a(1_250_000_100))));
}
#[test]
fn parent_retains_exact_starting_reference_and_compares_later_reported_settings_without_permission()
{
    use autotune_benchmark_worker::workload::starting::{
        ReportedSnapshot, Scope, ScopeDraft, Snapshot,
    };
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let scope = Scope::freeze(
        ScopeDraft {
            session: [1; 16],
            capture_nonce: [77; 16],
            clock_token: [5; 16],
            frequency: 1_000_000_000,
            epoch: a(0),
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
            read_approval_ref: "fabricated read scope; no native execution".into(),
        },
        &b,
    )
    .unwrap();
    let draft = Draft {
        raw_uuid: scope.draft().raw_uuid.clone(),
        raw_pci: scope.draft().raw_pci.clone(),
        observations: SettingMetric::ALL.map(|kind| Observation {
            kind,
            before: Some(a(2000 + kind as u64 * 2000)),
            after: Some(a(3000 + kind as u64 * 2000)),
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
    };
    let snapshot = Snapshot::fixture(&scope, a(1000), &draft, &b).unwrap();
    let bytes = snapshot
        .canonical()
        .replace("simulated_fixture", "native_getter_path");
    let reported = ReportedSnapshot::parse(
        &scope,
        bytes.as_bytes(),
        Digest::of(Domain::RawArtifact, bytes.as_bytes()),
        &b,
    )
    .unwrap();
    let mut h = Harness::with_starting(
        0,
        1_000_000_000,
        100,
        false,
        Some(reported.snapshot().digest()),
    );
    h.ready();
    h.settings(true);
    let run = h.m.bind_run(0, h.origin).unwrap();
    assert_eq!(
        h.parent
            .check_starting(&reported, &run, a(0))
            .unwrap()
            .power_limit_mw,
        215000
    );
    let first = h.parent.starting_check();
    assert!(first.unwrap().1.is_ok());
    assert!(!h.parent.native_admission_available());
    assert_eq!(
        h.parent.check_starting(&reported, &run, a(0)),
        Err(Fault::Protocol)
    );
    assert_eq!(h.parent.starting_check(), first);
}
