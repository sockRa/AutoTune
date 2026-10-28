//! Ordered encoder, session digest and strict decoder for the
//! `autotune.live-session` v1 envelope.
//!
//! Encoding validates the whole model before writing a byte; decoding goes
//! through the phase-1 reader, requires the exact field order, verifies the
//! session digest and then applies the same validation. Nothing here reads a
//! clock, launches a process or contacts a device.

use super::model::{
    AcceptPolicy, Admission, Artifacts, Clocks, Correctness, Coverage, Dispatch, EmptyJobLifecycle,
    Envelope, Hex64, Identity, Integrity, InternalAge, Lifecycle, Manifest, MatchState, Obs,
    Provenance, Reason, ResultEmptyJob, Retention, Role, SessionResult, Spelling, Telemetry,
    TelemetryValue, Verdict, WriterStatus,
};
use crate::evidence::canonical::{encode, FieldError, Object, Value};
use crate::evidence::{decode, Digest, Domain, Ratio};

/// The only accepted top-level `schema` value.
pub const SCHEMA: &str = "autotune.live-session";
/// The only accepted top-level `version` value.
pub const VERSION: &str = "1";

const TOP_LEVEL: &[&str] = &[
    "schema",
    "version",
    "provenance",
    "identity",
    "artifacts",
    "manifest",
    "clocks",
    "lifecycle",
    "dispatches",
    "telemetry",
    "integrity",
    "result",
    "digest",
];

const IDENTITY: &[&str] = &[
    "nonce",
    "nvml_uuid",
    "cuda_uuid",
    "pci_bus_id",
    "context_device",
    "boot",
    "uuid_match",
    "pci_match",
];

const ARTIFACTS: &[&str] = &[
    "source_commit",
    "worker_sha256",
    "lockfile_sha256",
    "sampler_sha256",
    "ptx_sha256",
    "kernel_entry",
    "generator_version",
    "oracle_version",
];

const MANIFEST: &[&str] = &[
    "manifest_digest",
    "profile_digest",
    "shape",
    "stop_policy",
    "approval_ref",
    "seed",
    "control_writes",
];

const CLOCKS: &[&str] = &[
    "qpc_frequency",
    "qpc_epoch_ticks",
    "interrupt_ticks",
    "unbiased_ticks",
    "resolution_ns",
    "utc_context",
    "discontinuities",
];

const LIFECYCLE: &[&str] = &[
    "requested_ns",
    "launch_ns",
    "ready_ns",
    "start_ns",
    "stop_requested_ns",
    "exit_ns",
    "empty_job",
];

const DISPATCH: &[&str] = &[
    "j",
    "role",
    "shape",
    "input_digest",
    "submit_ns",
    "query_ns",
    "readback_ns",
    "check_ns",
    "return_code",
    "checked_lanes",
    "mismatch",
    "nonfinite",
    "work",
    "output_digest",
    "retention",
];

const TELEMETRY: &[&str] = &[
    "metric",
    "unit",
    "device",
    "value",
    "slot",
    "query_start_ns",
    "query_end_ns",
    "receipt_ns",
    "internal_age",
    "vendor_code",
];

const INTEGRITY: &[&str] = &[
    "capacity_bytes",
    "observed_bytes",
    "failed",
    "late",
    "rejected",
    "overflow",
    "expected_slots",
    "observed_slots",
    "raw_records",
    "writer_status",
];

const RESULT: &[&str] = &[
    "admission",
    "correctness",
    "worker_exit",
    "sampler_exit",
    "empty_job",
    "native_completion",
    "coverage",
    "limitations",
    "verdict",
];

/// Typed rejection category. The field name travels beside it in
/// [`EnvelopeError`], so callers can name the offending field without parsing
/// text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvelopeErrorKind {
    /// The phase-1 reader rejected the bytes.
    Parse,
    /// The root value is not an object.
    Root,
    /// A section is not an object or array.
    NotObject,
    NotArray,
    /// A value is not text where text is required.
    NotText,
    /// The object has fewer fields than the exact list.
    FieldCount,
    /// A field name is missing or unknown.
    FieldName,
    /// A known field appears in the wrong position.
    FieldOrder,
    /// The object has more fields than the exact list.
    ExtraField,
    /// A key was repeated while building a document.
    DuplicateField,
    /// The fixed `schema` value is wrong.
    Schema,
    /// The fixed `version` value is wrong.
    Version,
    /// An enum spelling is not one of the exact lowercase values.
    UnknownVariant,
    /// Required text is empty.
    TextEmpty,
    /// Text contains a control character.
    TextControl,
    /// Text contains a non-ASCII character where ASCII is required.
    TextNonAscii,
    /// Text is longer than the field allows.
    TextLength,
    /// A canonical decimal integer was required.
    Integer,
    /// An exact `num`/`den` ratio was required.
    Ratio,
    /// A lowercase 64-hex (or 40-hex commit) value was required.
    Hex,
    /// A reason code is not ASCII `[a-z0-9_.-]{1,64}`.
    Reason,
    /// The recomputed session digest differs from the stored one.
    DigestMismatch,
    /// Two fields contradict each other.
    CrossField,
    /// A value is outside its allowed range, such as zero where positive is
    /// required.
    Range,
    /// `retention` is `full_success`, which v1 never stores.
    RetentionFullSuccess,
    /// `writer_status` is `unbounded`, which v1 never stores.
    WriterUnbounded,
    /// The provenance is not acceptable under the caller's policy.
    Provenance,
}

/// Typed rejection naming the offending field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeError {
    field: &'static str,
    kind: EnvelopeErrorKind,
}

impl EnvelopeError {
    const fn new(field: &'static str, kind: EnvelopeErrorKind) -> Self {
        Self { field, kind }
    }

    /// Name of the offending field, or `"envelope"` for whole-document errors.
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// Rejection category.
    pub fn kind(&self) -> EnvelopeErrorKind {
        self.kind
    }
}

/// Validate and encode one envelope.
///
/// The whole model, including every cross-field rule, is checked before any
/// byte is written. `retention == full_success` and
/// `writer_status == unbounded` are always errors.
pub fn encode_envelope(envelope: &Envelope) -> Result<String, EnvelopeError> {
    validate(envelope)?;
    let mut object = build_payload(envelope)?;
    let digest = Digest::of_value(Domain::Session, &Value::Object(object.clone()));
    put(&mut object, "digest", Value::Text(digest.to_hex()))?;
    Ok(encode(&Value::Object(object)))
}

