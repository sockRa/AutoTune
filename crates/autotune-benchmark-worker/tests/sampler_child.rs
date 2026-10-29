use autotune_benchmark_worker::{
    monitor::{Anchor, Metric, Ticket},
    sampler::{
        child::{ChildError, ChildGate},
        manifest::{CaptureManifest, Draft},
        wire::Message,
    },
};
fn manifest() -> CaptureManifest {
    CaptureManifest::freeze(Draft {
        session: [1; 16],
        run: [2; 16],
        clock_token: [3; 16],
        raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
        raw_pci: "0000:01:02.0".into(),
        source_commit: "a".repeat(40),
        source_tree: [4; 32],
        lockfile: [5; 32],
        controller: [6; 32],
        sampler: [7; 32],
        protocol: [8; 32],
        approval_ref: "separate exact approval".into(),
        output_root: "D:\\capture\\run".into(),
    })
    .unwrap()
}
#[test]
fn serial_child_rejects_unbootstrapped_parallel_duplicate_and_post_stop_getters() {
    let m = manifest();
    let b = m.binding(1_000_000_000).unwrap();
    let mut gate = ChildGate::new(b);
    let t = Ticket {
        binding: b.monitor,
        metric: Metric::Temperature,
        slot: 0,
        scheduled_ns: 0,
        start_ns: 1_000,
    };
    assert_eq!(gate.begin(t), Err(ChildError::NotReady));
    let origin = Anchor {
        clock_token: [3; 16],
        qpc_before: 0,
        qpc_after: 0,
        interrupt_100ns: 0,
        unbiased_100ns: 0,
    };
    gate.bootstrap(Message::Bootstrap {
        origin,
        raw_uuid: m.draft().raw_uuid.clone(),
        raw_pci: m.draft().raw_pci.clone(),
        manifest: m.canonical().into(),
    })
    .unwrap();
    gate.begin(t).unwrap();
    assert_eq!(gate.begin(t), Err(ChildError::Pending));
    gate.complete().unwrap();
    assert_eq!(gate.begin(t), Err(ChildError::Replay));
    gate.stop();
    let mut next = t;
    next.metric = Metric::Power;
    assert_eq!(gate.begin(next), Err(ChildError::Stopped));
}
#[test]
fn child_bootstrap_binds_exact_manifest_and_never_rebases_original_epoch() {
    let m = manifest();
    let mut b = m.binding(100).unwrap();
    b.manifest_digest = [9; 32];
    let mut gate = ChildGate::new(b);
    let origin = Anchor {
        clock_token: [3; 16],
        qpc_before: 0,
        qpc_after: 0,
        interrupt_100ns: 0,
        unbiased_100ns: 0,
    };
    assert!(gate
        .bootstrap(Message::Bootstrap {
            origin,
            raw_uuid: m.draft().raw_uuid.clone(),
            raw_pci: m.draft().raw_pci.clone(),
            manifest: m.canonical().into()
        })
        .is_err());
}

#[test]
fn cleanup_message_follows_one_sticky_stop_ack_and_never_claims_exit() {
    use autotune_benchmark_worker::sampler::child::ChildShutdown;
    let first = Anchor {
        clock_token: [3; 16],
        qpc_before: 100,
        qpc_after: 100,
        interrupt_100ns: 1,
        unbiased_100ns: 1,
    };
    let mut shutdown = ChildShutdown::new();
    assert!(shutdown.finished(first, false).is_err());
    assert!(matches!(
        shutdown.stop(first, true),
        Some(Message::StopAck { pending: true, .. })
    ));
    let later = Anchor {
        qpc_before: 200,
        qpc_after: 200,
        interrupt_100ns: 2,
        unbiased_100ns: 2,
        ..first
    };
    assert!(shutdown.stop(later, true).is_none());
    assert_eq!(shutdown.first_stop(), Some(first));
    match shutdown.finished(later, false).unwrap() {
        Message::RoleFinished {
            pending, cleanup, ..
        } => {
            assert!(!pending);
            assert_eq!(cleanup, "nvml_shutdown_attempted_result_unknown");
        }
        _ => panic!("wrong message"),
    }
    assert!(shutdown.finished(later, false).is_err());
}

#[test]
fn independent_stop_wins_before_admission_and_completion_cannot_reopen_it() {
    use autotune_benchmark_worker::sampler::child::NativeAdmission;
    let stopped = NativeAdmission::default();
    stopped.stop();
    assert_eq!(stopped.begin(), Err(ChildError::Stopped));
    assert!(!stopped.pending());
    let inflight = NativeAdmission::default();
    inflight.begin().unwrap();
    assert_eq!(inflight.begin(), Err(ChildError::Pending));
    inflight.stop();
    assert!(inflight.pending());
    inflight.complete();
    assert!(!inflight.pending());
    assert_eq!(inflight.begin(), Err(ChildError::Stopped));
}
