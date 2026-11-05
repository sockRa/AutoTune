mod worker_finish_tests {
    use super::*;
    use autotune_benchmark_worker::{
        sampler::runtime::IoObservation,
        workload::{
            control::{Shared, State as Control},
            producer::Cleanup,
            teardown,
            worker_finish::{ClockChain, Fault as FinishFault, State},
        },
    };
    fn setup() -> (Budget, WorkerBinding, Arc<Shared>) {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
        let shared = Arc::new(Shared::default());
        let _control = Control::new(binding, a(0), shared.clone(), &b).unwrap();
        (b, binding, shared)
    }
    fn cleanup() -> Cleanup {
        Cleanup {
            attempted: true,
            before: Some(a(200)),
            after: Some(a(300)),
            result: Some(Ok(())),
        }
    }
    fn io(code: Result<(), u32>) -> [IoObservation; 4] {
        std::array::from_fn(|i| IoObservation {
            lane: if i % 2 == 0 {
                Lane::Ordinary
            } else {
                Lane::Control
            },
            reader: i < 2,
            finished: false,
            cancellation: code,
        })
    }
    fn finish(s: &mut State, shared: &Shared) {
        s.record_cleanup(Some(cleanup()), None);
        shared.close();
        s.completion_queued(Ok(()));
        assert!(s.drain(Some(a(400)), true, true));
    }
    #[test]
    fn explicit_cleanup_queue_control_and_io_completion_are_independent_facts() {
        let (b, binding, shared) = setup();
        let mut s = State::new(binding, shared.clone(), a(100), 3_000_000_000, &b).unwrap();
        finish(&mut s, &shared);
        assert!(!s.complete());
        s.io_started(io(Err(5)), Some(a(500)));
        assert!(!s.io_poll([false; 4], Some(a(600))));
        assert!(!s.complete());
        assert!(s.io_poll([true; 4], Some(a(700))));
        assert!(s.complete());
        assert_eq!(s.io_shutdown.unwrap()[0].cancellation, Err(5));
        assert!(!s.io_shutdown.unwrap()[0].finished);
        assert_eq!(s.io_completion, Some([true; 4]));
        assert_eq!(s.binding(), binding);
    }
    #[test]
    fn original_cooperative_expiry_is_sticky_even_when_late_threads_finish() {
        let (b, binding, shared) = setup();
        let mut c = Control::new(binding, a(0), shared.clone(), &b).unwrap();
        c.event(Some(a(0)));
        let mut s = State::new(binding, shared.clone(), a(100), 3_000_000_000, &b).unwrap();
        s.record_cleanup(Some(cleanup()), None);
        s.completion_queued(Ok(()));
        assert!(!s.drain(Some(a(1_000_000_000)), true, true));
        assert_eq!(s.fault, Some(FinishFault::Deadline));
        s.io_started(io(Ok(())), Some(a(1_000_000_100)));
        assert!(!s.io_poll([true; 4], Some(a(1_000_000_200))));
        assert!(!s.complete());
        assert_eq!(s.io_completion, Some([true; 4]));
        assert_eq!(s.original_deadline_ns(), 3_000_000_000);
    }
    #[test]
    fn raw_cleanup_result_never_masks_missing_or_failed_validation() {
        for mode in 0..3 {
            let (b, binding, shared) = setup();
            let mut s = State::new(binding, shared.clone(), a(100), 3_000_000_000, &b).unwrap();
            let raw = if mode == 0 {
                None
            } else {
                let mut c = cleanup();
                if mode == 1 {
                    c.result = Some(Err(NativeFault {
                        operation: "cuMemFree_v2",
                        raw_status: 700,
                    }));
                }
                Some(c)
            };
            let validation = (mode == 2).then_some(teardown::Fault::Clock);
            s.record_cleanup(raw, validation);
            shared.close();
            s.completion_queued(Ok(()));
            assert!(s.drain(Some(a(400)), true, true));
            s.io_started(io(Ok(())), Some(a(500)));
            assert!(s.io_poll([true; 4], Some(a(600))));
            assert_eq!(s.cleanup, raw);
            assert_eq!(s.teardown_fault, validation);
            assert_eq!(s.fault, Some(FinishFault::Cleanup));
            assert!(!s.complete());
        }
    }
    #[test]
    fn duplicate_or_misordered_completion_and_io_keep_first_owners() {
        let (b, binding, shared) = setup();
        let mut s = State::new(binding, shared.clone(), a(100), 3_000_000_000, &b).unwrap();
        s.completion_queued(Err(
            autotune_benchmark_worker::workload::control::Fault::Send,
        ));
        s.completion_queued(Ok(()));
        assert!(s.completion.unwrap().is_err());
        assert_eq!(s.fault, Some(FinishFault::Protocol));
        s.io_started(io(Err(7)), Some(a(200)));
        s.io_started(io(Ok(())), Some(a(300)));
        assert_eq!(s.io_shutdown.unwrap()[0].cancellation, Err(7));
        assert!(!s.complete());
        let mut s = State::new(binding, shared, a(100), 3_000_000_000, &b).unwrap();
        let mut raw = io(Ok(()));
        raw.swap(0, 1);
        s.io_started(raw, Some(a(200)));
        assert!(s.fault.is_some());
        assert_eq!(s.io_shutdown, Some(raw));
    }
    #[test]
    fn output_drain_or_cancellation_alone_cannot_hide_unfinished_io() {
        let (b, binding, shared) = setup();
        let mut s = State::new(binding, shared.clone(), a(100), 1000, &b).unwrap();
        finish(&mut s, &shared);
        s.io_started(io(Ok(())), Some(a(500)));
        assert!(!s.io_poll([true, false, true, true], Some(a(600))));
        assert!(!s.complete());
        assert!(!s.io_poll([true; 4], Some(a(1000))));
        assert_eq!(s.fault, Some(FinishFault::Deadline));
        assert!(!s.complete());
    }
    #[test]
    fn finish_constructor_uses_original_bound_and_preserves_bracketed_first() {
        let (b, binding, shared) = setup();
        let mut first = a(1000);
        first.qpc_before = 900;
        first.qpc_after = 1100;
        let s = State::new(binding, shared.clone(), first, 2000, &b).unwrap();
        assert_eq!(s.history.raw(), &[first]);
        assert!(matches!(
            State::new(binding, shared.clone(), a(2000), 2000, &b),
            Err(FinishFault::Deadline)
        ));
        let tiny = Budget::new(1).unwrap();
        assert!(matches!(
            State::new(binding, shared.clone(), a(100), 2000, &tiny),
            Err(FinishFault::Capacity)
        ));
        assert_eq!(tiny.used(), 0);
        let foreign = Arc::new(Shared::default());
        assert!(matches!(
            State::new(binding, foreign, a(100), 2000, &b),
            Err(FinishFault::Protocol)
        ));
    }
    #[test]
    fn continuous_chain_preserves_first_bad_anchor_and_never_reseeds() {
        for mode in 0..3 {
            let (b, _, _) = setup();
            let m = manifest(&b);
            let binding =
                WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(1_000_000_000))
                    .unwrap();
            let mut chain = ClockChain::new(binding, a(1_000_001_000)).unwrap();
            assert_eq!(
                chain.observe(Some(a(1_000_002_000))),
                Some(a(1_000_002_000))
            );
            let raw = match mode {
                0 => None,
                1 => Some(a(1_000_001_500)),
                _ => {
                    let mut bad = a(1_000_003_000);
                    bad.clock_token = [9; 16];
                    Some(bad)
                }
            };
            assert_eq!(chain.observe(raw), None);
            assert_eq!(chain.bad(), raw);
            assert_eq!(chain.missing(), mode == 0);
            assert_eq!(chain.observe(Some(a(1_000_004_000))), None);
            assert_eq!(chain.bad(), raw);
            assert_eq!(chain.last(), a(1_000_002_000));
        }
    }
    #[test]
    fn actual_producer_cleanup_result_flows_to_original_bound_finalization() {
        let b = Budget::new(24 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let mut f = fixture(&b, &m, 0);
        f.producer.start_fixture(start(&m, f.binding)).unwrap();
        f.stop.store(true, Ordering::SeqCst);
        f.producer.advance(&mut Sink::default());
        let shared = Arc::new(Shared::default());
        let mut c = Control::new(f.binding, a(0), shared.clone(), &b).unwrap();
        c.event(Some(a(10_000)));
        let first = a(f.time.load(Ordering::SeqCst));
        let mut s = State::new(
            f.binding,
            shared,
            first,
            f.producer.exit_deadline_ns().unwrap(),
            &b,
        )
        .unwrap();
        f.producer.cleanup();
        let cleanup = f.producer.cleanup_observation();
        s.record_cleanup(cleanup, f.producer.teardown_ledger().fault());
        s.completion_queued(Ok(()));
        let n = f.time.load(Ordering::SeqCst);
        assert!(s.drain(Some(a(n)), true, true));
        s.io_started(io(Ok(())), Some(a(n + 1000)));
        assert!(s.io_poll([true; 4], Some(a(n + 2000))));
        assert!(s.complete());
        assert_eq!(s.cleanup, cleanup);
        assert_eq!(f.producer.end(), Some(End::Stopped));
        assert_eq!(f.stats.closes.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn later_shared_tightening_and_unknown_pending_prevent_success() {
        let (b, binding, shared) = setup();
        let mut s = State::new(binding, shared.clone(), a(100), 3_000_000_000, &b).unwrap();
        finish(&mut s, &shared);
        let mut c = Control::new(binding, a(0), shared.clone(), &b).unwrap();
        c.event(Some(a(400)));
        assert!(!s.drain(Some(a(1_000_000_400)), true, true));
        assert_eq!(s.original_deadline_ns(), 3_000_000_000);
        assert!(!s.complete());
        let (b, binding, shared) = setup();
        shared.admission().begin().unwrap();
        let mut s = State::new(binding, shared.clone(), a(100), 3_000_000_000, &b).unwrap();
        finish(&mut s, &shared);
        s.io_started(io(Ok(())), Some(a(500)));
        assert!(s.io_poll([true; 4], Some(a(600))));
        assert!(!s.complete());
        assert!(shared.admission().pending());
    }
}
