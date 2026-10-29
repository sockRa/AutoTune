#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    if let Err(error) = autotune_benchmark_worker::sampler::child_windows::sampler_main() {
        eprintln!("sampler: {error}");
        std::process::exit(2);
    }
    #[cfg(not(windows))]
    std::process::exit(2);
}
