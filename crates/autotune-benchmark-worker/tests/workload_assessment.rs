use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, wire::Lane},
    workload::{
        assessment::{Audit, Fault, Progress},
        dispatch::{Call, Operation, Phase, Record},
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
fn binding(m: &PilotManifest) -> WorkerBinding {
    WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap()
}
fn record(j: u32, start: u64) -> Record {
    use Operation::*;
    Record {
        dispatch: j,
        phase: if j < 128 {
            Phase::Warmup
        } else {
            Phase::Measured
        },
        seed: 0,
        lanes: 65536,
        steps: 4096,
        calls: [
            Input, UploadA, UploadB, UploadX, Sentinel, Launch, Query, Readback, Check, Hash,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, operation)| Call {
            operation,
            before: Some(a(start + i as u64 * 20_000)),
            after: Some(a(start + i as u64 * 20_000 + 10_000)),
            status: if matches!(operation, Input | Check | Hash) {
                None
            } else {
                Some(0)
            },
        })
        .collect(),
        pending: false,
        readback_lanes: Some(65536),
        checked_lanes: 65536,
        mismatches: 0,
        nonfinite: 0,
        first_failure: None,
        verified_fma: 268435456,
        input_sha256: Some(digest(20)),
        output_sha256: Some(digest(21)),
    }
}
fn warmup(audit: &mut Audit) {
    for j in 0..128 {
        audit
            .observe(
                &Message::DispatchRecord(record(j, (j as u64 + 1) * 1_000_000)),
                Some(a((j as u64 + 1) * 1_000_000 + 200_000)),
            )
            .unwrap();
    }
}
#[test]
fn actual_encoded_complete_plan_needs_phase_entry_and_all_reported_checks() {
    let b = Budget::new(16 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut audit = Audit::new(binding, a(0), &b).unwrap();
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b).unwrap();
    let mut retained = Vec::new();
    for j in 0..384 {
        if j == 128 {
            let frame = tx
                .encode(&Message::PhaseEntered {
                    phase: Phase::Measured,
                    anchor: a(129_000_000),
                })
                .unwrap();
            let decoded = rx.decode(frame.as_bytes()).unwrap();
            audit
                .observe(decoded.message(), Some(a(134_000_000)))
                .unwrap();
            retained.push(decoded);
        }
        let start = (j as u64 + 1) * 1_000_000;
        let frame = tx
            .encode(&Message::DispatchRecord(record(j, start)))
            .unwrap();
        let decoded = rx.decode(frame.as_bytes()).unwrap();
        // Buffered source timestamps can precede the previous controller receipt.
        audit
            .observe(decoded.message(), Some(a(start + 5_200_000)))
            .unwrap();
        retained.push(decoded);
    }
    assert_eq!(retained.len(), 385);
    assert_eq!(
        audit.progress(),
        Progress {
            warmup: 128,
            measured: 256
        }
    );
    assert!(audit.reports_complete());
    assert_eq!(audit.fault(), None);
    assert_eq!(
        audit.observe(
            &Message::DispatchRecord(record(383, 400_000_000)),
            Some(a(401_000_000))
        ),
        Err(Fault::Sequence)
    );
    assert!(audit.reports_complete());
    assert_eq!(audit.progress().measured, 256);
}
#[test]
fn gaps_duplicates_missing_phase_and_late_records_cannot_reopen_counts() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    for j in [1, 127, 128] {
        let mut audit = Audit::new(binding, a(0), &b).unwrap();
        assert_eq!(
            audit.observe(
                &Message::DispatchRecord(record(j, 1_000_000)),
                Some(a(2_000_000))
            ),
            Err(Fault::Sequence)
        );
        assert_eq!(
            audit.progress(),
            Progress {
                warmup: 0,
                measured: 0
            }
        );
        assert_eq!(
            audit.observe(
                &Message::DispatchRecord(record(0, 3_000_000)),
                Some(a(4_000_000))
            ),
            Err(Fault::Closed)
        );
        assert_eq!(audit.fault(), Some(Fault::Sequence));
    }
    let mut audit = Audit::new(binding, a(0), &b).unwrap();
    warmup(&mut audit);
    assert_eq!(
        audit.observe(
            &Message::DispatchRecord(record(128, 129_000_000)),
            Some(a(130_000_000))
        ),
        Err(Fault::Sequence)
    );
    assert_eq!(
        audit.progress(),
        Progress {
            warmup: 128,
            measured: 0
        }
    );
}
#[test]
fn missing_failed_or_reordered_native_calls_and_unchecked_outputs_never_count() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut bad = Vec::new();
    let mut r = record(0, 1_000_000);
    r.calls[5].status = Some(-1);
    bad.push(r);
    let mut r = record(0, 1_000_000);
    r.calls[6].status = None;
    bad.push(r);
    let mut r = record(0, 1_000_000);
    r.calls[6].status = Some(600);
    bad.push(r);
    let mut r = record(0, 1_000_000);
    r.calls.swap(5, 7);
    bad.push(r);
    let mut r = record(0, 1_000_000);
    r.calls.remove(4);
    bad.push(r);
    let mut r = record(0, 1_000_000);
    r.calls[7].status = None;
    bad.push(r);
    let mut r = record(0, 1_000_000);
    r.pending = true;
    r.verified_fma = 0;
    bad.push(r);
    let mut r = record(0, 1_000_000);
    r.checked_lanes = 65535;
    r.verified_fma = 0;
    bad.push(r);
    for r in bad {
        let original = r.clone();
        let mut audit = Audit::new(binding, a(0), &b).unwrap();
        assert!(audit
            .observe(&Message::DispatchRecord(r.clone()), Some(a(2_000_000)))
            .is_err());
        assert_eq!(r, original);
        assert_eq!(audit.progress().warmup, 0);
    }
}
#[test]
fn valid_pending_query_observations_precede_single_returned_completion() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut r = record(0, 1_000_000);
    let mut pending = r.calls[6].clone();
    pending.status = Some(600);
    pending.before = Some(a(1_110_000));
    pending.after = Some(a(1_115_000));
    r.calls.insert(6, pending);
    let mut audit = Audit::new(binding, a(0), &b).unwrap();
    assert_eq!(
        audit
            .observe(&Message::DispatchRecord(r), Some(a(2_000_000)))
            .unwrap()
            .warmup,
        1
    );
}
#[test]
fn clocks_are_original_ordered_and_never_synthesized_from_missing_or_future_anchors() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    for kind in 0..5 {
        let mut r = record(0, 1_000_000);
        let mut receipt = Some(a(2_000_000));
        match kind {
            0 => r.calls[2].before = Some(a(0)),
            1 => r.calls[2].after = None,
            2 => r.calls[2].after.as_mut().unwrap().interrupt_100ns += 100_000,
            3 => receipt = None,
            _ => receipt = Some(a(1_100_000)),
        }
        let mut audit = Audit::new(binding, a(0), &b).unwrap();
        assert_eq!(
            audit.observe(&Message::DispatchRecord(r), receipt),
            Err(Fault::Clock)
        );
        assert_eq!(audit.progress().warmup, 0);
    }
}
#[test]
fn independent_ticks_close_silent_worker_and_queued_receipts_do_not_regress_tick_clock() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut audit = Audit::new(binding, a(0), &b).unwrap();
    audit.tick(Some(a(3_000_000))).unwrap();
    assert_eq!(
        audit
            .observe(
                &Message::DispatchRecord(record(0, 1_000_000)),
                Some(a(2_000_000))
            )
            .unwrap()
            .warmup,
        1
    );
    let deadline = audit.deadline_ns();
    assert_eq!(deadline, 5_000_000_000);
    assert_eq!(audit.tick(Some(a(deadline))), Err(Fault::Deadline));
    assert_eq!(
        audit.observe(
            &Message::DispatchRecord(record(1, 4_000_000)),
            Some(a(5_000_000))
        ),
        Err(Fault::Closed)
    );
    assert_eq!(audit.deadline_ns(), deadline);
    assert_eq!(audit.fault(), Some(Fault::Deadline));
}
#[test]
fn measurement_deadline_freezes_raw_entry_and_late_entry_cannot_rebase_it() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut audit = Audit::new(binding, a(0), &b).unwrap();
    warmup(&mut audit);
    assert_eq!(audit.deadline_ns(), 35_000_000_000);
    audit
        .observe(
            &Message::PhaseEntered {
                phase: Phase::Measured,
                anchor: a(129_000_000),
            },
            Some(a(200_000_000)),
        )
        .unwrap();
    assert_eq!(audit.deadline_ns(), 10_129_000_000);
    assert_eq!(audit.tick(Some(a(10_129_000_000))), Err(Fault::Deadline));
    let mut audit = Audit::new(binding, a(0), &b).unwrap();
    warmup(&mut audit);
    // Entry was buffered before this tick; it cannot retroactively extend time.
    audit.tick(Some(a(11_000_000_000))).unwrap();
    assert_eq!(
        audit.observe(
            &Message::PhaseEntered {
                phase: Phase::Measured,
                anchor: a(129_000_000)
            },
            Some(a(200_000_000))
        ),
        Err(Fault::Deadline)
    );
    assert_eq!(audit.deadline_ns(), 10_129_000_000);
}
#[test]
fn stream_and_cycle_deadline_equality_and_fixed_metadata_budget_are_enforced() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    assert!(matches!(
        Audit::new(binding, a(0), &Budget::new(1).unwrap()),
        Err(Fault::Capacity)
    ));
    let before = b.used();
    let audit = Audit::new(binding, a(0), &b).unwrap();
    assert!(b.used() > before);
    drop(audit);
    assert_eq!(b.used(), before);
    for kind in 0..3 {
        let mut r = record(0, 1_000_000);
        if kind == 0 {
            let submitted = r.calls[1].before.unwrap().qpc_before;
            for c in &mut r.calls[6..] {
                for anchor in [&mut c.before, &mut c.after] {
                    *anchor = Some(a(anchor.unwrap().qpc_before + 99_890_000));
                }
            }
            assert_eq!(r.calls[6].after.unwrap().qpc_after - submitted, 100_000_000);
        } else if kind == 1 {
            r.calls[9].after = Some(a(251_000_000));
        } else {
            // Upload/sentinel time belongs to the stream submission window.
            r.calls[1].after = Some(a(101_030_000));
            for c in &mut r.calls[2..] {
                for anchor in [&mut c.before, &mut c.after] {
                    *anchor = Some(a(anchor.unwrap().qpc_before + 100_000_000));
                }
            }
        }
        let mut audit = Audit::new(binding, a(0), &b).unwrap();
        assert_eq!(
            audit.observe(&Message::DispatchRecord(r), Some(a(300_000_000))),
            Err(Fault::Deadline)
        );
        assert_eq!(audit.progress().warmup, 0);
    }
}
