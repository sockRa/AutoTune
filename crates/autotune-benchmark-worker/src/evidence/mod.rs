//! Canonical live-evidence primitives: bytes, decimal integers/ratios, digests and
//! strict decoding. Values are injected; a passing codec never produces live
//! evidence or authorizes native work.
pub mod canonical;
pub mod digest;
pub mod envelope;
pub mod parse;

pub use canonical::{
    encode, AccessError, FieldError, Object, ObjectError, Ratio, RatioError, Value,
};
pub use digest::{Digest, DigestError, Domain};
pub use envelope::{
    decode_envelope, encode_envelope, AcceptPolicy, Admission, Artifacts, Clocks, ControlWrites,
    Correctness, Coverage, Dispatch, EmptyJobLifecycle, Envelope, EnvelopeError, EnvelopeErrorKind,
    Hex64, Identity, Integrity, InternalAge, Lifecycle, Manifest, MatchState, NativeCompletion,
    Obs, Provenance, Reason, ReasonError, ResultEmptyJob, Retention, Role, SessionResult,
    Telemetry, TelemetryValue, Verdict, WriterStatus, SCHEMA, VERSION,
};
pub use parse::{decode, ParseError, MAX_DEPTH};