/// Decode bounded canonical bytes into a validated envelope.
///
/// The phase-1 reader applies `max_bytes` and the depth bound. The document
/// must be a root object with exactly the 13 top-level keys in order and a
/// `digest` last; the session digest is recomputed over the payload without
/// `digest` before any section is parsed. `AcceptPolicy::RequireLive` then
/// rejects `simulated_fixture`.
pub fn decode_envelope(
    input: &[u8],
    max_bytes: usize,
    policy: AcceptPolicy,
) -> Result<Envelope, EnvelopeError> {
    let value = decode(input, max_bytes)
        .map_err(|_| EnvelopeError::new("envelope", EnvelopeErrorKind::Parse))?;
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("envelope", EnvelopeErrorKind::Root))?;
    require_keys(object, TOP_LEVEL)?;

    let claimed = hex_known(field(object, "digest")?, "digest")?;
    let payload = Value::Object(object.without("digest"));
    if Digest::of_value(Domain::Session, &payload) != claimed {
        return Err(EnvelopeError::new(
            "digest",
            EnvelopeErrorKind::DigestMismatch,
        ));
    }

    if text_known(field(object, "schema")?, "schema")? != SCHEMA {
        return Err(EnvelopeError::new("schema", EnvelopeErrorKind::Schema));
    }
    if text_known(field(object, "version")?, "version")? != VERSION {
        return Err(EnvelopeError::new("version", EnvelopeErrorKind::Version));
    }

    let provenance = enum_field::<Provenance>(field(object, "provenance")?, "provenance")?;
    if policy == AcceptPolicy::RequireLive && provenance == Provenance::SimulatedFixture {
        return Err(EnvelopeError::new(
            "provenance",
            EnvelopeErrorKind::Provenance,
        ));
    }

    let envelope = Envelope {
        provenance,
        identity: parse_identity(field(object, "identity")?)?,
        artifacts: parse_artifacts(field(object, "artifacts")?)?,
        manifest: parse_manifest(field(object, "manifest")?)?,
        clocks: parse_clocks(field(object, "clocks")?)?,
        lifecycle: parse_lifecycle(field(object, "lifecycle")?)?,
        dispatches: parse_dispatches(field(object, "dispatches")?)?,
        telemetry: parse_telemetry(field(object, "telemetry")?)?,
        integrity: parse_integrity(field(object, "integrity")?)?,
        result: parse_result(field(object, "result")?)?,
    };
    validate(&envelope)?;
    Ok(envelope)
}

// --- decoding -------------------------------------------------------------

fn field<'a>(object: &'a Object, key: &'static str) -> Result<&'a Value, EnvelopeError> {
    object
        .get(key)
        .ok_or_else(|| EnvelopeError::new(key, EnvelopeErrorKind::FieldCount))
}

fn require_keys(object: &Object, keys: &[&'static str]) -> Result<(), EnvelopeError> {
    // Name the first shifted or unknown field precisely, then let the phase-1
    // exact-field helper decide whether the object is acceptable at all.
    for (index, (name, _)) in object.iter().enumerate() {
        if let Some(expected) = keys.get(index) {
            if name != *expected {
                let kind = if keys.contains(&name) {
                    EnvelopeErrorKind::FieldOrder
                } else {
                    EnvelopeErrorKind::FieldName
                };
                return Err(EnvelopeError::new(expected, kind));
            }
        }
    }
    match object.require_exact(keys) {
        Ok(()) => Ok(()),
        Err(FieldError::Count) if object.len() < keys.len() => Err(EnvelopeError::new(
            keys[object.len()],
            EnvelopeErrorKind::FieldCount,
        )),
        Err(FieldError::Count) => Err(EnvelopeError::new(
            keys[keys.len() - 1],
            EnvelopeErrorKind::ExtraField,
        )),
        Err(FieldError::Name) => Err(EnvelopeError::new("envelope", EnvelopeErrorKind::FieldName)),
    }
}

fn text_known(value: &Value, field: &'static str) -> Result<String, EnvelopeError> {
    value
        .as_text()
        .map(str::to_owned)
        .ok_or_else(|| EnvelopeError::new(field, EnvelopeErrorKind::NotText))
}

fn u64_known(value: &Value, field: &'static str) -> Result<u64, EnvelopeError> {
    value
        .as_u64()
        .map_err(|_| EnvelopeError::new(field, EnvelopeErrorKind::Integer))
}

fn i64_known(value: &Value, field: &'static str) -> Result<i64, EnvelopeError> {
    value
        .as_i64()
        .map_err(|_| EnvelopeError::new(field, EnvelopeErrorKind::Integer))
}

fn hex_known(value: &Value, field: &'static str) -> Result<Hex64, EnvelopeError> {
    let text = value
        .as_text()
        .ok_or_else(|| EnvelopeError::new(field, EnvelopeErrorKind::NotText))?;
    Digest::parse(text).map_err(|_| EnvelopeError::new(field, EnvelopeErrorKind::Hex))
}

fn ratio_known(value: &Value, field: &'static str) -> Result<Ratio, EnvelopeError> {
    value
        .as_ratio()
        .map_err(|_| EnvelopeError::new(field, EnvelopeErrorKind::Ratio))
}

fn reason_known(value: &Value, field: &'static str) -> Result<Reason, EnvelopeError> {
    let text = value
        .as_text()
        .ok_or_else(|| EnvelopeError::new(field, EnvelopeErrorKind::Reason))?;
    Reason::new(text).map_err(|_| EnvelopeError::new(field, EnvelopeErrorKind::Reason))
}

fn enum_field<T: Spelling>(value: &Value, field: &'static str) -> Result<T, EnvelopeError> {
    let text = value
        .as_text()
        .ok_or_else(|| EnvelopeError::new(field, EnvelopeErrorKind::NotText))?;
    T::VARIANTS
        .iter()
        .copied()
        .find(|variant| variant.as_str() == text)
        .ok_or_else(|| EnvelopeError::new(field, EnvelopeErrorKind::UnknownVariant))
}

fn obs_parse<T>(
    value: &Value,
    name: &'static str,
    known: impl Fn(&Value) -> Result<T, EnvelopeError>,
) -> Result<Obs<T>, EnvelopeError> {
    if let Some(object) = value.as_object() {
        if object.get("unavailable").is_some() {
            require_keys(object, &["unavailable"])?;
            let reason = reason_known(field(object, "unavailable")?, name)?;
            return Ok(Obs::Unavailable(reason));
        }
    }
    known(value).map(Obs::Known)
}

fn telemetry_value_known(
    value: &Value,
    field: &'static str,
) -> Result<TelemetryValue, EnvelopeError> {
    if value.as_text().is_some() {
        return i64_known(value, field).map(TelemetryValue::Int);
    }
    if value.as_object().is_some() {
        return ratio_known(value, field).map(TelemetryValue::Ratio);
    }
    Err(EnvelopeError::new(field, EnvelopeErrorKind::NotText))
}

fn parse_identity(value: &Value) -> Result<Identity, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("identity", EnvelopeErrorKind::NotObject))?;
    require_keys(object, IDENTITY)?;
    Ok(Identity {
        nonce: text_known(field(object, "nonce")?, "nonce")?,
        nvml_uuid: obs_parse(field(object, "nvml_uuid")?, "nvml_uuid", |value| {
            text_known(value, "nvml_uuid")
        })?,
        cuda_uuid: obs_parse(field(object, "cuda_uuid")?, "cuda_uuid", |value| {
            text_known(value, "cuda_uuid")
        })?,
        pci_bus_id: obs_parse(field(object, "pci_bus_id")?, "pci_bus_id", |value| {
            text_known(value, "pci_bus_id")
        })?,
        context_device: obs_parse(
            field(object, "context_device")?,
            "context_device",
            |value| text_known(value, "context_device"),
        )?,
        boot: obs_parse(field(object, "boot")?, "boot", |value| {
            text_known(value, "boot")
        })?,
        uuid_match: enum_field(field(object, "uuid_match")?, "uuid_match")?,
        pci_match: enum_field(field(object, "pci_match")?, "pci_match")?,
    })
}

