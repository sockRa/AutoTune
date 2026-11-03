#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
fn main() {
    // SAFETY: one fixed executable entry, before any inherited endpoint adoption.
    // The pilot launcher's explicit allowlist supplies exclusive pipe ownership.
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let code = if autotune_benchmark_worker::workload::starting_entry::is_starting(&args) {
        match unsafe { autotune_benchmark_worker::workload::starting_entry::entry() } {
            Ok(outcome) => outcome.exit_code(),
            Err(_) => 2,
        }
    } else {
        match unsafe { autotune_benchmark_worker::workload::sampler_entry::entry() } {
            Ok(outcome) => outcome.exit_code(),
            Err(_) => 2,
        }
    };
    std::process::exit(code);
}
#[cfg(not(windows))]
fn main() {
    std::process::exit(2);
}
