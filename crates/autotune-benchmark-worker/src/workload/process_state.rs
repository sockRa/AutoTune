//! Timestamped retained-handle observations, never a reusable Start/native token.
use super::launch_origin::{Kind, Origin};
pub use super::starting_export::ClockFault;
#[cfg(windows)]
use crate::evidence::{Digest, Domain, Value};
use crate::{
    monitor::{
        clock::{Clock, ClockError},
        Anchor, Binding,
    },
    sampler::supervisor::ProcessObservation,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub binding: Binding,
    pub epoch: Anchor,
    pub deadline_ns: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Times {
    pub creation: u64,
    pub exit: u64,
    pub kernel: u64,
    pub user: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub flags: u32,
    pub active_limit: u32,
    pub user_100ns: i64,
    pub process_memory: u64,
    pub job_memory: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Returned<T> {
    pub status: i32,
    pub value: T,
    pub error: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitRead {
    pub limits: Limits,
    pub returned_bytes: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pid {
    pub value: u32,
    pub error: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Pid(Pid),
    Times(Returned<Times>),
    JobFlags(Returned<u32>),
    Limits(Returned<LimitRead>),
    Process(ProcessObservation),
    Membership(Returned<i32>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Pid,
    Times,
    JobFlags,
    Limits,
    Process,
    Membership,
}
const OPERATIONS: [Operation; 6] = [
    Operation::Pid,
    Operation::Times,
    Operation::JobFlags,
    Operation::Limits,
    Operation::Process,
    Operation::Membership,
];
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Call {
    pub requested: bool,
    pub before: Option<Anchor>,
    pub attempted: bool,
    pub output: Option<Output>,
    pub after: Option<Anchor>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Missing,
    Untrusted,
    Binding,
    Clock,
    Deadline,
    Stopped,
    Shape,
    Os(Operation),
    Identity,
    JobFlags,
    Limits,
    Membership,
    Wait,
    Exit,
    Accounting,
    Termination,
    Context,
    Duplicate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceKind {
    #[cfg(any(windows, test))]
    RetainedHandle,
    Fixture,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    origin: Origin,
    window: Window,
    expected_limits: Limits,
    limit_bytes: u32,
    observed_clock: (u64, [u8; 16]),
    source: SourceKind,
    calls: [Call; 6],
    fault: Option<Fault>,
    capture_error: Option<ClockFault>,
    clock_error: Option<ClockError>,
    rejected_anchor: Option<Anchor>,
    cancelled_before: bool,
    cancelled_after: Option<bool>,
}
impl Snapshot {
    pub fn origin(&self) -> Origin {
        self.origin
    }
    pub fn calls(&self) -> &[Call; 6] {
        &self.calls
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn capture_error(&self) -> Option<ClockFault> {
        self.capture_error
    }
    pub fn clock_error(&self) -> Option<ClockError> {
        self.clock_error
    }
    pub fn rejected_anchor(&self) -> Option<Anchor> {
        self.rejected_anchor
    }
    pub fn expected_limits(&self) -> Limits {
        self.expected_limits
    }
    pub fn window(&self) -> Window {
        self.window
    }
    pub fn observed_clock(&self) -> (u64, [u8; 16]) {
        self.observed_clock
    }
    pub fn cancelled(&self) -> (bool, Option<bool>) {
        (self.cancelled_before, self.cancelled_after)
    }
    pub fn fixture(
        origin: Origin,
        window: Window,
        limits: Limits,
        limit_bytes: u32,
        calls: [Call; 6],
    ) -> Self {
        Self {
            origin,
            window,
            expected_limits: limits,
            limit_bytes,
            observed_clock: (window.binding.qpc_frequency, window.epoch.clock_token),
            source: SourceKind::Fixture,
            calls,
            fault: None,
            capture_error: None,
            clock_error: None,
            rejected_anchor: None,
            cancelled_before: false,
            cancelled_after: None,
        }
    }
}
#[cfg(any(windows, test))]
pub(crate) trait Source {
    fn capture(&mut self) -> Result<Anchor, ClockFault>;
    fn cancelled(&self) -> bool;
    fn clock_identity(&self) -> (u64, [u8; 16]);
    fn read(&mut self, op: Operation) -> Output;
}
fn output_failure(op: Operation, output: Output) -> Option<Fault> {
    let failed = match (op, output) {
        (Operation::Pid, Output::Pid(r)) => r.value == 0 || r.error.is_some(),
        (Operation::Times, Output::Times(r)) => r.status == 0 || r.error.is_some(),
        (Operation::JobFlags, Output::JobFlags(r)) => r.status == 0 || r.error.is_some(),
        (Operation::Limits, Output::Limits(r)) => r.status == 0 || r.error.is_some(),
        (Operation::Process, Output::Process(r)) => {
            r.wait_error.is_some() || r.exit_error.is_some() || r.accounting_error.is_some()
        }
        (Operation::Membership, Output::Membership(r)) => r.status == 0 || r.error.is_some(),
        _ => return Some(Fault::Shape),
    };
    failed.then_some(Fault::Os(op))
}
#[cfg(any(windows, test))]
pub(crate) fn collect(
    origin: Origin,
    window: Window,
    limits: Limits,
    limit_bytes: u32,
    kind: SourceKind,
    s: &mut impl Source,
) -> Snapshot {
    let mut out = Snapshot::fixture(origin, window, limits, limit_bytes, [Call::default(); 6]);
    out.source = kind;
    out.observed_clock = s.clock_identity();
    out.cancelled_before = s.cancelled();
    let mut clock = match Clock::new(window.binding, window.epoch) {
        Ok(c) => c,
        Err(e) => {
            out.clock_error = Some(e);
            out.fault = Some(Fault::Clock);
            return out;
        }
    };
    if out.observed_clock != (window.binding.qpc_frequency, window.epoch.clock_token) {
        out.fault = Some(Fault::Binding);
        return out;
    }
    let mut last = window.epoch;
    for (i, op) in OPERATIONS.into_iter().enumerate() {
        // Starting-read normal completion is proved by direct wait/exit and its
        // original job accounting, not assumptions about post-exit membership.
        if op == Operation::Membership && origin.kind() == Kind::Starting {
            continue;
        }
        out.calls[i].requested = true;
        if s.cancelled() {
            out.fault.get_or_insert(Fault::Stopped);
            break;
        }
        let before = observe(s, &mut out, &mut clock, &mut last);
        out.calls[i].before = before;
        if before.is_none() {
            break;
        }
        if s.cancelled() {
            out.fault.get_or_insert(Fault::Stopped);
            break;
        }
        out.calls[i].attempted = true;
        let output = s.read(op);
        out.calls[i].output = Some(output);
        if let Some(f) = output_failure(op, output) {
            out.fault.get_or_insert(f);
        }
        let after = observe(s, &mut out, &mut clock, &mut last);
        out.calls[i].after = after;
        if s.cancelled() {
            out.fault.get_or_insert(Fault::Stopped);
        }
        if out.fault.is_some() {
            break;
        }
    }
    out.cancelled_after = Some(s.cancelled());
    out
}
#[cfg(any(windows, test))]
fn observe(
    s: &mut impl Source,
    out: &mut Snapshot,
    clock: &mut Clock,
    last: &mut Anchor,
) -> Option<Anchor> {
    let a = match s.capture() {
        Ok(a) => a,
        Err(e) => {
            out.capture_error.get_or_insert(e);
            out.fault.get_or_insert(Fault::Clock);
            return None;
        }
    };
    let result = if a.qpc_before < last.qpc_after {
        Err(ClockError::Regression)
    } else {
        clock.observe(a)
    };
    match result {
        Err(e) => {
            out.clock_error.get_or_insert(e);
            out.rejected_anchor.get_or_insert(a);
            out.fault.get_or_insert(Fault::Clock);
            None
        }
        Ok(n) if n >= out.window.deadline_ns => {
            out.rejected_anchor.get_or_insert(a);
            out.fault.get_or_insert(Fault::Deadline);
            None
        }
        Ok(_) => {
            *last = a;
            Some(a)
        }
    }
}
/// Point-in-time OS-scope consistency only. It says nothing about subsequent
/// liveness, GPU quiescence, aggregate resources or permission to dispatch.
pub fn assess(snapshot: &Snapshot, expected: Origin, window: Window) -> Result<(), Fault> {
    if snapshot.source == SourceKind::Fixture {
        return Err(Fault::Untrusted);
    }
    if snapshot.origin != expected || snapshot.window != window {
        return Err(Fault::Binding);
    }
    if let Some(f) = snapshot.fault {
        return Err(f);
    }
    if snapshot.cancelled_before || snapshot.cancelled_after == Some(true) {
        return Err(Fault::Stopped);
    }
    if snapshot.cancelled_after.is_none() {
        return Err(Fault::Missing);
    }
    let mut clock = Clock::new(window.binding, window.epoch).map_err(|_| Fault::Clock)?;
    let mut last = window.epoch;
    for (i, op) in OPERATIONS.into_iter().enumerate() {
        let call = snapshot.calls[i];
        if op == Operation::Membership && expected.kind() == Kind::Starting {
            if call != Call::default() {
                return Err(Fault::Shape);
            }
            continue;
        }
        if !call.requested || !call.attempted || call.output.is_none() {
            return Err(Fault::Missing);
        }
        for a in [call.before, call.after] {
            let a = a.ok_or(Fault::Clock)?;
            if a.qpc_before < last.qpc_after {
                return Err(Fault::Clock);
            }
            if clock.observe(a).map_err(|_| Fault::Clock)? >= window.deadline_ns {
                return Err(Fault::Deadline);
            }
            last = a;
        }
        if let Some(f) = output_failure(op, call.output.unwrap()) {
            return Err(f);
        }
    }
    let output = |i: usize| snapshot.calls[i].output.ok_or(Fault::Missing);
    match output(0)? {
        Output::Pid(r) if r.value == expected.pid() => {}
        _ => return Err(Fault::Identity),
    }
    match output(1)? {
        Output::Times(r) if r.value.creation == expected.creation() => {}
        _ => return Err(Fault::Identity),
    }
    match output(2)? {
        Output::JobFlags(r) if r.value == 0 => {}
        _ => return Err(Fault::JobFlags),
    }
    match output(3)? {
        Output::Limits(r)
            if r.value.limits == snapshot.expected_limits
                && r.value.returned_bytes == snapshot.limit_bytes => {}
        _ => return Err(Fault::Limits),
    }
    let p = match output(4)? {
        Output::Process(p) => p,
        _ => return Err(Fault::Shape),
    };
    if p.creation != expected.creation() {
        return Err(Fault::Identity);
    }
    if p.termination.is_some() {
        return Err(Fault::Termination);
    }
    let exited = expected.kind() == Kind::Starting;
    if p.wait_status != if exited { 0 } else { 258 } {
        return Err(Fault::Wait);
    }
    if p.exit_code != if exited { Some(0) } else { None } {
        return Err(Fault::Exit);
    }
    if p.active_processes != Some(if exited { 0 } else { 1 }) {
        return Err(Fault::Accounting);
    }
    if !exited {
        match output(5)? {
            Output::Membership(r) if r.value != 0 => {}
            _ => return Err(Fault::Membership),
        }
    }
    Ok(())
}

/// A historical decision-time frame. It is not a lease on later process state;
/// a future Start issuer must collect as part of that decision, not reuse it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub window: Window,
    pub observed_clock: (u64, [u8; 16]),
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    pub before_error: Option<ClockFault>,
    pub after_error: Option<ClockFault>,
    pub snapshots: [Option<Snapshot>; 3],
    pub checks: [Result<(), Fault>; 3],
    pub clock_result: Result<(), Fault>,
    pub result: Result<(), Fault>,
}
impl Frame {
    /// Full bounded process-state evidence, including failed/partial calls.
    /// This is a report projection only and cannot be reused as an admission
    /// token or a later liveness claim.
    #[cfg(windows)]
    pub(crate) fn raw_evidence_value(&self) -> Value {
        let checks = self.checks.map(|check| match check {
            Ok(()) => Value::Text("ok".into()),
            Err(error) => Value::Text(format!("{error:?}")),
        });
        object([
            ("window", window_value(self.window)),
            (
                "observed_clock",
                object([
                    ("frequency", Value::U64(self.observed_clock.0)),
                    ("token", Value::Text(hex(&self.observed_clock.1))),
                ]),
            ),
            ("before", self.before.map_or(Value::Null, anchor_value)),
            ("after", self.after.map_or(Value::Null, anchor_value)),
            (
                "before_error",
                self.before_error
                    .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
            ),
            (
                "after_error",
                self.after_error
                    .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
            ),
            (
                "snapshots",
                Value::Array(
                    self.snapshots
                        .iter()
                        .map(|snapshot| {
                            snapshot.map_or(Value::Null, |snapshot| snapshot.raw_evidence_value())
                        })
                        .collect(),
                ),
            ),
            ("checks", Value::Array(checks.into())),
            ("clock_result", result_value(self.clock_result)),
            ("result", result_value(self.result)),
        ])
    }

    /// Commit the exact successful retained-handle observations for a parent
    /// grant. Failed/fixture frames have no commitment eligible for issuance.
    #[cfg(windows)]
    pub(super) fn native_grant_commitment(&self) -> Option<Digest> {
        if self.result.is_err()
            || self.clock_result.is_err()
            || self.checks.iter().any(Result::is_err)
            || self.before_error.is_some()
            || self.after_error.is_some()
            || self.before.is_none()
            || self.after.is_none()
            || self.snapshots.iter().any(Option::is_none)
        {
            return None;
        }
        let snapshots = self
            .snapshots
            .map(|snapshot| snapshot.as_ref().and_then(Snapshot::native_grant_value));
        if snapshots.iter().any(Option::is_none) {
            return None;
        }
        let binding = self.window.binding;
        let payload = object([
            ("frequency", Value::U64(binding.qpc_frequency)),
            ("session", Value::Text(hex(&binding.session))),
            ("run", Value::Text(hex(&binding.run))),
            ("device_uuid", Value::Text(hex(&binding.device.uuid))),
            ("device_domain", Value::U64(binding.device.domain.into())),
            ("device_bus", Value::U64(binding.device.bus.into())),
            ("device_number", Value::U64(binding.device.device.into())),
            ("epoch", anchor_value(self.window.epoch)),
            ("deadline_ns", Value::U64(self.window.deadline_ns)),
            ("clock_frequency", Value::U64(self.observed_clock.0)),
            ("clock_token", Value::Text(hex(&self.observed_clock.1))),
            (
                "before",
                anchor_value(self.before.expect("checked before anchor")),
            ),
            (
                "after",
                anchor_value(self.after.expect("checked after anchor")),
            ),
            (
                "snapshots",
                Value::Array(
                    snapshots
                        .into_iter()
                        .map(|value| value.expect("checked snapshot"))
                        .collect(),
                ),
            ),
        ]);
        Some(Digest::of_value(Domain::RawArtifact, &payload))
    }
}
impl Snapshot {
    #[cfg(windows)]
    fn raw_evidence_value(&self) -> Value {
        let calls = self.calls.map(|call| {
            object([
                ("requested", Value::Bool(call.requested)),
                ("before", call.before.map_or(Value::Null, anchor_value)),
                ("attempted", Value::Bool(call.attempted)),
                ("output", call.output.map_or(Value::Null, output_value)),
                ("after", call.after.map_or(Value::Null, anchor_value)),
            ])
        });
        let limits = self.expected_limits;
        object([
            ("origin", origin_value(self.origin)),
            ("window", window_value(self.window)),
            ("source", Value::Text(format!("{:?}", self.source))),
            ("flags", Value::U64(limits.flags.into())),
            ("active_limit", Value::U64(limits.active_limit.into())),
            ("user_100ns", Value::I64(limits.user_100ns)),
            ("process_memory", Value::U64(limits.process_memory)),
            ("job_memory", Value::U64(limits.job_memory)),
            ("limit_bytes", Value::U64(self.limit_bytes.into())),
            ("clock_frequency", Value::U64(self.observed_clock.0)),
            ("clock_token", Value::Text(hex(&self.observed_clock.1))),
            ("calls", Value::Array(calls.into())),
            (
                "fault",
                self.fault
                    .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
            ),
            (
                "capture_error",
                self.capture_error
                    .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
            ),
            (
                "clock_error",
                self.clock_error
                    .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
            ),
            (
                "rejected_anchor",
                self.rejected_anchor.map_or(Value::Null, anchor_value),
            ),
            ("cancelled_before", Value::Bool(self.cancelled_before)),
            (
                "cancelled_after",
                self.cancelled_after.map_or(Value::Null, Value::Bool),
            ),
        ])
    }

    #[cfg(windows)]
    fn native_grant_value(&self) -> Option<Value> {
        if self.fault.is_some()
            || self.capture_error.is_some()
            || self.clock_error.is_some()
            || self.rejected_anchor.is_some()
            || self.cancelled_before
            || self.cancelled_after != Some(false)
            || self.source != SourceKind::RetainedHandle
            || self.calls.iter().any(|call| {
                call.requested != call.attempted
                    || call.requested
                        && (call.before.is_none() || call.after.is_none() || call.output.is_none())
            })
        {
            return None;
        }
        let limits = self.expected_limits;
        let calls = self.calls.map(|call| {
            object([
                ("requested", Value::Bool(call.requested)),
                ("before", call.before.map_or(Value::Null, anchor_value)),
                ("attempted", Value::Bool(call.attempted)),
                ("output", call.output.map_or(Value::Null, output_value)),
                ("after", call.after.map_or(Value::Null, anchor_value)),
            ])
        });
        Some(object([
            ("origin", origin_value(self.origin)),
            ("flags", Value::U64(limits.flags.into())),
            ("active_limit", Value::U64(limits.active_limit.into())),
            ("user_100ns", Value::I64(limits.user_100ns)),
            ("process_memory", Value::U64(limits.process_memory)),
            ("job_memory", Value::U64(limits.job_memory)),
            ("limit_bytes", Value::U64(self.limit_bytes.into())),
            ("clock_frequency", Value::U64(self.observed_clock.0)),
            ("clock_token", Value::Text(hex(&self.observed_clock.1))),
            ("calls", Value::Array(calls.into())),
        ]))
    }
}
#[cfg(windows)]
fn origin_value(origin: Origin) -> Value {
    object([
        (
            "kind",
            Value::Text(
                match origin.kind() {
                    Kind::Starting => "starting",
                    Kind::Sampler => "sampler",
                    Kind::Worker => "worker",
                }
                .into(),
            ),
        ),
        ("binding", Value::Text(origin.binding().to_hex())),
        ("declaration", Value::Text(origin.declaration().to_hex())),
        ("image", Value::Text(origin.image().to_hex())),
        ("pid", Value::U64(origin.pid().into())),
        ("creation", Value::U64(origin.creation())),
        (
            "consent",
            origin.consent().map_or(Value::Null, |capture| {
                object([
                    ("request", Value::Text(capture.request.to_hex())),
                    ("receipt", anchor_value(capture.receipt)),
                ])
            }),
        ),
    ])
}
#[cfg(windows)]
fn window_value(window: Window) -> Value {
    let binding = window.binding;
    object([
        ("session", Value::Text(hex(&binding.session))),
        ("run", Value::Text(hex(&binding.run))),
        (
            "device",
            object([
                ("uuid", Value::Text(hex(&binding.device.uuid))),
                ("domain", Value::U64(binding.device.domain.into())),
                ("bus", Value::U64(binding.device.bus.into())),
                ("device", Value::U64(binding.device.device.into())),
            ]),
        ),
        ("clock_token", Value::Text(hex(&binding.clock_token))),
        ("qpc_frequency", Value::U64(binding.qpc_frequency)),
        ("epoch", anchor_value(window.epoch)),
        ("deadline_ns", Value::U64(window.deadline_ns)),
    ])
}
#[cfg(windows)]
fn result_value(result: Result<(), Fault>) -> Value {
    result.map_or_else(
        |error| Value::Text(format!("error:{error:?}")),
        |_| Value::Text("ok".into()),
    )
}
#[cfg(windows)]
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 15)] as char);
    }
    text
}
#[cfg(windows)]
fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = crate::evidence::Object::new();
    for (key, value) in fields {
        object.insert(key, value).expect("fixed unique grant keys");
    }
    Value::Object(object)
}
#[cfg(windows)]
fn anchor_value(anchor: Anchor) -> Value {
    object([
        ("token", Value::Text(hex(&anchor.clock_token))),
        ("before", Value::U64(anchor.qpc_before)),
        ("after", Value::U64(anchor.qpc_after)),
        ("interrupt", Value::U64(anchor.interrupt_100ns)),
        ("unbiased", Value::U64(anchor.unbiased_100ns)),
    ])
}
#[cfg(windows)]
fn output_value(output: Output) -> Value {
    match output {
        Output::Pid(pid) => object([
            ("kind", Value::Text("pid".into())),
            ("value", Value::U64(pid.value.into())),
            (
                "error",
                pid.error.map_or(Value::Null, |v| Value::U64(v.into())),
            ),
        ]),
        Output::Times(result) => object([
            ("kind", Value::Text("times".into())),
            ("status", Value::I64(result.status.into())),
            ("creation", Value::U64(result.value.creation)),
            ("exit", Value::U64(result.value.exit)),
            ("kernel", Value::U64(result.value.kernel)),
            ("user", Value::U64(result.value.user)),
            (
                "error",
                result.error.map_or(Value::Null, |v| Value::U64(v.into())),
            ),
        ]),
        Output::JobFlags(result) => object([
            ("kind", Value::Text("job_flags".into())),
            ("status", Value::I64(result.status.into())),
            ("value", Value::U64(result.value.into())),
            (
                "error",
                result.error.map_or(Value::Null, |v| Value::U64(v.into())),
            ),
        ]),
        Output::Limits(result) => object([
            ("kind", Value::Text("limits".into())),
            ("status", Value::I64(result.status.into())),
            ("flags", Value::U64(result.value.limits.flags.into())),
            (
                "active_limit",
                Value::U64(result.value.limits.active_limit.into()),
            ),
            ("user_100ns", Value::I64(result.value.limits.user_100ns)),
            (
                "process_memory",
                Value::U64(result.value.limits.process_memory),
            ),
            ("job_memory", Value::U64(result.value.limits.job_memory)),
            (
                "returned_bytes",
                Value::U64(result.value.returned_bytes.into()),
            ),
            (
                "error",
                result.error.map_or(Value::Null, |v| Value::U64(v.into())),
            ),
        ]),
        Output::Process(result) => object([
            ("kind", Value::Text("process".into())),
            ("creation", Value::U64(result.creation)),
            ("wait_status", Value::U64(result.wait_status.into())),
            (
                "wait_error",
                result
                    .wait_error
                    .map_or(Value::Null, |v| Value::U64(v.into())),
            ),
            (
                "exit_code",
                result
                    .exit_code
                    .map_or(Value::Null, |v| Value::U64(v.into())),
            ),
            (
                "exit_error",
                result
                    .exit_error
                    .map_or(Value::Null, |v| Value::U64(v.into())),
            ),
            (
                "termination",
                result.termination.map_or(Value::Null, |v| match v {
                    Ok(()) => Value::Text("ok".into()),
                    Err(code) => Value::I64(code.into()),
                }),
            ),
            (
                "active_processes",
                result
                    .active_processes
                    .map_or(Value::Null, |v| Value::U64(v.into())),
            ),
            (
                "accounting_error",
                result
                    .accounting_error
                    .map_or(Value::Null, |v| Value::U64(v.into())),
            ),
        ]),
        Output::Membership(result) => object([
            ("kind", Value::Text("membership".into())),
            ("status", Value::I64(result.status.into())),
            ("value", Value::I64(result.value.into())),
            (
                "error",
                result.error.map_or(Value::Null, |v| Value::U64(v.into())),
            ),
        ]),
    }
}
#[cfg(any(windows, test))]
pub(crate) fn screen_frame(frame: &mut Frame, expected: [Origin; 3], context: bool) {
    frame.checks = std::array::from_fn(|i| {
        if expected[i].kind() != [Kind::Starting, Kind::Sampler, Kind::Worker][i] {
            return Err(Fault::Binding);
        }
        assess(
            frame.snapshots[i].as_ref().ok_or(Fault::Missing)?,
            expected[i],
            frame.window,
        )
    });
    frame.clock_result = (|| {
        if frame.observed_clock
            != (
                frame.window.binding.qpc_frequency,
                frame.window.epoch.clock_token,
            )
        {
            return Err(Fault::Binding);
        }
        if frame.before_error.is_some() || frame.after_error.is_some() {
            return Err(Fault::Clock);
        }
        let mut c =
            Clock::new(frame.window.binding, frame.window.epoch).map_err(|_| Fault::Clock)?;
        let mut last = frame.window.epoch;
        let mut observe = |a: Option<Anchor>| {
            let a = a.ok_or(Fault::Clock)?;
            if a.qpc_before < last.qpc_after {
                return Err(Fault::Clock);
            }
            if c.observe(a).map_err(|_| Fault::Clock)? >= frame.window.deadline_ns {
                return Err(Fault::Deadline);
            }
            last = a;
            Ok(())
        };
        observe(frame.before)?;
        for snapshot in frame.snapshots.iter().flatten() {
            for call in snapshot.calls {
                if call.requested {
                    observe(call.before)?;
                    observe(call.after)?;
                }
            }
        }
        observe(frame.after)
    })();
    frame.result = if !context {
        Err(Fault::Context)
    } else if expected
        .iter()
        .enumerate()
        .any(|(i, o)| expected[..i].iter().any(|p| p.pid() == o.pid()))
    {
        Err(Fault::Binding)
    } else {
        frame
            .checks
            .iter()
            .copied()
            .find(Result::is_err)
            .unwrap_or(frame.clock_result)
    };
}
#[cfg(windows)]
pub struct Processes<'a> {
    pub starting: &'a super::windows_process::Process<super::starting_bootstrap::Declaration>,
    pub sampler: &'a super::windows_process::Process,
    pub worker: &'a super::windows_process::Process,
    pub setup: &'a mut super::worker_setup_delivery::ReadyDelivery,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        evidence::{Digest, Domain},
        worker::Identity,
    };
    fn a(n: u64) -> Anchor {
        Anchor {
            clock_token: [1; 16],
            qpc_before: n,
            qpc_after: n,
            interrupt_100ns: n / 100,
            unbiased_100ns: n / 100,
        }
    }
    fn window() -> Window {
        Window {
            binding: Binding {
                session: [1; 16],
                run: [2; 16],
                device: Identity {
                    uuid: [3; 16],
                    domain: 0,
                    bus: 1,
                    device: 0,
                },
                clock_token: [1; 16],
                qpc_frequency: 1_000_000_000,
            },
            epoch: a(0),
            deadline_ns: 1_000_000,
        }
    }
    fn origin(kind: Kind) -> Origin {
        let d = Digest::of(Domain::RawArtifact, b"CPU model");
        Origin::new(kind, d, d, d, 42, 123).unwrap()
    }
    fn limits() -> Limits {
        Limits {
            flags: 0x230a,
            active_limit: 1,
            user_100ns: 450_000_000,
            process_memory: 1_073_741_824,
            job_memory: 1_073_741_824,
        }
    }
    struct Fake {
        kind: Kind,
        pid: u32,
        tick: usize,
        reads: usize,
        fail_capture: Option<usize>,
        bad_anchor: Option<usize>,
        regression_at: Option<usize>,
        identity: Option<(u64, [u8; 16])>,
        stop_after: Option<usize>,
        changed: Option<(Operation, Output)>,
        offset: u64,
    }
    impl Fake {
        fn new(kind: Kind) -> Self {
            Self {
                kind,
                pid: 42,
                tick: 0,
                reads: 0,
                fail_capture: None,
                bad_anchor: None,
                regression_at: None,
                identity: None,
                stop_after: None,
                changed: None,
                offset: 0,
            }
        }
    }
    impl Source for Fake {
        fn capture(&mut self) -> Result<Anchor, ClockFault> {
            let at = self.tick;
            self.tick += 1;
            if self.fail_capture == Some(at) {
                return Err(ClockFault::CounterFailed {
                    before: true,
                    code: 9,
                });
            }
            let mut v = a(self.offset + self.tick as u64 * 100);
            if self.bad_anchor == Some(at) {
                v.clock_token = [9; 16];
            }
            if self.regression_at == Some(at) {
                v = a(0);
            }
            Ok(v)
        }
        fn cancelled(&self) -> bool {
            self.stop_after.is_some_and(|n| self.reads >= n)
        }
        fn clock_identity(&self) -> (u64, [u8; 16]) {
            self.identity.unwrap_or((1_000_000_000, [1; 16]))
        }
        fn read(&mut self, op: Operation) -> Output {
            self.reads += 1;
            if let Some((kind, value)) = self.changed {
                if kind == op {
                    return value;
                }
            }
            match op {
                Operation::Pid => Output::Pid(Pid {
                    value: self.pid,
                    error: None,
                }),
                Operation::Times => Output::Times(Returned {
                    status: 1,
                    value: Times {
                        creation: 123,
                        exit: 0,
                        kernel: 10,
                        user: 20,
                    },
                    error: None,
                }),
                Operation::JobFlags => Output::JobFlags(Returned {
                    status: 1,
                    value: 0,
                    error: None,
                }),
                Operation::Limits => Output::Limits(Returned {
                    status: 1,
                    value: LimitRead {
                        limits: limits(),
                        returned_bytes: 144,
                    },
                    error: None,
                }),
                Operation::Process => Output::Process(ProcessObservation {
                    creation: 123,
                    wait_status: if self.kind == Kind::Starting { 0 } else { 258 },
                    wait_error: None,
                    exit_code: if self.kind == Kind::Starting {
                        Some(0)
                    } else {
                        None
                    },
                    exit_error: None,
                    termination: None,
                    active_processes: Some(if self.kind == Kind::Starting { 0 } else { 1 }),
                    accounting_error: None,
                }),
                Operation::Membership => Output::Membership(Returned {
                    status: 1,
                    value: 1,
                    error: None,
                }),
            }
        }
    }
    fn snapshot(kind: Kind) -> Snapshot {
        collect(
            origin(kind),
            window(),
            limits(),
            144,
            SourceKind::RetainedHandle,
            &mut Fake::new(kind),
        )
    }
    #[test]
    fn live_and_exited_model_roles_have_distinct_wait_job_and_membership_requirements() {
        for kind in [Kind::Starting, Kind::Sampler, Kind::Worker] {
            let s = snapshot(kind);
            assert_eq!(assess(&s, origin(kind), window()), Ok(()));
            assert_eq!(s.calls[5].attempted, kind != Kind::Starting);
        }
    }
    #[test]
    fn public_fixture_data_never_becomes_an_os_observation() {
        let s = snapshot(Kind::Worker);
        let f = Snapshot::fixture(
            s.origin,
            s.window,
            s.expected_limits,
            s.limit_bytes,
            s.calls,
        );
        assert_eq!(assess(&f, s.origin, s.window), Err(Fault::Untrusted));
    }
    #[test]
    fn every_capture_failure_prevents_successor_reads_and_retains_returned_output() {
        for at in 0usize..12 {
            let mut f = Fake::new(Kind::Worker);
            f.fail_capture = Some(at);
            let s = collect(
                origin(Kind::Worker),
                window(),
                limits(),
                144,
                SourceKind::Fixture,
                &mut f,
            );
            assert_eq!(f.reads, at.div_ceil(2));
            assert_eq!(s.fault, Some(Fault::Clock));
            assert!(s.capture_error.is_some());
            if at % 2 == 1 {
                assert!(s.calls[at / 2].output.is_some());
            }
        }
    }
    #[test]
    fn stop_after_each_return_keeps_that_result_and_does_not_read_another_operation() {
        for stop in 0..=6 {
            let mut f = Fake::new(Kind::Worker);
            f.stop_after = Some(stop);
            let s = collect(
                origin(Kind::Worker),
                window(),
                limits(),
                144,
                SourceKind::Fixture,
                &mut f,
            );
            assert_eq!(f.reads, stop);
            assert_eq!(s.fault, Some(Fault::Stopped));
            if stop > 0 {
                assert!(s.calls[stop - 1].output.is_some());
            }
        }
    }
    #[test]
    fn failed_raw_return_has_priority_over_a_later_clock_failure_and_keeps_both() {
        let mut f = Fake::new(Kind::Worker);
        let raw = Output::Pid(Pid {
            value: 0,
            error: Some(6),
        });
        f.changed = Some((Operation::Pid, raw));
        f.fail_capture = Some(1);
        let s = collect(
            origin(Kind::Worker),
            window(),
            limits(),
            144,
            SourceKind::Fixture,
            &mut f,
        );
        assert_eq!(s.fault, Some(Fault::Os(Operation::Pid)));
        assert_eq!(s.calls[0].output, Some(raw));
        assert!(s.capture_error.is_some());
        assert_eq!(f.reads, 1);
    }
    #[test]
    fn clock_regression_token_failure_and_original_deadline_equality_close_collection() {
        let mut f = Fake::new(Kind::Worker);
        f.bad_anchor = Some(3);
        let s = collect(
            origin(Kind::Worker),
            window(),
            limits(),
            144,
            SourceKind::Fixture,
            &mut f,
        );
        assert_eq!(f.reads, 2);
        assert_eq!(s.fault, Some(Fault::Clock));
        assert!(s.rejected_anchor.is_some());
        let mut f = Fake::new(Kind::Worker);
        f.offset = 999_900;
        let s = collect(
            origin(Kind::Worker),
            window(),
            limits(),
            144,
            SourceKind::Fixture,
            &mut f,
        );
        assert_eq!(s.fault, Some(Fault::Deadline));
        assert_eq!(f.reads, 0);
        let mut f = Fake::new(Kind::Worker);
        f.regression_at = Some(3);
        let s = collect(
            origin(Kind::Worker),
            window(),
            limits(),
            144,
            SourceKind::Fixture,
            &mut f,
        );
        assert_eq!(s.clock_error, Some(ClockError::Regression));
        assert_eq!(f.reads, 2);
        let mut f = Fake::new(Kind::Worker);
        f.identity = Some((1, [9; 16]));
        let s = collect(
            origin(Kind::Worker),
            window(),
            limits(),
            144,
            SourceKind::Fixture,
            &mut f,
        );
        assert_eq!(s.fault, Some(Fault::Binding));
        assert_eq!(f.reads, 0);
        assert_eq!(s.cancelled(), (false, None));
    }
    #[test]
    fn fresh_pid_creation_job_flags_and_limits_must_match_the_retained_origin_and_policy() {
        let original = snapshot(Kind::Worker);
        for (index, output, expected) in [
            (
                0,
                Output::Pid(Pid {
                    value: 99,
                    error: None,
                }),
                Fault::Identity,
            ),
            (
                1,
                Output::Times(Returned {
                    status: 1,
                    value: Times {
                        creation: 999,
                        ..Default::default()
                    },
                    error: None,
                }),
                Fault::Identity,
            ),
            (
                2,
                Output::JobFlags(Returned {
                    status: 1,
                    value: 1,
                    error: None,
                }),
                Fault::JobFlags,
            ),
            (
                3,
                Output::Limits(Returned {
                    status: 1,
                    value: LimitRead {
                        limits: Limits {
                            active_limit: 2,
                            ..limits()
                        },
                        returned_bytes: 144,
                    },
                    error: None,
                }),
                Fault::Limits,
            ),
            (
                3,
                Output::Limits(Returned {
                    status: 1,
                    value: LimitRead {
                        limits: limits(),
                        returned_bytes: 0,
                    },
                    error: None,
                }),
                Fault::Limits,
            ),
            (
                5,
                Output::Membership(Returned {
                    status: 1,
                    value: 0,
                    error: None,
                }),
                Fault::Membership,
            ),
        ] {
            let mut s = original;
            s.calls[index].output = Some(output);
            assert_eq!(assess(&s, s.origin, s.window), Err(expected));
        }
    }
    #[test]
    fn wait_exit_accounting_and_termination_remain_independent_facts() {
        let original = snapshot(Kind::Worker);
        let base = match original.calls[4].output.unwrap() {
            Output::Process(p) => p,
            _ => unreachable!(),
        };
        for (p, expected) in [
            (
                ProcessObservation {
                    wait_status: 0,
                    ..base
                },
                Fault::Wait,
            ),
            (
                ProcessObservation {
                    exit_code: Some(0),
                    ..base
                },
                Fault::Exit,
            ),
            (
                ProcessObservation {
                    active_processes: Some(0),
                    ..base
                },
                Fault::Accounting,
            ),
            (
                ProcessObservation {
                    termination: Some(Err(5)),
                    ..base
                },
                Fault::Termination,
            ),
        ] {
            let mut s = original;
            s.calls[4].output = Some(Output::Process(p));
            assert_eq!(assess(&s, s.origin, s.window), Err(expected));
        }
        let mut exited = snapshot(Kind::Starting);
        let mut p = match exited.calls[4].output.unwrap() {
            Output::Process(p) => p,
            _ => unreachable!(),
        };
        p.exit_code = Some(1);
        exited.calls[4].output = Some(Output::Process(p));
        assert_eq!(
            assess(&exited, exited.origin, exited.window),
            Err(Fault::Exit)
        );
    }
    fn frame() -> (Frame, [Origin; 3]) {
        let kinds = [Kind::Starting, Kind::Sampler, Kind::Worker];
        let origins = std::array::from_fn(|i| {
            let o = origin(kinds[i]);
            Origin::new(
                o.kind(),
                o.binding(),
                o.declaration(),
                o.image(),
                42 + i as u32,
                o.creation(),
            )
            .unwrap()
        });
        let snapshots = std::array::from_fn(|i| {
            let mut f = Fake::new(kinds[i]);
            f.pid = origins[i].pid();
            f.offset = 1000 + i as u64 * 2000;
            Some(collect(
                origins[i],
                window(),
                limits(),
                144,
                SourceKind::RetainedHandle,
                &mut f,
            ))
        });
        (
            Frame {
                window: window(),
                observed_clock: (1_000_000_000, [1; 16]),
                before: Some(a(100)),
                after: Some(a(8000)),
                before_error: None,
                after_error: None,
                snapshots,
                checks: [Err(Fault::Missing); 3],
                clock_result: Err(Fault::Clock),
                result: Err(Fault::Context),
            },
            origins,
        )
    }
    #[test]
    fn batch_requires_causal_original_clocks_and_accepted_parent_context() {
        let (mut f, o) = frame();
        screen_frame(&mut f, o, true);
        assert_eq!(f.result, Ok(()));
        let (mut f, o) = frame();
        screen_frame(&mut f, o, false);
        assert_eq!(f.result, Err(Fault::Context));
        assert_eq!(f.checks, [Ok(()); 3]);
        let (mut f, o) = frame();
        f.after = Some(a(200));
        screen_frame(&mut f, o, true);
        assert_eq!(f.clock_result, Err(Fault::Clock));
        assert!(f.result.is_err());
        let (mut f, o) = frame();
        f.after = Some(a(f.window.deadline_ns));
        screen_frame(&mut f, o, true);
        assert_eq!(f.result, Err(Fault::Deadline));
    }
    #[test]
    fn mismatched_or_missing_owner_and_os_errors_cannot_be_masked_by_other_good_roles() {
        let (mut f, o) = frame();
        f.snapshots[2] = None;
        screen_frame(&mut f, o, true);
        assert_eq!(f.checks, [Ok(()), Ok(()), Err(Fault::Missing)]);
        let (mut f, mut o) = frame();
        o.swap(1, 2);
        screen_frame(&mut f, o, true);
        assert_eq!(f.result, Err(Fault::Binding));
        let mut s = snapshot(Kind::Worker);
        s.calls[5].output = Some(Output::Membership(Returned {
            status: 0,
            value: 1,
            error: Some(5),
        }));
        assert_eq!(
            assess(&s, s.origin, s.window),
            Err(Fault::Os(Operation::Membership))
        );
    }
}
