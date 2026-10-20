//! Actual Windows publication with injected clocks and an in-process fake worker.
//! Not process-survival, native quiescence or hardware-restoration evidence.
#![cfg(windows)]
#[path = "../../autotune-recovery-core/tests/support/journal.rs"]
mod support;
use autotune_recovery_contract::{process::*, storage::*, *};
use autotune_recovery_core::*;
use autotune_recovery_windows::storage::*;
use support::*;

#[test]
fn reducer_and_fake_worker_roundtrip_publish_exact_baselines_in_real_windows_store() {
    for baseline in [230_000, 247_123] {
        let root = TestRoot::provision_fixture().unwrap();
        let mut store = Store::open(&root).unwrap();
        let initial = store.scan().active.clone();
        let mut driver = Driver::start(baseline, FakeBehavior::Normal, initial, |next| {
            store
                .publish(next)
                .expect("actual flush/replace/readback with retained lineage");
        });
        driver.capture_seal_arm();
        driver.call(Operation::CandidateOnce, 501);
        driver.call(Operation::Current, 502);
        driver.recover();
        driver.close();
        assert!(driver.actor.guardian().evidence().observed_restoration);
        assert!(driver.actor.guardian().evidence().publication_verified);
        let expected = driver.actor.published().clone();
        let writes = driver.publications;
        drop(driver);
        assert_eq!(store.scan().active, expected);
        assert_eq!(store.scan().active.sequence(), writes as u64);
        drop(store);
        let cold = Store::open(&root).unwrap();
        assert_eq!(cold.scan().active, expected);
        assert!(
            cold.reconciliation_only(),
            "even clean data cannot recreate old authority"
        );
        let JournalPayload::Snapshot(s) = &cold.scan().active.payload else {
            panic!()
        };
        assert_eq!(s.sessions[0].state, StoredState::RestoredVerified);
        assert_eq!(
            s.sessions[0].manifest.as_ref().unwrap().capture.readings_mw,
            [baseline; 2]
        );
        assert_eq!(
            s.sessions[0].manifest.as_ref().unwrap().capture.default_mw,
            Some(215_000)
        );
        assert_eq!(
            s.sessions[0]
                .calls
                .iter()
                .filter(|c| c.permit.operation == Operation::CandidateOnce)
                .count(),
            1
        );
        assert_eq!(
            s.sessions[0]
                .calls
                .iter()
                .filter(|c| c.permit.operation == Operation::RestoreBaseline)
                .count(),
            1
        );
    }
}

#[test]
fn cancellation_and_error_after_fake_change_remain_in_actual_terminal_journal() {
    for behavior in [FakeBehavior::Normal, FakeBehavior::ErrorAfterChange] {
        let root = TestRoot::provision_fixture().unwrap();
        let mut store = Store::open(&root).unwrap();
        let initial = store.scan().active.clone();
        let mut driver = Driver::start(247_123, behavior, initial, |next| {
            store.publish(next).unwrap();
        });
        driver.capture_seal_arm();
        driver.call(Operation::CandidateOnce, 501);
        if behavior == FakeBehavior::Normal {
            let effects = driver.actor.fault(Fault::Cancel, 501, 501).unwrap();
            driver.effects(effects);
        }
        driver.recover();
        driver.close();
        let evidence = driver.actor.guardian().evidence();
        assert!(evidence.observed_restoration && !evidence.faults.is_empty());
        if behavior == FakeBehavior::ErrorAfterChange {
            assert_eq!(evidence.candidate_api, Some(-7));
        }
        drop(driver);
        let JournalPayload::Snapshot(s) = &store.scan().active.payload else {
            panic!()
        };
        assert_eq!(s.sessions[0].state, StoredState::RestoredVerified);
        assert!(!s.sessions[0].notices.is_empty());
        assert!(s.sessions[0].calls.iter().any(|c| matches!(
            &c.completion,
            Some(StoredCompletion {
                outcome: Completion::Returned {
                    result: RawResult::Api { .. },
                    ..
                },
                ..
            })
        )));
    }
}