fn parse_artifacts(value: &Value) -> Result<Artifacts, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("artifacts", EnvelopeErrorKind::NotObject))?;
    require_keys(object, ARTIFACTS)?;
    Ok(Artifacts {
        source_commit: text_known(field(object, "source_commit")?, "source_commit")?,
        worker_sha256: obs_parse(field(object, "worker_sha256")?, "worker_sha256", |value| {
            hex_known(value, "worker_sha256")
        })?,
        lockfile_sha256: obs_parse(
            field(object, "lockfile_sha256")?,
            "lockfile_sha256",
            |value| hex_known(value, "lockfile_sha256"),
        )?,
        sampler_sha256: obs_parse(
            field(object, "sampler_sha256")?,
            "sampler_sha256",
            |value| hex_known(value, "sampler_sha256"),
        )?,
        ptx_sha256: obs_parse(field(object, "ptx_sha256")?, "ptx_sha256", |value| {
            hex_known(value, "ptx_sha256")
        })?,
        kernel_entry: text_known(field(object, "kernel_entry")?, "kernel_entry")?,
        generator_version: text_known(field(object, "generator_version")?, "generator_version")?,
        oracle_version: text_known(field(object, "oracle_version")?, "oracle_version")?,
    })
}

fn parse_manifest(value: &Value) -> Result<Manifest, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("manifest", EnvelopeErrorKind::NotObject))?;
    require_keys(object, MANIFEST)?;
    Ok(Manifest {
        manifest_digest: obs_parse(
            field(object, "manifest_digest")?,
            "manifest_digest",
            |value| hex_known(value, "manifest_digest"),
        )?,
        profile_digest: obs_parse(
            field(object, "profile_digest")?,
            "profile_digest",
            |value| hex_known(value, "profile_digest"),
        )?,
        shape: obs_parse(field(object, "shape")?, "shape", |value| {
            text_known(value, "shape")
        })?,
        stop_policy: obs_parse(field(object, "stop_policy")?, "stop_policy", |value| {
            text_known(value, "stop_policy")
        })?,
        approval_ref: obs_parse(field(object, "approval_ref")?, "approval_ref", |value| {
            text_known(value, "approval_ref")
        })?,
        seed: obs_parse(field(object, "seed")?, "seed", |value| {
            u64_known(value, "seed")
        })?,
        control_writes: enum_field(field(object, "control_writes")?, "control_writes")?,
    })
}

fn parse_clocks(value: &Value) -> Result<Clocks, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("clocks", EnvelopeErrorKind::NotObject))?;
    require_keys(object, CLOCKS)?;
    Ok(Clocks {
        qpc_frequency: u64_known(field(object, "qpc_frequency")?, "qpc_frequency")?,
        qpc_epoch_ticks: u64_known(field(object, "qpc_epoch_ticks")?, "qpc_epoch_ticks")?,
        interrupt_ticks: u64_known(field(object, "interrupt_ticks")?, "interrupt_ticks")?,
        unbiased_ticks: obs_parse(
            field(object, "unbiased_ticks")?,
            "unbiased_ticks",
            |value| u64_known(value, "unbiased_ticks"),
        )?,
        resolution_ns: u64_known(field(object, "resolution_ns")?, "resolution_ns")?,
        utc_context: obs_parse(field(object, "utc_context")?, "utc_context", |value| {
            text_known(value, "utc_context")
        })?,
        discontinuities: u64_known(field(object, "discontinuities")?, "discontinuities")?,
    })
}

fn parse_lifecycle(value: &Value) -> Result<Lifecycle, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("lifecycle", EnvelopeErrorKind::NotObject))?;
    require_keys(object, LIFECYCLE)?;
    Ok(Lifecycle {
        requested_ns: obs_parse(field(object, "requested_ns")?, "requested_ns", |value| {
            u64_known(value, "requested_ns")
        })?,
        launch_ns: obs_parse(field(object, "launch_ns")?, "launch_ns", |value| {
            u64_known(value, "launch_ns")
        })?,
        ready_ns: obs_parse(field(object, "ready_ns")?, "ready_ns", |value| {
            u64_known(value, "ready_ns")
        })?,
        start_ns: obs_parse(field(object, "start_ns")?, "start_ns", |value| {
            u64_known(value, "start_ns")
        })?,
        stop_requested_ns: obs_parse(
            field(object, "stop_requested_ns")?,
            "stop_requested_ns",
            |value| u64_known(value, "stop_requested_ns"),
        )?,
        exit_ns: obs_parse(field(object, "exit_ns")?, "exit_ns", |value| {
            u64_known(value, "exit_ns")
        })?,
        empty_job: enum_field(field(object, "empty_job")?, "empty_job")?,
    })
}

