#[test]
fn setup_delivery_rejects_foreign_binding_deadline_and_already_completed_setup_before_send() {
    for mode in 0..3 {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let deadline = p.shared.setup_observations().0.unwrap();
        let shared = p.shared.clone();
        let artifact_binding = if mode == 0 {
            WorkerBinding::new(&m.bind_run(1, 0).unwrap(), 1_000_000_000, a(0)).unwrap()
        } else {
            binding
        };
        let artifact = setup_artifact(artifact_binding, deadline + u64::from(mode == 1), &b);
        let mut s = runtime(p, &b);
        if mode == 2 {
            shared.complete_setup(binding, a(1000)).unwrap();
        }
        let mut sink = Sink::default();
        assert!(s.send_setup(&artifact, &mut sink, &b).is_err());
        assert!(sink.frames.is_empty());
        assert_eq!(s.setup_delivery().unwrap().sent, 0);
        assert!(s.ready().is_none());
        assert!(shared.admission().stopped());
    }
}
fn setup_artifact(
    binding: WorkerBinding,
    deadline_raw: u64,
    b: &Budget,
) -> autotune_benchmark_worker::workload::worker_setup::Artifact {
    use autotune_benchmark_worker::workload::worker_setup::{Artifact, Facts, Preparation};
    Artifact::recorded(
        binding,
        Facts {
            first: a(300),
            deadline_raw,
            preparation: Preparation::Ready,
            cpu_fault: None,
            native_fault: None,
            entries: &[],
            pending: false,
            fault: None,
            overflow: None,
            rejected: None,
            file_checks: &[],
            file_overflow: None,
            file: None,
            load_error: None,
            environment: Default::default(),
            driver_version: Some(12090),
            module_loading_mode: Some(1),
        },
        b,
    )
    .unwrap()
}
#[test]
fn runtime_setup_delivery_ready_and_fixture_dispatch_keep_one_end_to_end_sequence() {
    use autotune_benchmark_worker::workload::worker_setup_delivery::Receiver;
    let b = Budget::new(32 * 1024 * 1024).unwrap();
    let parent_budget = Budget::new(32 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let p = parts(&b, &m);
    let binding = p.bootstrap.binding();
    let deadline = p.shared.setup_observations().0.unwrap();
    let stats = p.stats.clone();
    let time = p.time.clone();
    let artifact = setup_artifact(binding, deadline, &b);
    let mut s = runtime(p, &b);
    let mut sink = Sink::default();
    s.send_setup(&artifact, &mut sink, &b).unwrap();
    let sent = s.setup_delivery().unwrap();
    assert_eq!(sent.sent, sent.count);
    assert_eq!(sent.digest, artifact.digest());
    assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
    s.announce(facts(binding), &mut sink).unwrap();
    let mut receiver = Receiver::new(binding, a(100), deadline, parent_budget.clone()).unwrap();
    for (i, frame) in sink.frames.iter().enumerate() {
        let raw = RawReceived::new(
            Lane::Ordinary,
            Framer::new(parent_budget.clone())
                .unwrap()
                .read(&mut Bytes(frame.as_bytes())),
            Some(a(100_000 + i as u64 * 1000)),
        );
        receiver.accept(raw).unwrap();
    }
    let mut delivered = receiver.into_ready().ok().unwrap();
    assert_eq!(delivered.receipt().canonical(), artifact.canonical());
    assert!(delivered.receipt().reported().simulated);
    let first = sink.frames.len();
    assert!(s.packet(start_packet(binding, 14_000, 15_000, &b)).is_ok());
    time.store(16_000, Ordering::SeqCst);
    s.start_fixture().unwrap();
    s.advance(&mut sink);
    s.advance(&mut sink);
    assert_eq!(stats.submits.load(Ordering::SeqCst), 1);
    for (i, frame) in sink.frames[first..].iter().enumerate() {
        let packet = packet(
            frame.as_bytes(),
            Some(a(200_000 + i as u64 * 1000)),
            delivered.decoder(),
            &parent_budget,
        );
        assert!(packet.message().is_ok());
    }
}
#[test]
fn setup_queue_failure_retains_exact_frame_and_forbids_ready_or_retry() {
    let b = Budget::new(32 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let p = parts(&b, &m);
    let binding = p.bootstrap.binding();
    let deadline = p.shared.setup_observations().0.unwrap();
    let shared = p.shared.clone();
    let stats = p.stats.clone();
    let artifact = setup_artifact(binding, deadline, &b);
    let mut s = runtime(p, &b);
    let mut sink = Sink {
        reject: true,
        ..Default::default()
    };
    assert!(s.send_setup(&artifact, &mut sink, &b).is_err());
    assert_eq!(s.setup_delivery().unwrap().sent, 0);
    assert!(s.ready().is_none());
    assert!(shared.admission().stopped());
    let failed = s.producer().failed_frame().unwrap().as_bytes().to_vec();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    assert!(matches!(
        packet(&failed, Some(a(100_000)), &mut rx, &b).message(),
        Ok(Message::SetupChunk(_))
    ));
    sink.reject = false;
    assert!(s.send_setup(&artifact, &mut sink, &b).is_err());
    assert!(s.announce(facts(binding), &mut sink).is_err());
    assert_eq!(s.producer().failed_frame().unwrap().as_bytes(), failed);
    assert!(sink.frames.is_empty());
    assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
}
#[test]
fn setup_post_handoff_expiry_keeps_progress_without_announcing_ready_or_renewing_time() {
    let b = Budget::new(32 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let p = parts(&b, &m);
    let binding = p.bootstrap.binding();
    let deadline = p.shared.setup_observations().0.unwrap();
    let deadline_ns = p.bootstrap.deadline_ns();
    let time = p.time.clone();
    let shared = p.shared.clone();
    let stats = p.stats.clone();
    let artifact = setup_artifact(binding, deadline, &b);
    let mut s = runtime(p, &b);
    let mut sink = Sink {
        jump: Some((1, time, deadline_ns)),
        ..Default::default()
    };
    assert!(s.send_setup(&artifact, &mut sink, &b).is_err());
    assert_eq!(sink.frames.len(), 1);
    assert_eq!(s.setup_delivery().unwrap().sent, 1);
    assert!(s.producer().failed_frame().is_none());
    assert!(s.ready().is_none());
    assert_eq!(shared.setup_observations().0, Some(deadline));
    assert!(shared.admission().stopped());
    assert!(s.announce(facts(binding), &mut sink).is_err());
    assert_eq!(sink.frames.len(), 1);
    assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
}
#[test]
fn setup_stop_or_budget_pressure_before_queueing_keeps_source_artifact_and_has_no_successor() {
    for memory_pressure in [false, true] {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let p = parts(&b, &m);
        let binding = p.bootstrap.binding();
        let deadline = p.shared.setup_observations().0.unwrap();
        let shared = p.shared.clone();
        let stats = p.stats.clone();
        let artifact = setup_artifact(binding, deadline, &b);
        let original = artifact.canonical().to_owned();
        let mut s = runtime(p, &b);
        let mut sink = Sink::default();
        let _pressure = if memory_pressure {
            Some(b.reserve(32 * 1024 * 1024 - b.used()).unwrap())
        } else {
            shared.close();
            None
        };
        assert!(s.send_setup(&artifact, &mut sink, &b).is_err());
        assert!(s.announce(facts(binding), &mut sink).is_err());
        assert_eq!(s.setup_delivery().unwrap().sent, 0);
        assert!(sink.frames.is_empty());
        assert!(shared.admission().stopped());
        assert_eq!(artifact.canonical(), original);
        assert_eq!(stats.submits.load(Ordering::SeqCst), 0);
    }
}
