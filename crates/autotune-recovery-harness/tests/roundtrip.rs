//! End-to-end fake round trips through real Windows processes, pipes, storage
//! and the independent worker policy. Device state is synthetic; no hardware.
#![cfg(windows)]
use autotune_recovery_contract::{
    decode, process::RoundTripReport, storage::*, Completion, Operation, FRAME_LIMIT,
};
use autotune_recovery_windows::storage::Store;
use std::{
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn run(case: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_autotune-recovery-harness"))
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
    decode(line, FRAME_LIMIT).expect("strict round-trip report")
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
    let cold =
        Store::open(std::path::Path::new(&report.fixture_root)).expect("cold read-only open");
    assert!(cold.reconciliation_only());
    let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload else {
        panic!("terminal snapshot")
    };
    let session = &snapshot.sessions[0];
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
