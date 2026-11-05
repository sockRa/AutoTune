//! Pre-manifest host observations. API facts and owner assertions stay distinct;
//! neither a matching artifact nor its reported source grants execution consent.
use super::{
    manifest::PilotDraft,
    settings::{parse_anchor, raw_anchor},
    starting::Scope,
};
use crate::{
    evidence::{decode, Digest, Domain, Value},
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{exact, get, hex, object, string, unhex},
    },
};

const CAP: usize = 16 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Binding,
    Clock,
    Deadline,
    Stopped,
    Protocol,
    Missing,
    Elevated,
    Architecture,
    Conditions,
    Simulated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Version,
    Architecture,
    Elevation,
    PowerStatus,
    PowerScheme,
}
const OPERATIONS: [Operation; 5] = [
    Operation::Version,
    Operation::Architecture,
    Operation::Elevation,
    Operation::PowerStatus,
    Operation::PowerScheme,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoolReturn {
    pub returned: i32,
    pub error: Option<u32>,
}
impl BoolReturn {
    pub fn success(self) -> bool {
        self.returned != 0 && self.error.is_none()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raw {
    Version {
        status: i32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
    },
    Architecture {
        call: BoolReturn,
        native: u16,
        process_machine: u16,
        process_x64: bool,
    },
    Elevation {
        open: BoolReturn,
        query: Option<BoolReturn>,
        returned_bytes: u32,
        elevated: u32,
        close: Option<BoolReturn>,
    },
    PowerStatus {
        call: BoolReturn,
        ac: u8,
        battery_flags: u8,
        battery_percent: u8,
        saver: u8,
        lifetime: u32,
        full_lifetime: u32,
    },
    PowerScheme {
        status: u32,
        pointer_present: bool,
        guid: Option<[u8; 16]>,
        released: Option<bool>,
        release_error: Option<u32>,
    },
}
impl Raw {
    fn operation(self) -> Operation {
        match self {
            Self::Version { .. } => Operation::Version,
            Self::Architecture { .. } => Operation::Architecture,
            Self::Elevation { .. } => Operation::Elevation,
            Self::PowerStatus { .. } => Operation::PowerStatus,
            Self::PowerScheme { .. } => Operation::PowerScheme,
        }
    }
}
pub trait Reader {
    /// A fixed bounded group of host-only API calls. No GPU/driver/tool calls.
    /// Failed OS-resource cleanup owners stay with the reader, never retried.
    fn read(&mut self, operation: Operation) -> Raw;
}
#[derive(Clone, Debug)]
pub struct Conditions {
    pub asserted_at: Anchor,
    /// Owner assertion only: Some(false) declares no known competing activity.
    pub background_load: Option<bool>,
    pub competing_tuner: Option<bool>,
    /// Reference to the separately retained assertion, never workload consent.
    pub reference: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub operation: Operation,
    pub before: Option<Anchor>,
    pub raw: Option<Raw>,
    pub after: Option<Anchor>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Fixture,
    WindowsApi,
}
pub struct Snapshot {
    scope: Scope,
    conditions: Conditions,
    source: Source,
    observations: [Option<Observation>; 5],
    halt: Option<Fault>,
    canonical: String,
    digest: Digest,
    _allocation: Reservation,
}
impl Snapshot {
    /// Injectable collection is permanently labelled simulated_fixture.
    pub fn collect_fixture(
        scope: &Scope,
        conditions: Conditions,
        reader: &mut impl Reader,
        clock: impl FnMut() -> Option<Anchor>,
        stopped: impl FnMut() -> bool,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        Self::collect(
            scope,
            conditions,
            reader,
            clock,
            stopped,
            budget,
            Source::Fixture,
        )
    }
    #[cfg(windows)]
    pub(super) fn collect_windows(
        scope: &Scope,
        conditions: Conditions,
        reader: &mut super::host_environment_windows::WindowsReader,
        clock: impl FnMut() -> Option<Anchor>,
        stopped: impl FnMut() -> bool,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        Self::collect(
            scope,
            conditions,
            reader,
            clock,
            stopped,
            budget,
            Source::WindowsApi,
        )
    }
    fn collect(
        scope: &Scope,
        conditions: Conditions,
        reader: &mut impl Reader,
        mut capture: impl FnMut() -> Option<Anchor>,
        mut stopped: impl FnMut() -> bool,
        budget: &Budget,
        source: Source,
    ) -> Result<Self, Fault> {
        if conditions.reference.capacity() > 1024 || conditions.reference.contains('\0') {
            return Err(Fault::Capacity);
        }
        // All retained/model/codec capacity is acquired before the first read.
        let allocation = budget
            .reserve(CAP * 2 + std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let _codec = budget.reserve(CAP * 16).map_err(|_| Fault::Capacity)?;
        let scope =
            Scope::parse(scope.canonical().as_bytes(), budget).map_err(|_| Fault::Binding)?;
        let d = scope.draft();
        let mut clock =
            Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| Fault::Clock)?;
        let mut last = d.epoch;
        let deadline = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .preflight_ns;
        let mut result = Self {
            scope,
            conditions,
            source,
            observations: [None; 5],
            halt: None,
            canonical: String::new(),
            digest: Digest::of(Domain::RawArtifact, b""),
            _allocation: allocation,
        };
        for (i, operation) in OPERATIONS.into_iter().enumerate() {
            result.observations[i] = Some(Observation {
                operation,
                before: capture(),
                raw: None,
                after: None,
            });
            let o = result.observations[i]
                .as_mut()
                .expect("retained observation");
            let checked = check(&mut clock, &mut last, o.before, deadline);
            let pre = checked.and_then(|_| {
                if stopped() {
                    Err(Fault::Stopped)
                } else {
                    Ok(())
                }
            });
            if let Err(f) = pre {
                result.halt = Some(f);
                break;
            }
            o.raw = Some(reader.read(operation)); // Raw return precedes post-clock callback.
            o.after = capture();
            let checked = check(&mut clock, &mut last, o.after, deadline);
            let post = checked.and_then(|_| {
                if stopped() {
                    Err(Fault::Stopped)
                } else if o.raw.is_none_or(|r| r.operation() != operation) {
                    Err(Fault::Protocol)
                } else {
                    Ok(())
                }
            });
            if let Err(f) = post {
                result.halt = Some(f);
                break;
            }
        }
        result.finish()?;
        Ok(result)
    }
    fn finish(&mut self) -> Result<(), Fault> {
        self.canonical = object([
            ("version", text("1")),
            ("scope", text("pilot_host_environment")),
            ("request_digest", text(&self.scope.digest().to_hex())),
            ("request", text(self.scope.canonical())),
            (
                "source",
                text(match self.source {
                    Source::Fixture => "simulated_fixture",
                    Source::WindowsApi => "windows_api",
                }),
            ),
            (
                "conditions",
                object([
                    ("source", text("owner_assertion")),
                    ("asserted_at", raw_anchor(self.conditions.asserted_at)),
                    (
                        "background_load",
                        optional_bool(self.conditions.background_load),
                    ),
                    (
                        "competing_tuner",
                        optional_bool(self.conditions.competing_tuner),
                    ),
                    ("reference", text(&self.conditions.reference)),
                ]),
            ),
            (
                "worker_environment_declared",
                text(super::launch::WORKER_ENVIRONMENT.trim_end_matches('\0')),
            ),
            (
                "observations",
                Value::Array(
                    self.observations
                        .iter()
                        .flatten()
                        .map(|o| {
                            object([
                                ("operation", text(operation_name(o.operation))),
                                ("before", anchor_value(o.before)),
                                ("raw", o.raw.map(raw_value).unwrap_or(Value::Null)),
                                ("after", anchor_value(o.after)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "halt",
                self.halt
                    .map(|f| text(fault_name(f)))
                    .unwrap_or(Value::Null),
            ),
        ])
        .to_canonical();
        if self.canonical.len() > CAP {
            return Err(Fault::Capacity);
        }
        self.digest = Digest::of(Domain::RawArtifact, self.canonical.as_bytes());
        Ok(())
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    pub fn observations(&self) -> impl Iterator<Item = &Observation> {
        self.observations.iter().flatten()
    }
    pub fn halt(&self) -> Option<Fault> {
        self.halt
    }
    pub fn conditions(&self) -> &Conditions {
        &self.conditions
    }
    /// Checks raw host facts only, not runtime mapping, peer identity, fresh
    /// safety, assertion truth, human consent, native authority or qualification.
    pub fn host_facts(&self) -> Result<(), Fault> {
        if let Some(f) = self.halt {
            return Err(f);
        }
        let d = self.scope.draft();
        let mut clock =
            Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| Fault::Clock)?;
        let mut last = d.epoch;
        let deadline = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .preflight_ns;
        check(
            &mut clock,
            &mut last,
            Some(self.conditions.asserted_at),
            deadline,
        )?;
        for (i, operation) in OPERATIONS.into_iter().enumerate() {
            let o = self.observations[i].ok_or(Fault::Missing)?;
            if o.operation != operation {
                return Err(Fault::Protocol);
            }
            check(&mut clock, &mut last, o.before, deadline)?;
            check(&mut clock, &mut last, o.after, deadline)?;
            let raw = o.raw.ok_or(Fault::Missing)?;
            if raw.operation() != operation {
                return Err(Fault::Protocol);
            }
            match raw {
                Raw::Version {
                    status: 0,
                    major,
                    build,
                    platform: 2,
                    ..
                } if major > 0 && build > 0 => (),
                Raw::Architecture {
                    call,
                    native: 0x8664,
                    process_machine: 0,
                    process_x64: true,
                } if call.success() => (),
                Raw::Architecture { .. } => return Err(Fault::Architecture),
                Raw::Elevation {
                    open,
                    query: Some(query),
                    returned_bytes: 4,
                    elevated,
                    close: Some(close),
                } if open.success() && query.success() && close.success() => {
                    if elevated != 0 {
                        return Err(Fault::Elevated);
                    }
                }
                Raw::PowerStatus {
                    call,
                    ac: 0..=1,
                    saver: 0..=1,
                    ..
                } if call.success() => (),
                Raw::PowerScheme {
                    status: 0,
                    pointer_present: true,
                    guid: Some(g),
                    released: Some(true),
                    release_error: None,
                } if g != [0; 16] => (),
                _ => return Err(Fault::Missing),
            }
        }
        if self.conditions.background_load != Some(false)
            || self.conditions.competing_tuner != Some(false)
            || self.conditions.reference.trim().is_empty()
        {
            return Err(Fault::Conditions);
        }
        Ok(())
    }
    /// Reference checks accept only the reported Windows API path, never a
    /// fixture. The source tag and matching bytes still authenticate no reads.
    pub fn check_manifest(
        &self,
        draft: &PilotDraft,
        frequency: u64,
        epoch: Anchor,
    ) -> Result<(), Fault> {
        if self.source != Source::WindowsApi {
            return Err(Fault::Simulated);
        }
        let s = self.scope.draft();
        if draft.environment_evidence != self.digest
            || draft.session != s.session
            || draft.clock_token != s.clock_token
            || draft.raw_uuid != s.raw_uuid
            || draft.raw_pci != s.raw_pci
            || draft.source_commit != s.source_commit
            || draft.artifacts != s.artifacts
            || draft.runs.contains(&s.capture_nonce)
            || frequency != s.frequency
            || epoch != s.epoch
        {
            return Err(Fault::Binding);
        }
        self.host_facts()
    }
    pub fn check_run(
        &self,
        run: &super::manifest::BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
    ) -> Result<(), Fault> {
        self.check_manifest(run.manifest_draft(), frequency, epoch)?;
        let last = self
            .observations()
            .last()
            .and_then(|o| o.after)
            .ok_or(Fault::Clock)?;
        let s = self.scope.draft();
        let mut clock =
            Clock::temporal(s.frequency, s.clock_token, s.epoch).map_err(|_| Fault::Clock)?;
        let elapsed = clock.observe(last).map_err(|_| Fault::Clock)?;
        if elapsed > run.origin_ns() {
            return Err(Fault::Deadline);
        }
        Ok(())
    }
}
/// A strictly read peer claim. Its source label does not authenticate an OS API.
pub struct Reported(Snapshot);
impl Reported {
    pub fn snapshot(&self) -> &Snapshot {
        &self.0
    }
    /// Compose the two pre-manifest artifacts' reference/readability checks.
    /// This freezes declarations only; actual peer/runtime/consent admission is
    /// still required and cannot be issued by either reported artifact.
    pub fn freeze_manifest(
        &self,
        starting: &super::starting::ReportedSnapshot,
        draft: PilotDraft,
        frequency: u64,
        epoch: Anchor,
        budget: &Budget,
    ) -> Result<super::manifest::PilotManifest, Fault> {
        if starting.snapshot().scope().digest() != self.0.scope.digest() {
            return Err(Fault::Binding);
        }
        self.0.check_manifest(&draft, frequency, epoch)?;
        starting
            .check_manifest(&draft, frequency, epoch)
            .map_err(|_| Fault::Binding)?;
        super::manifest::PilotManifest::freeze(draft, budget).map_err(|e| match e {
            super::manifest::Error::Capacity => Fault::Capacity,
            _ => Fault::Binding,
        })
    }
    pub fn read(
        scope: &Scope,
        bytes: &[u8],
        expected: Digest,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        if bytes.len() > CAP {
            return Err(Fault::Capacity);
        }
        if Digest::of(Domain::RawArtifact, bytes) != expected {
            return Err(Fault::Binding);
        }
        let allocation = budget
            .reserve(CAP * 2 + std::mem::size_of::<Snapshot>())
            .map_err(|_| Fault::Capacity)?;
        let _codec = budget.reserve(CAP * 16).map_err(|_| Fault::Capacity)?;
        crate::sampler::wire::validate_complexity_limit(bytes, 512).map_err(|_| Fault::Capacity)?;
        let v = decode(bytes, CAP).map_err(|_| Fault::Protocol)?;
        let o = obj(
            &v,
            &[
                "version",
                "scope",
                "request_digest",
                "request",
                "source",
                "conditions",
                "worker_environment_declared",
                "observations",
                "halt",
            ],
        )?;
        if txt(get(o, "version"))? != "1"
            || txt(get(o, "scope"))? != "pilot_host_environment"
            || txt(get(o, "request_digest"))? != scope.digest().to_hex()
            || txt(get(o, "request"))? != scope.canonical()
            || txt(get(o, "worker_environment_declared"))?
                != super::launch::WORKER_ENVIRONMENT.trim_end_matches('\0')
        {
            return Err(Fault::Binding);
        }
        let source = match txt(get(o, "source"))? {
            "windows_api" => Source::WindowsApi,
            "simulated_fixture" => Source::Fixture,
            _ => return Err(Fault::Protocol),
        };
        let c = obj(
            get(o, "conditions"),
            &[
                "source",
                "asserted_at",
                "background_load",
                "competing_tuner",
                "reference",
            ],
        )?;
        let reference = txt(get(c, "reference"))?;
        if txt(get(c, "source"))? != "owner_assertion"
            || reference.len() > 1024
            || reference.contains('\0')
        {
            return Err(Fault::Protocol);
        }
        let conditions = Conditions {
            asserted_at: parse_anchor(get(c, "asserted_at"))
                .map_err(|_| Fault::Clock)?
                .ok_or(Fault::Clock)?,
            background_load: opt_bool(get(c, "background_load"))?,
            competing_tuner: opt_bool(get(c, "competing_tuner"))?,
            reference: reference.into(),
        };
        let halt = if get(o, "halt") == &Value::Null {
            None
        } else {
            Some(
                [
                    Fault::Capacity,
                    Fault::Binding,
                    Fault::Clock,
                    Fault::Deadline,
                    Fault::Stopped,
                    Fault::Protocol,
                    Fault::Missing,
                    Fault::Elevated,
                    Fault::Architecture,
                    Fault::Conditions,
                    Fault::Simulated,
                ]
                .into_iter()
                .find(|f| Some(fault_name(*f)) == get(o, "halt").as_text())
                .ok_or(Fault::Protocol)?,
            )
        };
        let entries = get(o, "observations").as_array().ok_or(Fault::Protocol)?;
        if entries.is_empty() || entries.len() > 5 || (halt.is_none() && entries.len() != 5) {
            return Err(Fault::Protocol);
        }
        let mut observations = [None; 5];
        for (i, value) in entries.iter().enumerate() {
            let e = obj(value, &["operation", "before", "raw", "after"])?;
            let operation = OPERATIONS[i];
            if txt(get(e, "operation"))? != operation_name(operation) {
                return Err(Fault::Protocol);
            }
            let raw = if get(e, "raw") == &Value::Null {
                None
            } else {
                Some(parse_raw(get(e, "raw"))?)
            };
            let before = parse_anchor(get(e, "before")).map_err(|_| Fault::Clock)?;
            let after = parse_anchor(get(e, "after")).map_err(|_| Fault::Clock)?;
            if (before.is_none() || raw.is_none() || after.is_none())
                && (halt.is_none() || i + 1 != entries.len())
            {
                return Err(Fault::Protocol);
            }
            observations[i] = Some(Observation {
                operation,
                before,
                raw,
                after,
            });
        }
        let scope =
            Scope::parse(scope.canonical().as_bytes(), budget).map_err(|_| Fault::Binding)?;
        let mut snapshot = Snapshot {
            scope,
            conditions,
            source,
            observations,
            halt,
            canonical: String::new(),
            digest: expected,
            _allocation: allocation,
        };
        snapshot.finish()?;
        if snapshot.canonical.as_bytes() != bytes {
            return Err(Fault::Protocol);
        }
        Ok(Self(snapshot))
    }
}
fn obj<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a crate::evidence::Object, Fault> {
    exact(v, keys).map_err(|_| Fault::Protocol)
}
fn txt(v: &Value) -> Result<&str, Fault> {
    string(v).map_err(|_| Fault::Protocol)
}
fn number<T: TryFrom<u64>>(v: &Value) -> Result<T, Fault> {
    T::try_from(v.as_u64().map_err(|_| Fault::Protocol)?).map_err(|_| Fault::Protocol)
}
fn signed(v: &Value) -> Result<i32, Fault> {
    i32::try_from(v.as_i64().map_err(|_| Fault::Protocol)?).map_err(|_| Fault::Protocol)
}
fn boolean(v: &Value) -> Result<bool, Fault> {
    if let Value::Bool(b) = v {
        Ok(*b)
    } else {
        Err(Fault::Protocol)
    }
}
fn opt_bool(v: &Value) -> Result<Option<bool>, Fault> {
    if v == &Value::Null {
        Ok(None)
    } else {
        boolean(v).map(Some)
    }
}
fn opt_num(v: &Value) -> Result<Option<u32>, Fault> {
    if v == &Value::Null {
        Ok(None)
    } else {
        number(v).map(Some)
    }
}
fn parse_bool_return(v: &Value) -> Result<BoolReturn, Fault> {
    let o = obj(v, &["returned", "error"])?;
    Ok(BoolReturn {
        returned: signed(get(o, "returned"))?,
        error: opt_num(get(o, "error"))?,
    })
}
fn opt_return(v: &Value) -> Result<Option<BoolReturn>, Fault> {
    if v == &Value::Null {
        Ok(None)
    } else {
        parse_bool_return(v).map(Some)
    }
}
fn parse_raw(v: &Value) -> Result<Raw, Fault> {
    let outer = obj(v, &["operation", "result"])?;
    let operation = OPERATIONS
        .into_iter()
        .find(|o| Some(operation_name(*o)) == get(outer, "operation").as_text())
        .ok_or(Fault::Protocol)?;
    let v = get(outer, "result");
    Ok(match operation {
        Operation::Version => {
            let o = obj(v, &["status", "major", "minor", "build", "platform"])?;
            Raw::Version {
                status: signed(get(o, "status"))?,
                major: number(get(o, "major"))?,
                minor: number(get(o, "minor"))?,
                build: number(get(o, "build"))?,
                platform: number(get(o, "platform"))?,
            }
        }
        Operation::Architecture => {
            let o = obj(v, &["call", "native", "process_machine", "process_x64"])?;
            Raw::Architecture {
                call: parse_bool_return(get(o, "call"))?,
                native: number(get(o, "native"))?,
                process_machine: number(get(o, "process_machine"))?,
                process_x64: boolean(get(o, "process_x64"))?,
            }
        }
        Operation::Elevation => {
            let o = obj(v, &["open", "query", "returned_bytes", "elevated", "close"])?;
            Raw::Elevation {
                open: parse_bool_return(get(o, "open"))?,
                query: opt_return(get(o, "query"))?,
                returned_bytes: number(get(o, "returned_bytes"))?,
                elevated: number(get(o, "elevated"))?,
                close: opt_return(get(o, "close"))?,
            }
        }
        Operation::PowerStatus => {
            let o = obj(
                v,
                &[
                    "call",
                    "ac",
                    "battery_flags",
                    "battery_percent",
                    "saver",
                    "lifetime",
                    "full_lifetime",
                ],
            )?;
            Raw::PowerStatus {
                call: parse_bool_return(get(o, "call"))?,
                ac: number(get(o, "ac"))?,
                battery_flags: number(get(o, "battery_flags"))?,
                battery_percent: number(get(o, "battery_percent"))?,
                saver: number(get(o, "saver"))?,
                lifetime: number(get(o, "lifetime"))?,
                full_lifetime: number(get(o, "full_lifetime"))?,
            }
        }
        Operation::PowerScheme => {
            let o = obj(
                v,
                &[
                    "status",
                    "pointer_present",
                    "guid",
                    "released",
                    "release_error",
                ],
            )?;
            Raw::PowerScheme {
                status: number(get(o, "status"))?,
                pointer_present: boolean(get(o, "pointer_present"))?,
                guid: if get(o, "guid") == &Value::Null {
                    None
                } else {
                    Some(unhex(get(o, "guid")).map_err(|_| Fault::Protocol)?)
                },
                released: opt_bool(get(o, "released"))?,
                release_error: opt_num(get(o, "release_error"))?,
            }
        }
    })
}
fn check(
    clock: &mut Clock,
    last: &mut Anchor,
    anchor: Option<Anchor>,
    deadline: u64,
) -> Result<(), Fault> {
    let a = anchor.ok_or(Fault::Clock)?;
    if a.qpc_before < last.qpc_after {
        return Err(Fault::Clock);
    }
    let n = clock.observe(a).map_err(|_| Fault::Clock)?;
    *last = a;
    if n >= deadline {
        return Err(Fault::Deadline);
    }
    Ok(())
}
fn anchor_value(a: Option<Anchor>) -> Value {
    a.map(raw_anchor).unwrap_or(Value::Null)
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn uint(n: impl Into<u64>) -> Value {
    Value::U64(n.into())
}
fn optional_bool(b: Option<bool>) -> Value {
    b.map(Value::Bool).unwrap_or(Value::Null)
}
fn bool_return(r: BoolReturn) -> Value {
    object([
        ("returned", Value::I64(i64::from(r.returned))),
        ("error", r.error.map(uint).unwrap_or(Value::Null)),
    ])
}
fn raw_value(r: Raw) -> Value {
    object([
        ("operation", text(operation_name(r.operation()))),
        ("result", raw_body(r)),
    ])
}
fn raw_body(r: Raw) -> Value {
    match r {
        Raw::Version {
            status,
            major,
            minor,
            build,
            platform,
        } => object([
            ("status", Value::I64(i64::from(status))),
            ("major", uint(major)),
            ("minor", uint(minor)),
            ("build", uint(build)),
            ("platform", uint(platform)),
        ]),
        Raw::Architecture {
            call,
            native,
            process_machine,
            process_x64,
        } => object([
            ("call", bool_return(call)),
            ("native", uint(native)),
            ("process_machine", uint(process_machine)),
            ("process_x64", Value::Bool(process_x64)),
        ]),
        Raw::Elevation {
            open,
            query,
            returned_bytes,
            elevated,
            close,
        } => object([
            ("open", bool_return(open)),
            ("query", query.map(bool_return).unwrap_or(Value::Null)),
            ("returned_bytes", uint(returned_bytes)),
            ("elevated", uint(elevated)),
            ("close", close.map(bool_return).unwrap_or(Value::Null)),
        ]),
        Raw::PowerStatus {
            call,
            ac,
            battery_flags,
            battery_percent,
            saver,
            lifetime,
            full_lifetime,
        } => object([
            ("call", bool_return(call)),
            ("ac", uint(ac)),
            ("battery_flags", uint(battery_flags)),
            ("battery_percent", uint(battery_percent)),
            ("saver", uint(saver)),
            ("lifetime", uint(lifetime)),
            ("full_lifetime", uint(full_lifetime)),
        ]),
        Raw::PowerScheme {
            status,
            pointer_present,
            guid,
            released,
            release_error,
        } => object([
            ("status", uint(status)),
            ("pointer_present", Value::Bool(pointer_present)),
            ("guid", guid.map(|g| text(&hex(&g))).unwrap_or(Value::Null)),
            ("released", optional_bool(released)),
            (
                "release_error",
                release_error.map(uint).unwrap_or(Value::Null),
            ),
        ]),
    }
}
fn operation_name(o: Operation) -> &'static str {
    match o {
        Operation::Version => "rtl_get_version",
        Operation::Architecture => "is_wow64_process2",
        Operation::Elevation => "process_token_elevation",
        Operation::PowerStatus => "get_system_power_status",
        Operation::PowerScheme => "power_get_active_scheme",
    }
}
fn fault_name(f: Fault) -> &'static str {
    match f {
        Fault::Capacity => "capacity",
        Fault::Binding => "binding",
        Fault::Clock => "clock",
        Fault::Deadline => "deadline",
        Fault::Stopped => "stopped",
        Fault::Protocol => "protocol",
        Fault::Missing => "missing",
        Fault::Elevated => "elevated",
        Fault::Architecture => "architecture",
        Fault::Conditions => "conditions",
        Fault::Simulated => "simulated",
    }
}
