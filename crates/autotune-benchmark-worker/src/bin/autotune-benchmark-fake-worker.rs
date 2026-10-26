#![cfg_attr(windows, windows_subsystem = "windows")]
// Deliberately fixed fake-only executable. Even native-cuda builds cannot
// select a native mode: this binary only references the process fixture module.
fn main() {
    #[cfg(windows)]
    if let Err(error) = autotune_benchmark_worker::process::fake_main() {
        eprintln!("fake worker: {error}");
        std::process::exit(2);
    }
    #[cfg(not(windows))]
    std::process::exit(2);
}
