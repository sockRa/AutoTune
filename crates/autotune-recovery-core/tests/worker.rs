#[path = "../../autotune-recovery-contract/tests/support/mod.rs"]
mod support;
use autotune_recovery_contract::{process::*, storage::*, *};
use autotune_recovery_core::worker::*;
fn profile() -> FakeProfile {
    FakeProfile {
        device: support::preparation().device,
        runtime: support::runtime(),
        baseline_mw: 247_123,
        minimum_mw: 100_000,
        maximum_mw: 300_000,
        default_mw: Some(215_000),
        behavior: FakeBehavior::Normal,
    }
}
fn policy() -> WorkerPolicy {
    let p = support::preparation();
    let mut w =
        WorkerPolicy::new(profile(), p.session, p.boot, [8; 32], p.operator.clone()).unwrap();
    w.prepare(p, [9; 32]).unwrap();
    w
}
fn call(
    w: &mut WorkerPolicy,
    ledger: &mut Vec<StoredCall>,
    op: Operation,
    time: u64,
    generation: u64,
    trigger: Option<u64>,
    hash: Hash,
) {
    let id = ledger.len() as u64 + 1;
    let seq = id * 2;
    let p = Permission {
        session: [1; 32],
        boot: [5; 32],
        digest: hash,
        generation,
        operation_id: id,
        operation: op,
        dispatched_ms: time,
        watchdog_ms: time + 1000,
        expiry_ms: time + 1000,
        recovery_trigger_ms: trigger,
    };
    let proof = DispatchProof {
        permit: p.clone(),
        intent_sequence: seq,
        intent_utc_ms: time,
        publication: Some([8; 32]),
    };
    let entry = w
        .admit(proof, time, time, true)
        .unwrap()
        .unwrap_or_else(|o| panic!("{o:?}"));
    w.revalidate_entry(&entry, time, true).unwrap();
    let result = w.fake_call(&entry).unwrap();
    let outcome = w.returned(entry, time, time, result).unwrap();
    let receipt = ResultReceipt {
        operation_id: id,
        sequence: seq + 1,
        received_utc_ms: time,
        publication: Some([9; 32]),
    };
    w.recorded(&receipt).unwrap();
    ledger.push(StoredCall {
        permit: p,
        intent_sequence: seq,
        intent_utc_ms: time,
        completion: Some(StoredCompletion {
            sequence: seq + 1,
            received_utc_ms: time,
            outcome,
        }),
    });
}
fn captured() -> (WorkerPolicy, Vec<StoredCall>, Manifest) {
    let mut w = policy();
    let mut ledger = vec![];
    let hash = support::preparation().digest().unwrap();
    for op in [
        Operation::Enumerate,
        Operation::Identity,
        Operation::Runtime,
        Operation::Bounds,
        Operation::Current,
    ] {
        call(&mut w, &mut ledger, op, 0, 1, None, hash);
    }
    call(&mut w, &mut ledger, Operation::Current, 500, 1, None, hash);
    let mut m = support::manifest(247_123);
    m.capture.trace = digest(b"stored-capture-v1", &canonical(&ledger).unwrap());
    w.seal(m.clone()).unwrap();
    (w, ledger, m)
}
#[test]
fn independent_worker_requires_capture_publications_exact_manifest_and_frozen_approval() {
    let (mut w, mut ledger, m) = captured();
    let arm = ArmCapsule {
        approval: support::approval(&m),
        commit: ArmCommit {
            boot_ms: 500,
            utc_ms: 500,
        },
        nonce_publication: [9; 32],
    };
    let mut bad = arm.clone();
    bad.approval.operator = "wrong".into();
    assert!(w.arm(&bad, 500, 500, true).is_err());
    w.arm(&arm, 500, 500, true).unwrap();
    assert!(w.arm(&arm, 500, 500, true).is_err());
    for op in [
        Operation::Enumerate,
        Operation::Identity,
        Operation::Runtime,
        Operation::Bounds,
        Operation::Current,
    ] {
        call(&mut w, &mut ledger, op, 500, 1, None, m.digest().unwrap());
    }
    call(
        &mut w,
        &mut ledger,
        Operation::CandidateOnce,
        501,
        1,
        None,
        m.digest().unwrap(),
    );
    call(
        &mut w,
        &mut ledger,
        Operation::Current,
        502,
        1,
        None,
        m.digest().unwrap(),
    );
    w.revoke(
        2,
        RecoveryWindow {
            trigger_ms: 502,
            deadline_ms: 5502,
        },
    )
    .unwrap();
    for op in [
        Operation::Enumerate,
        Operation::Identity,
        Operation::Runtime,
        Operation::Bounds,
        Operation::Current,
    ] {
        call(
            &mut w,
            &mut ledger,
            op,
            503,
            2,
            Some(502),
            m.digest().unwrap(),
        );
    }
    call(
        &mut w,
        &mut ledger,
        Operation::RestoreBaseline,
        504,
        2,
        Some(502),
        m.digest().unwrap(),
    );
    call(
        &mut w,
        &mut ledger,
        Operation::Current,
        505,
        2,
        Some(502),
        m.digest().unwrap(),
    );
    call(
        &mut w,
        &mut ledger,
        Operation::Current,
        755,
        2,
        Some(502),
        m.digest().unwrap(),
    );
    assert_eq!(w.phase(), WorkerPhase::Verified);
    assert_eq!(w.candidate_entries(), 1);
    assert_eq!(w.restore_entries(), 1);
}
#[test]
fn worker_rejects_overlap_receipt_loss_wrong_binding_and_guardian_loss_without_replacement() {
    let mut w = policy();
    let p = Permission {
        session: [1; 32],
        boot: [5; 32],
        digest: support::preparation().digest().unwrap(),
        generation: 1,
        operation_id: 1,
        operation: Operation::Enumerate,
        dispatched_ms: 0,
        watchdog_ms: 1000,
        expiry_ms: 1000,
        recovery_trigger_ms: None,
    };
    let proof = DispatchProof {
        permit: p,
        intent_sequence: 2,
        intent_utc_ms: 0,
        publication: Some([9; 32]),
    };
    let entry = w
        .admit(proof.clone(), 0, 0, true)
        .unwrap()
        .unwrap_or_else(|_| panic!());
    assert!(w.admit(proof, 0, 0, true).is_err());
    assert!(w.revalidate_entry(&entry, 0, false).is_err());
    let returned = w.not_entered(entry, "guardian lost".into(), 0).unwrap();
    assert!(matches!(returned, Completion::NotEntered { .. }));
    assert_eq!(w.candidate_entries(), 0);
    assert!(w
        .recorded(&ResultReceipt {
            operation_id: 2,
            sequence: 3,
            received_utc_ms: 0,
            publication: Some([9; 32])
        })
        .is_err());
}