fn parse_dispatches(value: &Value) -> Result<Vec<Dispatch>, EnvelopeError> {
    let values = value
        .as_array()
        .ok_or_else(|| EnvelopeError::new("dispatches", EnvelopeErrorKind::NotArray))?;
    values.iter().map(parse_dispatch).collect()
}

fn parse_dispatch(value: &Value) -> Result<Dispatch, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("dispatches", EnvelopeErrorKind::NotObject))?;
    require_keys(object, DISPATCH)?;
    Ok(Dispatch {
        j: u64_known(field(object, "j")?, "j")?,
        role: enum_field(field(object, "role")?, "role")?,
        shape: text_known(field(object, "shape")?, "shape")?,
        input_digest: hex_known(field(object, "input_digest")?, "input_digest")?,
        submit_ns: obs_parse(field(object, "submit_ns")?, "submit_ns", |value| {
            u64_known(value, "submit_ns")
        })?,
        query_ns: obs_parse(field(object, "query_ns")?, "query_ns", |value| {
            u64_known(value, "query_ns")
        })?,
        readback_ns: obs_parse(field(object, "readback_ns")?, "readback_ns", |value| {
            u64_known(value, "readback_ns")
        })?,
        check_ns: obs_parse(field(object, "check_ns")?, "check_ns", |value| {
            u64_known(value, "check_ns")
        })?,
        return_code: i64_known(field(object, "return_code")?, "return_code")?,
        checked_lanes: u64_known(field(object, "checked_lanes")?, "checked_lanes")?,
        mismatch: u64_known(field(object, "mismatch")?, "mismatch")?,
        nonfinite: u64_known(field(object, "nonfinite")?, "nonfinite")?,
        work: u64_known(field(object, "work")?, "work")?,
        output_digest: obs_parse(field(object, "output_digest")?, "output_digest", |value| {
            hex_known(value, "output_digest")
        })?,
        retention: enum_field(field(object, "retention")?, "retention")?,
    })
}

fn parse_telemetry(value: &Value) -> Result<Vec<Telemetry>, EnvelopeError> {
    let values = value
        .as_array()
        .ok_or_else(|| EnvelopeError::new("telemetry", EnvelopeErrorKind::NotArray))?;
    values.iter().map(parse_telemetry_entry).collect()
}

fn parse_telemetry_entry(value: &Value) -> Result<Telemetry, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("telemetry", EnvelopeErrorKind::NotObject))?;
    require_keys(object, TELEMETRY)?;
    Ok(Telemetry {
        metric: text_known(field(object, "metric")?, "metric")?,
        unit: text_known(field(object, "unit")?, "unit")?,
        device: text_known(field(object, "device")?, "device")?,
        value: obs_parse(field(object, "value")?, "value", |value| {
            telemetry_value_known(value, "value")
        })?,
        slot: u64_known(field(object, "slot")?, "slot")?,
        query_start_ns: obs_parse(
            field(object, "query_start_ns")?,
            "query_start_ns",
            |value| u64_known(value, "query_start_ns"),
        )?,
        query_end_ns: obs_parse(field(object, "query_end_ns")?, "query_end_ns", |value| {
            u64_known(value, "query_end_ns")
        })?,
        receipt_ns: obs_parse(field(object, "receipt_ns")?, "receipt_ns", |value| {
            u64_known(value, "receipt_ns")
        })?,
        internal_age: enum_field(field(object, "internal_age")?, "internal_age")?,
        vendor_code: obs_parse(field(object, "vendor_code")?, "vendor_code", |value| {
            text_known(value, "vendor_code")
        })?,
    })
}

fn parse_integrity(value: &Value) -> Result<Integrity, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("integrity", EnvelopeErrorKind::NotObject))?;
    require_keys(object, INTEGRITY)?;
    Ok(Integrity {
        capacity_bytes: u64_known(field(object, "capacity_bytes")?, "capacity_bytes")?,
        observed_bytes: u64_known(field(object, "observed_bytes")?, "observed_bytes")?,
        failed: u64_known(field(object, "failed")?, "failed")?,
        late: u64_known(field(object, "late")?, "late")?,
        rejected: u64_known(field(object, "rejected")?, "rejected")?,
        overflow: u64_known(field(object, "overflow")?, "overflow")?,
        expected_slots: u64_known(field(object, "expected_slots")?, "expected_slots")?,
        observed_slots: u64_known(field(object, "observed_slots")?, "observed_slots")?,
        raw_records: u64_known(field(object, "raw_records")?, "raw_records")?,
        writer_status: enum_field(field(object, "writer_status")?, "writer_status")?,
    })
}

