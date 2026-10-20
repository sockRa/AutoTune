#![forbid(unsafe_code)]
use autotune_power_trial::mock::{run, SCENARIOS};
use std::process::ExitCode;

fn help() {
    println!("AutoTune power trial — SIMULATION ONLY\nUsage: autotune-power-trial --mock [SCENARIO]\n       autotune-power-trial --help\nBare invocation only shows help. No live mode, NVML, commands or hardware writes.\nScenarios: {}", SCENARIOS.join(", "));
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {
            help();
            ExitCode::SUCCESS
        }
        [a] if a == "--help" || a == "-h" => {
            help();
            ExitCode::SUCCESS
        }
        [a] if a == "--mock" => emit("success"),
        [a, scenario] if a == "--mock" => emit(scenario),
        _ => {
            eprintln!("Invalid options. Simulation only; use --help.");
            ExitCode::from(64)
        }
    }
}

fn emit(scenario: &str) -> ExitCode {
    match run(scenario) {
        Some(report) => {
            let success = report.overall == "success";
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("serializable simulation report")
            );
            ExitCode::from(if success { 0 } else { 2 })
        }
        None => {
            eprintln!("Unknown mock scenario; use --help.");
            ExitCode::from(64)
        }
    }
}
