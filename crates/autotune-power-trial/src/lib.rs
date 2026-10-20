//! Deterministic safety model only. No hardware, OS processes, authentication or disk durability.
#![forbid(unsafe_code)]

pub mod engine;
pub mod journal;
pub mod manifest;
pub mod mock;