fn parse_result(value: &Value) -> Result<SessionResult, EnvelopeError> {
    let object = value
        .as_object()
        .ok_or_else(|| EnvelopeError::new("result", EnvelopeErrorKind::NotObject))?;
    require_keys(object, RESULT)?;
    let limitations = field(object, "limitations")?
        .as_array()
        .ok_or_else(|| EnvelopeError::new("limitations", EnvelopeErrorKind::NotArray))?
        .iter()
        .map(|value| reason_known(value, "limitations"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SessionResult {
        admission: enum_field(field(object, "admission")?, "admission")?,
        correctness: enum_field(field(object, "correctness")?, "correctness")?,
        worker_exit: obs_parse(field(object, "worker_exit")?, "worker_exit", |value| {
            i64_known(value, "worker_exit")
        })?,
        sampler_exit: obs_parse(field(object, "sampler_exit")?, "sampler_exit", |value| {
            i64_known(value, "sampler_exit")
        })?,
        empty_job: enum_field(field(object, "empty_job")?, "empty_job")?,
        native_completion: enum_field(field(object, "native_completion")?, "native_completion")?,
        coverage: enum_field(field(object, "coverage")?, "coverage")?,
        limitations,
        verdict: enum_field(field(object, "verdict")?, "verdict")?,
    })
}

// --- encoding -------------------------------------------------------------

fn put(object: &mut Object, key: &'static str, value: Value) -> Result<(), EnvelopeError> {
    object
        .insert(key, value)
        .map_err(|_| EnvelopeError::new(key, EnvelopeErrorKind::DuplicateField))
}

trait EncodeObs {
    fn to_value(&self) -> Value;
}

impl EncodeObs for u64 {
    fn to_value(&self) -> Value {
        Value::U64(*self)
    }
}

impl EncodeObs for i64 {
    fn to_value(&self) -> Value {
        Value::I64(*self)
    }
}

impl EncodeObs for String {
    fn to_value(&self) -> Value {
        Value::Text(self.clone())
    }
}

impl EncodeObs for Digest {
    fn to_value(&self) -> Value {
        Value::Text(self.to_hex())
    }
}

impl EncodeObs for TelemetryValue {
    fn to_value(&self) -> Value {
        match self {
            Self::Int(number) => Value::I64(*number),
            Self::Ratio(ratio) => Value::Ratio(*ratio),
        }
    }
}

fn unavailable_value(reason: &Reason) -> Value {
    let mut object = Object::new();
    object
        .insert("unavailable", Value::Text(reason.as_str().to_owned()))
        .expect("fresh object cannot contain a duplicate key");
    Value::Object(object)
}

fn obs_value<T: EncodeObs>(obs: &Obs<T>) -> Value {
    match obs {
        Obs::Known(value) => value.to_value(),
        Obs::Unavailable(reason) => unavailable_value(reason),
    }
}

fn build_payload(envelope: &Envelope) -> Result<Object, EnvelopeError> {
    let mut object = Object::new();
    put(&mut object, "schema", Value::Text(SCHEMA.to_owned()))?;
    put(&mut object, "version", Value::Text(VERSION.to_owned()))?;
    put(
        &mut object,
        "provenance",
        Value::Text(envelope.provenance.as_str().to_owned()),
    )?;
    put(&mut object, "identity", build_identity(&envelope.identity)?)?;
    put(
        &mut object,
        "artifacts",
        build_artifacts(&envelope.artifacts)?,
    )?;
    put(&mut object, "manifest", build_manifest(&envelope.manifest)?)?;
    put(&mut object, "clocks", build_clocks(&envelope.clocks)?)?;
    put(
        &mut object,
        "lifecycle",
        build_lifecycle(&envelope.lifecycle)?,
    )?;
    let dispatches = envelope
        .dispatches
        .iter()
        .map(build_dispatch)
        .collect::<Result<Vec<_>, _>>()?;
    put(&mut object, "dispatches", Value::Array(dispatches))?;
    let telemetry = envelope
        .telemetry
        .iter()
        .map(build_telemetry)
        .collect::<Result<Vec<_>, _>>()?;
    put(&mut object, "telemetry", Value::Array(telemetry))?;
    put(
        &mut object,
        "integrity",
        build_integrity(&envelope.integrity)?,
    )?;
    put(&mut object, "result", build_result(&envelope.result)?)?;
    Ok(object)
}

fn build_identity(identity: &Identity) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(&mut object, "nonce", Value::Text(identity.nonce.clone()))?;
    put(&mut object, "nvml_uuid", obs_value(&identity.nvml_uuid))?;
    put(&mut object, "cuda_uuid", obs_value(&identity.cuda_uuid))?;
    put(&mut object, "pci_bus_id", obs_value(&identity.pci_bus_id))?;
    put(
        &mut object,
        "context_device",
        obs_value(&identity.context_device),
    )?;
    put(&mut object, "boot", obs_value(&identity.boot))?;
    put(
        &mut object,
        "uuid_match",
        Value::Text(identity.uuid_match.as_str().to_owned()),
    )?;
    put(
        &mut object,
        "pci_match",
        Value::Text(identity.pci_match.as_str().to_owned()),
    )?;
    Ok(Value::Object(object))
}

fn build_artifacts(artifacts: &Artifacts) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(
        &mut object,
        "source_commit",
        Value::Text(artifacts.source_commit.clone()),
    )?;
    put(
        &mut object,
        "worker_sha256",
        obs_value(&artifacts.worker_sha256),
    )?;
    put(
        &mut object,
        "lockfile_sha256",
        obs_value(&artifacts.lockfile_sha256),
    )?;
    put(
        &mut object,
        "sampler_sha256",
        obs_value(&artifacts.sampler_sha256),
    )?;
    put(&mut object, "ptx_sha256", obs_value(&artifacts.ptx_sha256))?;
    put(
        &mut object,
        "kernel_entry",
        Value::Text(artifacts.kernel_entry.clone()),
    )?;
    put(
        &mut object,
        "generator_version",
        Value::Text(artifacts.generator_version.clone()),
    )?;
    put(
        &mut object,
        "oracle_version",
        Value::Text(artifacts.oracle_version.clone()),
    )?;
    Ok(Value::Object(object))
}

fn build_manifest(manifest: &Manifest) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(
        &mut object,
        "manifest_digest",
        obs_value(&manifest.manifest_digest),
    )?;
    put(
        &mut object,
        "profile_digest",
        obs_value(&manifest.profile_digest),
    )?;
    put(&mut object, "shape", obs_value(&manifest.shape))?;
    put(&mut object, "stop_policy", obs_value(&manifest.stop_policy))?;
    put(
        &mut object,
        "approval_ref",
        obs_value(&manifest.approval_ref),
    )?;
    put(&mut object, "seed", obs_value(&manifest.seed))?;
    put(
        &mut object,
        "control_writes",
        Value::Text(manifest.control_writes.as_str().to_owned()),
    )?;
    Ok(Value::Object(object))
}

fn build_clocks(clocks: &Clocks) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(
        &mut object,
        "qpc_frequency",
        Value::U64(clocks.qpc_frequency),
    )?;
    put(
        &mut object,
        "qpc_epoch_ticks",
        Value::U64(clocks.qpc_epoch_ticks),
    )?;
    put(
        &mut object,
        "interrupt_ticks",
        Value::U64(clocks.interrupt_ticks),
    )?;
    put(
        &mut object,
        "unbiased_ticks",
        obs_value(&clocks.unbiased_ticks),
    )?;
    put(
        &mut object,
        "resolution_ns",
        Value::U64(clocks.resolution_ns),
    )?;
    put(&mut object, "utc_context", obs_value(&clocks.utc_context))?;
    put(
        &mut object,
        "discontinuities",
        Value::U64(clocks.discontinuities),
    )?;
    Ok(Value::Object(object))
}

