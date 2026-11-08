//! Workload declarations and protocol binding. No native execution authority.
pub mod assessment;
pub mod bootstrap;
pub mod child_options;
#[cfg(windows)]
pub mod child_windows;
#[cfg(any(windows, test))]
mod collector_task;
pub mod consent;
pub mod control;
#[cfg(windows)]
pub mod control_windows;
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
pub mod cuda_setup_trace;
pub mod dispatch;
pub mod gate;
pub mod host_environment;
#[cfg(windows)]
pub mod host_environment_windows;
pub mod launch;
pub mod launch_origin;
pub mod manifest;
pub mod nvml_trace;
pub mod parent_context;
pub mod parent_peers;
pub mod parent_sampler;
pub mod parent_start;
pub mod parent_watch;
pub mod parent_watch_publication;
pub mod parent_watch_retirement;
#[cfg(windows)]
pub mod parent_watch_windows;
pub mod prestart;
pub mod process_state;
pub mod producer;
#[cfg(windows)]
pub mod retirement_collector;
pub mod runtime_evidence;
pub mod sampler_dispatch;
#[cfg(windows)]
pub mod sampler_entry;
#[cfg(windows)]
mod sampler_native;
pub mod sampler_runtime;
pub mod settings;
pub mod settings_transport;
pub mod setup_model;
pub mod starting;
pub mod starting_bootstrap;
pub mod starting_control;
#[cfg(windows)]
pub mod starting_control_windows;
pub mod starting_delivery;
#[cfg(windows)]
pub mod starting_entry;
pub mod starting_export;
#[cfg(windows)]
pub mod starting_launch;
pub mod starting_read;
pub mod starting_receive;
pub mod starting_runtime;
pub mod starting_session;
pub mod stop_requests;
pub mod teardown;
pub mod telemetry;
pub mod trace;
pub mod transport;
#[cfg(windows)]
pub mod windows_transport;
pub mod worker_setup;
pub mod worker_setup_assessment;
pub mod worker_setup_delivery;
#[cfg(windows)]
pub use crate::sampler::windows::workload_launch as windows_process;
pub mod wire;
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
pub mod worker_entry;
pub mod worker_finish;
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
pub mod worker_native;
pub mod worker_runtime;
