//! Actual owned-process/pipe observations, never hardware acceptance.
#![cfg(windows)]

use autotune_recovery_contract::{
    decode,
    process::{ProcessCheckReport, RoleStatus},
    Role, FRAME_LIMIT,
};
use std::{
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn run(args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_autotune-recovery-harness"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch only the fixed fake harness");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().expect("query owned child").is_some() {
            return child.wait_with_output().expect("collect owned child");
        }
        if Instant::now() >= deadline {
            child.kill().expect("terminate only this test's child");
            child.wait().expect("reap test child");
            panic!("fake transport diagnostic exceeded its bounded observation window");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn json_line(output: &Output) -> &[u8] {
    output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout)
}

#[test]
fn measured_bootstrap_mutual_pipe_roundtrip_returns_no_device_authority() {
    let out = run(&["--fake-ipc-check"]);
    assert!(out.status.success(), "IPC check failed: {:?}", out.status);
    let report: RoleStatus = decode(json_line(&out), FRAME_LIMIT).expect("strict status");
    assert_eq!(report.role, Role::Verifier);
    assert!(report.process.process.pid > 0);
    assert!(report.process.process.creation_100ns > 0);
    assert_ne!(report.process.process.binary, [0; 32]);
    assert_eq!(report.candidate_entries, 0);
    assert_eq!(report.restore_entries, 0);
    assert!(report.pending.is_none());
    assert!(report.worker.is_none());
    assert!(report.coordinator.is_none());
}

#[test]
fn actual_pipe_hostile_frames_peers_and_names_fail_closed_without_reconnect() {
    for case in [
        "roundtrip",
        "squatter",
        "wrong-peer",
        "wrong-mac",
        "bad-mac",
        "replay",
        "gap",
        "wrong-role",
        "wrong-digest",
        "oversize",
        "truncated",
        "unknown-field",
    ] {
        let out = run(&["--fake-transport-case", case]);
        assert!(out.status.success(), "case {case}: {:?}", out.status);
        let report: ProcessCheckReport =
            decode(json_line(&out), FRAME_LIMIT).expect("strict process report");
        assert!(report.passed, "case {case}");
        assert!(
            report.blocker.is_none(),
            "case {case} is blocked, not passed"
        );
        assert!(report.zero_device_permissions, "case {case}");
        if case != "roundtrip" {
            assert!(
                report.rejection.is_some(),
                "case {case} must observe rejection"
            );
        }
    }
}

#[test]
fn actual_owned_job_close_kills_member_but_preserves_authenticated_sibling() {
    let out = run(&["--fake-transport-case", "job-close"]);
    assert!(
        out.status.success(),
        "owned-job case failed: {:?}",
        out.status
    );
    let report: ProcessCheckReport = decode(json_line(&out), FRAME_LIMIT).unwrap();
    assert!(report.passed);
    assert!(report.blocker.is_none());
    assert!(report.zero_device_permissions);
    let child = report.child.expect("measured job member");
    let sibling = report.sibling.expect("measured out-of-job sibling");
    assert_ne!(child.process.pid, sibling.process.pid);
    assert_eq!(child.process.binary, sibling.process.binary);
}

#[test]
fn unsupported_enclosing_job_or_console_reports_blocker_not_false_acceptance() {
    for case in ["isolation", "console-close"] {
        let out = run(&["--fake-transport-case", case]);
        let report: ProcessCheckReport = decode(json_line(&out), FRAME_LIMIT).unwrap();
        assert!(report.zero_device_permissions);
        if report.blocker.is_some() {
            assert_eq!(out.status.code(), Some(2));
        } else {
            assert!(out.status.success());
            assert!(report.passed);
        }
        // This is a diagnostic outcome-contract check. A blocker is deliberately
        // NOT counted as passing guardian/worker-survival acceptance (W01/W02).
    }
}

#[test]
fn guardian_survival_or_enclosing_job_rejection_is_explicit_without_mutation() {
    let gate = run(&["--fake-transport-case", "isolation"]);
    let gate_report: ProcessCheckReport = decode(json_line(&gate), FRAME_LIMIT).unwrap();
    if gate_report.blocker.is_some() {
        assert!(gate_report.host_in_job);
        assert!(gate_report
            .host_job_flags
            .is_some_and(|flags| flags & 0x1800 == 0));
        assert!(gate_report.passed && gate_report.zero_device_permissions);
        assert_eq!(gate.status.code(), Some(2));
        let denied = run(&["--fake-process-check", "observe"]);
        assert_eq!(denied.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&denied.stderr)
            .contains("Context(\"suspended detached guardian launch\", Win32(5))"));
        // Cargo's enclosing job commonly forbids breakaway. This branch proves
        // only rejection, not survival. Run this exact test executable outside
        // that job to exercise all lifecycle actions below; never relax the gate.
        return;
    }
    assert!(gate.status.success() && gate_report.passed);
    let console = run(&["--fake-transport-case", "console-close"]);
    let console_report: ProcessCheckReport = decode(json_line(&console), FRAME_LIMIT).unwrap();
    let mut actions = vec![
        "observe",
        "kill-coordinator",
        "close-job",
        "expired-getter",
        "deny-candidate",
    ];
    if console_report.blocker.is_none() {
        assert!(console.status.success() && console_report.passed);
        actions.push("close-console");
    } else {
        assert_eq!(console.status.code(), Some(2));
        assert!(console_report.zero_device_permissions && !console_report.passed);
        eprintln!(
            "W01 console-closure acceptance remains BLOCKED on this host: {}",
            console_report.blocker.as_deref().unwrap()
        );
    }
    for action in actions {
        let out = run(&["--fake-process-check", action]);
        assert!(
            out.status.success(),
            "guardian diagnostic {action}: {:?}; {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let reports: Vec<RoleStatus> = out
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| decode(line, FRAME_LIMIT).expect("canonical measured guardian report"))
            .collect();
        assert_eq!(
            reports.len(),
            2,
            "readiness and post-action report for {action}"
        );
        let ready = &reports[0];
        let final_report = &reports[1];
        assert_eq!(ready.role, Role::Guardian);
        assert_eq!(
            ready.process, final_report.process,
            "guardian replaced in {action}"
        );
        assert_eq!(
            ready.worker, final_report.worker,
            "worker replaced in {action}"
        );
        assert!(ready.worker.is_some());
        assert!(ready.coordinator.is_some());
        for report in &reports {
            assert!(
                report.no_job && report.no_console,
                "guardian independence in {action}"
            );
            assert_eq!(
                report.candidate_entries, 0,
                "diagnostic cannot authorize mutation"
            );
            assert_eq!(
                report.restore_entries, 0,
                "diagnostic cannot authorize restoration"
            );
            assert!(report.pending.is_none());
        }
        // This proves the read-only lifecycle/entry-rejection slice only. No
        // candidate is run, and no fake or physical restoration is established.
    }
}
