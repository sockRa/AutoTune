use autotune_benchmark_worker::{
    evidence::{Digest, Reason},
    monitor::Anchor,
    sampler::{
        budget::Budget,
        wire::{Lane, WireError},
    },
    workload::{
        gate::{Action, Gate},
        manifest::{Artifacts, PilotDraft, PilotManifest},
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
fn ready(binding: WorkerBinding) -> Message {
    Message::Ready {
        anchor: a(1_000_000),
        identity: binding.monitor().device,
        context: binding.monitor().device,
        worker_digest: digest(9),
        driver_version: 12090,
        compute_major: 7,
        compute_minor: 5,
        prepared_lanes: 65536,
    }
}
#[test]
fn encoded_handshake_retains_binding_budget_and_admits_only_one_start_intent() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    let mut gate = Gate::new(binding).unwrap();
    let frame = tx
        .encode(&Message::Bootstrap {
            manifest: m.canonical().into(),
        })
        .unwrap();
    assert_eq!(
        u32::from_le_bytes(frame.as_bytes()[..4].try_into().unwrap()) as usize,
        frame.as_bytes().len()
    );
    let decoded = rx.decode(frame.as_bytes()).unwrap();
    assert_eq!(
        gate.accept(decoded.message(), Some(a(100_000))).unwrap(),
        Action::Prepare
    );
    gate.ready(&ready(binding)).unwrap();
    let frame = tx
        .encode(&Message::Start {
            issued: a(2_000_000),
        })
        .unwrap();
    let decoded = rx.decode(frame.as_bytes()).unwrap();
    assert!(matches!(
        gate.accept(decoded.message(), Some(a(3_000_000))).unwrap(),
        Action::Start {
            warmup_deadline_ns: 5_002_000_000,
            ..
        }
    ));
    assert!(gate.accept(decoded.message(), Some(a(4_000_000))).is_err());
    assert!(gate.stopped());
    assert!(b.used() > m.canonical().len());
}
#[test]
fn foreign_role_direction_sequence_and_capacity_poison_without_accepting_a_successor() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    let frame = tx
        .encode(&Message::Start {
            issued: a(1_000_000),
        })
        .unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    rx.decode(frame.as_bytes()).unwrap();
    assert!(rx.decode(frame.as_bytes()).is_err());
    assert!(matches!(
        rx.decode(frame.as_bytes()),
        Err(WireError::Poisoned)
    ));
    let mut wrong =
        Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    assert!(wrong.decode(frame.as_bytes()).is_err());
    assert!(matches!(
        wrong.decode(frame.as_bytes()),
        Err(WireError::Poisoned)
    ));
    let mut bad_send =
        Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    assert!(bad_send.encode(&ready(binding)).is_err());
    assert!(matches!(
        bad_send.encode(&Message::Start {
            issued: a(1_000_000)
        }),
        Err(WireError::Poisoned)
    ));
    let tight = Budget::new(8192).unwrap();
    let mut limited = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, tight).unwrap();
    assert!(limited
        .encode(&Message::Start {
            issued: a(1_000_000)
        })
        .is_err());
    assert!(matches!(
        limited.encode(&Message::Start {
            issued: a(1_000_000)
        }),
        Err(WireError::Poisoned)
    ));
}
#[test]
fn stop_before_ready_and_clock_loss_never_become_start_or_exit_proof() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let mut gate = Gate::new(binding).unwrap();
    let stop = Message::Stop {
        reason: Reason::new("requested").unwrap(),
        trigger_ns: 0,
        cooperative_ns: 1_000_000_000,
        exit_ns: 3_000_000_000,
    };
    assert_eq!(gate.accept(&stop, Some(a(0))).unwrap(), Action::Stop);
    let first = gate.stop_window();
    assert!(gate.ready(&ready(binding)).is_err());
    assert_eq!(gate.fault(), None);
    let later = Message::Stop {
        reason: Reason::new("later").unwrap(),
        trigger_ns: 1_000_000_000,
        cooperative_ns: 2_000_000_000,
        exit_ns: 4_000_000_000,
    };
    assert_eq!(
        gate.accept(&later, Some(a(1_000_000_000))).unwrap(),
        Action::Stop
    );
    assert_eq!(gate.stop_window(), first);
    assert!(gate
        .accept(
            &Message::Start {
                issued: a(1_000_000)
            },
            Some(a(2_000_000))
        )
        .is_err());
    let mut gate = Gate::new(binding).unwrap();
    assert!(gate
        .accept(
            &Message::Bootstrap {
                manifest: m.canonical().into()
            },
            None
        )
        .is_err());
    assert!(gate.stopped());
    assert!(gate
        .accept(
            &Message::Start {
                issued: a(1_000_000)
            },
            Some(a(2_000_000))
        )
        .is_err());
}