#[test]
fn cancellation_before_approval_or_candidate_cannot_create_restore_authority() {
    for arm_first in [false, true] {
        let (mut worker, _, manifest) = captured();
        if arm_first {
            worker
                .arm(
                    &ArmCapsule {
                        approval: support::approval(&manifest),
                        commit: ArmCommit {
                            boot_ms: 500,
                            utc_ms: 500,
                        },
                        nonce_publication: [9; 32],
                    },
                    500,
                    500,
                    true,
                )
                .unwrap();
        }
        worker
            .revoke(
                2,
                RecoveryWindow {
                    trigger_ms: 500,
                    deadline_ms: 5500,
                },
            )
            .unwrap();
        assert_eq!(
            worker.phase(),
            WorkerPhase::Failed,
            "no possible candidate mutation exists, so no restoration authority exists"
        );
        assert_eq!(worker.candidate_entries(), 0);
        assert_eq!(worker.restore_entries(), 0);
    }
}

#[test]
fn one_permit_cannot_execute_twice_or_relabel_an_entered_call_as_not_entered() {
    let mut worker = policy();
    let permit = Permission {
        session: [1; 32],
        boot: [5; 32],
        digest: support::preparation().digest().unwrap(),
        generation: 1,
        operation_id: 1,
        operation: Operation::Enumerate,
        dispatched_ms: 0,
        watchdog_ms: 1000,
        expiry_ms: 1000,
        recovery_trigger_ms: None,
    };
    let entry = worker
        .admit(
            DispatchProof {
                permit,
                intent_sequence: 2,
                intent_utc_ms: 0,
                publication: Some([9; 32]),
            },
            0,
            0,
            true,
        )
        .unwrap()
        .unwrap_or_else(|_| panic!());
    worker.fake_call(&entry).unwrap();
    assert!(
        worker.fake_call(&entry).is_err(),
        "a permit is not a reusable call handle"
    );
    assert!(worker
        .not_entered(entry, "false no-entry evidence".into(), 1)
        .is_err());
}
