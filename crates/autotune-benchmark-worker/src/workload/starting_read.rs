//! Strict parent-side reader for the bounded starting response. It re-verifies
//! canonicality, the parent's frozen scope/declaration/epoch binding and the
//! delivered payload digest before exposing any retained fact. It creates no
//! process, issues no workload admission and proves neither execution nor
//! qualification.
use super::{starting::Scope, starting_bootstrap::Declaration};
use crate::{
    evidence::{decode, Digest, Domain, Object, Value},
    monitor::Anchor,
    sampler::budget::{Budget, Reservation},
};
/// Hard bound on the canonical response, matching the exporter.
pub const CAP: usize = super::starting_export::CAP;
/// Fixed bound shared with the per-call ledger.
const CALLS_MAX: usize = 3077;
const OPERATIONS: [&str; 4] = ["open", "describe", "read", "cleanup"];
const ANCHOR: [&str; 4] = [
    "qpc_before",
    "qpc_after",
    "interrupt_100ns",
    "unbiased_100ns",
];
const TOP: [&str; 24] = [
    "version",
    "scope",
    "request_digest",
    "request",
    "declaration",
    "frequency",
    "epoch",
    "first_entry",
    "receipt",
    "deadline_ns",
    "info",
    "native_calls",
    "native_fault",
    "native_overflow",
    "native_rejected",
    "snapshot",
    "snapshot_error",
    "trace",
    "control",
    "native_failure",
    "clock_error",
    "control_error",
    "event_clone_error",
    "unreported_completion",
];
const NATIVE_FAULTS: [&str; 5] = ["capacity", "clock", "deadline", "stopped", "protocol"];
const TRACE_FAULTS: [&str; 9] = [
    "capacity", "clock", "deadline", "control", "protocol", "runtime", "read", "cleanup", "drain",
];
const CONTROL_FAULTS: [&str; 6] = [
    "clock",
    "capacity",
    "deadline",
    "protocol",
    "transport",
    "cleanup",
];
const SOURCES: [&str; 2] = ["native_getter_path", "simulated_fixture"];
const REASONS: [&str; 9] = [
    "runtime_missing",
    "runtime_load_failed",
    "symbol_missing",
    "unsupported",
    "permission_denied",
    "query_failed",
    "gpu_lost",
    "invalid_value",
    "unsupported_platform",
];
const BLOCKS: [&str; 11] = [
    "capacity", "identity", "clock", "phase", "stopped", "gpu_lost", "missing", "unit", "symbol",
    "value", "scope",
];
/// Typed rejection reason. No partially accepted response escapes the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Incomplete,
    Capacity,
    Parse(crate::evidence::ParseError),
    Shape,
    Scope,
    Declaration,
    Epoch,
    Deadline,
    Digest,
    Payload,
}
/// Validated retained facts. A missing value stays explicit rather than being
/// synthesized or treated as success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub frequency: u64,
    pub epoch: Anchor,
    pub first_entry: Anchor,
    pub receipt: Anchor,
    pub deadline_ns: u64,
    pub native_calls: usize,
    pub native_fault: Option<String>,
    pub snapshot: Option<Digest>,
    pub snapshot_error: Option<String>,
    pub trace_fault: Option<String>,
    pub cleanup_status: Option<i64>,
    pub output_drained: bool,
    pub control_finished: bool,
    pub completion: Option<Anchor>,
    pub observed_frequency: Option<u64>,
    pub control_fault: Option<String>,
    pub clock_error: Option<String>,
    pub control_error: Option<String>,
    pub native_failure: Option<String>,
}
/// One strictly read response bound to the parent's frozen scope/declaration.
pub struct Receipt {
    summary: Summary,
    snapshot: Option<super::starting::ReportedSnapshot>,
    canonical: String,
    digest: Digest,
    scope: Digest,
    declaration: Digest,
    _allocation: Reservation,
}
impl Receipt {
    /// Read at most `CAP` bytes. Capacity is reserved first, then the document is
    /// decoded canonically and every binding is re-verified against the parent's
    /// scope/declaration, the original read deadline and the delivered digest.
    /// The chunk/session arrival deadline is checked separately by reception.
    pub fn read(
        bytes: &[u8],
        scope: &Scope,
        declaration: &Declaration,
        expected: Digest,
        deadline_ns: u64,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        if bytes.len() > CAP {
            return Err(Fault::Capacity);
        }
        if declaration.scope_digest() != scope.digest() {
            return Err(Fault::Declaration);
        }
        if deadline_ns
            != autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .preflight_ns
        {
            return Err(Fault::Deadline);
        }
        let _codec = budget.reserve(CAP * 8).map_err(|_| Fault::Capacity)?;
        crate::sampler::wire::validate_complexity_limit_wide(bytes, 131072)
            .map_err(|_| Fault::Capacity)?;
        let allocation = budget
            .reserve(CAP + std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let value = decode(bytes, CAP).map_err(Fault::Parse)?;
        let document = match value {
            Value::Object(document) => document,
            _ => return Err(Fault::Shape),
        };
        document.require_exact(&TOP).map_err(|_| Fault::Shape)?;
        if text(&document, "version")? != "1"
            || text(&document, "scope")? != "pilot_starting_settings_response"
            || text(&document, "request_digest")? != scope.digest().to_hex()
            || text(&document, "request")? != scope.canonical()
        {
            return Err(Fault::Scope);
        }
        if text(&document, "declaration")? != declaration.tag().to_hex() {
            return Err(Fault::Declaration);
        }
        let d = scope.draft();
        let frequency = uint(&document, "frequency")?;
        if frequency != d.frequency {
            return Err(Fault::Epoch);
        }
        let with_token = |value| {
            anchor(value).map(|mut a| {
                a.clock_token = d.clock_token;
                a
            })
        };
        let epoch = with_token(field(&document, "epoch")?)?;
        let first_entry = with_token(field(&document, "first_entry")?)?;
        let receipt = with_token(field(&document, "receipt")?)?;
        if epoch != d.epoch {
            return Err(Fault::Epoch);
        }
        let mut clock = crate::monitor::clock::Clock::temporal(d.frequency, d.clock_token, epoch)
            .map_err(|_| Fault::Epoch)?;
        let mut previous = epoch;
        for a in [declaration.entry(), first_entry, receipt] {
            if a.qpc_before < previous.qpc_after {
                return Err(Fault::Epoch);
            }
            if clock.observe(a).map_err(|_| Fault::Epoch)? >= deadline_ns {
                return Err(Fault::Deadline);
            }
            previous = a;
        }
        let observed_deadline = uint(&document, "deadline_ns")?;
        if observed_deadline != deadline_ns {
            return Err(Fault::Deadline);
        }
        info(field(&document, "info")?, scope)?;
        let native_calls = calls(field(&document, "native_calls")?)?;
        let native_fault = native_fault(field(&document, "native_fault")?)?;
        overflow(field(&document, "native_overflow")?)?;
        rejected(field(&document, "native_rejected")?)?;
        let snapshot = snapshot(field(&document, "snapshot")?, scope, budget)?;
        optional_text(field(&document, "snapshot_error")?, &BLOCKS)?;
        trace(field(&document, "trace")?)?;
        control(field(&document, "control")?)?;
        native_failure(field(&document, "native_failure")?)?;
        clock_error(field(&document, "clock_error")?)?;
        optional_text(field(&document, "control_error")?, &CONTROL_FAULTS)?;
        optional_u32(field(&document, "event_clone_error")?)?;
        if !matches!(field(&document, "unreported_completion")?, Value::Null) {
            completion(field(&document, "unreported_completion")?)?;
        }
        if Digest::of(Domain::RawArtifact, bytes) != expected {
            return Err(Fault::Digest);
        }
        let trace_object = object(field(&document, "trace")?)?;
        let control_object = object(field(&document, "control")?)?;
        let summary = Summary {
            frequency,
            epoch,
            first_entry,
            receipt,
            deadline_ns: observed_deadline,
            native_calls,
            native_fault,
            snapshot: snapshot.as_ref().map(|s| s.snapshot().digest()),
            snapshot_error: optional_text(field(&document, "snapshot_error")?, &BLOCKS)?,
            trace_fault: optional_text(field(trace_object, "fault")?, &TRACE_FAULTS)?,
            cleanup_status: optional_int(field(trace_object, "cleanup_status")?)?,
            output_drained: boolean_field(trace_object, "output_drained")?,
            control_finished: boolean_field(trace_object, "control_finished")?,
            completion: match field(control_object, "completion")? {
                Value::Null => None,
                c => Some(with_token(field(object(c)?, "anchor")?)?),
            },
            observed_frequency: optional_uint(field(control_object, "observed_frequency")?)?,
            control_fault: optional_text(field(control_object, "fault")?, &CONTROL_FAULTS)?,
            clock_error: clock_error_kind(field(&document, "clock_error")?),
            control_error: optional_text(field(&document, "control_error")?, &CONTROL_FAULTS)?,
            native_failure: match field(&document, "native_failure")? {
                Value::Null => None,
                other => Some(text(object(other)?, "reason")?.to_owned()),
            },
        };
        Ok(Self {
            summary,
            snapshot,
            canonical: String::from_utf8(bytes.to_vec()).map_err(|_| Fault::Shape)?,
            digest: expected,
            scope: scope.digest(),
            declaration: declaration.tag(),
            _allocation: allocation,
        })
    }
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
    /// Strict reported artifact for subsequent reference/readability checks.
    /// Source claims authenticate no actual native read or consent.
    pub fn snapshot(&self) -> Option<&super::starting::ReportedSnapshot> {
        self.snapshot.as_ref()
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    pub fn scope_digest(&self) -> Digest {
        self.scope
    }
    pub fn declaration(&self) -> Digest {
        self.declaration
    }
}
fn field<'a>(object: &'a Object, key: &str) -> Result<&'a Value, Fault> {
    object.get(key).ok_or(Fault::Shape)
}
fn object(value: &Value) -> Result<&Object, Fault> {
    value.as_object().ok_or(Fault::Shape)
}
fn array(value: &Value) -> Result<&[Value], Fault> {
    value.as_array().ok_or(Fault::Shape)
}
fn text<'a>(object: &'a Object, key: &str) -> Result<&'a str, Fault> {
    field(object, key)?.as_text().ok_or(Fault::Shape)
}
fn uint(object: &Object, key: &str) -> Result<u64, Fault> {
    field(object, key)?.as_u64().map_err(|_| Fault::Shape)
}
fn int(object: &Object, key: &str) -> Result<i64, Fault> {
    field(object, key)?.as_i64().map_err(|_| Fault::Shape)
}
fn boolean(value: &Value) -> Result<bool, Fault> {
    match value {
        Value::Bool(value) => Ok(*value),
        _ => Err(Fault::Shape),
    }
}
fn boolean_field(object: &Object, key: &str) -> Result<bool, Fault> {
    boolean(field(object, key)?)
}
fn exact<'a>(value: &'a Value, keys: &[&str]) -> Result<&'a Object, Fault> {
    let object = object(value)?;
    object.require_exact(keys).map_err(|_| Fault::Shape)?;
    Ok(object)
}
fn optional_uint(value: &Value) -> Result<Option<u64>, Fault> {
    match value {
        Value::Null => Ok(None),
        other => other.as_u64().map(Some).map_err(|_| Fault::Shape),
    }
}
fn optional_int(value: &Value) -> Result<Option<i64>, Fault> {
    match value {
        Value::Null => Ok(None),
        other => other.as_i64().map(Some).map_err(|_| Fault::Shape),
    }
}
fn optional_u32(value: &Value) -> Result<Option<u32>, Fault> {
    optional_uint(value)?
        .map(|n| u32::try_from(n).map_err(|_| Fault::Shape))
        .transpose()
}
fn optional_i32(value: &Value) -> Result<Option<i32>, Fault> {
    optional_int(value)?
        .map(|n| i32::try_from(n).map_err(|_| Fault::Shape))
        .transpose()
}
fn optional_text(value: &Value, allowed: &[&str]) -> Result<Option<String>, Fault> {
    match value {
        Value::Null => Ok(None),
        Value::Text(text) => {
            if !allowed.is_empty() && !allowed.contains(&text.as_str()) {
                return Err(Fault::Shape);
            }
            Ok(Some(text.clone()))
        }
        _ => Err(Fault::Shape),
    }
}
fn anchor(value: &Value) -> Result<Anchor, Fault> {
    let object = exact(value, &ANCHOR)?;
    Ok(Anchor {
        clock_token: [0; 16],
        qpc_before: uint(object, "qpc_before")?,
        qpc_after: uint(object, "qpc_after")?,
        interrupt_100ns: uint(object, "interrupt_100ns")?,
        unbiased_100ns: uint(object, "unbiased_100ns")?,
    })
}
fn optional_anchor(value: &Value) -> Result<Option<Anchor>, Fault> {
    match value {
        Value::Null => Ok(None),
        other => anchor(other).map(Some),
    }
}
fn info(value: &Value, scope: &Scope) -> Result<(), Fault> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    let object = exact(value, &["raw_uuid", "raw_pci", "runtime"])?;
    if text(object, "raw_uuid")? != scope.draft().raw_uuid
        || text(object, "raw_pci")? != scope.draft().raw_pci
    {
        return Err(Fault::Scope);
    }
    match field(object, "runtime")? {
        Value::Null => Ok(()),
        Value::Text(raw) => {
            crate::sampler::wire::validate_complexity_limit(raw.as_bytes(), 256)
                .map_err(|_| Fault::Capacity)?;
            let runtime: autotune_probe::report::Runtime =
                serde_json::from_str(raw).map_err(|_| Fault::Shape)?;
            if serde_json::to_string(&runtime).map_err(|_| Fault::Shape)? != *raw {
                return Err(Fault::Shape);
            }
            Ok(())
        }
        _ => Err(Fault::Shape),
    }
}
fn calls(value: &Value) -> Result<usize, Fault> {
    let entries = array(value)?;
    if entries.len() > CALLS_MAX {
        return Err(Fault::Shape);
    }
    for entry in entries {
        let object = exact(entry, &["call", "before", "after", "attempted", "raw"])?;
        call_name(text(object, "call")?)?;
        optional_anchor(field(object, "before")?)?;
        optional_anchor(field(object, "after")?)?;
        boolean_field(object, "attempted")?;
        if !matches!(field(object, "raw")?, Value::Null) {
            raw_return(field(object, "raw")?)?;
        }
    }
    Ok(entries.len())
}
fn call_name(name: &str) -> Result<(), Fault> {
    if [
        "initialize",
        "count",
        "cuda_version",
        "driver_version",
        "nvml_version",
    ]
    .contains(&name)
    {
        return Ok(());
    }
    for prefix in ["handle:", "uuid:", "pci:"] {
        if let Some(index) = name.strip_prefix(prefix) {
            let parsed = index.parse::<u32>().map_err(|_| Fault::Shape)?;
            if parsed.to_string() == index {
                return Ok(());
            }
        }
    }
    Err(Fault::Shape)
}
fn raw_return(value: &Value) -> Result<(), Fault> {
    let object = exact(value, &["status", "output"])?;
    i32::try_from(int(object, "status")?).map_err(|_| Fault::Shape)?;
    output(field(object, "output")?)
}
fn output(value: &Value) -> Result<(), Fault> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    let object = object(value)?;
    match text(object, "kind")? {
        "count" => {
            exact(value, &["kind", "value"])?;
            u32::try_from(uint(object, "value")?).map_err(|_| Fault::Shape)?;
        }
        "handle_is_null" => {
            exact(value, &["kind", "value"])?;
            boolean_field(object, "value")?;
        }
        "text" => {
            exact(value, &["kind", "hex"])?;
            hex_text(text(object, "hex")?, 256)?;
        }
        "pci" => {
            exact(value, &["kind", "legacy", "numbers", "bus_id"])?;
            hex_text(text(object, "legacy")?, 16)?;
            let numbers = array(field(object, "numbers")?)?;
            if numbers.len() != 5 {
                return Err(Fault::Shape);
            }
            for number in numbers {
                u32::try_from(number.as_u64().map_err(|_| Fault::Shape)?)
                    .map_err(|_| Fault::Shape)?;
            }
            hex_text(text(object, "bus_id")?, 32)?;
        }
        "version" => {
            exact(value, &["kind", "value"])?;
            i32::try_from(int(object, "value")?).map_err(|_| Fault::Shape)?;
        }
        _ => return Err(Fault::Shape),
    }
    Ok(())
}
fn hex_text(text: &str, bytes: usize) -> Result<(), Fault> {
    if text.len() != bytes * 2
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Fault::Shape);
    }
    Ok(())
}
fn native_fault(value: &Value) -> Result<Option<String>, Fault> {
    match value {
        Value::Null => Ok(None),
        Value::Text(text) if NATIVE_FAULTS.contains(&text.as_str()) => Ok(Some(text.clone())),
        Value::Text(text) => match text.strip_prefix("native:") {
            Some(status) => {
                let n = status.parse::<i32>().map_err(|_| Fault::Shape)?;
                if n.to_string() != status {
                    return Err(Fault::Shape);
                }
                Ok(Some(text.clone()))
            }
            None => Err(Fault::Shape),
        },
        _ => Err(Fault::Shape),
    }
}
fn overflow(value: &Value) -> Result<(), Fault> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    let object = exact(value, &["call", "anchor"])?;
    call_name(text(object, "call")?)?;
    optional_anchor(field(object, "anchor")?)?;
    Ok(())
}
fn rejected(value: &Value) -> Result<(), Fault> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    let object = exact(value, &["call", "return", "anchor"])?;
    call_name(text(object, "call")?)?;
    raw_return(field(object, "return")?)?;
    optional_anchor(field(object, "anchor")?)?;
    Ok(())
}
fn snapshot(
    value: &Value,
    scope: &Scope,
    budget: &Budget,
) -> Result<Option<super::starting::ReportedSnapshot>, Fault> {
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    let object = exact(value, &["canonical", "digest", "source"])?;
    if !SOURCES.contains(&text(object, "source")?) {
        return Err(Fault::Shape);
    }
    let canonical = text(object, "canonical")?.as_bytes();
    let digest = Digest::parse(text(object, "digest")?).map_err(|_| Fault::Payload)?;
    let report = super::starting::ReportedSnapshot::parse(scope, canonical, digest, budget)
        .map_err(|_| Fault::Payload)?;
    let source = match report.snapshot().source() {
        super::settings::Source::NativeGetterPath => "native_getter_path",
        super::settings::Source::SimulatedFixture => "simulated_fixture",
    };
    if text(object, "source")? != source {
        return Err(Fault::Payload);
    }
    Ok(Some(report))
}
fn trace(value: &Value) -> Result<(), Fault> {
    let object = exact(
        value,
        &[
            "calls",
            "fault",
            "cleanup_status",
            "stop_trigger",
            "drain_receipt",
            "output_drained",
            "control_finished",
        ],
    )?;
    let calls = array(field(object, "calls")?)?;
    if calls.len() != OPERATIONS.len() {
        return Err(Fault::Shape);
    }
    for (entry, name) in calls.iter().zip(OPERATIONS) {
        let call = exact(
            entry,
            &[
                "operation",
                "requested",
                "before",
                "after",
                "attempted",
                "returned",
            ],
        )?;
        if text(call, "operation")? != name {
            return Err(Fault::Shape);
        }
        optional_anchor(field(call, "before")?)?;
        boolean_field(call, "requested")?;
        optional_anchor(field(call, "after")?)?;
        boolean_field(call, "attempted")?;
        boolean_field(call, "returned")?;
    }
    optional_text(field(object, "fault")?, &TRACE_FAULTS)?;
    optional_i32(field(object, "cleanup_status")?)?;
    optional_anchor(field(object, "stop_trigger")?)?;
    optional_anchor(field(object, "drain_receipt")?)?;
    boolean_field(object, "output_drained")?;
    boolean_field(object, "control_finished")?;
    Ok(())
}
fn control(value: &Value) -> Result<(), Fault> {
    let object = exact(
        value,
        &[
            "completion",
            "completion_receipt",
            "finished",
            "fault",
            "window",
            "trigger",
            "event_status",
            "event_error",
            "transport_error",
            "observed_frequency",
            "rejected_receipt",
        ],
    )?;
    if !matches!(field(object, "completion")?, Value::Null) {
        completion(field(object, "completion")?)?;
    }
    optional_anchor(field(object, "completion_receipt")?)?;
    match field(object, "finished")? {
        Value::Null | Value::Bool(_) => {}
        _ => return Err(Fault::Shape),
    }
    optional_text(field(object, "fault")?, &CONTROL_FAULTS)?;
    if !matches!(field(object, "window")?, Value::Null) {
        let window = exact(
            field(object, "window")?,
            &["trigger_ns", "cooperative_ns", "exit_ns"],
        )?;
        uint(window, "trigger_ns")?;
        uint(window, "cooperative_ns")?;
        uint(window, "exit_ns")?;
    }
    optional_anchor(field(object, "trigger")?)?;
    optional_u32(field(object, "event_status")?)?;
    optional_u32(field(object, "event_error")?)?;
    optional_i32(field(object, "transport_error")?)?;
    optional_uint(field(object, "observed_frequency")?)?;
    optional_anchor(field(object, "rejected_receipt")?)?;
    Ok(())
}
fn completion(value: &Value) -> Result<(), Fault> {
    let object = exact(
        value,
        &[
            "anchor",
            "initialized_session",
            "cleanup_attempted",
            "cleanup_status",
        ],
    )?;
    anchor(field(object, "anchor")?)?;
    boolean_field(object, "initialized_session")?;
    boolean_field(object, "cleanup_attempted")?;
    optional_i32(field(object, "cleanup_status")?)?;
    Ok(())
}
fn native_failure(value: &Value) -> Result<(), Fault> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    let object = exact(value, &["reason", "vendor_status", "detail"])?;
    if !REASONS.contains(&text(object, "reason")?) {
        return Err(Fault::Shape);
    }
    optional_i32(field(object, "vendor_status")?)?;
    text(object, "detail")?;
    Ok(())
}
fn clock_error(value: &Value) -> Result<(), Fault> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    let object = object(value)?;
    match text(object, "kind")? {
        "zero_token" => {
            exact(value, &["kind"])?;
        }
        "frequency_failed" => {
            exact(value, &["kind", "code"])?;
            optional_u32(field(object, "code")?)?;
        }
        "invalid_frequency" => {
            exact(value, &["kind", "value"])?;
            int(object, "value")?;
        }
        "counter_failed" => {
            exact(value, &["kind", "at", "code"])?;
            reading(text(object, "at")?)?;
            optional_u32(field(object, "code")?)?;
        }
        "invalid_counter" => {
            exact(value, &["kind", "at", "value"])?;
            reading(text(object, "at")?)?;
            int(object, "value")?;
        }
        "anchor_order" => {
            exact(value, &["kind", "before", "after"])?;
            uint(object, "before")?;
            uint(object, "after")?;
        }
        _ => return Err(Fault::Shape),
    }
    Ok(())
}
fn clock_error_kind(value: &Value) -> Option<String> {
    object(value)
        .ok()
        .and_then(|object| object.get("kind"))
        .and_then(Value::as_text)
        .map(str::to_owned)
}
fn reading(value: &str) -> Result<(), Fault> {
    match value {
        "before" | "after" => Ok(()),
        _ => Err(Fault::Shape),
    }
}
