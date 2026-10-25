mod common;

#[test]
fn bare_help_and_invalid_live_backend_role_flags_do_not_start_workers() {
    let output = common::command().output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Arming is blocked"));
    for arg in ["--live", "--backend", "--worker", "--guardian", "--replay"] {
        assert_eq!(
            common::command().arg(arg).output().unwrap().status.code(),
            Some(64)
        );
    }
}

#[test]
fn platform_check_reports_blocker_not_success() {
    let output = common::command().arg("--platform-check").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("P1_acceptance=blocked"));
    assert!(text.contains("P3=not_tested"));
}