fn build_lifecycle(lifecycle: &Lifecycle) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(
        &mut object,
        "requested_ns",
        obs_value(&lifecycle.requested_ns),
    )?;
    put(&mut object, "launch_ns", obs_value(&lifecycle.launch_ns))?;
    put(&mut object, "ready_ns", obs_value(&lifecycle.ready_ns))?;
    put(&mut object, "start_ns", obs_value(&lifecycle.start_ns))?;
    put(
        &mut object,
        "stop_requested_ns",
        obs_value(&lifecycle.stop_requested_ns),
    )?;
    put(&mut object, "exit_ns", obs_value(&lifecycle.exit_ns))?;
    put(
        &mut object,
        "empty_job",
        Value::Text(lifecycle.empty_job.as_str().to_owned()),
    )?;
    Ok(Value::Object(object))
}

fn build_dispatch(dispatch: &Dispatch) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(&mut object, "j", Value::U64(dispatch.j))?;
    put(
        &mut object,
        "role",
        Value::Text(dispatch.role.as_str().to_owned()),
    )?;
    put(&mut object, "shape", Value::Text(dispatch.shape.clone()))?;
    put(
        &mut object,
        "input_digest",
        Value::Text(dispatch.input_digest.to_hex()),
    )?;
    put(&mut object, "submit_ns", obs_value(&dispatch.submit_ns))?;
    put(&mut object, "query_ns", obs_value(&dispatch.query_ns))?;
    put(&mut object, "readback_ns", obs_value(&dispatch.readback_ns))?;
    put(&mut object, "check_ns", obs_value(&dispatch.check_ns))?;
    put(&mut object, "return_code", Value::I64(dispatch.return_code))?;
    put(
        &mut object,
        "checked_lanes",
        Value::U64(dispatch.checked_lanes),
    )?;
    put(&mut object, "mismatch", Value::U64(dispatch.mismatch))?;
    put(&mut object, "nonfinite", Value::U64(dispatch.nonfinite))?;
    put(&mut object, "work", Value::U64(dispatch.work))?;
    put(
        &mut object,
        "output_digest",
        obs_value(&dispatch.output_digest),
    )?;
    put(
        &mut object,
        "retention",
        Value::Text(dispatch.retention.as_str().to_owned()),
    )?;
    Ok(Value::Object(object))
}

fn build_telemetry(entry: &Telemetry) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(&mut object, "metric", Value::Text(entry.metric.clone()))?;
    put(&mut object, "unit", Value::Text(entry.unit.clone()))?;
    put(&mut object, "device", Value::Text(entry.device.clone()))?;
    put(&mut object, "value", obs_value(&entry.value))?;
    put(&mut object, "slot", Value::U64(entry.slot))?;
    put(
        &mut object,
        "query_start_ns",
        obs_value(&entry.query_start_ns),
    )?;
    put(&mut object, "query_end_ns", obs_value(&entry.query_end_ns))?;
    put(&mut object, "receipt_ns", obs_value(&entry.receipt_ns))?;
    put(
        &mut object,
        "internal_age",
        Value::Text(entry.internal_age.as_str().to_owned()),
    )?;
    put(&mut object, "vendor_code", obs_value(&entry.vendor_code))?;
    Ok(Value::Object(object))
}

fn build_integrity(integrity: &Integrity) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(
        &mut object,
        "capacity_bytes",
        Value::U64(integrity.capacity_bytes),
    )?;
    put(
        &mut object,
        "observed_bytes",
        Value::U64(integrity.observed_bytes),
    )?;
    put(&mut object, "failed", Value::U64(integrity.failed))?;
    put(&mut object, "late", Value::U64(integrity.late))?;
    put(&mut object, "rejected", Value::U64(integrity.rejected))?;
    put(&mut object, "overflow", Value::U64(integrity.overflow))?;
    put(
        &mut object,
        "expected_slots",
        Value::U64(integrity.expected_slots),
    )?;
    put(
        &mut object,
        "observed_slots",
        Value::U64(integrity.observed_slots),
    )?;
    put(
        &mut object,
        "raw_records",
        Value::U64(integrity.raw_records),
    )?;
    put(
        &mut object,
        "writer_status",
        Value::Text(integrity.writer_status.as_str().to_owned()),
    )?;
    Ok(Value::Object(object))
}

fn build_result(result: &SessionResult) -> Result<Value, EnvelopeError> {
    let mut object = Object::new();
    put(
        &mut object,
        "admission",
        Value::Text(result.admission.as_str().to_owned()),
    )?;
    put(
        &mut object,
        "correctness",
        Value::Text(result.correctness.as_str().to_owned()),
    )?;
    put(&mut object, "worker_exit", obs_value(&result.worker_exit))?;
    put(&mut object, "sampler_exit", obs_value(&result.sampler_exit))?;
    put(
        &mut object,
        "empty_job",
        Value::Text(result.empty_job.as_str().to_owned()),
    )?;
    put(
        &mut object,
        "native_completion",
        Value::Text(result.native_completion.as_str().to_owned()),
    )?;
    put(
        &mut object,
        "coverage",
        Value::Text(result.coverage.as_str().to_owned()),
    )?;
    put(
        &mut object,
        "limitations",
        Value::Array(
            result
                .limitations
                .iter()
                .map(|reason| Value::Text(reason.as_str().to_owned()))
                .collect(),
        ),
    )?;
    put(
        &mut object,
        "verdict",
        Value::Text(result.verdict.as_str().to_owned()),
    )?;
    Ok(Value::Object(object))
}

// --- validation -----------------------------------------------------------

fn validate(envelope: &Envelope) -> Result<(), EnvelopeError> {
    validate_identity(&envelope.identity)?;
    validate_artifacts(&envelope.artifacts)?;
    validate_manifest(&envelope.manifest)?;
    validate_clocks(&envelope.clocks)?;
    validate_lifecycle(&envelope.lifecycle)?;
    validate_dispatches(&envelope.lifecycle, &envelope.dispatches)?;
    validate_telemetry(&envelope.telemetry)?;
    validate_integrity(&envelope.integrity)?;
    validate_telemetry_cross(&envelope.telemetry, &envelope.integrity)?;
    validate_empty_job(&envelope.lifecycle, &envelope.result)?;
    validate_verdict(envelope)
}

