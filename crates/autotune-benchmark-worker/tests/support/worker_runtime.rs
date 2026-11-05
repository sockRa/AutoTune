mod worker_runtime_tests {
    include!("setup_runtime_delivery.rs");
    use super::*;
    use autotune_benchmark_worker::{
        sampler::framing::{Framer, ReadStep, Source},
        workload::{
            bootstrap::{Bound, Frame},
            control::{Shared, State},
            launch::{Arguments, Declaration, Role},
            transport::{RawReceived, WorkerReceived},
            worker_runtime::{Fault as RuntimeFault, ReadyFacts, Session},
        },
    };
    type ClockFn = Box<dyn FnMut() -> Option<Anchor>>;
    type StopFn = Box<dyn FnMut() -> Option<StopWindow>>;
    type Runtime = Session<Fake, ClockFn, StopFn>;
    struct Parts {
        bootstrap: Bound,
        shared: Arc<Shared>,
        driver: Fake,
        clock: ClockFn,
        stop: StopFn,
        stats: Arc<Stats>,
        time: Arc<AtomicU64>,
    }
    fn parts(b: &Budget, m: &PilotManifest) -> Parts {
        let d = Declaration::new(
            &m.bind_run(0, 0).unwrap(),
            1_000_000_000,
            a(0),
            Role::Worker,
            b,
        )
        .unwrap();
        let args = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
        let frame = Frame::encode(&d, m, a(100), b).unwrap();
        let bootstrap =
            Bound::parse(&args, Role::Worker, frame.as_bytes(), a(200), a(300), b).unwrap();
        let binding = bootstrap.binding();
        let shared = Arc::new(Shared::default());
        let _control = State::new(binding, a(300), shared.clone(), b).unwrap();
        shared
            .configure_setup(binding, bootstrap.deadline_ns())
            .unwrap();
        let stats = Arc::new(Stats::default());
        let time = Arc::new(AtomicU64::new(10_000));
        let tick = time.clone();
        let clock: ClockFn = Box::new(move || Some(a(tick.fetch_add(1000, Ordering::SeqCst))));
        let stop: StopFn = Box::new(|| None);
        let shape = bootstrap.run().plan().shape;
        let driver = Fake {
            identity: binding.monitor().device,
            shape,
            provenance: Provenance::SimulatedFixture,
            output: vec![f32::NAN; shape.lanes() as usize],
            stats: stats.clone(),
            pending: 0,
            pending_left: 0,
            stale: false,
            partial: false,
            fail: false,
        };
        Parts {
            bootstrap,
            shared,
            driver,
            clock,
            stop,
            stats,
            time,
        }
    }
    fn runtime(p: Parts, b: &Budget) -> Runtime {
        match Session::prepare(&p.bootstrap, p.shared, p.driver, p.clock, p.stop, b) {
            Ok(s) => s,
            Err(_) => panic!("fixture preparation failed"),
        }
    }
    fn facts(binding: WorkerBinding) -> ReadyFacts {
        ReadyFacts {
            context: binding.monitor().device,
            driver_version: 12090,
            compute_major: 7,
            compute_minor: 5,
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
    fn packet(
        bytes: &[u8],
        receipt: Option<Anchor>,
        rx: &mut Channel,
        b: &Budget,
    ) -> WorkerReceived {
        RawReceived::new(
            Lane::Ordinary,
            Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes)),
            receipt,
        )
        .decode_worker(rx)
    }
    fn start_packet(
        binding: WorkerBinding,
        issued: u64,
        receipt: u64,
        b: &Budget,
    ) -> WorkerReceived {
        let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let frame = tx.encode(&Message::Start { issued: a(issued) }).unwrap();
        let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        packet(frame.as_bytes(), Some(a(receipt)), &mut rx, b)
    }
    #[test]
    fn ready_start_and_dispatch_share_one_sequence_and_original_clock() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let stats = p.stats.clone();
        let time = p.time.clone();
        let shared = p.shared.clone();
        let mut s = runtime(p, &b);
        let mut sink = Sink::default();
        s.announce(facts(binding), &mut sink).unwrap();
        assert_eq!(s.ready_receipt(), Some(a(11_000)));
        assert_eq!(shared.setup_observations().1, Some(110));
        assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
        // Parent issue can race our post-Ready queue timestamp. Do not rewind
        // the local observation clock to that earlier peer-issued anchor.
        assert!(s.packet(start_packet(binding, 10_500, 12_000, &b)).is_ok());
        s.start_fixture().unwrap();
        time.store(13_000, Ordering::SeqCst);
        s.advance(&mut sink);
        s.advance(&mut sink);
        assert_eq!(stats.submits.load(Ordering::SeqCst), 1);
        assert_eq!(s.producer().reports_sent(), 1);
        let mut rx =
            Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
        let decoded = sink
            .frames
            .iter()
            .map(|f| rx.decode(f.as_bytes()).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(decoded[0].message(), Message::Ready { .. }));
        assert_eq!(decoded.len(), 2);
        assert!(matches!(decoded[1].message(), Message::DispatchRecord(_)));
        assert!(s.start_packet().is_some());
    }
    #[test]
    fn ready_send_failure_keeps_exact_frame_and_prevents_start_or_retry() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let stats = p.stats.clone();
        let shared = p.shared.clone();
        let mut s = runtime(p, &b);
        let mut sink = Sink {
            reject: true,
            ..Default::default()
        };
        assert!(s.announce(facts(binding), &mut sink).is_err());
        let original = s.producer().failed_frame().unwrap().as_bytes().to_vec();
        assert!(s.ready().is_some());
        assert!(s.ready_receipt().is_none());
        assert!(shared.admission().stopped());
        sink.reject = false;
        assert!(s.announce(facts(binding), &mut sink).is_err());
        assert_eq!(s.producer().failed_frame().unwrap().as_bytes(), original);
        assert!(sink.frames.is_empty());
        assert!(s.start_fixture().is_err());
        s.advance(&mut sink);
        assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn late_ready_queue_receipt_keeps_raw_clock_and_original_deadline_failure() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let deadline = p.bootstrap.deadline_ns();
        let time = p.time.clone();
        let shared = p.shared.clone();
        let mut s = runtime(p, &b);
        let mut sink = Sink {
            jump: Some((1, time, deadline)),
            ..Default::default()
        };
        assert_eq!(
            s.announce(facts(binding), &mut sink),
            Err(RuntimeFault::Producer(Fault::Deadline))
        );
        assert_eq!(sink.frames.len(), 1);
        assert!(s.ready_receipt().is_none());
        assert_eq!(s.history().raw().last(), Some(&a(deadline)));
        assert!(shared.admission().stopped());
        assert!(s.start_fixture().is_err());
    }
    #[test]
    fn encoded_start_rejections_keep_packets_and_never_dispatch() {
        for mode in 0..4 {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let binding = p.bootstrap.binding();
            let deadline = p.bootstrap.deadline_ns();
            let stats = p.stats.clone();
            let mut s = runtime(p, &b);
            let mut sink = Sink::default();
            if mode != 0 {
                s.announce(facts(binding), &mut sink).unwrap();
            }
            let packet = if mode == 1 {
                start_packet(binding, 9_000, 12_000, &b)
            } else if mode == 2 {
                start_packet(binding, deadline - 1000, deadline, &b)
            } else {
                start_packet(binding, 12_000, 13_000, &b)
            };
            assert!(s.packet(packet).is_ok());
            if mode == 3 {
                assert!(s.packet(start_packet(binding, 14_000, 15_000, &b)).is_ok());
            }
            assert!(s.fault().is_some());
            assert!(s.start_fixture().is_err());
            s.advance(&mut sink);
            assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
            assert!(s.rejected().is_some() || s.start_packet().is_some());
        }
    }
    #[test]
    fn inherited_setup_observation_prevents_regression_and_stop_blocks_ready() {
        for stopped in [false, true] {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let binding = p.bootstrap.binding();
            let shared = p.shared.clone();
            let mut s = runtime(p, &b);
            if stopped {
                shared.close();
            } else {
                s.inherit_setup(a(20_000)).unwrap();
            }
            assert!(s.announce(facts(binding), &mut Sink::default()).is_err());
            assert!(s.fault().is_some());
            assert_eq!(s.producer().reports_sent(), 0);
            assert!(shared.admission().stopped());
        }
    }
    #[test]
    fn failed_preparation_returns_original_driver_and_callbacks_without_cleanup() {
        for wrong_binding in [false, true] {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let mut p = parts(&b, &m);
            let stats = p.stats.clone();
            let time = p.time.clone();
            if wrong_binding {
                p.driver.identity.uuid = [9; 16];
            }
            let tiny = Budget::new(1).unwrap();
            let result = Producer::prepare_retained(
                p.bootstrap.binding(),
                p.driver,
                p.clock,
                p.stop,
                if wrong_binding { &b } else { &tiny },
            );
            let mut failure = match result {
                Err(f) => f,
                Ok(_) => panic!("expected retained failure"),
            };
            assert_eq!(
                failure.fault,
                if wrong_binding {
                    Fault::Binding
                } else {
                    Fault::Capacity
                }
            );
            assert_eq!(failure.driver.shape(), p.bootstrap.run().plan().shape);
            assert_eq!((failure.clock)(), Some(a(10_000)));
            assert_eq!(time.load(Ordering::SeqCst), 11_000);
            assert_eq!((failure.stop)(), None);
            assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
            assert_eq!(tiny.used(), 0);
        }
    }
    #[test]
    fn mismatched_shared_setup_deadline_returns_preparation_owner_before_ready() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let stats = p.stats.clone();
        let foreign = Arc::new(Shared::default());
        let _state = State::new(p.bootstrap.binding(), a(300), foreign.clone(), &b).unwrap();
        foreign
            .configure_setup(p.bootstrap.binding(), 1_000_000_000)
            .unwrap();
        let failure =
            match Session::prepare(&p.bootstrap, foreign.clone(), p.driver, p.clock, p.stop, &b) {
                Err(f) => f,
                Ok(_) => panic!("expected wrong original deadline"),
            };
        assert_eq!(failure.fault, Fault::Binding);
        assert!(foreign.admission().stopped());
        assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn shared_stop_or_late_protocol_fault_after_start_never_enters_compute() {
        for protocol in [false, true] {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let binding = p.bootstrap.binding();
            let shared = p.shared.clone();
            let stats = p.stats.clone();
            let time = p.time.clone();
            let mut s = runtime(p, &b);
            let mut sink = Sink::default();
            s.announce(facts(binding), &mut sink).unwrap();
            assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
            s.start_fixture().unwrap();
            time.store(14_000, Ordering::SeqCst);
            if protocol {
                assert!(s.packet(start_packet(binding, 14_000, 15_000, &b)).is_ok());
            } else {
                shared.close();
            }
            s.advance(&mut sink);
            assert!(s.fault().is_some());
            assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
        }
    }
    #[test]
    fn session_metadata_failure_retains_owner_and_native_fixture_start_is_rejected() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let stats = p.stats.clone();
        let tiny = Budget::new(1).unwrap();
        let mut failure =
            match Session::prepare(&p.bootstrap, p.shared, p.driver, p.clock, p.stop, &tiny) {
                Err(f) => f,
                Ok(_) => panic!("expected metadata failure"),
            };
        assert_eq!(failure.fault, Fault::Capacity);
        assert_eq!((failure.clock)(), Some(a(10_000)));
        assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
        assert_eq!(tiny.used(), 0);
        let mut p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let stats = p.stats.clone();
        p.driver.provenance = Provenance::Native;
        let mut s = runtime(p, &b);
        s.announce(facts(binding), &mut Sink::default()).unwrap();
        assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
        assert_eq!(
            s.start_fixture(),
            Err(RuntimeFault::Producer(Fault::Admission))
        );
        assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn known_shared_stop_drains_only_the_existing_dispatch_then_cleans_up() {
        for pending in [0, 2] {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let mut p = parts(&b, &m);
            p.driver.pending = pending;
            let binding = p.bootstrap.binding();
            let stats = p.stats.clone();
            let time = p.time.clone();
            let shared = p.shared.clone();
            let mut control = State::new(binding, a(300), shared.clone(), &b).unwrap();
            let mut s = runtime(p, &b);
            let mut sink = Sink::default();
            s.announce(facts(binding), &mut sink).unwrap();
            assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
            s.start_fixture().unwrap();
            time.store(14_000, Ordering::SeqCst);
            s.advance(&mut sink);
            let trigger = time.load(Ordering::SeqCst);
            assert!(control.event(Some(a(trigger))).is_some());
            for _ in 0..8 {
                s.advance(&mut sink);
            }
            assert_eq!(s.fault(), None);
            assert_eq!(s.producer().end(), Some(End::Stopped));
            assert_eq!(stats.submits.load(Ordering::SeqCst), 1);
            assert_eq!(stats.queries.load(Ordering::SeqCst), pending + 1);
            let record = s.producer().last_record().unwrap().record();
            assert!(!record.pending);
            assert_eq!(record.checked_lanes, 65536);
            assert_eq!(
                s.producer().exit_deadline_ns(),
                Some(trigger + 3_000_000_000)
            );
            assert_eq!(
                s.producer().stop_history().entries.iter().flatten().count(),
                1
            );
            s.cleanup();
            s.cleanup();
            assert_eq!(stats.closes.load(Ordering::SeqCst), 1);
            assert_eq!(s.producer().teardown_ledger().fault(), None);
        }
    }
    #[test]
    fn delayed_original_shared_stop_expires_drain_without_query_or_new_grace() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let stats = p.stats.clone();
        let time = p.time.clone();
        let shared = p.shared.clone();
        let mut control = State::new(binding, a(300), shared.clone(), &b).unwrap();
        let mut s = runtime(p, &b);
        let mut sink = Sink::default();
        s.announce(facts(binding), &mut sink).unwrap();
        assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
        s.start_fixture().unwrap();
        time.store(999_900_000, Ordering::SeqCst);
        s.advance(&mut sink);
        assert!(control
            .local_stop(Some(a(0)), Some(a(time.load(Ordering::SeqCst))))
            .is_some());
        time.store(1_000_000_000, Ordering::SeqCst);
        s.advance(&mut sink);
        assert_eq!(s.producer().end(), Some(End::Stopped));
        assert_eq!(stats.submits.load(Ordering::SeqCst), 1);
        assert_eq!(stats.queries.load(Ordering::SeqCst), 0);
        assert!(s.producer().last_record().unwrap().record().pending);
        assert_eq!(s.producer().exit_deadline_ns(), Some(3_000_000_000));
        s.cleanup();
        assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
        assert!(!s.producer().cleanup_observation().unwrap().attempted);
    }
    #[test]
    fn earlier_shared_trigger_tightens_drain_and_preserves_each_observed_window() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let mut p = parts(&b, &m);
        p.driver.pending = 2;
        let binding = p.bootstrap.binding();
        let stats = p.stats.clone();
        let time = p.time.clone();
        let shared = p.shared.clone();
        let mut control = State::new(binding, a(300), shared, &b).unwrap();
        let mut s = runtime(p, &b);
        let mut sink = Sink::default();
        s.announce(facts(binding), &mut sink).unwrap();
        assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
        s.start_fixture().unwrap();
        time.store(14_000, Ordering::SeqCst);
        s.advance(&mut sink);
        let first = time.load(Ordering::SeqCst);
        control.event(Some(a(first)));
        s.advance(&mut sink);
        control.local_stop(Some(a(0)), Some(a(time.load(Ordering::SeqCst))));
        for _ in 0..4 {
            s.advance(&mut sink);
        }
        let history = s.producer().stop_history();
        assert_eq!(history.entries.iter().flatten().count(), 2);
        assert_eq!(history.entries[0].unwrap().window.trigger_ns, first);
        assert_eq!(history.entries[1].unwrap().window.trigger_ns, 0);
        assert!(
            history.entries[1].unwrap().anchor.qpc_before
                > history.entries[0].unwrap().anchor.qpc_before
        );
        assert_eq!(s.producer().exit_deadline_ns(), Some(3_000_000_000));
        assert_eq!(s.producer().end(), Some(End::Stopped));
        assert_eq!(stats.submits.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn bounded_stop_history_exhaustion_retains_overflow_and_blocks_successors() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let mut p = parts(&b, &m);
        p.driver.pending = 20;
        let binding = p.bootstrap.binding();
        let stats = p.stats.clone();
        let time = p.time.clone();
        let shared = p.shared.clone();
        let mut control = State::new(binding, a(300), shared, &b).unwrap();
        let mut s = runtime(p, &b);
        let mut sink = Sink::default();
        s.announce(facts(binding), &mut sink).unwrap();
        assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
        s.start_fixture().unwrap();
        time.store(10_000_000, Ordering::SeqCst);
        s.advance(&mut sink);
        for i in 0..9 {
            control.local_stop(
                Some(a(1_000_000 - i * 1000)),
                Some(a(time.load(Ordering::SeqCst))),
            );
            s.advance(&mut sink);
        }
        assert_eq!(s.producer().end(), Some(End::Fault(Fault::Capacity)));
        let history = s.producer().stop_history();
        assert_eq!(history.entries.iter().flatten().count(), 8);
        assert_eq!(history.overflow.unwrap().window.trigger_ns, 992_000);
        assert_eq!(stats.submits.load(Ordering::SeqCst), 1);
        assert_eq!(stats.queries.load(Ordering::SeqCst), 8);
        s.advance(&mut sink);
        assert_eq!(stats.queries.load(Ordering::SeqCst), 8);
    }
    #[test]
    fn owned_raw_receive_uses_one_decoder_and_preserves_bracketed_start_receipt() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let time = p.time.clone();
        let stats = p.stats.clone();
        let mut s = runtime(p, &b);
        let mut sink = Sink::default();
        s.announce(facts(binding), &mut sink).unwrap();
        let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let frame = tx.encode(&Message::Start { issued: a(12_000) }).unwrap();
        let mut receipt = a(13_000);
        receipt.qpc_before = 12_900;
        receipt.qpc_after = 13_100;
        let raw = RawReceived::new(
            Lane::Ordinary,
            Framer::new(b.clone())
                .unwrap()
                .read(&mut Bytes(frame.as_bytes())),
            Some(receipt),
        );
        assert!(s.raw_packet(raw).is_ok());
        assert_eq!(s.history().raw().last(), Some(&receipt));
        s.start_fixture().unwrap();
        time.store(14_000, Ordering::SeqCst);
        s.advance(&mut sink);
        s.advance(&mut sink);
        assert_eq!(stats.submits.load(Ordering::SeqCst), 1);
        assert_eq!(s.producer().reports_sent(), 1);
        assert_eq!(s.start_packet().unwrap().raw().receipt(), Some(receipt));
        #[cfg(windows)]
        {
            let _poll: fn(
                &mut Runtime,
                &autotune_benchmark_worker::workload::windows_transport::InheritedTransport,
            ) -> Result<(), WorkerReceived> = Runtime::poll_native;
        }
    }
    #[test]
    fn idle_receive_keeps_original_setup_deadline_and_missing_or_regressing_clocks() {
        for mode in 0..3 {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let deadline = p.bootstrap.deadline_ns();
            let shared = p.shared.clone();
            let mut s = runtime(p, &b);
            s.idle(Some(a(1000))).unwrap();
            let anchor = match mode {
                0 => Some(a(deadline)),
                1 => None,
                _ => Some(a(500)),
            };
            assert_eq!(
                s.idle(anchor),
                Err(RuntimeFault::Producer(if mode == 0 {
                    Fault::Deadline
                } else {
                    Fault::Clock
                }))
            );
            if let Some(anchor) = anchor {
                assert_eq!(s.history().raw().last(), Some(&anchor));
            } else {
                assert!(s.history().missing());
            }
            assert!(shared.admission().stopped());
            assert!(s.start_fixture().is_err());
        }
    }
    #[test]
    fn receive_os_failure_and_eof_remain_separate_from_clock_or_child_completion() {
        for eof in [false, true] {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let binding = p.bootstrap.binding();
            let stats = p.stats.clone();
            let mut s = runtime(p, &b);
            s.announce(facts(binding), &mut Sink::default()).unwrap();
            if eof {
                s.receive_finished();
                assert!(s.input_finished());
                assert_eq!(s.transport_fault(), None);
                assert_eq!(s.fault(), Some(RuntimeFault::Protocol));
            } else {
                s.receive_failed(Some(-32), None);
                assert_eq!(s.transport_fault(), Some((Some(-32), None)));
                assert_eq!(s.fault(), Some(RuntimeFault::Producer(Fault::Clock)));
                assert!(s.history().missing());
            }
            assert!(s.start_fixture().is_err());
            s.advance(&mut Sink::default());
            assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
            assert!(s.producer().end().is_none());
        }
    }
    #[test]
    fn full_original_run_receive_history_fits_without_growth_or_new_epoch() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let end = p.bootstrap.run().plan().envelope_ns;
        let mut s = runtime(p, &b);
        s.announce(facts(binding), &mut Sink::default()).unwrap();
        assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
        s.start_fixture().unwrap();
        let tick = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .controller_tick_ns;
        let baseline = b.used();
        for n in 1..end / tick {
            s.idle(Some(a(n * tick))).unwrap();
            s.idle(Some(a(n * tick))).unwrap();
        }
        assert_eq!(b.used(), baseline);
        assert_eq!(s.fault(), None);
        assert!(s.history().overflow().is_none());
        assert_eq!(
            s.idle(Some(a(end))),
            Err(RuntimeFault::Producer(Fault::Deadline))
        );
        assert_eq!(s.history().raw().last(), Some(&a(end)));
        assert_eq!(b.used(), baseline);
    }
    #[test]
    fn before_start_stop_disposes_ready_resources_without_start_or_compute() {
        for ready_sent in [false, true] {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let binding = p.bootstrap.binding();
            let shared = p.shared.clone();
            let stats = p.stats.clone();
            let mut control = State::new(binding, a(300), shared.clone(), &b).unwrap();
            let mut s = runtime(p, &b);
            if ready_sent {
                s.announce(facts(binding), &mut Sink::default()).unwrap();
            }
            control.event(Some(a(1000)));
            s.cleanup();
            let report = s.prestart_disposition();
            assert!(report.requested());
            assert_eq!(report.fault, None);
            assert!(report.cleanup.attempted);
            assert_eq!(report.cleanup.result, Some(Ok(())));
            assert_eq!(report.ledger.entries().count(), 8);
            assert_eq!(s.producer().end(), Some(End::Stopped));
            assert_eq!(s.producer().exit_deadline_ns(), Some(3_000_001_000));
            assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
            assert_eq!(stats.queries.load(Ordering::SeqCst), 0);
            assert_eq!(stats.cleanup_calls.load(Ordering::SeqCst), 8);
            s.cleanup();
            assert_eq!(stats.cleanup_calls.load(Ordering::SeqCst), 8);
            assert!(s.start_fixture().is_err());
            s.advance(&mut Sink::default());
            assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
            assert!(!shared.admission().pending());
            assert!(shared.admission().stopped());
        }
    }
    #[test]
    fn denied_start_can_release_resources_after_raw_start_without_fabricating_admission() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let shared = p.shared.clone();
        let stats = p.stats.clone();
        let time = p.time.clone();
        let mut control = State::new(binding, a(300), shared, &b).unwrap();
        let mut s = runtime(p, &b);
        s.announce(facts(binding), &mut Sink::default()).unwrap();
        assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
        time.store(14_000, Ordering::SeqCst);
        control.event(Some(a(13_000)));
        s.cleanup();
        assert!(s.start_packet().is_some());
        assert_eq!(s.prestart_disposition().fault, None);
        assert_eq!(s.cleanup_observation().unwrap().result, Some(Ok(())));
        assert_eq!(s.teardown_fault(), None);
        assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
        assert_eq!(s.producer().reports_sent(), 0);
        assert_eq!(stats.closes.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn rejected_start_keeps_unknown_control_closed_and_preserves_first_fault() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let mut p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let shared = p.shared.clone();
        let stats = p.stats.clone();
        let time = p.time.clone();
        p.driver.provenance = Provenance::Native;
        let mut control = State::new(binding, a(300), shared, &b).unwrap();
        let mut s = runtime(p, &b);
        s.announce(facts(binding), &mut Sink::default()).unwrap();
        assert!(s.packet(start_packet(binding, 12_000, 13_000, &b)).is_ok());
        assert_eq!(
            s.start_fixture(),
            Err(RuntimeFault::Producer(Fault::Admission))
        );
        time.store(14_000, Ordering::SeqCst);
        control.event(Some(a(13_000)));
        s.cleanup();
        assert_eq!(s.prestart_disposition().fault, Some(Fault::Admission));
        assert!(!s.cleanup_observation().unwrap().attempted);
        assert_eq!(s.producer().end(), Some(End::Fault(Fault::Admission)));
        assert_eq!(s.fault(), Some(RuntimeFault::Producer(Fault::Admission)));
        assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
        assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
        s.cleanup();
        assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn unknown_expired_or_host_pending_before_start_never_enters_cleanup() {
        for mode in 0..4 {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let binding = p.bootstrap.binding();
            let shared = p.shared.clone();
            let stats = p.stats.clone();
            let time = p.time.clone();
            let mut control = State::new(binding, a(300), shared.clone(), &b).unwrap();
            let mut s = runtime(p, &b);
            if mode == 0 {
                shared.close();
            } else {
                if mode == 1 {
                    shared.admission().begin().unwrap();
                }
                control.event(Some(a(1000)));
                if mode == 2 {
                    time.store(1_000_001_000, Ordering::SeqCst);
                }
                if mode == 3 {
                    control.fail(autotune_benchmark_worker::workload::control::Fault::Clock);
                }
            }
            s.cleanup();
            assert!(s.prestart_disposition().fault.is_some());
            assert!(!s.cleanup_observation().unwrap().attempted);
            assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
            assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
            if mode == 1 {
                assert!(shared.admission().pending());
            }
            time.store(20_000, Ordering::SeqCst);
            s.cleanup();
            assert_eq!(stats.closes.load(Ordering::SeqCst), 0);
        }
    }
    #[test]
    fn prestart_teardown_is_prepaid_and_preserves_failure_without_retry() {
        for failed in 0..=8 {
            let b = Budget::new(24 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let p = parts(&b, &m);
            let binding = p.bootstrap.binding();
            let shared = p.shared.clone();
            let stats = p.stats.clone();
            let mut control = State::new(binding, a(300), shared, &b).unwrap();
            let mut s = runtime(p, &b);
            control.event(Some(a(1000)));
            stats.cleanup_fail_at.store(failed, Ordering::SeqCst);
            let _pressure = b.reserve(24 * 1024 * 1024 - b.used()).unwrap();
            let baseline = b.used();
            s.cleanup();
            assert_eq!(b.used(), baseline);
            assert_eq!(
                stats.cleanup_calls.load(Ordering::SeqCst),
                if failed == 0 { 8 } else { failed }
            );
            assert_eq!(s.prestart_disposition().fault.is_some(), failed != 0);
            if failed != 0 {
                assert!(s
                    .prestart_disposition()
                    .ledger
                    .entries()
                    .last()
                    .unwrap()
                    .raw
                    .is_some());
            }
            s.cleanup();
            assert_eq!(
                stats.cleanup_calls.load(Ordering::SeqCst),
                if failed == 0 { 8 } else { failed }
            );
            assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
        }
    }
    #[test]
    fn earliest_original_window_not_a_late_disposition_attempt_sets_exit() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let shared = p.shared.clone();
        let time = p.time.clone();
        let mut control = State::new(binding, a(300), shared, &b).unwrap();
        let mut s = runtime(p, &b);
        control.event(Some(a(1000)));
        control.local_stop(Some(a(10_000)), Some(a(20_000)));
        time.store(900_000_000, Ordering::SeqCst);
        s.cleanup();
        assert_eq!(s.prestart_disposition().fault, None);
        assert_eq!(s.prestart_disposition().window.unwrap().trigger_ns, 1000);
        assert_eq!(s.producer().exit_deadline_ns(), Some(3_000_001_000));
    }
}
