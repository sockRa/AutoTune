#![cfg(windows)]
use autotune_benchmark_worker::process::*;
use std::time::{Duration, Instant};

#[test]
#[ignore = "owned Windows process acceptance; run explicitly and serially"]
fn real_owner_loss_retains_child_exit_but_cannot_query_lost_job() {
    let evidence = owner_loss_fixture().unwrap();
    assert_eq!(evidence.owner_wait_status, 0);
    assert_eq!(evidence.child_wait_status, 0);
    assert!(evidence.child_exit_code.is_some());
    assert_eq!(evidence.empty_job_observed, None);
}

fn frames(process: &mut FakeProcess, count: usize) -> Vec<Frame> {
    let start = Instant::now();
    let mut frames = vec![];
    while frames.len() < count && start.elapsed() < Duration::from_secs(2) {
        match process.receive() {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(error) => panic!("fixture status: {error}"),
        }
    }
    assert_eq!(frames.len(), count);
    frames
}
#[test]
#[ignore = "owned Windows process acceptance; run explicitly and serially"]
fn normal_and_cooperative_have_independent_exit_and_job_evidence() {
    for profile in [Profile::Normal, Profile::Cooperative] {
        let mut process = FakeProcess::launch(profile, [1; 16]).unwrap();
        assert_ne!(process.creation_time(), 0);
        let first = frames(&mut process, 1);
        assert_eq!(first[0].kind, 1);
        process.request_stop().unwrap();
        let last = frames(&mut process, 1);
        assert!(matches!(last[0].kind, 2 | 3));
        assert!(observe_until_exit(&process, Duration::from_secs(3)).exited_and_empty());
    }
}
#[test]
#[ignore = "owned Windows process acceptance; run explicitly and serially"]
fn done_and_termination_request_are_not_exit_proof() {
    let mut process = FakeProcess::launch(Profile::DoneThenHang, [2; 16]).unwrap();
    assert_eq!(frames(&mut process, 2)[1].kind, 2);
    let before = process.observe(0);
    println!(
        "owned job diagnostic: {:?}; {:?}",
        before,
        process.accounting_diagnostic()
    );
    assert!(!before.exited_and_empty());
    assert_eq!(before.active_processes, Some(1));
    process.request_termination().unwrap();
    assert!(observe_until_exit(&process, Duration::from_secs(3)).exited_and_empty());
}

#[test]
#[ignore = "owned Windows process acceptance; run explicitly and serially"]
fn eof_is_not_exit_and_never_becomes_success() {
    let mut process = FakeProcess::launch(Profile::EofThenHang, [4; 16]).unwrap();
    frames(&mut process, 1);
    let start = Instant::now();
    loop {
        if process.receive().is_err() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(process.receive().is_err());
    assert!(!process.observe(0).exited_and_empty());
    process.request_termination().unwrap();
    assert!(observe_until_exit(&process, Duration::from_secs(3)).exited_and_empty());
}
#[test]
#[ignore = "owned Windows process acceptance; run explicitly and serially"]
fn child_and_handle_inheritance_are_restricted() {
    for (profile, kind) in [(Profile::ChildAttempt, 4), (Profile::Inheritance, 5)] {
        let mut process = FakeProcess::launch(profile, [3; 16]).unwrap();
        let frames = frames(&mut process, 2);
        assert_eq!(frames[1].kind, kind);
        if kind == 5 {
            assert_eq!(frames[1].value, 0);
        } else {
            assert_eq!(
                frames[1].value,
                u64::from(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_QUOTA)
            );
        }
        assert!(observe_until_exit(&process, Duration::from_secs(3)).exited_and_empty());
    }
}