fn check_text(text: &str, field: &'static str) -> Result<(), EnvelopeError> {
    if text.is_empty() {
        return Err(EnvelopeError::new(field, EnvelopeErrorKind::TextEmpty));
    }
    if text.chars().any(char::is_control) {
        return Err(EnvelopeError::new(field, EnvelopeErrorKind::TextControl));
    }
    Ok(())
}

fn check_known_text(obs: &Obs<String>, field: &'static str) -> Result<(), EnvelopeError> {
    if let Obs::Known(text) = obs {
        check_text(text, field)?;
    }
    Ok(())
}

fn known_u64(obs: &Obs<u64>) -> Option<u64> {
    match obs {
        Obs::Known(value) => Some(*value),
        Obs::Unavailable(_) => None,
    }
}

/// Reject known values that move backwards. Unknown values are skipped, so a
/// partially observed sequence still has to agree wherever both ends are known.
fn require_non_decreasing(fields: &[(&'static str, Option<u64>)]) -> Result<(), EnvelopeError> {
    let mut previous: Option<u64> = None;
    for (name, value) in fields {
        if let Some(value) = *value {
            if let Some(previous) = previous {
                if value < previous {
                    return Err(EnvelopeError::new(name, EnvelopeErrorKind::CrossField));
                }
            }
            previous = Some(value);
        }
    }
    Ok(())
}

fn validate_identity(identity: &Identity) -> Result<(), EnvelopeError> {
    let nonce = &identity.nonce;
    if nonce.is_empty() {
        return Err(EnvelopeError::new("nonce", EnvelopeErrorKind::TextEmpty));
    }
    if nonce.len() > 128 {
        return Err(EnvelopeError::new("nonce", EnvelopeErrorKind::TextLength));
    }
    for character in nonce.chars() {
        if character.is_control() {
            return Err(EnvelopeError::new("nonce", EnvelopeErrorKind::TextControl));
        }
        if !(' '..='~').contains(&character) {
            return Err(EnvelopeError::new("nonce", EnvelopeErrorKind::TextNonAscii));
        }
    }
    check_known_text(&identity.nvml_uuid, "nvml_uuid")?;
    check_known_text(&identity.cuda_uuid, "cuda_uuid")?;
    check_known_text(&identity.pci_bus_id, "pci_bus_id")?;
    check_known_text(&identity.context_device, "context_device")?;
    check_known_text(&identity.boot, "boot")?;
    if identity.uuid_match == MatchState::Match
        && !(identity.nvml_uuid.is_known() && identity.cuda_uuid.is_known())
    {
        return Err(EnvelopeError::new(
            "uuid_match",
            EnvelopeErrorKind::CrossField,
        ));
    }
    if identity.pci_match == MatchState::Match && !identity.pci_bus_id.is_known() {
        return Err(EnvelopeError::new(
            "pci_match",
            EnvelopeErrorKind::CrossField,
        ));
    }
    Ok(())
}

fn validate_artifacts(artifacts: &Artifacts) -> Result<(), EnvelopeError> {
    let commit = &artifacts.source_commit;
    if commit.is_empty() {
        return Err(EnvelopeError::new(
            "source_commit",
            EnvelopeErrorKind::TextEmpty,
        ));
    }
    if commit.chars().any(char::is_control) {
        return Err(EnvelopeError::new(
            "source_commit",
            EnvelopeErrorKind::TextControl,
        ));
    }
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EnvelopeError::new("source_commit", EnvelopeErrorKind::Hex));
    }
    check_text(&artifacts.kernel_entry, "kernel_entry")?;
    check_text(&artifacts.generator_version, "generator_version")?;
    check_text(&artifacts.oracle_version, "oracle_version")
}

fn validate_manifest(manifest: &Manifest) -> Result<(), EnvelopeError> {
    check_known_text(&manifest.shape, "shape")?;
    check_known_text(&manifest.stop_policy, "stop_policy")?;
    check_known_text(&manifest.approval_ref, "approval_ref")
}

fn validate_clocks(clocks: &Clocks) -> Result<(), EnvelopeError> {
    if clocks.qpc_frequency == 0 {
        return Err(EnvelopeError::new(
            "qpc_frequency",
            EnvelopeErrorKind::Range,
        ));
    }
    if clocks.resolution_ns == 0 {
        return Err(EnvelopeError::new(
            "resolution_ns",
            EnvelopeErrorKind::Range,
        ));
    }
    check_known_text(&clocks.utc_context, "utc_context")
}

fn validate_lifecycle(lifecycle: &Lifecycle) -> Result<(), EnvelopeError> {
    require_non_decreasing(&[
        ("requested_ns", known_u64(&lifecycle.requested_ns)),
        ("launch_ns", known_u64(&lifecycle.launch_ns)),
        ("ready_ns", known_u64(&lifecycle.ready_ns)),
        ("start_ns", known_u64(&lifecycle.start_ns)),
        ("stop_requested_ns", known_u64(&lifecycle.stop_requested_ns)),
        ("exit_ns", known_u64(&lifecycle.exit_ns)),
    ])
}