#[test]
fn actual_store_keeps_unknown_call_after_policy_worker_loss_without_restore_race() {
    let root = TestRoot::provision_fixture().unwrap();
    let mut store = Store::open(&root).unwrap();
    let initial = store.scan().active.clone();
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, initial, |next| {
        store.publish(next).unwrap();
    });
    driver.capture_seal_arm();
    let (permit, entry) = driver.dispatch(Operation::CandidateOnce, 501);
    driver.worker.fake_call(&entry).unwrap();
    let effects = driver.actor.fault(Fault::WorkerLost, 502, 502).unwrap();
    driver.now = 502;
    driver.effects(effects);
    assert_eq!(driver.actor.guardian().pending(), Some(&permit));
    assert!(driver
        .actor
        .reserve(Operation::RestoreBaseline, 503, 503)
        .is_err());
    drop(driver);
    drop(store);
    let cold = Store::open(&root).unwrap();
    assert!(cold.reconciliation_only());
    let JournalPayload::Snapshot(s) = &cold.scan().active.payload else {
        panic!()
    };
    assert_eq!(s.sessions[0].state, StoredState::RecoveryRequired);
    assert!(s.sessions[0].calls.last().unwrap().completion.is_none());
    assert!(s.sessions[0]
        .calls
        .iter()
        .all(|c| c.permit.operation != Operation::RestoreBaseline));
}

#[test]
fn failure_after_real_replace_keeps_cleanup_in_ram_and_cold_store_unresolved() {
    use std::{cell::RefCell, rc::Rc};
    let root = TestRoot::provision_fixture().unwrap();
    let store = Rc::new(RefCell::new(Store::open(&root).unwrap()));
    let initial = store.borrow().scan().active.clone();
    let writer = Rc::clone(&store);
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, initial, move |next| {
        writer.borrow_mut().publish(next).unwrap();
    });
    driver.capture_seal_arm();
    let (permit, entry) = driver.dispatch(Operation::CandidateOnce, 501);
    let result = driver.worker.fake_call(&entry).unwrap();
    let outcome = driver.worker.returned(entry, 501, 501, result).unwrap();
    let effects = driver.actor.completed(&permit, outcome, 501, 501).unwrap();
    let envelope = effects
        .iter()
        .find_map(|e| match e {
            autotune_recovery_core::journal::JournalEffect::Publish(e) => Some(e.as_ref().clone()),
            _ => None,
        })
        .unwrap();
    let failure = store.borrow_mut().publish_with(envelope, |step| {
        if step == PublicationStep::Replaced {
            Err(StorageError::Invalid("injected post-replace failure"))
        } else {
            Ok(())
        }
    });
    assert!(failure.is_err());
    let effects = driver.actor.storage_failure(501, 501).unwrap();
    driver.effects(effects);
    driver
        .worker
        .storage_failed(driver.actor.guardian().recovery_window().unwrap())
        .unwrap();
    let JournalPayload::Snapshot(snapshot) = &driver.actor.working().payload else {
        panic!()
    };
    let receipt = snapshot.sessions[0]
        .calls
        .last()
        .unwrap()
        .completion
        .as_ref()
        .unwrap();
    driver
        .worker
        .recorded(&ResultReceipt {
            operation_id: permit.operation_id,
            sequence: receipt.sequence,
            received_utc_ms: receipt.received_utc_ms,
            publication: None,
        })
        .unwrap();
    let successful_publications = driver.publications;
    driver.recover();
    assert!(driver.actor.guardian().evidence().observed_restoration);
    assert!(!driver.actor.guardian().evidence().publication_verified);
    assert_eq!(
        driver.publications, successful_publications,
        "the poisoned publisher is not replaced"
    );
    assert!(driver.actor.close().is_err());
    drop(driver);
    drop(store);
    let cold = Store::open(&root).unwrap();
    assert!(cold.reconciliation_only() && cold.scan().active.read_only_required());
    let JournalPayload::Snapshot(s) = &cold.scan().active.payload else {
        panic!()
    };
    assert!(
        s.sessions[0]
            .calls
            .iter()
            .all(|c| c.permit.operation != Operation::RestoreBaseline),
        "RAM cleanup must not be misrepresented as a durable restore record"
    );
}
