#[path = "../../autotune-recovery-contract/tests/support/mod.rs"]
mod support;
use autotune_recovery_contract::*;
use autotune_recovery_core::*;
use support::*;

fn ack(g: &mut Guardian, now: u64) -> Vec<Effect> {
    g.publication_ack(now, now, true).unwrap()
}
fn execute(g: &mut Guardian, op: Operation, result: RawResult, now: u64) -> Permission {
    assert!(matches!(
        g.reserve(op, now, now).unwrap().last(),
        Some(Effect::PublishIntent(_))
    ));
    let effects = ack(g, now);
    let p = match effects.last().unwrap() {
        Effect::Dispatch(p) => p.clone(),
        other => panic!("{other:?}"),
    };
    p.validate_entry(now, &[5; 32], true).unwrap();
    g.return_and_idle(
        &p,
        Completion::Returned {
            entry_ms: now,
            return_ms: now,
            result,
        },
        now,
        now,
    )
    .unwrap();
    ack(g, now);
    p
}
fn chain(g: &mut Guardian, baseline: u32, now: u64) {
    execute(
        g,
        Operation::Enumerate,
        RawResult::Identity(preparation().device),
        now,
    );
    execute(
        g,
        Operation::Identity,
        RawResult::Identity(preparation().device),
        now,
    );
    execute(g, Operation::Runtime, RawResult::Runtime(runtime()), now);
    execute(
        g,
        Operation::Bounds,
        RawResult::Bounds {
            minimum_mw: 100_000,
            maximum_mw: 300_000,
        },
        now,
    );
    execute(g, Operation::Current, RawResult::Value(baseline), now);
}
fn captured(b: u32) -> Guardian {
    let mut g = Guardian::new(preparation(), 0, 0).unwrap();
    g.transport_ready(true, true, true, true).unwrap();
    ack(&mut g, 0);
    chain(&mut g, b, 0);
    assert!(g.reserve(Operation::Current, 499, 499).is_err());
    execute(&mut g, Operation::Current, RawResult::Value(b), 500);
    let mut m = manifest(b);
    m.capture.trace = g.capture_trace_digest().unwrap();
    g.seal(m.clone(), m.digest().unwrap()).unwrap();
    g
}
fn armed(b: u32) -> Guardian {
    let mut g = captured(b);
    let a = approval(g.manifest().unwrap());
    g.approve(&a, &[]).unwrap();
    ack(&mut g, 500);
    chain(&mut g, b, 500);
    g
}
fn candidate_pending(g: &mut Guardian, now: u64) -> Permission {
    g.reserve(Operation::CandidateOnce, now, now).unwrap();
    ack(g, now)
        .into_iter()
        .find_map(|e| {
            if let Effect::Dispatch(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .unwrap()
}

#[test]
fn readiness_and_every_getter_require_publication() {
    let mut g = Guardian::new(preparation(), 0, 0).unwrap();
    assert!(g.reserve(Operation::Current, 0, 0).is_err());
    assert!(g.transport_ready(true, false, true, true).is_err());
    g.transport_ready(true, true, true, true).unwrap();
    assert!(g.reserve(Operation::Enumerate, 0, 0).is_err());
    ack(&mut g, 0);
    g.reserve(Operation::Enumerate, 0, 0).unwrap();
    assert!(g.reserve(Operation::Identity, 0, 0).is_err());
    assert_eq!(g.pending().unwrap().operation, Operation::Enumerate);
}

#[test]
fn normal_two_fresh_baselines_explicit_restore_and_separated_reads() {
    for b in [230_000, 247_123] {
        let mut g = armed(b);
        execute(
            &mut g,
            Operation::CandidateOnce,
            RawResult::Api { status: 0 },
            501,
        );
        execute(&mut g, Operation::Current, RawResult::Value(b - 5000), 502);
        assert_eq!(g.phase(), Phase::Recovering);
        chain(&mut g, b - 5000, 503);
        execute(
            &mut g,
            Operation::RestoreBaseline,
            RawResult::Api { status: 0 },
            504,
        );
        execute(&mut g, Operation::Current, RawResult::Value(b), 505);
        assert!(g.reserve(Operation::Current, 754, 754).is_err());
        execute(&mut g, Operation::Current, RawResult::Value(b), 755);
        assert_eq!(g.phase(), Phase::RestoredObserved);
        assert!(g.evidence().observed_restoration);
        assert!(!g.evidence().publication_verified);
        g.close().unwrap();
        ack(&mut g, 755);
        assert!(g.evidence().publication_verified);
        assert!(g.reserve(Operation::CandidateOnce, 755, 755).is_err());
    }
}

#[test]
fn timeout_cancel_worker_loss_never_clear_pending_or_replace_writer() {
    for fault in [
        Fault::Cancel,
        Fault::ParentLost,
        Fault::WorkerLost,
        Fault::Overflow,
    ] {
        let mut g = armed(247_123);
        let p = candidate_pending(&mut g, 501);
        g.fault(fault);
        let deadline = g.cleanup_deadline();
        g.fault(Fault::Cancel);
        assert_eq!(g.cleanup_deadline(), deadline);
        g.tick(1501, 1501).unwrap();
        assert_eq!(g.pending(), Some(&p));
        assert!(g.reserve(Operation::RestoreBaseline, 1501, 1501).is_err());
        assert!(g.reserve(Operation::Current, 1501, 1501).is_err());
    }
}

#[test]
fn old_generation_exact_atomic_return_retires_once_without_new_candidate() {
    let mut g = armed(247_123);
    let p = candidate_pending(&mut g, 501);
    g.fault(Fault::Cancel);
    assert!(g.generation() > p.generation);
    let outcome = Completion::Returned {
        entry_ms: 501,
        return_ms: 502,
        result: RawResult::Api { status: -1 },
    };
    g.return_and_idle(&p, outcome.clone(), 502, 502).unwrap();
    ack(&mut g, 502);
    assert!(g.return_and_idle(&p, outcome, 502, 502).is_err());
    assert!(g.reserve(Operation::CandidateOnce, 502, 502).is_err());
    assert_eq!(g.next_operation(), Some(Operation::Enumerate));
    assert_eq!(g.evidence().candidate_api, Some(-1));
}

#[test]
fn pre_mutation_storage_stall_stops_and_late_ack_cannot_reopen() {
    let mut g = Guardian::new(preparation(), 0, 0).unwrap();
    g.transport_ready(true, true, true, true).unwrap();
    g.tick(100, 100).unwrap();
    assert_eq!(g.phase(), Phase::Aborted);
    assert!(g.storage_failed());
    assert!(g.publication_ack(101, 101, true).unwrap().is_empty());
    assert!(g.reserve(Operation::Enumerate, 101, 101).is_err());
}

#[test]
fn post_mutation_stalled_writer_allows_only_ram_cleanup_no_durable_close() {
    let mut g = armed(247_123);
    let p = candidate_pending(&mut g, 501);
    g.return_and_idle(
        &p,
        Completion::Returned {
            entry_ms: 501,
            return_ms: 501,
            result: RawResult::Api { status: 0 },
        },
        501,
        501,
    )
    .unwrap();
    g.tick(601, 601).unwrap();
    assert!(g.storage_failed());
    assert_eq!(g.phase(), Phase::Recovering);
    assert!(matches!(
        g.reserve(Operation::Enumerate, 601, 601).unwrap().last(),
        Some(Effect::Dispatch(_))
    ));
    assert!(!g.evidence().publication_verified);
    assert!(g.close().is_err());
    assert!(g.publication_ack(602, 602, true).unwrap().is_empty());
}

#[test]
fn expiry_wins_ties_and_no_late_dispatch_from_intent_ack() {
    let mut g = armed(247_123);
    g.reserve(Operation::CandidateOnce, 501, 501).unwrap();
    g.fault(Fault::Cancel);
    assert!(!ack(&mut g, 502)
        .iter()
        .any(|e| matches!(e, Effect::Dispatch(_))));
    assert!(g.pending().is_none());
    assert!(g.reserve(Operation::CandidateOnce, 502, 502).is_err());
    let mut g = armed(247_123);
    g.heartbeat(1499, 1499).unwrap();
    g.tick(2500, 2500).unwrap();
    assert_ne!(g.phase(), Phase::Armed);
}

#[test]
fn lost_completion_and_cleanup_deadline_keep_uncertainty() {
    let mut g = armed(247_123);
    let p = candidate_pending(&mut g, 501);
    g.fault(Fault::Cancel);
    g.tick(5501, 5501).unwrap();
    assert_eq!(g.phase(), Phase::RecoveryRequired);
    assert_eq!(g.pending(), Some(&p));
    assert!(!g.evidence().observed_restoration);
}

#[test]
fn exact_approval_and_consumed_nonce_gate_arm() {
    let mut g = captured(247_123);
    let mut a = approval(g.manifest().unwrap());
    a.operator = "wrong".into();
    assert!(g.approve(&a, &[]).is_err());
    let a = approval(g.manifest().unwrap());
    assert!(g.approve(&a, &[preparation().nonce]).is_err());
    assert_eq!(g.phase(), Phase::Manifest);
}

#[test]
fn original_clock_anchor_detects_cumulative_drift_and_suspend() {
    let sample = ClockSample {
        interrupt_ms: 0,
        unbiased_ms: 0,
        utc_ms: 1000,
        uncertainty_ms: 1,
    };
    let mut clock = ClockGuard::new(sample).unwrap();
    for i in 1..=20 {
        clock
            .observe(ClockSample {
                interrupt_ms: i * 100,
                unbiased_ms: i * 100,
                utc_ms: 1000 + i * 101,
                uncertainty_ms: 1,
            })
            .unwrap();
    }
    assert!(clock
        .observe(ClockSample {
            interrupt_ms: 2100,
            unbiased_ms: 2100,
            utc_ms: 3121,
            uncertainty_ms: 1
        })
        .is_err());
    let mut clock = ClockGuard::new(sample).unwrap();
    assert!(clock
        .observe(ClockSample {
            interrupt_ms: 100,
            unbiased_ms: 97,
            utc_ms: 1100,
            uncertainty_ms: 1
        })
        .is_err());
}

fn recovering(b: u32, status: i32) -> Guardian {
    let mut g = armed(b);
    execute(
        &mut g,
        Operation::CandidateOnce,
        RawResult::Api { status },
        501,
    );
    if status == 0 {
        execute(&mut g, Operation::Current, RawResult::Value(b - 5000), 502);
    }
    chain(&mut g, b - 5000, 503);
    g
}

#[test]
fn failed_candidate_and_restore_api_results_do_not_hide_observed_baseline() {
    let b = 247_123;
    let mut g = recovering(b, -9);
    execute(
        &mut g,
        Operation::RestoreBaseline,
        RawResult::Api { status: -7 },
        504,
    );
    execute(&mut g, Operation::Current, RawResult::Value(b), 505);
    execute(&mut g, Operation::Current, RawResult::Value(b), 755);
    assert!(g.evidence().observed_restoration);
    assert_eq!(g.evidence().candidate_api, Some(-9));
    assert_eq!(g.evidence().restore_api, [-7]);
    assert_eq!(g.evidence().candidate_readback, None);
    assert!(g.evidence().calls.iter().all(|c| c.completion.is_some()));
}

#[test]
fn second_restore_requires_unsuccessful_verification_and_never_a_third() {
    let b = 247_123;
    let mut g = recovering(b, 0);
    execute(
        &mut g,
        Operation::RestoreBaseline,
        RawResult::Api { status: -7 },
        504,
    );
    assert!(g.reserve(Operation::RestoreBaseline, 504, 504).is_err());
    execute(&mut g, Operation::Current, RawResult::Value(b - 1), 505);
    chain(&mut g, b - 1, 506);
    execute(
        &mut g,
        Operation::RestoreBaseline,
        RawResult::Api { status: -8 },
        507,
    );
    execute(&mut g, Operation::Current, RawResult::Value(b - 1), 508);
    assert_eq!(g.phase(), Phase::RecoveryRequired);
    assert!(g.reserve(Operation::RestoreBaseline, 508, 508).is_err());
    assert_eq!(g.evidence().restore_api, [-7, -8]);
}

#[test]
fn cleanup_identity_drift_blocks_instead_of_retrying_or_blind_restoring() {
    let mut g = armed(247_123);
    execute(
        &mut g,
        Operation::CandidateOnce,
        RawResult::Api { status: -1 },
        501,
    );
    let mut other = preparation().device;
    other.uuid = "SYNTHETIC-replacement".into();
    execute(
        &mut g,
        Operation::Enumerate,
        RawResult::Identity(other),
        502,
    );
    assert_eq!(g.phase(), Phase::RecoveryRequired);
    assert_eq!(g.next_operation(), None);
    assert!(g.evidence().restore_api.is_empty());
}

#[test]
fn late_completion_after_budget_is_evidence_only_and_wrong_permit_never_retires() {
    let mut g = armed(247_123);
    let p = candidate_pending(&mut g, 501);
    g.fault(Fault::Cancel);
    let outcome = Completion::Returned {
        entry_ms: 501,
        return_ms: 5502,
        result: RawResult::Api { status: -17 },
    };
    let mut wrong = p.clone();
    wrong.operation_id += 1;
    assert!(g
        .return_and_idle(&wrong, outcome.clone(), 5502, 5502)
        .is_err());
    assert_eq!(g.pending(), Some(&p));
    g.return_and_idle(&p, outcome, 5502, 5502).unwrap();
    ack(&mut g, 5502);
    assert_eq!(g.phase(), Phase::RecoveryRequired);
    assert_eq!(g.evidence().candidate_api, Some(-17));
    assert!(g.pending().is_none());
    assert!(g.next_operation().is_none());
}

#[test]
fn cancel_before_capture_intent_ack_never_dispatches_late_getter() {
    let mut g = Guardian::new(preparation(), 0, 0).unwrap();
    g.transport_ready(true, true, true, true).unwrap();
    ack(&mut g, 0);
    g.reserve(Operation::Enumerate, 0, 0).unwrap();
    g.fault(Fault::Cancel);
    assert!(!ack(&mut g, 1)
        .iter()
        .any(|e| matches!(e, Effect::Dispatch(_))));
    assert!(g.pending().is_none());
    assert_eq!(g.phase(), Phase::Aborted);
}

#[test]
fn missed_normal_recovery_is_retained_after_watchdog_and_late_return() {
    let mut g = armed(247_123);
    let p = candidate_pending(&mut g, 501);
    g.tick(1501, 1501).unwrap();
    let frozen = g.cleanup_deadline();
    g.tick(3500, 3500).unwrap();
    assert!(g.evidence().normal_recovery_missed);
    assert_eq!(g.cleanup_deadline(), frozen);
    g.return_and_idle(
        &p,
        Completion::Returned {
            entry_ms: 501,
            return_ms: 3501,
            result: RawResult::Api { status: 0 },
        },
        3501,
        3501,
    )
    .unwrap();
    ack(&mut g, 3501);
    assert!(g.evidence().normal_recovery_missed);
    assert_eq!(g.cleanup_deadline(), frozen);
}
