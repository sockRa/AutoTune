#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    if let Err(error) = autotune_benchmark_worker::sampler::controller_windows::controller_main() {
        eprintln!("sampler capture incomplete/unqualified: {error}");
        std::process::exit(2);
    }
    #[cfg(not(windows))]
    std::process::exit(2);
}
