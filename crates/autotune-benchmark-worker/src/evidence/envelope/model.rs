//! Typed `autotune.live-session` v1 model: fixed-order sections, observations
//! with validated reason codes and closed enum spellings.
//!
//! The model carries injected values only. Constructing or validating it never
//! reads a clock, launches a process, calls a driver or grants consent.

use crate::evidence::Digest;

/// Lowercase 64-hex value. Phase-1 [`Digest`] parsing enforces the form, so the
/// envelope reuses it instead of defining a second hex parser.
pub type Hex64 = Digest;

/// Error returned when constructing a [`Reason`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonError {
    /// The code is empty.
    Empty,
    /// The code is longer than 64 bytes.
    TooLong,
    /// The code contains a byte outside `[a-z0-9_.-]`.
    Character,
}

/// Validated `unavailable` reason code: ASCII `[a-z0-9_.-]{1,64}`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reason(String);

impl Reason {
    /// Validate and own a reason code.
    pub fn new(text: &str) -> Result<Self, ReasonError> {
        if text.is_empty() {
            return Err(ReasonError::Empty);
        }
        if text.len() > 64 {
            return Err(ReasonError::TooLong);
        }
        if !text.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-')
        }) {
            return Err(ReasonError::Character);
        }
        Ok(Self(text.to_owned()))
    }

    /// Borrow the code.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A field value that is either known or explicitly unavailable.
///
/// `Unavailable` always carries one validated [`Reason`]; an absent value is
/// never inferred from an empty string, zero or null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Obs<T> {
    /// The caller supplied a value.
    Known(T),
    /// The caller supplied no value and named the reason.
    Unavailable(Reason),
}

impl<T> Obs<T> {
    /// Wrap a known value.
    pub fn known(value: T) -> Self {
        Self::Known(value)
    }

    /// Wrap a named absence.
    pub fn unavailable(reason: Reason) -> Self {
        Self::Unavailable(reason)
    }

    /// Borrow the known value, if any.
    pub fn as_known(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unavailable(_) => None,
        }
    }

    /// Whether the field is known.
    pub fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }
}

/// Closed enum spellings shared by the encoder and the strict decoder.
pub(crate) trait Spelling: Copy + 'static {
    const VARIANTS: &'static [Self];
    fn as_str(self) -> &'static str;
}

/// Where the session values came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    LiveObserved,
    SimulatedFixture,
}

impl Spelling for Provenance {
    const VARIANTS: &'static [Self] = &[Self::LiveObserved, Self::SimulatedFixture];
    fn as_str(self) -> &'static str {
        match self {
            Self::LiveObserved => "live_observed",
            Self::SimulatedFixture => "simulated_fixture",
        }
    }
}

/// Which provenance a caller accepts while decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptPolicy {
    /// Reject `simulated_fixture` after the digest has been verified.
    RequireLive,
    /// Accept both provenance values without promoting either.
    AllowSimulated,
}

/// Identity comparison result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchState {
    Match,
    Mismatch,
    Unknown,
}

impl Spelling for MatchState {
    const VARIANTS: &'static [Self] = &[Self::Match, Self::Mismatch, Self::Unknown];
    fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Unknown => "unknown",
        }
    }
}

/// Control-write statement. v1 admits only `none`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlWrites {
    None,
}

impl Spelling for ControlWrites {
    const VARIANTS: &'static [Self] = &[Self::None];
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
        }
    }
}

/// Lifecycle empty-job observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyJobLifecycle {
    ObservedEmpty,
    ObservedNonempty,
    Unknown,
}

impl Spelling for EmptyJobLifecycle {
    const VARIANTS: &'static [Self] = &[Self::ObservedEmpty, Self::ObservedNonempty, Self::Unknown];
    fn as_str(self) -> &'static str {
        match self {
            Self::ObservedEmpty => "observed_empty",
            Self::ObservedNonempty => "observed_nonempty",
            Self::Unknown => "unknown",
        }
    }
}

/// Dispatch role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Warmup,
    Measured,
}

impl Spelling for Role {
    const VARIANTS: &'static [Self] = &[Self::Warmup, Self::Measured];
    fn as_str(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Measured => "measured",
        }
    }
}

/// What the dispatcher retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retention {
    DigestOnly,
    FullFailure,
    FullSuccess,
}

impl Spelling for Retention {
    const VARIANTS: &'static [Self] = &[Self::DigestOnly, Self::FullFailure, Self::FullSuccess];
    fn as_str(self) -> &'static str {
        match self {
            Self::DigestOnly => "digest_only",
            Self::FullFailure => "full_failure",
            Self::FullSuccess => "full_success",
        }
    }
}

/// Telemetry internal-age assessment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InternalAge {
    Unknown,
    Assessed,
}

impl Spelling for InternalAge {
    const VARIANTS: &'static [Self] = &[Self::Unknown, Self::Assessed];
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Assessed => "assessed",
        }
    }
}

/// Integrity writer bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterStatus {
    Bounded,
    Unbounded,
}

impl Spelling for WriterStatus {
    const VARIANTS: &'static [Self] = &[Self::Bounded, Self::Unbounded];
    fn as_str(self) -> &'static str {
        match self {
            Self::Bounded => "bounded",
            Self::Unbounded => "unbounded",
        }
    }
}

/// Admission state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Admitted,
    Blocked,
    NotQualified,
}

impl Spelling for Admission {
    const VARIANTS: &'static [Self] = &[Self::Admitted, Self::Blocked, Self::NotQualified];
    fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Blocked => "blocked",
            Self::NotQualified => "not_qualified",
        }
    }
}

