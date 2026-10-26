//! Pure FMA design validation, deterministic inputs and CPU reference checking.
//! No workload, device, process, clock, I/O or execution-approval capability.
#![forbid(unsafe_code)]

pub mod input;
pub mod profile;
pub mod reference;
