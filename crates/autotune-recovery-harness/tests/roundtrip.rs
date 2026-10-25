//! End-to-end fake round trips through real Windows processes, pipes, storage
//! and the independent worker policy. Device state is synthetic; no hardware.
#![cfg(windows)]
mod common;
use autotune_recovery_contract::{
    decode, process::RoundTripReport, storage::*, Completion, Operation, FRAME_LIMIT,
};
use autotune_recovery_windows::storage::Store;
use std::{
    process::{Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn run(case: &str) -> Output {
    let mut child = common::command()
        .args(["--fake-roundtrip", case])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fixed fake harness only");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().expect("owned child").is_some() {
            return child.wait_with_output().expect("collect");
        }
        if Instant::now() >= deadline {
            child.kill().expect("terminate only this test child");
            child.wait().expect("reap");
            panic!("round trip {case} exceeded its bounded observation window");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn report(output: &Output) -> RoundTripReport {
    let line = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
    decode(line, FRAME_LIMIT).unwrap_or_else(|error| {
        panic!(
            "strict round-trip report: {error:?}; stdout_bytes={}; stderr={}",
            output.stdout.len(),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_common(report: &RoundTripReport) {
    assert_eq!(report.baseline_mw, 247_123);
    assert_eq!(report.target_mw, 242_123);
    assert_eq!(report.delta_mw, 5_000);
    assert_ne!(report.guardian.process.pid, report.worker.process.pid);
    assert_ne!(report.guardian.process.pid, report.coordinator.process.pid);
    assert_ne!(report.worker.process.pid, report.coordinator.process.pid);
    assert_eq!(report.guardian.process.binary, report.worker.process.binary);
    assert_eq!(
        report.guardian.process.binary,
        report.coordinator.process.binary
    );
    assert_eq!(report.restore_entries, 1);
    assert_eq!(report.restore_api, vec![0]);
    assert!(report.observed_restored);
    assert!(report.publication_verified);
    assert_eq!(report.final_state, StoredState::RestoredVerified);
    assert_eq!(report.cold_state, StoredState::RestoredVerified);
    assert!(report.cold_read_only);
    assert!(report.pending.is_none());
    assert_eq!(report.baseline_reads.len(), 2);
    assert_eq!(report.baseline_reads[0].1, report.baseline_mw);
    assert_eq!(report.baseline_reads[1].1, report.baseline_mw);
    assert!(report.baseline_reads[1].0 >= report.baseline_reads[0].0 + 250);
    assert_eq!(report.witness_current_mw, report.baseline_mw);
    assert!(!report.witness_calls.is_empty());
    for pair in report.witness_calls.windows(2) {
        assert!(pair[0].effect_ms.unwrap() <= pair[1].invocation.entry_ms);
        assert!(pair[0].invocation.permit.operation_id < pair[1].invocation.permit.operation_id);
    }
    assert!(report
        .witness_calls
        .iter()
        .filter(|c| c.invocation.permit.operation == Operation::RestoreBaseline)
        .all(|c| c.invocation.write_mw == Some(report.baseline_mw)));
    let cold =
        Store::open(std::path::Path::new(&report.fixture_root)).expect("cold read-only open");
    assert!(cold.reconciliation_only());
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!("terminal snapshot")
    };
    let session = &snapshot.sessions[0];
    for observed in &report.witness_calls {
        let call = session
            .calls
            .iter()
            .find(|c| c.permit == observed.invocation.permit)
            .expect("every entry has its exact journaled intent");
        let Some(StoredCompletion {
            outcome:
                Completion::Returned {
                    entry_ms,
                    return_ms,
                    result,
                },
            sequence,
            ..
        }) = &call.completion
        else {
            panic!("witness is not substituted for worker completion")
        };
        assert!(call.intent_sequence < *sequence);
        assert_eq!(*entry_ms, observed.invocation.entry_ms);
        assert!(*return_ms >= observed.effect_ms.unwrap());
        assert_eq!(Some(result), observed.result.as_ref());
    }
    assert_eq!(session.state, StoredState::RestoredVerified);
    assert!(session.calls.iter().all(|call| call.completion.is_some()));
    assert!(
        session
            .calls
            .iter()
            .filter(|call| call.permit.operation == Operation::CandidateOnce)
            .count()
            <= 1
    );
    assert_eq!(
        session
            .calls
            .iter()
            .filter(|call| call.permit.operation == Operation::RestoreBaseline)
            .count(),
        1
    );
    assert_eq!(
        session.manifest.as_ref().unwrap().capture.readings_mw,
        [report.baseline_mw; 2]
    );
    assert_eq!(
        session.manifest.as_ref().unwrap().capture.default_mw,
        Some(215_000),
        "default is context only, never the restoration target"
    );
}

#[test]
fn enclosing_cargo_job_rejects_guardian_launch_with_zero_device_authority() {
    let out = run("normal");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("suspended detached guardian launch") && stderr.contains("Win32(5)"),
        "unexpected rejection: {stderr}"
    );
    assert!(out.stdout.is_empty(), "no device report may be fabricated");
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn real_process_roundtrip_restores_fresh_captured_baseline_and_cold_scans_read_only() {
    let out = run("normal");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_common(&report);
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.candidate_api, Some(0));
    assert_eq!(report.candidate_readback, Some(report.target_mw));
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn error_after_fake_change_skips_optional_readback_but_still_restores_and_verifies() {
    let out = run("error-after-change");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_common(&report);
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.candidate_api, Some(-7));
    assert_eq!(report.candidate_readback, None);
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn missing_candidate_api_executes_no_setter_and_still_restores_conservatively() {
    let out = run("missing-candidate");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_common(&report);
    assert_eq!(report.candidate_entries, 0, "no setter entry is claimed");
    assert_eq!(report.candidate_api, None);
    assert_eq!(report.candidate_readback, None);
    let cold = Store::open(std::path::Path::new(&report.fixture_root)).unwrap();
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!()
    };
    assert!(snapshot.sessions[0]
        .calls
        .iter()
        .filter(|call| call.permit.operation == Operation::CandidateOnce)
        .all(|call| matches!(
            call.completion,
            Some(StoredCompletion {
                outcome: Completion::NotEntered { .. },
                ..
            })
        )));
    assert!(snapshot.sessions[0]
        .calls
        .iter()
        .any(|call| call.permit.operation == Operation::RestoreBaseline));
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn cancellation_after_candidate_delivers_revoke_and_restores_without_readback() {
    let out = run("cancel-after-candidate");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_common(&report);
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.candidate_api, Some(0));
    assert_eq!(report.candidate_readback, None);
    let cold = Store::open(std::path::Path::new(&report.fixture_root)).unwrap();
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!()
    };
    assert!(snapshot.sessions[0]
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::Cancel));
    assert_eq!(report.recovery, snapshot.sessions[0].recovery);
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn post_candidate_publisher_failure_delivers_ram_only_cleanup_and_blocks_cold_start() {
    let out = run("storage-failure-after-candidate");
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.candidate_api, Some(0));
    assert_eq!(report.candidate_readback, None);
    assert_eq!(report.restore_entries, 1);
    assert_eq!(report.restore_api, vec![0]);
    assert!(report.observed_restored);
    assert!(!report.publication_verified);
    assert_eq!(report.final_state, StoredState::RecoveryRequired);
    assert_ne!(report.cold_state, StoredState::RestoredVerified);
    assert!(report.cold_read_only);
    assert!(report.pending.is_none());
    assert_eq!(report.baseline_reads.len(), 2);
    assert!(report
        .baseline_reads
        .iter()
        .all(|(_, value)| *value == report.baseline_mw));
    assert!(report.baseline_reads[1].0 >= report.baseline_reads[0].0 + 250);
    let cold = Store::open(std::path::Path::new(&report.fixture_root)).unwrap();
    assert!(cold.reconciliation_only());
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!()
    };
    assert!(snapshot.sessions[0].nonce_consumed);
    assert_eq!(report.recovery, snapshot.sessions[0].recovery);
    assert!(
        snapshot.sessions[0]
            .calls
            .iter()
            .all(|call| call.permit.operation != Operation::RestoreBaseline),
        "RAM cleanup must not invent disk evidence"
    );
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn worker_killed_at_candidate_entry_retains_late_effect_and_pending_without_restore() {
    let out = run("worker-killed-during-candidate");
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_eq!(report.final_state, StoredState::RecoveryRequired);
    assert!(!report.observed_restored);
    assert!(!report.publication_verified);
    assert_eq!(report.restore_entries, 0);
    assert!(report.restore_api.is_empty());
    assert!(report.baseline_reads.is_empty());
    assert!(
        report.candidate_api.is_none(),
        "oracle result is not worker completion"
    );
    let pending = report
        .pending
        .as_ref()
        .expect("uncertain dispatched candidate");
    assert_eq!(pending.operation, Operation::CandidateOnce);
    let late = report
        .witness_calls
        .last()
        .expect("external witness survives worker");
    assert_eq!(&late.invocation.permit, pending);
    assert!(late.worker_lost_before_effect);
    assert!(late.effect_ms.unwrap() >= late.invocation.entry_ms);
    assert_eq!(report.witness_current_mw, report.target_mw);
    assert_eq!(late.invocation.write_mw, Some(report.target_mw));
    assert_eq!(
        report
            .witness_calls
            .iter()
            .filter(|c| c.invocation.permit.operation == Operation::CandidateOnce)
            .count(),
        1
    );
    assert!(report
        .witness_calls
        .iter()
        .all(|c| c.invocation.permit.operation != Operation::RestoreBaseline));
    assert!(report.cold_read_only);
    let cold = Store::open(std::path::Path::new(&report.fixture_root)).unwrap();
    assert!(cold.reconciliation_only());
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!()
    };
    let call = snapshot.sessions[0].calls.last().unwrap();
    assert_eq!(&call.permit, pending);
    assert!(
        call.completion.is_none(),
        "late fake effect cannot manufacture idle"
    );
    assert!(snapshot.sessions[0].nonce_consumed);
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn unrelated_fresh_baseline_is_restored_by_external_witness_not_default_or_history() {
    let out = run("alternate-baseline");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_eq!(report.baseline_mw, 231_457);
    assert_eq!(report.target_mw, 226_457);
    assert_eq!(report.witness_current_mw, 231_457);
    assert!(report.observed_restored && report.publication_verified);
    assert_eq!(report.final_state, StoredState::RestoredVerified);
    assert_eq!(report.candidate_readback, Some(226_457));
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.restore_entries, 1);
    assert_eq!(report.baseline_reads.len(), 2);
    assert!(report.baseline_reads.iter().all(|(_, v)| *v == 231_457));
    assert!(report.baseline_reads[1].0 >= report.baseline_reads[0].0 + 250);
    let restore = report
        .witness_calls
        .iter()
        .find(|c| c.invocation.permit.operation == Operation::RestoreBaseline)
        .unwrap();
    assert_eq!(restore.invocation.write_mw, Some(231_457));
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn blocked_candidate_watchdog_keeps_lane_pending_until_same_worker_returns() {
    let out = run("blocked-candidate-returns");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_common(&report);
    assert_eq!(report.in_call_controls_acknowledged, 1);
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.candidate_readback, None);
    let candidate = report
        .witness_calls
        .iter()
        .find(|c| c.invocation.permit.operation == Operation::CandidateOnce)
        .unwrap();
    assert!(candidate.effect_ms.unwrap() >= candidate.invocation.permit.watchdog_ms);
    let recovery = report.recovery.as_ref().unwrap();
    assert!(recovery.trigger_ms >= candidate.invocation.permit.watchdog_ms);
    assert!(
        candidate.effect_ms.unwrap() >= recovery.trigger_ms,
        "guardian fault releases the effect barrier"
    );
    assert!(!candidate.worker_lost_before_effect);
    let cold = Store::open(std::path::Path::new(&report.fixture_root)).unwrap();
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!()
    };
    let session = &snapshot.sessions[0];
    assert!(session
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::Watchdog));
    let candidate = session
        .calls
        .iter()
        .find(|c| c.permit.operation == Operation::CandidateOnce)
        .unwrap();
    let Some(StoredCompletion {
        outcome: Completion::Returned { return_ms, .. },
        sequence,
        ..
    }) = &candidate.completion
    else {
        panic!()
    };
    assert!(*return_ms >= candidate.permit.watchdog_ms);
    let restore = session
        .calls
        .iter()
        .find(|c| c.permit.operation == Operation::RestoreBaseline)
        .unwrap();
    assert!(
        restore.intent_sequence > *sequence,
        "late effect alone cannot retire a permit"
    );
    assert!(restore.permit.dispatched_ms >= *return_ms);
    assert_eq!(recovery.deadline_ms, recovery.trigger_ms + 5000);
    assert!(report.baseline_reads[1].0 < recovery.deadline_ms);
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn blocked_candidate_real_heartbeat_loss_revokes_before_return_without_rebasing_cleanup() {
    let out = run("heartbeat-loss-during-blocked-candidate");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_common(&report);
    assert_eq!(report.in_call_controls_acknowledged, 1);
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.candidate_readback, None);
    let window = report.recovery.as_ref().unwrap();
    assert_eq!(window.deadline_ms, window.trigger_ms + 5000);
    let candidate = report
        .witness_calls
        .iter()
        .find(|c| c.invocation.permit.operation == Operation::CandidateOnce)
        .unwrap();
    assert!(candidate.effect_ms.unwrap() >= window.trigger_ms);
    assert!(report.baseline_reads[1].0 < window.deadline_ms);
    let cold = Store::open(std::path::Path::new(&report.fixture_root)).unwrap();
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!()
    };
    assert!(snapshot.sessions[0]
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::Heartbeat));
    assert_eq!(snapshot.sessions[0].recovery.as_ref(), Some(window));
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn blocked_candidate_storage_failure_notifies_in_call_then_serializes_ram_cleanup() {
    assert_blocked_storage_cleanup("storage-failure-during-blocked-candidate");
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn blocked_candidate_storage_stall_quarantines_late_publication_and_uses_ram_cleanup() {
    assert_blocked_storage_cleanup("storage-stall-during-blocked-candidate");
}

fn assert_blocked_storage_cleanup(case: &str) {
    let out = run(case);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_eq!(report.in_call_controls_acknowledged, 2);
    assert_eq!(report.candidate_entries, 1);
    assert_eq!(report.candidate_api, Some(0));
    assert_eq!(report.candidate_readback, None);
    assert_eq!(report.restore_entries, 1);
    assert_eq!(report.restore_api, vec![0]);
    assert!(report.observed_restored);
    assert!(!report.publication_verified);
    assert_eq!(report.final_state, StoredState::RecoveryRequired);
    assert!(report.pending.is_none());
    let window = report.recovery.as_ref().unwrap();
    assert_eq!(window.deadline_ms, window.trigger_ms + 5000);
    let candidate = report
        .witness_calls
        .iter()
        .find(|c| c.invocation.permit.operation == Operation::CandidateOnce)
        .unwrap();
    assert!(candidate.effect_ms.unwrap() >= candidate.invocation.permit.watchdog_ms);
    assert!(candidate.effect_ms.unwrap() >= window.trigger_ms);
    assert_eq!(candidate.invocation.write_mw, Some(report.target_mw));
    for pair in report.witness_calls.windows(2) {
        assert!(pair[0].effect_ms.unwrap() <= pair[1].invocation.entry_ms);
    }
    assert_eq!(report.witness_current_mw, report.baseline_mw);
    assert_eq!(report.baseline_reads.len(), 2);
    assert!(report
        .baseline_reads
        .iter()
        .all(|(_, value)| *value == report.baseline_mw));
    assert!(report.baseline_reads[1].0 >= report.baseline_reads[0].0 + 250);
    assert!(report.baseline_reads[1].0 < window.deadline_ms);
    let cold = Store::open(std::path::Path::new(&report.fixture_root)).unwrap();
    assert!(cold.reconciliation_only());
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!()
    };
    let session = &snapshot.sessions[0];
    assert!(session.nonce_consumed);
    let pending = session.calls.last().unwrap();
    assert_eq!(pending.permit, candidate.invocation.permit);
    assert!(
        pending.completion.is_none(),
        "RAM completion must not manufacture disk evidence"
    );
    assert!(!session
        .calls
        .iter()
        .any(|c| c.permit.operation == Operation::RestoreBaseline));
    assert_ne!(session.state, StoredState::RestoredVerified);
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn startup_owner_conflict_reports_original_rejection_before_readiness_not_broken_pipe() {
    let out = run("startup-owner-conflict");
    assert_eq!(out.status.code(), Some(2));
    let error = String::from_utf8_lossy(&out.stderr);
    // owner.lock is opened with NtCreateFile: STATUS_SHARING_VIOLATION,
    // not the corresponding Win32 ERROR_SHARING_VIOLATION (32).
    assert!(
        error.contains("RemoteRejection(\"Storage(Nt(-1073741757))\")"),
        "original exclusive-owner rejection must survive: {error}"
    );
    assert!(
        !error.contains("Win32(109)"),
        "EOF must not substitute for a delivered rejection"
    );
    assert!(
        out.stdout.is_empty(),
        "no readiness/device report may be invented"
    );
}

#[test]
#[ignore = "requires direct execution outside Cargo's breakaway-restricted job"]
fn missing_restore_reports_original_entry_guard_failure_not_broken_pipe() {
    let out = run("missing-restore");
    assert_eq!(out.status.code(), Some(2));
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("RemoteRejection")
            && error.contains("unexpected fake entry rejection during the round trip"),
        "original worker-policy guard must survive: {error}"
    );
    assert!(!error.contains("Win32(109)"));
    assert!(
        out.stdout.is_empty(),
        "rejection cannot fabricate successful recovery"
    );
}
