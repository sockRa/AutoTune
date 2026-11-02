//! Workload declarations and protocol binding. No native execution authority.
pub mod assessment;
pub mod bootstrap;
#[cfg(windows)]
pub mod child_windows;
pub mod control;
#[cfg(windows)]
pub mod control_windows;
pub mod dispatch;
pub mod gate;
pub mod launch;
pub mod manifest;
pub mod parent_sampler;
pub mod producer;
pub mod sampler_dispatch;
#[cfg(windows)]
pub mod sampler_entry;
pub mod sampler_runtime;
pub mod settings;
pub mod settings_transport;
pub mod starting;
pub mod starting_bootstrap;
pub mod starting_control;
#[cfg(windows)]
pub mod starting_control_windows;
pub mod telemetry;
pub mod trace;
pub mod transport;
#[cfg(windows)]
pub mod windows_transport;
#[cfg(windows)]
pub use crate::sampler::windows::workload_launch as windows_process;
pub mod wire;