fn validate_dispatches(
    lifecycle: &Lifecycle,
    dispatches: &[Dispatch],
) -> Result<(), EnvelopeError> {
    let start_ns = known_u64(&lifecycle.start_ns);
    for (index, dispatch) in dispatches.iter().enumerate() {
        check_text(&dispatch.shape, "shape")?;
        let ordered = if index == 0 {
            dispatch.j == 0
        } else {
            dispatch.j > dispatches[index - 1].j
        };
        if !ordered {
            return Err(EnvelopeError::new("j", EnvelopeErrorKind::CrossField));
        }
        require_non_decreasing(&[
            ("submit_ns", known_u64(&dispatch.submit_ns)),
            ("query_ns", known_u64(&dispatch.query_ns)),
            ("readback_ns", known_u64(&dispatch.readback_ns)),
            ("check_ns", known_u64(&dispatch.check_ns)),
        ])?;
        if let (Some(start_ns), Some(submit_ns)) = (start_ns, known_u64(&dispatch.submit_ns)) {
            if submit_ns < start_ns {
                return Err(EnvelopeError::new(
                    "submit_ns",
                    EnvelopeErrorKind::CrossField,
                ));
            }
        }
        if dispatch.mismatch > dispatch.checked_lanes {
            return Err(EnvelopeError::new(
                "mismatch",
                EnvelopeErrorKind::CrossField,
            ));
        }
        if dispatch.nonfinite > dispatch.checked_lanes {
            return Err(EnvelopeError::new(
                "nonfinite",
                EnvelopeErrorKind::CrossField,
            ));
        }
        match dispatch.retention {
            Retention::FullSuccess => {
                return Err(EnvelopeError::new(
                    "retention",
                    EnvelopeErrorKind::RetentionFullSuccess,
                ));
            }
            Retention::DigestOnly if !dispatch.output_digest.is_known() => {
                return Err(EnvelopeError::new(
                    "output_digest",
                    EnvelopeErrorKind::CrossField,
                ));
            }
            Retention::FullFailure if dispatch.mismatch == 0 && dispatch.nonfinite == 0 => {
                return Err(EnvelopeError::new(
                    "retention",
                    EnvelopeErrorKind::CrossField,
                ));
            }
            Retention::DigestOnly | Retention::FullFailure => {}
        }
    }
    Ok(())
}

fn validate_telemetry(telemetry: &[Telemetry]) -> Result<(), EnvelopeError> {
    for entry in telemetry {
        check_text(&entry.metric, "metric")?;
        check_text(&entry.unit, "unit")?;
        check_text(&entry.device, "device")?;
        check_known_text(&entry.vendor_code, "vendor_code")?;
        require_non_decreasing(&[
            ("query_start_ns", known_u64(&entry.query_start_ns)),
            ("query_end_ns", known_u64(&entry.query_end_ns)),
            ("receipt_ns", known_u64(&entry.receipt_ns)),
        ])?;
        if entry.internal_age == InternalAge::Assessed
            && !(entry.query_start_ns.is_known()
                && entry.query_end_ns.is_known()
                && entry.receipt_ns.is_known())
        {
            return Err(EnvelopeError::new(
                "internal_age",
                EnvelopeErrorKind::CrossField,
            ));
        }
    }
    Ok(())
}

fn validate_integrity(integrity: &Integrity) -> Result<(), EnvelopeError> {
    if integrity.capacity_bytes == 0 {
        return Err(EnvelopeError::new(
            "capacity_bytes",
            EnvelopeErrorKind::Range,
        ));
    }
    if integrity.observed_bytes > integrity.capacity_bytes {
        return Err(EnvelopeError::new(
            "observed_bytes",
            EnvelopeErrorKind::CrossField,
        ));
    }
    if integrity.observed_slots > integrity.expected_slots {
        return Err(EnvelopeError::new(
            "observed_slots",
            EnvelopeErrorKind::CrossField,
        ));
    }
    if integrity.writer_status == WriterStatus::Unbounded {
        return Err(EnvelopeError::new(
            "writer_status",
            EnvelopeErrorKind::WriterUnbounded,
        ));
    }
    Ok(())
}

fn validate_telemetry_cross(
    telemetry: &[Telemetry],
    integrity: &Integrity,
) -> Result<(), EnvelopeError> {
    for (index, entry) in telemetry.iter().enumerate() {
        if telemetry[..index]
            .iter()
            .any(|other| other.metric == entry.metric && other.slot == entry.slot)
        {
            return Err(EnvelopeError::new("slot", EnvelopeErrorKind::CrossField));
        }
    }
    // Pair uniqueness above makes each entry one metric-slot observation.
    // Aggregate cardinality does not define per-metric slot-index ranges.
    if telemetry.len() as u64 != integrity.observed_slots {
        return Err(EnvelopeError::new(
            "observed_slots",
            EnvelopeErrorKind::CrossField,
        ));
    }
    Ok(())
}

fn validate_empty_job(lifecycle: &Lifecycle, result: &SessionResult) -> Result<(), EnvelopeError> {
    let agrees = match lifecycle.empty_job {
        EmptyJobLifecycle::ObservedEmpty => result.empty_job == ResultEmptyJob::Observed,
        EmptyJobLifecycle::ObservedNonempty => result.empty_job == ResultEmptyJob::NotObserved,
        EmptyJobLifecycle::Unknown => result.empty_job == ResultEmptyJob::Unknown,
    };
    if !agrees {
        return Err(EnvelopeError::new(
            "empty_job",
            EnvelopeErrorKind::CrossField,
        ));
    }
    Ok(())
}

fn validate_verdict(envelope: &Envelope) -> Result<(), EnvelopeError> {
    let result = &envelope.result;
    if result.verdict != Verdict::Usable {
        return Ok(());
    }
    let identity = &envelope.identity;
    let integrity = &envelope.integrity;
    let measured: Vec<&Dispatch> = envelope
        .dispatches
        .iter()
        .filter(|dispatch| dispatch.role == Role::Measured)
        .collect();
    let measured_ok = !measured.is_empty()
        && measured.iter().all(|dispatch| {
            dispatch.retention == Retention::DigestOnly
                && dispatch.output_digest.is_known()
                && dispatch.return_code == 0
                && dispatch.mismatch == 0
                && dispatch.nonfinite == 0
                && dispatch.checked_lanes > 0
        });
    let sampler_ok = match result.sampler_exit {
        Obs::Known(0) | Obs::Unavailable(_) => true,
        Obs::Known(_) => false,
    };
    let integrity_ok = integrity.failed == 0
        && integrity.late == 0
        && integrity.rejected == 0
        && integrity.overflow == 0
        && integrity.observed_slots == integrity.expected_slots;
    let identity_ok =
        identity.uuid_match == MatchState::Match && identity.pci_match == MatchState::Match;
    let supported = result.admission == Admission::Admitted
        && result.correctness == Correctness::Passed
        && result.coverage == Coverage::Complete
        && result.limitations.is_empty()
        && result.worker_exit == Obs::Known(0)
        && sampler_ok
        && measured_ok
        && integrity_ok
        && identity_ok;
    if !supported {
        return Err(EnvelopeError::new("verdict", EnvelopeErrorKind::CrossField));
    }
    Ok(())
}
