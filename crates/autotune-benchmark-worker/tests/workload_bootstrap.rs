use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, wire::Lane},
    workload::{
        bootstrap::{Bound, EntryClock, Error, Frame},
        gate::Gate,
        launch::{Arguments, Declaration, Role},
        manifest::{Artifacts, PilotDraft, PilotManifest},
        wire::Message,
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

#[test]
fn both_roles_bind_first_frame_to_exact_arguments_manifest_and_raw_anchors() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    for role in [Role::Worker, Role::Sampler] {
        let run = m.bind_run(1, 0).unwrap();
        let d = Declaration::new(&run, 1_000_000_000, a(0), role, &b).unwrap();
        let args = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
        let frame = Frame::encode(&d, &m, a(100), &b).unwrap();
        let bound = Bound::parse(&args, role, frame.as_bytes(), a(200), a(300), &b).unwrap();
        assert_eq!(bound.role(), role);
        assert_eq!(bound.run().run(), run.run());
        assert_eq!(bound.run().plan(), run.plan());
        assert_eq!(bound.manifest().canonical(), m.canonical());
        assert_eq!(bound.setup_entry(), a(100));
        assert_eq!(bound.receipt(), a(300));
        assert_eq!(bound.deadline_ns(), 10_000_000_100);
    }
}
#[test]
fn foreign_role_arguments_manifest_frame_or_canonical_shape_never_binds() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    let args = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
    let frame = Frame::encode(&d, &m, a(100), &b).unwrap();
    assert!(Bound::parse(&args, Role::Sampler, frame.as_bytes(), a(200), a(300), &b).is_err());
    let other = PilotManifest::parse(
        m.canonical()
            .replace(&"a".repeat(40), &"b".repeat(40))
            .as_bytes(),
        &b,
    )
    .unwrap();
    assert!(Frame::encode(&d, &other, a(100), &b).is_err());
    let text = std::str::from_utf8(&frame.as_bytes()[4..]).unwrap();
    for changed in [
        text.replace("pilot_bootstrap", "sampler"),
        text.replace(
            "\"process_role\":\"worker\"",
            "\"process_role\":\"sampler\"",
        ),
        text.replace("\"version\":\"1\"", "\"version\":\"2\""),
        format!("{text} "),
    ] {
        let mut bytes = ((changed.len() + 4) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(changed.as_bytes());
        assert!(Bound::parse(&args, Role::Worker, &bytes, a(200), a(300), &b).is_err());
    }
    assert!(Bound::parse(
        &args,
        Role::Worker,
        &frame.as_bytes()[..20],
        a(200),
        a(300),
        &b
    )
    .is_err());
}
#[test]
fn original_setup_bound_and_full_child_clock_chain_are_required() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    let args = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
    let frame = Frame::encode(&d, &m, a(100), &b).unwrap();
    for (first, receipt) in [
        (a(99), a(300)),
        (a(200), a(199)),
        (a(200), a(10_000_000_100)),
    ] {
        assert!(Bound::parse(&args, Role::Worker, frame.as_bytes(), first, receipt, &b).is_err());
    }
    let mut wrong = a(300);
    wrong.clock_token = [9; 16];
    assert!(Bound::parse(&args, Role::Worker, frame.as_bytes(), a(200), wrong, &b).is_err());
}
#[test]
fn worker_gate_cannot_rebase_parent_setup_at_a_late_child_receipt() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    let args = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
    let frame = Frame::encode(&d, &m, a(0), &b).unwrap();
    let bound = Bound::parse(
        &args,
        Role::Worker,
        frame.as_bytes(),
        a(1_000_000_000),
        a(9_000_000_000),
        &b,
    )
    .unwrap();
    let mut gate = Gate::from_bootstrap(&bound).unwrap();
    let ready = Message::Ready {
        anchor: a(10_000_000_000),
        identity: bound.binding().monitor().device,
        context: bound.binding().monitor().device,
        worker_digest: digest(9),
        driver_version: 12090,
        compute_major: 7,
        compute_minor: 5,
        prepared_lanes: 65536,
    };
    assert!(gate.ready(&ready).is_err());
    let sampler = Declaration::new(&run, 1_000_000_000, a(0), Role::Sampler, &b).unwrap();
    let args = Arguments::parse(&sampler.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
    let frame = Frame::encode(&sampler, &m, a(0), &b).unwrap();
    let bound = Bound::parse(&args, Role::Sampler, frame.as_bytes(), a(100), a(200), &b).unwrap();
    assert!(Gate::from_bootstrap(&bound).is_err());
}
#[test]
fn bootstrap_capacity_failure_preserves_source_bytes_and_accounting() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    let args = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
    let frame = Frame::encode(&d, &m, a(0), &b).unwrap();
    let small = Budget::new(100).unwrap();
    assert!(matches!(
        Bound::parse(
            &args,
            Role::Worker,
            frame.as_bytes(),
            a(100),
            a(200),
            &small
        ),
        Err(Error::Capacity)
    ));
    assert_eq!(small.used(), 0);
    assert!(frame.as_bytes().len() > 100);
    let base = b.used();
    {
        let _bound =
            Bound::parse(&args, Role::Worker, frame.as_bytes(), a(100), a(200), &b).unwrap();
        assert!(b.used() > base);
    }
    assert_eq!(b.used(), base);
}
#[test]
fn pre_manifest_temporal_checks_preserve_regression_and_missing_clock_without_identity() {
    let b = Budget::new(1024 * 1024).unwrap();
    let base = b.used();
    {
        let mut clock = EntryClock::new(1_000_000_000, a(0), a(100), &b).unwrap();
        assert_eq!(clock.observe(Some(a(300))), Ok(300));
        assert_eq!(clock.observe(Some(a(200))), Err(Error::Clock));
        assert_eq!(clock.raw(), &[a(100), a(300), a(200)]);
        assert_eq!(clock.observe(Some(a(400))), Err(Error::Clock));
        let mut missing = EntryClock::new(1_000_000_000, a(0), a(100), &b).unwrap();
        assert_eq!(missing.observe(None), Err(Error::Clock));
        assert!(missing.missing());
        assert_eq!(missing.raw(), &[a(100)]);
    }
    assert_eq!(b.used(), base);
}
#[test]
fn temporal_history_overflow_keeps_first_extra_anchor_without_eviction() {
    let b = Budget::new(1024 * 1024).unwrap();
    let mut clock = EntryClock::new(1_000_000_000, a(0), a(100), &b).unwrap();
    for n in 2..2000 {
        let anchor = a(n * 100);
        if clock.observe(Some(anchor)).is_err() {
            assert_eq!(clock.overflow(), Some(anchor));
            assert_eq!(clock.raw().first(), Some(&a(100)));
            return;
        }
    }
    panic!("bounded history unexpectedly grew");
}