#[test]
fn independently_written_start_frame_pins_exact_worker_keys_and_clock_epoch() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b).unwrap();
    let frame = tx
        .encode(&Message::Start {
            issued: a(1_000_000),
        })
        .unwrap();
    // Hand-authored bytes, not produced through worker codec helpers. Only the
    // separately tested pilot declaration's input digests are substituted.
    let payload=r#"{"version":"1","role":"worker","lane":"ordinary","direction":"to_worker","sequence":"0","binding":{"session":"01010101010101010101010101010101","run":"02020202020202020202020202020202","device":{"uuid":"03030303030303030303030303030303","domain":"0","bus":"1","device":"2"},"manifest":"$manifest","profile":"$profile","worker":"0909090909090909090909090909090909090909090909090909090909090909","sampler":"0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a","clock_token":"05050505050505050505050505050505","frequency":"1000000000","epoch":{"qpc_before":"0","qpc_after":"0","interrupt_100ns":"0","unbiased_100ns":"0"},"origin_ns":"0","plan":{"seed":"0","lanes":"65536","steps":"4096","warmup":"128","measured":"256","envelope_ns":"45000000000","phase_caps_ns":["10000000000","10000000000","5000000000","10000000000","3000000000"]}},"kind":"start","body":{"issued":{"qpc_before":"1000000","qpc_after":"1000000","interrupt_100ns":"10000","unbiased_100ns":"10000"}}}"#
        .replace("$manifest",&m.digest().to_hex()).replace("$profile",&m.profile_digest().to_hex());
    let mut expected = ((payload.len() + 4) as u32).to_le_bytes().to_vec();
    expected.extend_from_slice(payload.as_bytes());
    assert_eq!(frame.as_bytes(), expected);
}
#[test]
fn all_worker_observations_roundtrip_signed_status_and_do_not_drop_ownership_charges() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let initial = b.used();
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    {
        for (lane, message) in [
            (Lane::Ordinary, ready(binding)),
            (
                Lane::Control,
                Message::StopAck {
                    anchor: a(2_000_000),
                    pending: true,
                },
            ),
            (
                Lane::Control,
                Message::RoleFinished {
                    anchor: a(3_000_000),
                    pending: true,
                    cleanup_attempted: true,
                    cleanup_status: Some(i32::MIN),
                    teardown: Default::default(),
                },
            ),
            (
                Lane::Control,
                Message::RoleFinished {
                    anchor: a(3_000_000),
                    pending: false,
                    cleanup_attempted: false,
                    cleanup_status: None,
                    teardown: Default::default(),
                },
            ),
        ] {
            let mut tx = Channel::new(binding, lane, Direction::FromWorker, b.clone()).unwrap();
            let mut rx = Channel::new(binding, lane, Direction::FromWorker, b.clone()).unwrap();
            let frame = tx.encode(&message).unwrap();
            let decoded = rx.decode(frame.as_bytes()).unwrap();
            assert_eq!(decoded.message(), &message);
        }
        let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        let frame = tx
            .encode(&Message::Bootstrap {
                manifest: m.canonical().into(),
            })
            .unwrap();
        let decoded = rx.decode(frame.as_bytes()).unwrap();
        drop(frame);
        assert!(b.used() >= initial + 8192 + m.canonical().len());
        drop(decoded);
        assert_eq!(b.used(), initial + 8192);
    }
    assert_eq!(b.used(), initial);
}
fn altered(frame: &[u8], from: &str, to: &str) -> Vec<u8> {
    let body = std::str::from_utf8(&frame[4..])
        .unwrap()
        .replacen(from, to, 1);
    let mut bytes = ((body.len() + 4) as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}
#[test]
fn malformed_or_cross_role_frames_fail_closed_and_leave_raw_bytes_available() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    let frame = tx
        .encode(&Message::Start {
            issued: a(1_000_000),
        })
        .unwrap();
    for (from, to) in [
        ("\"role\":\"worker\"", "\"role\":\"sampler\""),
        ("\"version\":\"1\"", "\"version\":1"),
        ("\"version\":\"1\"", "\"version\":\"1\",\"x\":\"1\""),
        (
            "\"sequence\":\"0\"",
            "\"sequence\":\"0\",\"sequence\":\"0\"",
        ),
        ("\"warmup\":\"128\"", "\"warmup\":\"127\""),
        ("\"frequency\":\"1000000000\"", "\"frequency\":\"1000000\""),
        ("\"kind\":\"start\"", "\"kind\":\"retry\""),
    ] {
        let bytes = altered(frame.as_bytes(), from, to);
        let original = bytes.clone();
        let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
        assert!(rx.decode(&bytes).is_err());
        assert_eq!(bytes, original);
        assert!(matches!(
            rx.decode(frame.as_bytes()),
            Err(WireError::Poisoned)
        ));
    }
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b).unwrap();
    assert!(rx.decode(&frame.as_bytes()[..3]).is_err());
    assert!(matches!(
        rx.decode(frame.as_bytes()),
        Err(WireError::Poisoned)
    ));
}
#[test]
fn reserved_control_capacity_survives_ordinary_exhaustion_on_actual_worker_codec() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let control = b.partition(2 * 1024 * 1024).unwrap();
    let mut ordinary =
        Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    let mut stop =
        Channel::new(binding, Lane::Control, Direction::ToWorker, control.clone()).unwrap();
    let mut stop_rx = Channel::new(binding, Lane::Control, Direction::ToWorker, control).unwrap();
    let _full = b.reserve(8 * 1024 * 1024 - b.used()).unwrap();
    assert!(ordinary
        .encode(&Message::Start {
            issued: a(1_000_000)
        })
        .is_err());
    let frame = stop
        .encode(&Message::Stop {
            reason: Reason::new("requested").unwrap(),
            trigger_ns: 0,
            cooperative_ns: 1_000_000_000,
            exit_ns: 3_000_000_000,
        })
        .unwrap();
    assert!(matches!(
        stop_rx.decode(frame.as_bytes()).unwrap().message(),
        Message::Stop { .. }
    ));
}
#[test]
fn deadline_equality_and_overlapping_ready_start_brackets_never_admit_start() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    for (issued, receipt) in [
        (a(20_000_000_000), a(20_000_000_000)),
        (a(2_000_000), a(5_002_000_000)),
        (a(500_000), a(2_000_000)),
    ] {
        let mut gate = Gate::new(binding).unwrap();
        gate.accept(
            &Message::Bootstrap {
                manifest: m.canonical().into(),
            },
            Some(a(100_000)),
        )
        .unwrap();
        gate.ready(&ready(binding)).unwrap();
        assert!(gate
            .accept(&Message::Start { issued }, Some(receipt))
            .is_err());
        assert!(gate.stopped());
        let first = gate.fault();
        assert!(gate
            .accept(
                &Message::Start {
                    issued: a(3_000_000)
                },
                Some(a(4_000_000))
            )
            .is_err());
        assert_eq!(gate.fault(), first);
    }
}
#[test]
fn setup_duration_is_frozen_at_prepare_and_stop_cannot_borrow_future_time() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    for ready_ns in [10_000_000_000, 19_000_000_000] {
        let mut gate = Gate::new(binding).unwrap();
        gate.accept(
            &Message::Bootstrap {
                manifest: m.canonical().into(),
            },
            Some(a(0)),
        )
        .unwrap();
        let mut observed = ready(binding);
        if let Message::Ready { anchor, .. } = &mut observed {
            *anchor = a(ready_ns);
        }
        assert!(gate.ready(&observed).is_err());
        assert!(gate.stopped());
    }
    let mut gate = Gate::new(binding).unwrap();
    gate.accept(
        &Message::Bootstrap {
            manifest: m.canonical().into(),
        },
        Some(a(0)),
    )
    .unwrap();
    gate.ready(&ready(binding)).unwrap();
    assert!(gate
        .accept(
            &Message::Start {
                issued: a(9_000_000_000)
            },
            Some(a(10_000_000_000))
        )
        .is_err());
    let mut gate = Gate::new(binding).unwrap();
    assert!(gate
        .accept(
            &Message::Stop {
                reason: Reason::new("requested").unwrap(),
                trigger_ns: 40_000_000_000,
                cooperative_ns: 41_000_000_000,
                exit_ns: 43_000_000_000,
            },
            Some(a(1_000_000_000))
        )
        .is_err());
    assert!(gate.stopped());
    assert_eq!(gate.stop_window(), None);
    // A genuinely late receipt retains already-expired original ceilings.
    let mut gate = Gate::new(binding).unwrap();
    assert_eq!(
        gate.accept(
            &Message::Stop {
                reason: Reason::new("requested").unwrap(),
                trigger_ns: 46_000_000_000,
                cooperative_ns: 45_000_000_000,
                exit_ns: 45_000_000_000,
            },
            Some(a(47_000_000_000))
        )
        .unwrap(),
        Action::Stop
    );
    assert_eq!(gate.stop_window().unwrap().exit_ns, 45_000_000_000);
}
