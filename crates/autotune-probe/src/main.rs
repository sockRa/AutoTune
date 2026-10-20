use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let report = match args.as_slice() {
        [] => autotune_probe::run_live(),
        [flag] if flag == "--help" => {
            println!("autotune-probe [--mock [SCENARIO] | --help]\nRead-only NVIDIA capability probe. Default: live Windows NVML.\nMock scenarios: {}", autotune_probe::mock::SCENARIOS.join(", "));
            return ExitCode::SUCCESS;
        }
        [flag] if flag == "--mock" => autotune_probe::run_mock("supported").unwrap(),
        [flag, scenario] if flag == "--mock" => {
            match scenario.to_str().and_then(autotune_probe::run_mock) {
                Some(report) => report,
                None => return usage(),
            }
        }
        _ => return usage(),
    };
    for diagnostic in &report.diagnostics {
        eprintln!("{}", serde_json::to_string(diagnostic).unwrap());
    }
    if report.status == autotune_probe::report::CollectionStatus::Incomplete {
        eprintln!(
            "{{\"reason\":\"incomplete_collection\",\"detail\":\"inspect per-field failures\"}}"
        );
    }
    let stdout = io::stdout();
    let mut output = stdout.lock();
    if let Err(error) = serde_json::to_writer_pretty(&mut output, &report)
        .map_err(io::Error::other)
        .and_then(|()| output.write_all(b"\n"))
    {
        eprintln!(
            "{}",
            serde_json::json!({"reason":"output_failed", "detail":error.to_string()})
        );
        return ExitCode::from(2);
    }
    ExitCode::from(report.exit_code())
}

fn usage() -> ExitCode {
    eprintln!("{{\"reason\":\"usage_error\",\"detail\":\"use --help; expected no arguments or --mock [SCENARIO]\"}}");
    ExitCode::from(64)
}
