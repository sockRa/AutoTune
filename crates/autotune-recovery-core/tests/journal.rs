#[path = "support/journal.rs"]
mod support;
use autotune_recovery_contract::{process::*, storage::*, *};
use autotune_recovery_core::{journal::*, *};
use support::*;

#[test]
fn journal_adapter_roundtrip_uses_exact_published_capture_arm_and_call_lineage() {
    for baseline in [230_000, 247_123] {
        let mut prior = anchor();
        let mut driver = Driver::start(baseline, FakeBehavior::Normal, prior.clone(), |next| {
            next.follows(&prior).unwrap();
            prior = next;
        });
        driver.capture_seal_arm();
        driver.call(Operation::CandidateOnce, 501);
        driver.call(Operation::Current, 502);
        driver.recover();
        driver.close();
        assert!(driver.actor.guardian().evidence().observed_restoration);
        assert!(driver.actor.guardian().evidence().publication_verified);
        assert_eq!(
            driver.actor.guardian().evidence().candidate_readback,
            Some(baseline - 5000)
        );
        assert_eq!(driver.actor.guardian().evidence().restore_api, vec![0]);
        assert_eq!(driver.actor.working(), driver.actor.published());
        let JournalPayload::Snapshot(snapshot) = &driver.actor.published().payload else {
            panic!()
        };
        assert_eq!(snapshot.sessions[0].state, StoredState::RestoredVerified);
        assert_eq!(
            snapshot.sessions[0]
                .manifest
                .as_ref()
                .unwrap()
                .capture
                .readings_mw,
            [baseline; 2]
        );
        assert!(snapshot.sessions[0]
            .calls
            .iter()
            .all(|c| c.completion.is_some()));
        assert!(
            JournaledGuardian::new(
                snapshot.sessions[0].preparation.clone(),
                driver.actor.published().clone(),
                800,
                800
            )
            .is_err(),
            "cold journal is not authority"
        );
    }
}

#[test]
fn journal_adapter_cancellation_and_api_error_keep_failure_after_restoration() {
    for behavior in [FakeBehavior::Normal, FakeBehavior::ErrorAfterChange] {
        let mut prior = anchor();
        let mut driver = Driver::start(247_123, behavior, prior.clone(), |next| {
            next.follows(&prior).unwrap();
            prior = next;
        });
        driver.capture_seal_arm();
        driver.call(Operation::CandidateOnce, 501);
        if behavior == FakeBehavior::Normal {
            let effects = driver.actor.fault(Fault::Cancel, 501, 501).unwrap();
            driver.effects(effects);
        }
        driver.recover();
        driver.close();
        assert!(driver.actor.guardian().evidence().observed_restoration);
        assert!(!driver.actor.guardian().evidence().faults.is_empty());
        if behavior == FakeBehavior::ErrorAfterChange {
            assert_eq!(driver.actor.guardian().evidence().candidate_api, Some(-7));
        }
        let JournalPayload::Snapshot(s) = &driver.actor.published().payload else {
            panic!()
        };
        assert!(!s.sessions[0].notices.is_empty());
    }
}

#[test]
fn lost_worker_keeps_exact_pending_intent_and_rejects_competing_restoration() {
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, anchor(), |_| {});
    driver.capture_seal_arm();
    let (permit, entry) = driver.dispatch(Operation::CandidateOnce, 501);
    driver.worker.fake_call(&entry).unwrap();
    // No ReturnAndIdle. A lost worker is not a completion or fencing receipt.
    let effects = driver.actor.fault(Fault::WorkerLost, 502, 502).unwrap();
    driver.now = 502;
    driver.effects(effects);
    assert_eq!(driver.actor.guardian().pending(), Some(&permit));
    assert_eq!(driver.actor.guardian().phase(), Phase::RecoveryRequired);
    assert!(driver
        .actor
        .reserve(Operation::RestoreBaseline, 503, 503)
        .is_err());
    assert!(driver.actor.close().is_err());
    let JournalPayload::Snapshot(s) = &driver.actor.published().payload else {
        panic!()
    };
    assert_eq!(s.sessions[0].state, StoredState::RecoveryRequired);
    assert!(s.sessions[0].calls.last().unwrap().completion.is_none());
}

#[test]
fn post_mutation_storage_failure_allows_ram_cleanup_but_never_another_publisher() {
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, anchor(), |_| {});
    driver.capture_seal_arm();
    let (permit, entry) = driver.dispatch(Operation::CandidateOnce, 501);
    let result = driver.worker.fake_call(&entry).unwrap();
    let effects = driver.actor.storage_failure(501, 501).unwrap();
    driver.effects(effects);
    driver
        .worker
        .storage_failed(driver.actor.guardian().recovery_window().unwrap())
        .unwrap();
    let publications = driver.publications;
    driver.finish(permit, entry, result);
    driver.recover();
    assert_eq!(driver.publications, publications);
    assert!(driver.actor.guardian().evidence().observed_restoration);
    assert!(!driver.actor.guardian().evidence().publication_verified);
    assert!(driver.actor.close().is_err());
    assert!(driver.actor.published().read_only_required());
    assert!(driver
        .actor
        .acknowledge(driver.actor.working().checksum, 800, 800)
        .is_err());
}

