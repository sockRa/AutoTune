//! `autotune.live-session` v1 session envelope.
//!
//! Typed fixed-order sections, explicit provenance, one session digest,
//! all-or-nothing validation and retention refusal, built only on the phase-1
//! primitives. Values are injected: a validated envelope is not live evidence,
//! consent, admission or qualification, and this module never reads a clock,
//! launches a process or calls a driver.

pub mod codec;
pub mod model;

pub use codec::{
    decode_envelope, encode_envelope, EnvelopeError, EnvelopeErrorKind, SCHEMA, VERSION,
};
pub use model::{
    AcceptPolicy, Admission, Artifacts, Clocks, ControlWrites, Correctness, Coverage, Dispatch,
    EmptyJobLifecycle, Envelope, Hex64, Identity, Integrity, InternalAge, Lifecycle, Manifest,
    MatchState, NativeCompletion, Obs, Provenance, Reason, ReasonError, ResultEmptyJob, Retention,
    Role, SessionResult, Telemetry, TelemetryValue, Verdict, WriterStatus,
};
