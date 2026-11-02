#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
fn main() {
    // SAFETY: one fixed executable entry, before any inherited endpoint adoption.
    // The pilot launcher's explicit allowlist supplies exclusive pipe ownership.
    let code = match unsafe { autotune_benchmark_worker::workload::sampler_entry::entry() } {
        Ok(outcome) => outcome.exit_code(),
        Err(_) => 2,
    };
    std::process::exit(code);
}
#[cfg(not(windows))]
fn main() {
    std::process::exit(2);
}
