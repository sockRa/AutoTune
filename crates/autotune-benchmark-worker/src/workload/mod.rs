//! Workload declarations and protocol binding. No native execution authority.
pub mod assessment;
pub mod dispatch;
pub mod gate;
pub mod manifest;
pub mod producer;
pub mod settings;
pub mod settings_transport;
pub mod trace;
pub mod transport;
#[cfg(windows)]
pub mod windows_transport;
pub mod wire;
