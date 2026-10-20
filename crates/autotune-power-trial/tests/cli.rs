use autotune_power_trial::mock::SCENARIOS;
use std::process::Command;

fn invoke(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_autotune-power-trial"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn bare_and_help_are_nonmutating_and_live_options_are_rejected() {
    for args in [&[][..], &["--help"][..], &["-h"][..]] {
        let r = invoke(args);
        assert!(r.status.success());
        assert!(String::from_utf8(r.stdout)
            .unwrap()
            .contains("SIMULATION ONLY"));
    }
    for args in [
        &["--live"][..],
        &["--yes"][..],
        &["--mock", "unknown"][..],
        &["--mock", "success", "--live"][..],
        &["--recover"][..],
    ] {
        let r = invoke(args);
        assert_eq!(r.status.code(), Some(64));
        assert!(r.stdout.is_empty());
    }
}

#[test]
fn every_fixture_has_identical_json_trace_and_exit_on_two_runs() {
    for scenario in SCENARIOS {
        let a = invoke(&["--mock", scenario]);
        let b = invoke(&["--mock", scenario]);
        assert_eq!(a.stdout, b.stdout, "{scenario}");
        assert_eq!(a.status.code(), b.status.code());
        assert!(a.stderr.is_empty() && b.stderr.is_empty());
        let r: serde_json::Value = serde_json::from_slice(&a.stdout).unwrap();
        assert_eq!(r["provenance"], "simulated");
        assert_eq!(
            a.status.code(),
            Some(if r["overall"] == "success" { 0 } else { 2 }),
            "{scenario}"
        );
    }
    assert_eq!(
        invoke(&["--mock"]).stdout,
        invoke(&["--mock", "success"]).stdout
    );
}
