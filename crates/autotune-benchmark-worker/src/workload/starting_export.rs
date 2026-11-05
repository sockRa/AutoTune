//! Bounded canonical starting-readout response. This is raw retained evidence,
//! not native execution proof, consent, an admission token or a baseline verdict.
use super::{
    gate::StopWindow,
    nvml_trace::{self, Ledger},
    settings::{self, Block, Source},
    starting::{Scope, Snapshot},
    starting_bootstrap::Declaration,
    starting_control::{Completion, Fault as ControlFault},
    starting_runtime::{self, Trace},
};
use crate::{
    evidence::{Digest, Domain, Value},
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{anchor_value, hex, object},
    },
};
use autotune_probe::{
    nvml::{SamplerCall, SamplerInfo, SamplerOutput, SamplerReturn},
    report::Failure as NativeFailure,
};
pub const CAP: usize = 2 * 1024 * 1024;
const OPERATIONS: [&str; 4] = ["open", "describe", "read", "cleanup"];
/// Raw clock fault carried without inventing an anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockFault {
    ZeroToken,
    FrequencyFailed(u32),
    InvalidFrequency(i64),
    CounterFailed { before: bool, code: u32 },
    InvalidCounter { before: bool, value: i64 },
    AnchorOrder { before: u64, after: u64 },
}
#[cfg(any(windows, test))]
impl From<crate::monitor::windows_clock::CaptureError> for ClockFault {
    fn from(e: crate::monitor::windows_clock::CaptureError) -> Self {
        use crate::monitor::windows_clock::{CaptureError, CounterReading};
        match e {
            CaptureError::ZeroToken => Self::ZeroToken,
            CaptureError::FrequencyFailed(code) => Self::FrequencyFailed(code),
            CaptureError::InvalidFrequency(value) => Self::InvalidFrequency(value),
            CaptureError::CounterFailed { at, code } => Self::CounterFailed {
                before: at == CounterReading::Before,
                code,
            },
            CaptureError::InvalidCounter { at, value } => Self::InvalidCounter {
                before: at == CounterReading::Before,
                value,
            },
            CaptureError::AnchorOrder { before, after } => Self::AnchorOrder { before, after },
        }
    }
}
/// Observations owned by the independent control watcher. Absence is explicit.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlFacts {
    pub completion: Option<Completion>,
    pub completion_receipt: Option<Anchor>,
    pub finished: Option<bool>,
    pub fault: Option<ControlFault>,
    pub window: Option<StopWindow>,
    pub trigger: Option<Anchor>,
    pub event_status: Option<u32>,
    pub event_error: Option<u32>,
    pub transport_error: Option<i32>,
    pub observed_frequency: Option<u64>,
    pub rejected_receipt: Option<Anchor>,
}
pub struct Binding<'a> {
    pub scope: &'a Scope,
    pub declaration: &'a Declaration,
    pub first_entry: Anchor,
    pub receipt: Anchor,
    pub deadline_ns: u64,
}
/// Borrowed facts only; constructors never synthesize a missing anchor or value.
pub struct Facts<'a> {
    pub info: Option<&'a SamplerInfo>,
    pub ledger: &'a Ledger,
    pub snapshot: Option<&'a Snapshot>,
    pub snapshot_error: Option<Block>,
    pub trace: &'a Trace,
    pub control: ControlFacts,
    pub native_failure: Option<&'a NativeFailure>,
    pub clock_error: Option<ClockFault>,
    pub control_error: Option<ControlFault>,
    pub event_clone_error: Option<u32>,
    pub unreported_completion: Option<Completion>,
}
pub struct Response {
    scope: Digest,
    declaration: Digest,
    canonical: String,
    digest: Digest,
    _allocation: Reservation,
}
impl Response {
    pub fn scope_digest(&self) -> Digest {
        self.scope
    }
    pub fn declaration(&self) -> Digest {
        self.declaration
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    /// Reject foreign/stale scope or declaration before encoding. Retained faults
    /// and a missing snapshot remain explicit evidence rather than success.
    pub fn build(binding: Binding<'_>, facts: Facts<'_>, budget: &Budget) -> Result<Self, Block> {
        Self::build_inner(binding, facts, None, budget)
    }
    /// Explicit v2 runtime section. The legacy build keeps byte-exact v1 output.
    pub fn build_runtime(
        binding: Binding<'_>,
        facts: Facts<'_>,
        runtime: super::runtime_evidence::Input<'_>,
        budget: &Budget,
    ) -> Result<Self, Block> {
        Self::build_inner(binding, facts, Some(runtime), budget)
    }
    fn build_inner(
        binding: Binding<'_>,
        facts: Facts<'_>,
        runtime_input: Option<super::runtime_evidence::Input<'_>>,
        budget: &Budget,
    ) -> Result<Self, Block> {
        let scope = binding.scope;
        if binding.declaration.scope_digest() != scope.digest()
            || binding.declaration.tag().as_bytes() == &[0; 32]
            || facts.trace.scope_digest() != scope.digest()
            || facts
                .snapshot
                .is_some_and(|s| s.scope().digest() != scope.digest())
            || facts.info.is_some_and(|i| {
                i.raw_uuid != scope.draft().raw_uuid || i.raw_pci != scope.draft().raw_pci
            })
        {
            return Err(Block::Scope);
        }
        let _codec = budget.reserve(CAP * 6).map_err(|_| Block::Capacity)?;
        let allocation = budget
            .reserve(CAP + std::mem::size_of::<Self>())
            .map_err(|_| Block::Capacity)?;
        let d = scope.draft();
        let runtime_evidence = runtime_input
            .map(|input| super::runtime_evidence::value(input, facts.ledger))
            .transpose()?;
        let runtime = facts
            .info
            .map(|i| serde_json::to_string(&i.runtime))
            .transpose()
            .map_err(|_| Block::Scope)?;
        let mut document = object([
            (
                "version",
                Value::Text(if runtime_evidence.is_some() { "2" } else { "1" }.into()),
            ),
            (
                "scope",
                Value::Text("pilot_starting_settings_response".into()),
            ),
            ("request_digest", Value::Text(scope.digest().to_hex())),
            ("request", Value::Text(scope.canonical().into())),
            (
                "declaration",
                Value::Text(binding.declaration.tag().to_hex()),
            ),
            ("frequency", Value::U64(d.frequency)),
            ("epoch", anchor_value(d.epoch)),
            ("first_entry", anchor_value(binding.first_entry)),
            ("receipt", anchor_value(binding.receipt)),
            ("deadline_ns", Value::U64(binding.deadline_ns)),
            (
                "info",
                facts.info.map_or(Value::Null, |i| {
                    object([
                        ("raw_uuid", Value::Text(i.raw_uuid.clone())),
                        ("raw_pci", Value::Text(i.raw_pci.clone())),
                        ("runtime", runtime.clone().map_or(Value::Null, Value::Text)),
                    ])
                }),
            ),
            (
                "native_calls",
                Value::Array(facts.ledger.entries().iter().map(call_entry).collect()),
            ),
            (
                "native_fault",
                facts
                    .ledger
                    .fault()
                    .map_or(Value::Null, |f| Value::Text(fault_name(f))),
            ),
            (
                "native_overflow",
                facts
                    .ledger
                    .overflow()
                    .map_or(Value::Null, |(call, anchor)| {
                        object([
                            ("call", Value::Text(call_name(call))),
                            ("anchor", anchor.map_or(Value::Null, anchor_value)),
                        ])
                    }),
            ),
            (
                "native_rejected",
                facts
                    .ledger
                    .rejected_return()
                    .map_or(Value::Null, |(call, raw, anchor)| {
                        object([
                            ("call", Value::Text(call_name(call))),
                            ("return", raw_value(raw)),
                            ("anchor", anchor.map_or(Value::Null, anchor_value)),
                        ])
                    }),
            ),
            (
                "snapshot",
                facts.snapshot.map_or(Value::Null, |s| {
                    object([
                        ("canonical", Value::Text(s.canonical().into())),
                        ("digest", Value::Text(s.digest().to_hex())),
                        ("source", Value::Text(source_name(s.source()).into())),
                    ])
                }),
            ),
            (
                "snapshot_error",
                facts
                    .snapshot_error
                    .map_or(Value::Null, |b| Value::Text(settings::block_name(b).into())),
            ),
            ("trace", trace_value(facts.trace)),
            ("control", control_value(facts.control)),
            (
                "native_failure",
                facts.native_failure.map_or(Value::Null, |f| {
                    object([
                        (
                            "reason",
                            Value::Text(
                                serde_json::to_value(f.reason)
                                    .expect("closed reason")
                                    .as_str()
                                    .expect("reason text")
                                    .into(),
                            ),
                        ),
                        (
                            "vendor_status",
                            f.vendor_status
                                .map_or(Value::Null, |n| Value::I64(n.into())),
                        ),
                        ("detail", Value::Text(f.detail.clone())),
                    ])
                }),
            ),
            (
                "clock_error",
                facts.clock_error.map_or(Value::Null, clock_fault_value),
            ),
            (
                "control_error",
                facts
                    .control_error
                    .map_or(Value::Null, |f| Value::Text(control_fault_name(f))),
            ),
            (
                "event_clone_error",
                facts
                    .event_clone_error
                    .map_or(Value::Null, |n| Value::U64(n.into())),
            ),
            (
                "unreported_completion",
                facts
                    .unreported_completion
                    .map_or(Value::Null, completion_value),
            ),
        ]);
        if let Some(runtime) = runtime_evidence {
            let Value::Object(ref mut o) = document else {
                unreachable!("fixed response object")
            };
            o.insert("runtime", runtime).map_err(|_| Block::Scope)?;
        }
        let canonical = document.to_canonical();
        if canonical.len() > CAP {
            return Err(Block::Capacity);
        }
        let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
        Ok(Self {
            scope: scope.digest(),
            declaration: binding.declaration.tag(),
            canonical,
            digest,
            _allocation: allocation,
        })
    }
}
fn call_name(call: SamplerCall) -> String {
    match call {
        SamplerCall::Initialize => "initialize".into(),
        SamplerCall::Count => "count".into(),
        SamplerCall::Handle(i) => format!("handle:{i}"),
        SamplerCall::Uuid(i) => format!("uuid:{i}"),
        SamplerCall::Pci(i) => format!("pci:{i}"),
        SamplerCall::CudaVersion => "cuda_version".into(),
        SamplerCall::DriverVersion => "driver_version".into(),
        SamplerCall::NvmlVersion => "nvml_version".into(),
    }
}
fn output_value(output: SamplerOutput) -> Value {
    match output {
        SamplerOutput::None => Value::Null,
        SamplerOutput::Count(n) => object([
            ("kind", Value::Text("count".into())),
            ("value", Value::U64(n.into())),
        ]),
        SamplerOutput::HandleIsNull(v) => object([
            ("kind", Value::Text("handle_is_null".into())),
            ("value", Value::Bool(v)),
        ]),
        SamplerOutput::Text(text) => object([
            ("kind", Value::Text("text".into())),
            ("hex", Value::Text(hex(&text))),
        ]),
        SamplerOutput::Pci {
            legacy,
            numbers,
            bus_id,
        } => object([
            ("kind", Value::Text("pci".into())),
            ("legacy", Value::Text(hex(&legacy))),
            (
                "numbers",
                Value::Array(numbers.into_iter().map(|n| Value::U64(n.into())).collect()),
            ),
            ("bus_id", Value::Text(hex(&bus_id))),
        ]),
        SamplerOutput::Version(n) => object([
            ("kind", Value::Text("version".into())),
            ("value", Value::I64(n.into())),
        ]),
    }
}
fn raw_value(raw: SamplerReturn) -> Value {
    object([
        ("status", Value::I64(raw.status.into())),
        ("output", output_value(raw.output)),
    ])
}
fn call_entry(entry: &nvml_trace::Entry) -> Value {
    object([
        ("call", Value::Text(call_name(entry.call))),
        ("before", entry.before.map_or(Value::Null, anchor_value)),
        ("after", entry.after.map_or(Value::Null, anchor_value)),
        ("attempted", Value::Bool(entry.attempted)),
        ("raw", entry.raw.map_or(Value::Null, raw_value)),
    ])
}
fn fault_name(fault: nvml_trace::Fault) -> String {
    match fault {
        nvml_trace::Fault::Capacity => "capacity".into(),
        nvml_trace::Fault::Clock => "clock".into(),
        nvml_trace::Fault::Deadline => "deadline".into(),
        nvml_trace::Fault::Stopped => "stopped".into(),
        nvml_trace::Fault::Protocol => "protocol".into(),
        nvml_trace::Fault::Native(status) => format!("native:{status}"),
    }
}
fn source_name(source: Source) -> &'static str {
    match source {
        Source::NativeGetterPath => "native_getter_path",
        Source::SimulatedFixture => "simulated_fixture",
    }
}
fn trace_value(trace: &Trace) -> Value {
    object([
        (
            "calls",
            Value::Array(
                trace
                    .calls
                    .iter()
                    .zip(OPERATIONS)
                    .map(|(call, name)| {
                        object([
                            ("operation", Value::Text(name.into())),
                            ("requested", Value::Bool(call.requested)),
                            ("before", call.before.map_or(Value::Null, anchor_value)),
                            ("after", call.after.map_or(Value::Null, anchor_value)),
                            ("attempted", Value::Bool(call.attempted)),
                            ("returned", Value::Bool(call.returned)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "fault",
            trace
                .fault
                .map_or(Value::Null, |f| Value::Text(runtime_fault_name(f))),
        ),
        (
            "cleanup_status",
            trace
                .cleanup_status
                .map_or(Value::Null, |n| Value::I64(n.into())),
        ),
        (
            "stop_trigger",
            trace.stop_trigger.map_or(Value::Null, anchor_value),
        ),
        (
            "drain_receipt",
            trace.drain_receipt.map_or(Value::Null, anchor_value),
        ),
        ("output_drained", Value::Bool(trace.output_drained)),
        ("control_finished", Value::Bool(trace.control_finished)),
    ])
}
fn runtime_fault_name(fault: starting_runtime::Fault) -> String {
    match fault {
        starting_runtime::Fault::Capacity => "capacity".into(),
        starting_runtime::Fault::Clock => "clock".into(),
        starting_runtime::Fault::Deadline => "deadline".into(),
        starting_runtime::Fault::Control => "control".into(),
        starting_runtime::Fault::Protocol => "protocol".into(),
        starting_runtime::Fault::Runtime => "runtime".into(),
        starting_runtime::Fault::Read => "read".into(),
        starting_runtime::Fault::Cleanup => "cleanup".into(),
        starting_runtime::Fault::Drain => "drain".into(),
    }
}
fn completion_value(completion: Completion) -> Value {
    object([
        ("anchor", anchor_value(completion.anchor)),
        (
            "initialized_session",
            Value::Bool(completion.initialized_session),
        ),
        (
            "cleanup_attempted",
            Value::Bool(completion.cleanup_attempted),
        ),
        (
            "cleanup_status",
            completion
                .cleanup_status
                .map_or(Value::Null, |n| Value::I64(n.into())),
        ),
    ])
}
fn control_value(control: ControlFacts) -> Value {
    object([
        (
            "completion",
            control.completion.map_or(Value::Null, completion_value),
        ),
        (
            "completion_receipt",
            control.completion_receipt.map_or(Value::Null, anchor_value),
        ),
        (
            "finished",
            control.finished.map_or(Value::Null, Value::Bool),
        ),
        (
            "fault",
            control
                .fault
                .map_or(Value::Null, |f| Value::Text(control_fault_name(f))),
        ),
        (
            "window",
            control.window.map_or(Value::Null, |w| {
                object([
                    ("trigger_ns", Value::U64(w.trigger_ns)),
                    ("cooperative_ns", Value::U64(w.cooperative_ns)),
                    ("exit_ns", Value::U64(w.exit_ns)),
                ])
            }),
        ),
        ("trigger", control.trigger.map_or(Value::Null, anchor_value)),
        (
            "event_status",
            control
                .event_status
                .map_or(Value::Null, |n| Value::U64(n.into())),
        ),
        (
            "event_error",
            control
                .event_error
                .map_or(Value::Null, |n| Value::U64(n.into())),
        ),
        (
            "transport_error",
            control
                .transport_error
                .map_or(Value::Null, |n| Value::I64(n.into())),
        ),
        (
            "observed_frequency",
            control.observed_frequency.map_or(Value::Null, Value::U64),
        ),
        (
            "rejected_receipt",
            control.rejected_receipt.map_or(Value::Null, anchor_value),
        ),
    ])
}
fn control_fault_name(fault: ControlFault) -> String {
    match fault {
        ControlFault::Clock => "clock".into(),
        ControlFault::Capacity => "capacity".into(),
        ControlFault::Deadline => "deadline".into(),
        ControlFault::Protocol => "protocol".into(),
        ControlFault::Transport => "transport".into(),
        ControlFault::Cleanup => "cleanup".into(),
    }
}
fn clock_fault_value(fault: ClockFault) -> Value {
    match fault {
        ClockFault::ZeroToken => object([("kind", Value::Text("zero_token".into()))]),
        ClockFault::FrequencyFailed(code) => object([
            ("kind", Value::Text("frequency_failed".into())),
            ("code", Value::U64(code.into())),
        ]),
        ClockFault::InvalidFrequency(value) => object([
            ("kind", Value::Text("invalid_frequency".into())),
            ("value", Value::I64(value)),
        ]),
        ClockFault::CounterFailed { before, code } => object([
            ("kind", Value::Text("counter_failed".into())),
            ("at", Value::Text(reading_name(before).into())),
            ("code", Value::U64(code.into())),
        ]),
        ClockFault::InvalidCounter { before, value } => object([
            ("kind", Value::Text("invalid_counter".into())),
            ("at", Value::Text(reading_name(before).into())),
            ("value", Value::I64(value)),
        ]),
        ClockFault::AnchorOrder { before, after } => object([
            ("kind", Value::Text("anchor_order".into())),
            ("before", Value::U64(before)),
            ("after", Value::U64(after)),
        ]),
    }
}
fn reading_name(before: bool) -> &'static str {
    if before {
        "before"
    } else {
        "after"
    }
}
