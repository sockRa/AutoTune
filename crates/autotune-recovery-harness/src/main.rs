//! Fail-closed entry point. No live mode, arbitrary executable, or recovery replay.
#![forbid(unsafe_code)]
use std::process::ExitCode;

fn main() -> ExitCode {
    if let Ok(base) = std::env::var("AUTOTUNE_RECOVERY_DIAG") {
        let path = format!("{base}.{}.panic", std::process::id());
        std::panic::set_hook(Box::new(move |info| {
            let _ = std::fs::write(&path, format!("{info}\n"));
        }));
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if args.first().is_some_and(|s| s == "--fake-transport-case") && args.len() == 2 {
        use autotune_recovery_contract::process::{ProcessCase as C, WireFault as F};
        let case = match args[1].as_str() {
            "roundtrip" => C::RoundTrip,
            "squatter" => C::PipeSquatter,
            "wrong-peer" => C::WrongPeer,
            "wrong-mac" => C::WrongMac,
            "bad-mac" => C::Wire(F::BadMac),
            "replay" => C::Wire(F::Replay),
            "gap" => C::Wire(F::SequenceGap),
            "wrong-role" => C::Wire(F::WrongRole),
            "wrong-digest" => C::Wire(F::WrongDigest),
            "oversize" => C::Wire(F::Oversize),
            "truncated" => C::Wire(F::Truncated),
            "unknown-field" => C::Wire(F::UnknownField),
            "job-close" => C::JobClose,
            "console-close" => C::ConsoleClose,
            "isolation" => C::IsolationGate,
            _ => return ExitCode::from(64),
        };
        return match autotune_recovery_harness::runtime::process_case(case) {
            Ok(report) => {
                let blocked = report.blocker.is_some();
                println!(
                    "{}",
                    String::from_utf8(autotune_recovery_contract::canonical(&report).unwrap())
                        .unwrap()
                );
                if blocked {
                    ExitCode::from(2)
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(e) => {
                eprintln!("transport case failed: {e:?}");
                ExitCode::from(2)
            }
        };
    }
    #[cfg(windows)]
    if args.as_slice() == ["--fake-ipc-check"] {
        return match autotune_recovery_harness::runtime::ipc_check() {
            Ok(s) => {
                println!(
                    "{}",
                    String::from_utf8(autotune_recovery_contract::canonical(&s).unwrap()).unwrap()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("IPC diagnostic rejected: {e:?}");
                ExitCode::from(2)
            }
        };
    }
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "--fake-roundtrip") && args.len() <= 2 {
        use autotune_recovery_contract::process::FakeBehavior;
        let behavior = match args.get(1).map(String::as_str).unwrap_or("normal") {
            "normal" => FakeBehavior::Normal,
            "error-after-change" => FakeBehavior::ErrorAfterChange,
            "missing-candidate" => FakeBehavior::MissingCandidateApi,
            "missing-restore" => FakeBehavior::MissingRestoreApi,
            _ => return ExitCode::from(64),
        };
        return match autotune_recovery_harness::runtime::roundtrip_check(behavior) {
            Ok(report) => {
                println!(
                    "{}",
                    String::from_utf8(autotune_recovery_contract::canonical(&report).unwrap())
                        .unwrap()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("fake round trip failed: {e:?}");
                ExitCode::from(2)
            }
        };
    }
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "--internal-role") && args.len() == 2 {
        let role = match args[1].as_str() {
            "guardian" => autotune_recovery_contract::Role::Guardian,
            "worker" => autotune_recovery_contract::Role::Worker,
            "coordinator" => autotune_recovery_contract::Role::Coordinator,
            "verifier" => autotune_recovery_contract::Role::Verifier,
            _ => return ExitCode::from(64),
        };
        return match autotune_recovery_harness::runtime::internal_role(role) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                if let Ok(path) = std::env::var("AUTOTUNE_RECOVERY_DIAG") {
                    use std::io::Write;
                    let path = format!("{path}.{}", std::process::id());
                    if let Ok(mut file) = std::fs::File::create(path) {
                        let _ = writeln!(file, "role={role:?} error={e:?}");
                        let _ = file.sync_all();
                    }
                }
                eprintln!("fixed role rejected: {e:?}");
                ExitCode::from(2)
            }
        };
    }
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "--fake-process-check") && args.len() <= 2 {
        use autotune_recovery_contract::process::Control;
        let action = match args.get(1).map(String::as_str).unwrap_or("observe") {
            "observe" | "launch-and-exit" => Control::Observe,
            "kill-coordinator" => Control::TerminateCoordinator,
            "close-job" => Control::CloseCoordinatorJob,
            "close-console" => Control::CloseCoordinatorConsole,
            "expired-getter" => Control::ExpiredGetter,
            "deny-candidate" => Control::DeniedCandidate,
            _ => return ExitCode::from(64),
        };
        let exiting = args.get(1).is_some_and(|a| a == "launch-and-exit");
        return match autotune_recovery_harness::runtime::launch_check(action, exiting) {
            Ok(report) => {
                if !exiting {
                    println!(
                        "{}",
                        String::from_utf8(autotune_recovery_contract::canonical(&report).unwrap())
                            .unwrap()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("fake process check rejected: {e:?}");
                ExitCode::from(2)
            }
        };
    }
    match args.as_slice() {
        [] => help(),
        [arg] if arg == "--help" => help(),
        [arg] if arg == "--platform-check" => {
            println!("synthetic_only=true; P1_acceptance=blocked; P2=not_tested; P3=not_tested");
            println!("clock={:?}", autotune_recovery_windows::clock());
            println!(
                "current_process_isolation={:?}",
                autotune_recovery_windows::verify_isolation()
            );
            println!("arm={:?}", autotune_recovery_windows::require_arm_support());
            #[cfg(windows)]
            println!(
                "current_process_in_any_job={:?}",
                autotune_recovery_windows::process::in_any_job(
                    autotune_recovery_windows::process::current_process_handle()
                )
            );
            #[cfg(windows)]
            println!(
                "current_job_limits={:?}",
                autotune_recovery_windows::process::current_job_limits()
            );
            return ExitCode::from(2);
        }
        _ => {
            eprintln!("Unsupported arguments. No hardware, arbitrary-backend or recovery-replay mode exists.");
            return ExitCode::from(64);
        }
    }
    ExitCode::SUCCESS
}
fn help() {
    println!("autotune-recovery-harness [--help | --platform-check | --fake-ipc-check]\nFake-only recovery infrastructure under development. Arming is blocked.\n--platform-check samples clocks/isolation only; creates no child or journal.\n--fake-ipc-check launches one measured verifier for authenticated IPC; zero device permissions.\n--fake-transport-case CASE and --fake-process-check ACTION exercise fixed isolated diagnostic roles; see docs/windows-recovery.md.\nDiagnostics can create retained protected test fixtures and owned processes. Reports contain private process/token identity: review before exporting.\nNo NVML, hardware capability, production root, or automatic replay.");
}