#[test]
fn parent_sender_and_shared_framer_preserve_bootstrap_bytes_and_failed_owner() {
    use autotune_benchmark_worker::{
        sampler::framing::{Framer, ReadStep, Source},
        workload::{
            transport::{Queue, QueuedBytes, SendFailure, Sender},
            wire::Direction,
        },
    };
    use std::{cell::RefCell, rc::Rc};
    struct Q {
        budget: Budget,
        copy: Rc<RefCell<Option<QueuedBytes>>>,
        fail: bool,
    }
    impl Queue for Q {
        fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
            assert_eq!(lane, Lane::Ordinary);
            if self.fail {
                return Err(SendFailure::Queue);
            }
            *self.copy.borrow_mut() = Some(QueuedBytes::copy(bytes, &self.budget)?);
            Ok(())
        }
    }
    struct Bytes<'a>(&'a [u8]);
    impl Source for Bytes<'_> {
        fn read(&mut self, out: &mut [u8]) -> ReadStep {
            let n = out.len().min(self.0.len()).min(7);
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            ReadStep {
                reported: n,
                error: None,
            }
        }
    }
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    let args = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
    for fail in [false, true] {
        let copy = Rc::new(RefCell::new(None));
        let mut sender = Sender::new(
            Q {
                budget: b.clone(),
                copy: copy.clone(),
                fail,
            },
            d.binding(),
            Direction::ToWorker,
            &b,
        )
        .unwrap();
        let frame = Frame::encode(&d, &m, a(100), &b).unwrap();
        let pointer = frame.as_bytes().as_ptr();
        let expected = frame.as_bytes().to_vec();
        match sender.send_bootstrap(frame) {
            Ok(()) => {
                assert!(!fail);
                let bytes = copy.borrow_mut().take().unwrap();
                let packet = Framer::new(b.clone())
                    .unwrap()
                    .read(&mut Bytes(bytes.bytes()));
                assert_eq!(packet.bytes(), expected);
                assert_eq!(packet.rejection(), None);
                let bound =
                    Bound::parse(&args, Role::Worker, packet.bytes(), a(200), a(300), &b).unwrap();
                assert_eq!(bound.binding(), d.binding());
            }
            Err(frame) => {
                assert!(fail);
                assert_eq!(frame.as_bytes().as_ptr(), pointer);
                assert_eq!(frame.as_bytes(), expected);
                assert_eq!(sender.fault(Lane::Ordinary), Some(SendFailure::Queue));
                assert!(copy.borrow().is_none());
                assert!(sender.send_bootstrap(frame).is_err());
            }
        }
    }
}
