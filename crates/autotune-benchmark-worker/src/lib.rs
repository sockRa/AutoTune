//! Bounded injected benchmark worker. No live admission or default native backend.
//! Includes portable canonical evidence primitives (bytes, integers/ratios,
//! digests, strict decoding) that inject values and authorize nothing.
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
pub mod cuda;
pub mod evidence;
pub mod monitor;
#[cfg(windows)]
pub mod process;
pub mod sampler;
pub mod worker;
pub mod workload;
