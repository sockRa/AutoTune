//! Bounded injected benchmark worker. No live admission or default native backend.
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
pub mod cuda;
#[cfg(windows)]
pub mod process;
pub mod worker;
