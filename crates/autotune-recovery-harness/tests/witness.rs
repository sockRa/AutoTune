#![cfg(windows)]
use autotune_recovery_contract::{process::*, *};
use autotune_recovery_harness::{runtime::fake_profile, witness::Witness};

fn invocation(id: u64) -> (DispatchProof, FakeInvocation) {
    let permit = Permission {
        session: [1; 32],
        boot: [2; 32],
        digest: [3; 32],
        generation: 1,
        operation_id: id,
        operation: Operation::Current,
        dispatched_ms: 10,
        watchdog_ms: 1010,
        expiry_ms: 1010,
        recovery_trigger_ms: None,
    };
    (
        DispatchProof {
            permit: permit.clone(),
            intent_sequence: id,
            intent_utc_ms: 100,
            publication: Some([4; 32]),
        },
        FakeInvocation {
            permit,
            device: fake_profile().device,
            entry_ms: 11,
            write_mw: None,
        },
    )
}

#[test]
fn witness_preserves_unfinished_effect_rejects_overlap_and_never_releases_twice() {
    let mut w = Witness::new(fake_profile()).unwrap();
    let (proof, call) = invocation(1);
    w.enter(&proof, call).unwrap();
    assert!(w.calls()[0].result.is_none());
    let (next, other) = invocation(2);
    assert!(w.enter(&next, other.clone()).is_err());
    assert!(w.release(10, false).is_err());
    assert_eq!(w.release(12, true).unwrap(), RawResult::Value(247_123));
    assert!(w.calls()[0].worker_lost_before_effect);
    assert!(w.release(13, false).is_err());
    w.enter(&next, other).unwrap();
    assert_eq!(w.calls().len(), 2);
}

#[test]
fn witness_rejects_wrong_identity_permit_replay_and_raw_write_without_manifest() {
    let mut w = Witness::new(fake_profile()).unwrap();
    let (proof, call) = invocation(1);
    let mut wrong = call.clone();
    wrong.device.uuid.push('x');
    assert!(w.enter(&proof, wrong).is_err());
    let mut wrong = call.clone();
    wrong.permit.operation_id += 1;
    assert!(w.enter(&proof, wrong).is_err());
    let mut wrong = call.clone();
    wrong.write_mw = Some(215_000);
    assert!(w.enter(&proof, wrong).is_err());
    let mut setter = call.clone();
    setter.permit.operation = Operation::CandidateOnce;
    setter.write_mw = Some(242_123);
    let mut setter_proof = proof.clone();
    setter_proof.permit = setter.permit.clone();
    assert!(w.enter(&setter_proof, setter).is_err());
    w.enter(&proof, call.clone()).unwrap();
    w.release(12, false).unwrap();
    assert!(w.enter(&proof, call).is_err());
    assert_eq!(w.calls().len(), 1);
    assert_eq!(w.current_mw(), 247_123);
}
