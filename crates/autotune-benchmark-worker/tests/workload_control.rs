use autotune_benchmark_worker::{
    evidence::{Digest, Reason},
    monitor::Anchor,
    sampler::{
        budget::Budget,
        framing::{Framer, ReadStep, Source},
        wire::Lane,
    },
    workload::{
        control::{Fault, Shared, State},
        manifest::{Artifacts, PilotDraft, PilotManifest},
        transport::{RawReceived, WorkerReceived},
        wire::{Channel, Direction, Message, WorkerBinding},
    },
};
use std::sync::Arc;
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
fn setup() -> (Budget, WorkerBinding, Arc<Shared>, State) {
    let b = Budget::new(16 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let shared = Arc::new(Shared::default());
    let state = State::new(binding, a(0), shared.clone(), &b).unwrap();
    (b, binding, shared, state)
}
fn stop(trigger: u64) -> Message {
    Message::Stop {
        trigger_ns: trigger,
        cooperative_ns: trigger + 1_000_000_000,
        exit_ns: trigger + 3_000_000_000,
        reason: Reason::new("owner-stop").unwrap(),
    }
}
fn received(bytes: &[u8], at: Option<Anchor>, rx: &mut Channel, b: &Budget) -> WorkerReceived {
    let packet = Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes));
    RawReceived::new(Lane::Control, packet, at).decode_worker(rx)
}
#[test]
fn delayed_parent_stop_keeps_original_window_and_pending_ack() {
    let (b, binding, shared, mut state) = setup();
    shared.admission().begin().unwrap();
    let mut tx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let mut rx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let frame = tx.encode(&stop(1_000_000_000)).unwrap();
    let ack = state
        .packet(received(
            frame.as_bytes(),
            Some(a(1_500_000_000)),
            &mut rx,
            &b,
        ))
        .ok()
        .flatten()
        .unwrap();
    assert!(matches!(ack, Message::StopAck { pending: true, .. }));
    shared.admission().complete();
    assert!(shared.admission().begin().is_err());
    assert_eq!(state.window().unwrap().cooperative_ns, 2_000_000_000);
    assert_eq!(shared.cooperative_raw(), 20_000_000);
    let repeat = tx.encode(&stop(1_700_000_000)).unwrap();
    assert!(state
        .packet(received(
            repeat.as_bytes(),
            Some(a(1_800_000_000)),
            &mut rx,
            &b
        ))
        .ok()
        .flatten()
        .is_none());
    assert_eq!(state.window().unwrap().trigger_ns, 1_000_000_000);
    assert!(!state.tick(Some(a(2_000_000_000))));
    assert_eq!(state.fault(), Some(Fault::Deadline));
    assert_eq!(state.raw().len(), 2);
}
#[test]
fn local_trigger_and_event_merge_without_renewing_original_grace() {
    let (_, _, shared, mut state) = setup();
    assert!(state.event(Some(a(2_000_000_000))).is_some());
    assert!(state
        .local_stop(Some(a(1_000_000_000)), Some(a(2_100_000_000)))
        .is_none());
    assert_eq!(state.window().unwrap().trigger_ns, 1_000_000_000);
    assert_eq!(state.window().unwrap().exit_ns, 4_000_000_000);
    assert_eq!(shared.cooperative_raw(), 20_000_000);
    assert!(!state.tick(Some(a(2_100_000_100))));
}
#[test]
fn malformed_control_closes_before_decode_and_retains_exact_raw_bytes() {
    let (b, binding, shared, mut state) = setup();
    let mut rx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let bytes = [6, 0, 0, 0, b'{', b'}'];
    assert!(state
        .packet(received(&bytes, Some(a(100)), &mut rx, &b))
        .ok()
        .flatten()
        .is_none());
    assert!(shared.admission().stopped());
    assert_eq!(state.fault(), Some(Fault::Protocol));
    assert_eq!(state.raw()[0].raw().packet().bytes(), bytes);
    assert_eq!(shared.cooperative_raw(), 0);
}
#[test]
fn missing_regressing_and_foreign_clocks_are_preserved_without_substitution() {
    for at in [
        None,
        Some(a(100)),
        Some(Anchor {
            clock_token: [0; 16],
            ..a(300)
        }),
    ] {
        let (_, _, shared, mut state) = setup();
        assert!(state.tick(Some(a(200))));
        assert!(!state.tick(at));
        assert_eq!(state.fault(), Some(Fault::Clock));
        assert_eq!(state.clock().missing(), at.is_none());
        if let Some(a) = at {
            assert_eq!(state.clock().raw().last(), Some(&a));
        }
        assert!(shared.admission().stopped());
        assert_eq!(shared.cooperative_raw(), 0);
    }
}
#[test]
fn raw_overflow_is_not_evicted_and_later_input_returns_to_caller() {
    let (b, binding, _, mut state) = setup();
    let mut tx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let mut rx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let mut fifth = Vec::new();
    for i in 0..5 {
        let frame = tx.encode(&stop(0)).unwrap();
        if i == 4 {
            fifth = frame.as_bytes().to_vec();
        }
        assert!(state
            .packet(received(frame.as_bytes(), Some(a(i * 100)), &mut rx, &b))
            .is_ok());
    }
    assert_eq!(state.fault(), Some(Fault::Capacity));
    assert_eq!(state.raw().len(), 4);
    assert_eq!(state.overflow().unwrap().raw().packet().bytes(), fifth);
    let frame = tx.encode(&stop(0)).unwrap();
    let returned = match state.packet(received(frame.as_bytes(), Some(a(600)), &mut rx, &b)) {
        Err(r) => r,
        Ok(_) => panic!("expected owner back"),
    };
    assert_eq!(returned.raw().packet().bytes(), frame.as_bytes());
    assert_eq!(state.overflow().unwrap().raw().packet().bytes(), fifth);
}
#[test]
fn finished_retains_nonzero_cleanup_and_pending_as_separate_facts() {
    let (_, _, shared, mut state) = setup();
    state.event(Some(a(100)));
    shared.admission().begin_cleanup().unwrap();
    assert!(matches!(
        state.complete(Some(a(200)), true, Some(999)),
        Some(Message::RoleFinished {
            pending: true,
            cleanup_attempted: true,
            cleanup_status: Some(999),
            ..
        })
    ));
    assert!(state.finished());
    assert!(shared.admission().stopped());
    assert!(state.complete(Some(a(300)), true, Some(0)).is_none());
}
#[test]
fn expired_or_inconsistent_completion_cannot_be_reported_as_finished() {
    let (_, _, _, mut state) = setup();
    state.event(Some(a(0)));
    assert!(state
        .complete(Some(a(1_000_000_000)), false, None)
        .is_none());
    assert!(!state.finished());
    let (_, _, _, mut state) = setup();
    assert!(state.complete(Some(a(100)), false, Some(0)).is_none());
    assert_eq!(state.fault(), Some(Fault::Protocol));
}
#[test]
fn sampler_role_roundtrips_readiness_without_accepting_worker_start() {
    let (b, binding, _, _) = setup();
    let message = Message::SamplerReady {
        before: a(100),
        after: a(200),
        raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
        raw_pci: "0000:01:02.0".into(),
        sampler_digest: digest(10),
        runtime: "{\"unavailable\":true}".into(),
    };
    let mut tx =
        Channel::sampler(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let frame = tx.encode(&message).unwrap();
    let mut rx =
        Channel::sampler(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    assert_eq!(rx.decode(frame.as_bytes()).unwrap().message(), &message);
    let mut worker =
        Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    assert!(worker.decode(frame.as_bytes()).is_err());
    let mut sampler = Channel::sampler(binding, Lane::Ordinary, Direction::ToWorker, b).unwrap();
    assert!(sampler.encode(&Message::Start { issued: a(100) }).is_err());
}
#[test]
fn native_failure_preserves_unknown_or_foreign_clocks_and_signed_vendor_status() {
    let (b, binding, _, _) = setup();
    let message = Message::NativeFailure {
        stage: Reason::new("nvml-shutdown").unwrap(),
        before: None,
        after: Some(Anchor {
            clock_token: [0; 16],
            ..a(100)
        }),
        reason: autotune_probe::report::Reason::QueryFailed,
        vendor_status: Some(i32::MIN),
        detail: "raw failure".into(),
    };
    let mut tx =
        Channel::sampler(binding, Lane::Control, Direction::FromWorker, b.clone()).unwrap();
    let frame = tx.encode(&message).unwrap();
    let mut rx = Channel::sampler(binding, Lane::Control, Direction::FromWorker, b).unwrap();
    assert_eq!(rx.decode(frame.as_bytes()).unwrap().message(), &message);
}
#[test]
fn late_packet_is_retained_but_cannot_renew_or_ack_an_expired_stop() {
    let (b, binding, _, mut state) = setup();
    let mut tx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let mut rx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let frame = tx.encode(&stop(0)).unwrap();
    assert!(state
        .packet(received(
            frame.as_bytes(),
            Some(a(1_000_000_000)),
            &mut rx,
            &b
        ))
        .ok()
        .flatten()
        .is_none());
    assert_eq!(state.fault(), Some(Fault::Deadline));
    assert_eq!(state.raw().len(), 1);
}
#[test]
fn complete_run_history_fits_two_observations_per_tick_without_growth() {
    let (b, _, _, mut state) = setup();
    let m = manifest(&b);
    let end = m.bind_run(0, 0).unwrap().plan().envelope_ns;
    let tick = autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .controller_tick_ns;
    let baseline = b.used();
    for n in 1..end / tick {
        assert!(state.tick(Some(a(n * tick))));
        assert!(state.tick(Some(a(n * tick))));
    }
    assert!(!state.tick(Some(a(end))));
    assert_eq!(state.fault(), Some(Fault::Deadline));
    assert_eq!(b.used(), baseline);
}
#[test]
fn reserved_control_codec_still_works_when_ordinary_ram_is_exhausted() {
    let (b, binding, _, _) = setup();
    let control = b.partition(2 * 1024 * 1024).unwrap();
    let mut tx =
        Channel::sampler(binding, Lane::Control, Direction::ToWorker, control.clone()).unwrap();
    let mut rx = Channel::sampler(binding, Lane::Control, Direction::ToWorker, control).unwrap();
    let _pressure = b.reserve(16 * 1024 * 1024 - b.used()).unwrap();
    assert!(b.reserve(1).is_err());
    let frame = tx.encode(&stop(0)).unwrap();
    assert_eq!(rx.decode(frame.as_bytes()).unwrap().message(), &stop(0));
}
#[test]
fn control_rejects_a_foreign_full_manifest_even_if_monitor_identity_matches() {
    let (b, binding, shared, mut state) = setup();
    let text = manifest(&b)
        .canonical()
        .replace(&digest(9).to_hex(), &digest(99).to_hex());
    let m = PilotManifest::parse(text.as_bytes(), &b).unwrap();
    let foreign = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    assert_eq!(foreign.monitor(), binding.monitor());
    assert_ne!(foreign, binding);
    let mut tx = Channel::sampler(foreign, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let mut rx = Channel::sampler(foreign, Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let frame = tx.encode(&stop(0)).unwrap();
    assert!(state
        .packet(received(frame.as_bytes(), Some(a(100)), &mut rx, &b))
        .ok()
        .flatten()
        .is_none());
    assert_eq!(state.fault(), Some(Fault::Protocol));
    assert!(shared.admission().stopped());
    assert_eq!(state.raw()[0].raw().packet().bytes(), frame.as_bytes());
}