/// Correctness state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Correctness {
    Passed,
    Failed,
    NotChecked,
}

impl Spelling for Correctness {
    const VARIANTS: &'static [Self] = &[Self::Passed, Self::Failed, Self::NotChecked];
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::NotChecked => "not_checked",
        }
    }
}

/// Result empty-job observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultEmptyJob {
    Observed,
    NotObserved,
    Unknown,
}

impl Spelling for ResultEmptyJob {
    const VARIANTS: &'static [Self] = &[Self::Observed, Self::NotObserved, Self::Unknown];
    fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::NotObserved => "not_observed",
            Self::Unknown => "unknown",
        }
    }
}

/// Native completion statement. v1 admits only `unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCompletion {
    Unknown,
}

impl Spelling for NativeCompletion {
    const VARIANTS: &'static [Self] = &[Self::Unknown];
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
        }
    }
}

/// Coverage state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    Complete,
    Incomplete,
    Unknown,
}

impl Spelling for Coverage {
    const VARIANTS: &'static [Self] = &[Self::Complete, Self::Incomplete, Self::Unknown];
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Unknown => "unknown",
        }
    }
}

/// Session verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Blocked,
    Invalid,
    Inconclusive,
    Usable,
}

impl Spelling for Verdict {
    const VARIANTS: &'static [Self] = &[
        Self::Blocked,
        Self::Invalid,
        Self::Inconclusive,
        Self::Usable,
    ];
    fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Invalid => "invalid",
            Self::Inconclusive => "inconclusive",
            Self::Usable => "usable",
        }
    }
}

/// `identity` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub nonce: String,
    pub nvml_uuid: Obs<String>,
    pub cuda_uuid: Obs<String>,
    pub pci_bus_id: Obs<String>,
    pub context_device: Obs<String>,
    pub boot: Obs<String>,
    pub uuid_match: MatchState,
    pub pci_match: MatchState,
}

/// `artifacts` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifacts {
    pub source_commit: String,
    pub worker_sha256: Obs<Hex64>,
    pub lockfile_sha256: Obs<Hex64>,
    pub sampler_sha256: Obs<Hex64>,
    pub ptx_sha256: Obs<Hex64>,
    pub kernel_entry: String,
    pub generator_version: String,
    pub oracle_version: String,
}

/// `manifest` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub manifest_digest: Obs<Hex64>,
    pub profile_digest: Obs<Hex64>,
    pub shape: Obs<String>,
    pub stop_policy: Obs<String>,
    pub approval_ref: Obs<String>,
    pub seed: Obs<u64>,
    pub control_writes: ControlWrites,
}

/// `clocks` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clocks {
    pub qpc_frequency: u64,
    pub qpc_epoch_ticks: u64,
    pub interrupt_ticks: u64,
    pub unbiased_ticks: Obs<u64>,
    pub resolution_ns: u64,
    pub utc_context: Obs<String>,
    pub discontinuities: u64,
}

/// `lifecycle` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifecycle {
    pub requested_ns: Obs<u64>,
    pub launch_ns: Obs<u64>,
    pub ready_ns: Obs<u64>,
    pub start_ns: Obs<u64>,
    pub stop_requested_ns: Obs<u64>,
    pub exit_ns: Obs<u64>,
    pub empty_job: EmptyJobLifecycle,
}

/// One `dispatches` array element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispatch {
    pub j: u64,
    pub role: Role,
    pub shape: String,
    pub input_digest: Hex64,
    pub submit_ns: Obs<u64>,
    pub query_ns: Obs<u64>,
    pub readback_ns: Obs<u64>,
    pub check_ns: Obs<u64>,
    pub return_code: i64,
    pub checked_lanes: u64,
    pub mismatch: u64,
    pub nonfinite: u64,
    pub work: u64,
    pub output_digest: Obs<Hex64>,
    pub retention: Retention,
}

/// Telemetry value: a signed integer or an exact ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryValue {
    Int(i64),
    Ratio(crate::evidence::Ratio),
}

/// One `telemetry` array element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Telemetry {
    pub metric: String,
    pub unit: String,
    pub device: String,
    pub value: Obs<TelemetryValue>,
    pub slot: u64,
    pub query_start_ns: Obs<u64>,
    pub query_end_ns: Obs<u64>,
    pub receipt_ns: Obs<u64>,
    pub internal_age: InternalAge,
    pub vendor_code: Obs<String>,
}

/// `integrity` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integrity {
    pub capacity_bytes: u64,
    pub observed_bytes: u64,
    pub failed: u64,
    pub late: u64,
    pub rejected: u64,
    pub overflow: u64,
    pub expected_slots: u64,
    pub observed_slots: u64,
    pub raw_records: u64,
    pub writer_status: WriterStatus,
}

/// `result` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionResult {
    pub admission: Admission,
    pub correctness: Correctness,
    pub worker_exit: Obs<i64>,
    pub sampler_exit: Obs<i64>,
    pub empty_job: ResultEmptyJob,
    pub native_completion: NativeCompletion,
    pub coverage: Coverage,
    pub limitations: Vec<Reason>,
    pub verdict: Verdict,
}

/// Complete `autotune.live-session` v1 envelope. `schema` and `version` are
/// fixed and therefore not stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub provenance: Provenance,
    pub identity: Identity,
    pub artifacts: Artifacts,
    pub manifest: Manifest,
    pub clocks: Clocks,
    pub lifecycle: Lifecycle,
    pub dispatches: Vec<Dispatch>,
    pub telemetry: Vec<Telemetry>,
    pub integrity: Integrity,
    pub result: SessionResult,
}
