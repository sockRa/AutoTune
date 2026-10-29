#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(all(windows, feature = "sampler-process-fixture"))]
    if let Err(error) = autotune_benchmark_worker::sampler::child_windows::fixture_main() {
        eprintln!("sampler fixture: {error}");
        std::process::exit(2);
    }
    #[cfg(not(all(windows, feature = "sampler-process-fixture")))]
    std::process::exit(2);
}