#[test]
fn publication_receipts_are_checksum_bound_and_timeouts_cannot_reopen_admission() {
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, anchor(), |_| {});
    let effects = driver.actor.reserve(Operation::Enumerate, 0, 0).unwrap();
    let JournalEffect::Publish(envelope) = &effects[0] else {
        panic!()
    };
    let checksum = envelope.checksum;
    assert!(driver.actor.acknowledge([0; 32], 0, 0).is_err());
    let effects = driver.actor.tick(100, 100).unwrap();
    assert!(effects
        .iter()
        .all(|e| !matches!(e, JournalEffect::Dispatch(_) | JournalEffect::Publish(_))));
    assert!(driver.actor.publication_failed());
    assert!(driver.actor.acknowledge(checksum, 101, 101).is_err());
    assert!(driver
        .actor
        .reserve(Operation::Enumerate, 101, 101)
        .is_err());
}

#[test]
fn delayed_arm_acknowledgement_cannot_rebase_the_committed_admission_deadline() {
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, anchor(), |_| {});
    driver.capture_seal_only();
    let m = driver.manifest.as_ref().unwrap().clone();
    let approval = SimulatedApproval {
        manifest: m.digest().unwrap(),
        nonce: m.preparation.nonce,
        challenge: m.approval_challenge,
        operator: m.preparation.operator.clone(),
        full_and_summary_presented: true,
    };
    let effects = driver.actor.approve(approval.clone(), 500, 500).unwrap();
    let JournalEffect::Publish(consent) = &effects[0] else {
        panic!()
    };
    let effects = driver
        .actor
        .acknowledge(consent.checksum, 520, 520)
        .unwrap();
    let JournalEffect::Publish(arm) = &effects[0] else {
        panic!()
    };
    let JournalPayload::Snapshot(snapshot) = &arm.payload else {
        panic!()
    };
    let commit = snapshot.sessions[0].arm_commit.clone().unwrap();
    assert_eq!(commit.boot_ms, 520);
    driver.actor.acknowledge(arm.checksum, 550, 550).unwrap();
    driver
        .worker
        .arm(
            &ArmCapsule {
                approval,
                commit,
                nonce_publication: arm.checksum,
            },
            550,
            550,
            true,
        )
        .unwrap();
    driver.actor.heartbeat(1000, 1000).unwrap();
    driver.chain(1500);
    driver.now = 1600;
    let effects = driver
        .actor
        .reserve(Operation::CandidateOnce, 1600, 1600)
        .unwrap();
    driver.effects(effects);
    assert_eq!(
        driver.last_dispatch.unwrap().permit.expiry_ms,
        2520,
        "commit+2s, never acknowledgement+2s"
    );
}

#[test]
fn cancel_before_intent_ack_publishes_exact_unused_permit_retirement_without_dispatch() {
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, anchor(), |_| {});
    let effects = driver.actor.reserve(Operation::Enumerate, 0, 0).unwrap();
    let JournalEffect::Publish(intent) = &effects[0] else {
        panic!()
    };
    let effects = driver.actor.fault(Fault::Cancel, 1, 1).unwrap();
    assert!(effects
        .iter()
        .all(|e| !matches!(e, JournalEffect::Dispatch(_))));
    driver.now = 2;
    let effects = driver.actor.acknowledge(intent.checksum, 2, 2).unwrap();
    driver.effects(effects);
    assert!(driver.last_dispatch.is_none());
    assert_eq!(driver.actor.guardian().phase(), Phase::Aborted);
    let JournalPayload::Snapshot(s) = &driver.actor.published().payload else {
        panic!()
    };
    assert_eq!(s.sessions[0].state, StoredState::AbortedNoWrite);
    assert!(matches!(
        &s.sessions[0].calls[0].completion,
        Some(StoredCompletion {
            outcome: Completion::NotEntered { .. },
            ..
        })
    ));
    assert!(s.sessions[0]
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::Cancel));
}

#[test]
fn a_rejected_clock_regression_permanently_closes_admission() {
    let mut driver = Driver::start(247_123, FakeBehavior::Normal, anchor(), |_| {});
    driver.actor.tick(10, 10).unwrap();
    assert!(driver.actor.tick(9, 9).is_err());
    assert!(driver
        .actor
        .guardian()
        .evidence()
        .faults
        .contains(&Fault::Clock));
    assert_eq!(driver.actor.guardian().phase(), Phase::Aborted);
    assert!(driver.actor.reserve(Operation::Enumerate, 11, 11).is_err());
}
