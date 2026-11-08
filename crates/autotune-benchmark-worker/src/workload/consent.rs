//! Separate exact-request consent evidence. Only concrete interactive capture can
//! create a console receipt; wire data and fixture claims cannot authorize launch.
//! Workload receipts still do not issue NativeAdmission or replace fresh safety.
use super::{
    launch::{Declaration, Role},
    launch_origin::Kind,
    manifest::BoundRun,
    parent_sampler::ParentSampler,
    starting::Scope,
    starting_bootstrap,
    wire::WorkerBinding,
};
use crate::{
    evidence::{Digest, Domain, Value},
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{anchor_value, object},
    },
};
pub const CAP: usize = 64 * 1024;
pub const INPUT_CAP: usize = 96;
pub const MAX_EVENTS: usize = 256;
#[path = "consent_parent.rs"]
mod parent;
pub use parent::{Assessment, Decisions, Delivery, ParentFault, ParentInputs, Rejected};
pub(super) fn assess_parent(
    p: &ParentSampler,
    d: &Decisions,
    input: &ParentInputs<'_>,
) -> Assessment {
    parent::assess(p, d, input)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputEvent {
    pub kind: u16,
    pub down: i32,
    pub repeats: u16,
    pub key: u16,
    pub scan: u16,
    pub unicode: u16,
    pub controls: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    StartingRead,
    PilotSetup,
    Workload,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Binding,
    Context,
    Clock,
    Deadline,
    NonInteractive,
    Io,
    Input,
    Declined,
    Untrusted,
    Duplicate,
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "consent {self:?}")
    }
}
impl std::error::Error for Fault {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaunchTarget {
    pub kind: Kind,
    pub binding: Digest,
    pub declaration: Digest,
    pub image: Digest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Time {
    frequency: u64,
    epoch: Anchor,
    entry: Anchor,
    issued: Anchor,
    deadline: u64,
}
/// Identifies an owned local capture; it is not a digest/authentication of all
/// decision facts and cannot reconstruct a Record or grant launch permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRef {
    pub request: Digest,
    pub receipt: Anchor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkloadContext {
    binding: WorkerBinding,
    refs: super::parent_context::References,
    peers: [super::launch_origin::Origin; 3],
}
pub struct Request {
    stage: Stage,
    scope: Digest,
    targets: [Option<LaunchTarget>; 2],
    time: Time,
    canonical: String,
    digest: Digest,
    _allocation: Reservation,
    workload_context: Option<WorkloadContext>,
}
pub struct Setup<'a, 'r> {
    pub run: &'a BoundRun<'r>,
    pub scope: &'a Scope,
    pub worker: &'a Declaration,
    pub sampler: &'a Declaration,
    pub entry: Anchor,
    pub issued: Anchor,
}
fn d(v: Digest) -> Value {
    Value::Text(v.to_hex())
}
fn stage_name(s: Stage) -> &'static str {
    match s {
        Stage::StartingRead => "starting_read",
        Stage::PilotSetup => "pilot_setup",
        Stage::Workload => "workload",
    }
}
fn notice(stage: Stage) -> &'static str {
    match stage {
        Stage::StartingRead => concat!(
            "Read starting power limit and P0 offsets through NVML; ",
            "no benchmark or control writes"
        ),
        Stage::PilotSetup => concat!(
            "Start the fixed sampler and prepare CUDA context/module/buffers/reference; ",
            "no workload dispatch or control writes"
        ),
        Stage::Workload => concat!(
            "Execute only this bound pilot run's fixed checked warm-up and measured plan; ",
            "no tuning/control writes or automatic retries"
        ),
    }
}
fn target_value(t: LaunchTarget) -> Value {
    object([
        (
            "kind",
            Value::Text(
                match t.kind {
                    Kind::Starting => "starting",
                    Kind::Sampler => "sampler",
                    Kind::Worker => "worker",
                }
                .into(),
            ),
        ),
        ("binding", d(t.binding)),
        ("declaration", d(t.declaration)),
        ("image", d(t.image)),
    ])
}
impl Request {
    pub fn starting(
        scope: &Scope,
        declaration: &starting_bootstrap::Declaration,
        issued: Anchor,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let _codec = budget.reserve(CAP * 16).map_err(|_| Fault::Capacity)?;
        if declaration.scope_digest() != scope.digest()
            || declaration.image_digest() != scope.draft().artifacts.sampler
        {
            return Err(Fault::Binding);
        }
        let s = scope.draft();
        let target = LaunchTarget {
            kind: Kind::Starting,
            binding: scope.digest(),
            declaration: declaration.tag(),
            image: declaration.image_digest(),
        };
        let time = Time {
            frequency: s.frequency,
            epoch: s.epoch,
            entry: declaration.entry(),
            issued,
            deadline: autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .preflight_ns,
        };
        let details = object([
            ("scope", Value::Text(scope.canonical().into())),
            ("operation", Value::Text(notice(Stage::StartingRead).into())),
        ]);
        Self::freeze(
            Stage::StartingRead,
            scope.digest(),
            [Some(target), None],
            time,
            details,
            budget,
        )
    }
    pub fn setup(input: Setup<'_, '_>, budget: &Budget) -> Result<Self, Fault> {
        let _codec = budget.reserve(CAP * 16).map_err(|_| Fault::Capacity)?;
        let Setup {
            run,
            scope,
            worker,
            sampler,
            entry,
            issued,
        } = input;
        let s = scope.draft();
        let m = run.manifest_draft();
        let binding = WorkerBinding::new(run, s.frequency, s.epoch).map_err(|_| Fault::Binding)?;
        if worker.role() != Role::Worker
            || sampler.role() != Role::Sampler
            || worker.binding() != binding
            || sampler.binding() != binding
            || s.session != m.session
            || s.clock_token != m.clock_token
            || s.raw_uuid != m.raw_uuid
            || s.raw_pci != m.raw_pci
            || s.source_commit != m.source_commit
            || s.artifacts != m.artifacts
            || m.runs.contains(&s.capture_nonce)
        {
            return Err(Fault::Binding);
        }
        let n = Clock::new(binding.monitor(), s.epoch)
            .and_then(|mut c| c.observe(entry))
            .map_err(|_| Fault::Clock)?;
        if n < run.origin_ns() {
            return Err(Fault::Clock);
        }
        let deadline = run
            .phase_deadline(autotune_fma_reference::profile::Phase::Setup, n)
            .map_err(|_| Fault::Deadline)?;
        let transport = binding.transport_digest();
        let targets = [
            Some(LaunchTarget {
                kind: Kind::Worker,
                binding: transport,
                declaration: worker.tag(),
                image: worker.image_digest(),
            }),
            Some(LaunchTarget {
                kind: Kind::Sampler,
                binding: transport,
                declaration: sampler.tag(),
                image: sampler.image_digest(),
            }),
        ];
        let time = Time {
            frequency: s.frequency,
            epoch: s.epoch,
            entry,
            issued,
            deadline,
        };
        let details = object([
            ("scope", Value::Text(scope.canonical().into())),
            ("manifest", Value::Text(run.manifest_canonical().into())),
            ("manifest_digest", d(run.manifest_digest())),
            (
                "profile",
                Value::Text(super::manifest::pilot_profile_canonical()),
            ),
            ("operation", Value::Text(notice(Stage::PilotSetup).into())),
        ]);
        Self::freeze(
            Stage::PilotSetup,
            scope.digest(),
            targets,
            time,
            details,
            budget,
        )
    }
    pub fn workload(
        parent: &ParentSampler,
        run: &BoundRun<'_>,
        issued: Anchor,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let _codec = budget.reserve(CAP * 16).map_err(|_| Fault::Capacity)?;
        if !parent.context_open() {
            return Err(Fault::Context);
        }
        let context = parent
            .context_check()
            .and_then(|c| c.result.ok())
            .ok_or(Fault::Context)?;
        let peers = parent
            .peer_check()
            .filter(|p| p.result.is_ok())
            .ok_or(Fault::Context)?;
        let binding = parent.context_binding();
        if WorkerBinding::new(run, binding.monitor().qpc_frequency, binding.epoch).ok()
            != Some(binding)
            || peers.context != Some(context.references)
        {
            return Err(Fault::Binding);
        }
        let (entry, deadline) = parent.consent_window();
        let current = parent
            .current()
            .and_then(|r| r.raw().receipt())
            .ok_or(Fault::Context)?;
        let sampler = parent
            .ready()
            .and_then(|r| r.message().ok())
            .and_then(|m| match m {
                super::wire::Message::SamplerReady { after, .. } => Some(*after),
                _ => None,
            })
            .ok_or(Fault::Context)?;
        if [context.worker_ready, current, sampler]
            .iter()
            .any(|a| issued.qpc_before < a.qpc_after)
        {
            return Err(Fault::Clock);
        }
        let refs = context.references;
        let peer_values = peers
            .peers
            .iter()
            .map(|p| {
                let o = p.expected;
                object([
                    ("pid", Value::U64(o.pid().into())),
                    ("creation", Value::U64(o.creation())),
                    ("declaration", d(o.declaration())),
                    ("binding", d(o.binding())),
                    ("image", d(o.image())),
                    (
                        "consent_capture",
                        o.consent().map_or(Value::Null, |c| {
                            object([
                                ("request", d(c.request)),
                                ("receipt", anchor_value(c.receipt)),
                            ])
                        }),
                    ),
                ])
            })
            .collect();
        let time = Time {
            frequency: binding.monitor().qpc_frequency,
            epoch: binding.epoch,
            entry,
            issued,
            deadline,
        };
        let details = object([
            ("manifest", Value::Text(run.manifest_canonical().into())),
            ("manifest_digest", d(run.manifest_digest())),
            ("binding", binding.value()),
            (
                "profile",
                Value::Text(super::manifest::pilot_profile_canonical()),
            ),
            ("host", refs.host.map_or(Value::Null, d)),
            ("starting", refs.starting.map_or(Value::Null, d)),
            (
                "starting_response",
                refs.starting_response.map_or(Value::Null, d),
            ),
            ("worker_setup", d(refs.worker_setup)),
            ("current", refs.current.map_or(Value::Null, d)),
            ("sampler_ready", refs.sampler_ready.map_or(Value::Null, d)),
            (
                "current_response",
                refs.current_response.map_or(Value::Null, d),
            ),
            ("peers", Value::Array(peer_values)),
            ("operation", Value::Text(notice(Stage::Workload).into())),
        ]);
        let mut request = Self::freeze(
            Stage::Workload,
            refs.scope,
            [None, None],
            time,
            details,
            budget,
        )?;
        request.workload_context = Some(WorkloadContext {
            binding,
            refs,
            peers: peers.peers.map(|p| p.expected),
        });
        Ok(request)
    }
    fn freeze(
        stage: Stage,
        scope: Digest,
        targets: [Option<LaunchTarget>; 2],
        time: Time,
        details: Value,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let allocation = budget
            .reserve(CAP * 2 + std::mem::size_of::<Self>() + std::mem::size_of::<Record>())
            .map_err(|_| Fault::Capacity)?;
        let mut clock = Clock::temporal(time.frequency, time.epoch.clock_token, time.epoch)
            .map_err(|_| Fault::Clock)?;
        if time.entry.qpc_before < time.epoch.qpc_after
            || time.issued.qpc_before < time.entry.qpc_after
        {
            return Err(Fault::Clock);
        }
        for a in [time.entry, time.issued] {
            if clock.observe(a).map_err(|_| Fault::Clock)? >= time.deadline {
                return Err(Fault::Deadline);
            }
        }
        let canonical = object([
            ("version", Value::Text("2".into())),
            ("scope", Value::Text("owner_consent_request".into())),
            ("stage", Value::Text(stage_name(stage).into())),
            ("capture_scope", d(scope)),
            ("frequency", Value::U64(time.frequency)),
            ("epoch", anchor_value(time.epoch)),
            ("entry", anchor_value(time.entry)),
            ("issued", anchor_value(time.issued)),
            ("deadline_ns", Value::U64(time.deadline)),
            (
                "launch_targets",
                Value::Array(targets.into_iter().flatten().map(target_value).collect()),
            ),
            ("details", details),
        ])
        .to_canonical();
        if canonical.len() > CAP {
            return Err(Fault::Capacity);
        }
        let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
        Ok(Self {
            stage,
            scope,
            targets,
            time,
            canonical,
            digest,
            _allocation: allocation,
            workload_context: None,
        })
    }
    pub fn stage(&self) -> Stage {
        self.stage
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn scope(&self) -> Digest {
        self.scope
    }
    pub fn launch_targets(&self) -> &[Option<LaunchTarget>; 2] {
        &self.targets
    }
    /// Fixtures can retain claims but can never create an accepted launch record.
    pub fn record_fixture(self, facts: Facts) -> Record {
        Record::finish(self, facts, Source::Fixture)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoFault {
    pub kind: std::io::ErrorKind,
    pub code: Option<i32>,
}
impl From<std::io::Error> for IoFault {
    fn from(e: std::io::Error) -> Self {
        Self {
            kind: e.kind(),
            code: e.raw_os_error(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub input: [u8; INPUT_CAP],
    pub length: usize,
    pub overflow: bool,
    pub stdin_terminal: bool,
    pub stdout_terminal: bool,
    pub before: Option<Anchor>,
    pub receipt: Option<Anchor>,
    pub clock_error: Option<super::starting_export::ClockFault>,
    pub write_error: Option<IoFault>,
    pub read_error: Option<IoFault>,
    pub policy_error: Option<crate::monitor::clock::ClockError>,
    pub events: [InputEvent; MAX_EVENTS],
    pub event_count: usize,
    pub polls: u32,
    pub last_pending: u32,
    pub last_returned: u32,
    pub invalid_input: bool,
    pub deadline_expired: bool,
    pub rejected_anchor: Option<Anchor>,
    pub observed_clock: Option<(u64, [u8; 16])>,
}
impl Default for Facts {
    fn default() -> Self {
        Self {
            input: [0; INPUT_CAP],
            length: 0,
            overflow: false,
            stdin_terminal: false,
            stdout_terminal: false,
            before: None,
            receipt: None,
            clock_error: None,
            write_error: None,
            read_error: None,
            policy_error: None,
            events: [InputEvent::default(); MAX_EVENTS],
            event_count: 0,
            polls: 0,
            last_pending: 0,
            last_returned: 0,
            invalid_input: false,
            deadline_expired: false,
            rejected_anchor: None,
            observed_clock: None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Fixture,
    #[cfg(windows)]
    Console,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaunchAttempt {
    pub target: LaunchTarget,
    pub anchor: Option<Anchor>,
    pub clock_error: Option<super::starting_export::ClockFault>,
    pub result: Result<(), Fault>,
    pub observed_clock: Option<(u64, [u8; 16])>,
}
pub struct Record {
    request: Request,
    facts: Facts,
    source: Source,
    result: Result<(), Fault>,
    used: [bool; 2],
    usage_fault: Option<Fault>,
    attempts: [Option<LaunchAttempt>; 2],
    rejected: Option<LaunchAttempt>,
}
impl Record {
    /// Lossless bounded capture facts for the final workload raw export.
    /// The caller must reserve temporary serialization capacity before
    /// constructing this value; it records claims and console observations but
    /// does not turn them into authenticated consent.
    #[cfg(windows)]
    pub(crate) fn raw_evidence_value(&self) -> Value {
        let facts = &self.facts;
        let optional_anchor = |anchor: Option<Anchor>| anchor.map_or(Value::Null, anchor_value);
        let optional_error = |error: Option<String>| error.map_or(Value::Null, Value::Text);
        let attempt_value = |attempt: LaunchAttempt| {
            object([
                ("target", target_value(attempt.target)),
                ("anchor", optional_anchor(attempt.anchor)),
                (
                    "clock_error",
                    optional_error(attempt.clock_error.map(|error| format!("{error:?}"))),
                ),
                (
                    "result",
                    Value::Text(match attempt.result {
                        Ok(()) => "ok".to_owned(),
                        Err(error) => format!("{error:?}"),
                    }),
                ),
                (
                    "observed_clock",
                    attempt
                        .observed_clock
                        .map_or(Value::Null, |(frequency, token)| {
                            object([
                                ("frequency", Value::U64(frequency)),
                                (
                                    "token",
                                    Value::Text(crate::sampler::wire_model::hex(&token)),
                                ),
                            ])
                        }),
                ),
            ])
        };
        let events = facts.events[..facts.event_count.min(MAX_EVENTS)]
            .iter()
            .map(|event| {
                object([
                    ("kind", Value::U64(event.kind.into())),
                    ("down", Value::I64(event.down.into())),
                    ("repeats", Value::U64(event.repeats.into())),
                    ("key", Value::U64(event.key.into())),
                    ("scan", Value::U64(event.scan.into())),
                    ("unicode", Value::U64(event.unicode.into())),
                    ("controls", Value::U64(event.controls.into())),
                ])
            })
            .collect::<Vec<_>>();
        let source = match self.source {
            Source::Fixture => "simulated_fixture",
            #[cfg(windows)]
            Source::Console => "console",
        };
        let policy_error = facts.policy_error.map(|error| format!("{error:?}"));
        object([
            ("version", Value::Text("1".into())),
            ("scope", Value::Text("owner_consent_capture".into())),
            ("request", Value::Text(self.request.canonical().into())),
            ("source", Value::Text(source.into())),
            (
                "result",
                Value::Text(match self.result() {
                    Ok(()) => "accepted".into(),
                    Err(error) => format!("rejected:{error:?}"),
                }),
            ),
            (
                "capture_ref",
                self.capture_ref().map_or(Value::Null, |capture| {
                    object([
                        ("request", d(capture.request)),
                        ("receipt", anchor_value(capture.receipt)),
                    ])
                }),
            ),
            (
                "input_hex",
                Value::Text(crate::sampler::wire_model::hex(
                    &facts.input[..facts.length.min(INPUT_CAP)],
                )),
            ),
            ("input_length", Value::U64(facts.length as u64)),
            ("overflow", Value::Bool(facts.overflow)),
            ("stdin_terminal", Value::Bool(facts.stdin_terminal)),
            ("stdout_terminal", Value::Bool(facts.stdout_terminal)),
            ("before", optional_anchor(facts.before)),
            ("receipt", optional_anchor(facts.receipt)),
            (
                "clock_error",
                optional_error(facts.clock_error.map(|error| format!("{error:?}"))),
            ),
            (
                "write_error",
                facts.write_error.map_or(Value::Null, |error| {
                    object([
                        ("kind", Value::Text(format!("{:?}", error.kind))),
                        (
                            "code",
                            error
                                .code
                                .map_or(Value::Null, |code| Value::I64(code.into())),
                        ),
                    ])
                }),
            ),
            (
                "read_error",
                facts.read_error.map_or(Value::Null, |error| {
                    object([
                        ("kind", Value::Text(format!("{:?}", error.kind))),
                        (
                            "code",
                            error
                                .code
                                .map_or(Value::Null, |code| Value::I64(code.into())),
                        ),
                    ])
                }),
            ),
            ("policy_error", optional_error(policy_error)),
            ("events", Value::Array(events)),
            ("event_count", Value::U64(facts.event_count as u64)),
            ("polls", Value::U64(facts.polls.into())),
            ("last_pending", Value::U64(facts.last_pending.into())),
            ("last_returned", Value::U64(facts.last_returned.into())),
            ("invalid_input", Value::Bool(facts.invalid_input)),
            ("deadline_expired", Value::Bool(facts.deadline_expired)),
            ("rejected_anchor", optional_anchor(facts.rejected_anchor)),
            (
                "observed_clock",
                facts
                    .observed_clock
                    .map_or(Value::Null, |(frequency, token)| {
                        object([
                            ("frequency", Value::U64(frequency)),
                            (
                                "token",
                                Value::Text(crate::sampler::wire_model::hex(&token)),
                            ),
                        ])
                    }),
            ),
            (
                "launch_attempts",
                Value::Array(
                    self.attempts
                        .iter()
                        .flatten()
                        .copied()
                        .map(attempt_value)
                        .collect(),
                ),
            ),
            (
                "rejected_attempt",
                self.rejected.map_or(Value::Null, attempt_value),
            ),
            ("used", Value::Array(self.used.map(Value::Bool).into())),
            (
                "usage_fault",
                self.usage_fault
                    .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
            ),
        ])
    }

    /// Historical local identity survives rejection/revocation. Eligibility is
    /// always checked separately through result, source and original clocks.
    pub fn capture_ref(&self) -> Option<CaptureRef> {
        if self.source != Source::Fixture {
            self.facts.receipt.map(|receipt| CaptureRef {
                request: self.request.digest,
                receipt,
            })
        } else {
            None
        }
    }
    fn finish(request: Request, facts: Facts, source: Source) -> Self {
        let result = assess(&request, &facts).and_then(|()| {
            if source == Source::Fixture {
                Err(Fault::Untrusted)
            } else {
                Ok(())
            }
        });
        Self {
            request,
            facts,
            source,
            result,
            used: [false; 2],
            usage_fault: None,
            attempts: [None; 2],
            rejected: None,
        }
    }
    pub fn request(&self) -> &Request {
        &self.request
    }
    pub fn facts(&self) -> &Facts {
        &self.facts
    }
    pub fn result(&self) -> Result<(), Fault> {
        self.usage_fault.map_or(self.result, Err)
    }
    pub fn interactive(&self) -> bool {
        self.source != Source::Fixture && self.facts.stdin_terminal && self.facts.stdout_terminal
    }
    pub fn used(&self) -> [bool; 2] {
        self.used
    }
    pub fn attempts(&self) -> &[Option<LaunchAttempt>; 2] {
        &self.attempts
    }
    pub fn rejected_attempt(&self) -> Option<LaunchAttempt> {
        self.rejected
    }
    #[cfg(windows)]
    pub(crate) fn authorize_launch(
        &mut self,
        target: LaunchTarget,
        clock: &crate::monitor::windows_clock::WindowsClock,
    ) -> Result<CaptureRef, Fault> {
        let mut error = None;
        let now = if clock.clock_token() != self.request.time.epoch.clock_token
            || clock.qpc_frequency() != self.request.time.frequency
        {
            None
        } else {
            clock
                .capture()
                .map_err(|e| {
                    error = Some(e.into());
                })
                .ok()
        };
        self.record_launch(
            target,
            now,
            error,
            Some((clock.qpc_frequency(), clock.clock_token())),
        )?;
        self.capture_ref().ok_or(Fault::Untrusted)
    }
    #[cfg(test)]
    fn take_launch(
        &mut self,
        target: LaunchTarget,
        now: Option<Anchor>,
        error: Option<super::starting_export::ClockFault>,
    ) -> Result<(), Fault> {
        self.record_launch(target, now, error, None)
    }
    #[cfg(any(windows, test))]
    fn record_launch(
        &mut self,
        target: LaunchTarget,
        now: Option<Anchor>,
        error: Option<super::starting_export::ClockFault>,
        observed_clock: Option<(u64, [u8; 16])>,
    ) -> Result<(), Fault> {
        let index = self.request.targets.iter().position(|t| *t == Some(target));
        let result = (|| {
            self.result()?;
            let index = index.ok_or(Fault::Binding)?;
            if self.used[index] {
                return Err(Fault::Duplicate);
            }
            self.used[index] = true;
            if error.is_some() {
                return Err(Fault::Clock);
            }
            validate_now(
                &self.request,
                self.facts.receipt.ok_or(Fault::Clock)?,
                now.ok_or(Fault::Clock)?,
            )?;
            Ok(())
        })();
        let attempt = LaunchAttempt {
            target,
            anchor: now,
            clock_error: error,
            result,
            observed_clock,
        };
        if let Some(i) = index.filter(|i| self.attempts[*i].is_none()) {
            self.attempts[i] = Some(attempt);
        } else {
            self.rejected.get_or_insert(attempt);
        }
        if let Err(f) = result {
            self.usage_fault.get_or_insert(f);
        }
        result
    }
}
fn validate_now(request: &Request, after: Anchor, now: Anchor) -> Result<(), Fault> {
    if now.qpc_before < after.qpc_after {
        return Err(Fault::Clock);
    }
    let t = request.time;
    let mut clock =
        Clock::temporal(t.frequency, t.epoch.clock_token, t.epoch).map_err(|_| Fault::Clock)?;
    clock.observe(after).map_err(|_| Fault::Clock)?;
    if clock.observe(now).map_err(|_| Fault::Clock)? >= t.deadline {
        return Err(Fault::Deadline);
    }
    Ok(())
}
fn assess(request: &Request, facts: &Facts) -> Result<(), Fault> {
    if !facts.stdin_terminal || !facts.stdout_terminal {
        return Err(Fault::NonInteractive);
    }
    if facts.clock_error.is_some() || facts.policy_error.is_some() {
        return Err(Fault::Clock);
    }
    if facts.deadline_expired {
        return Err(Fault::Deadline);
    }
    if facts.write_error.is_some() || facts.read_error.is_some() {
        return Err(Fault::Io);
    }
    let before = facts.before.ok_or(Fault::Clock)?;
    let receipt = facts.receipt.ok_or(Fault::Clock)?;
    validate_now(request, request.time.issued, before)?;
    validate_now(request, before, receipt)?;
    if facts.overflow
        || facts.invalid_input
        || facts.length > INPUT_CAP
        || facts.event_count > MAX_EVENTS
    {
        return Err(Fault::Input);
    }
    let input = &facts.input[..facts.length];
    let line = input
        .strip_suffix(b"\r\n")
        .or_else(|| input.strip_suffix(b"\n"))
        .ok_or(Fault::Input)?;
    if line == b"decline" {
        return Err(Fault::Declined);
    }
    let mut expected = [0; 72];
    expected[..8].copy_from_slice(b"approve ");
    // No heap allocation while assessing a captured decision.
    for (i, n) in request.digest.as_bytes().iter().enumerate() {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        expected[8 + i * 2] = HEX[(n >> 4) as usize];
        expected[9 + i * 2] = HEX[(n & 15) as usize];
    }
    if line != expected {
        return Err(Fault::Input);
    }
    Ok(())
}
#[cfg(any(windows, test))]
fn input_event(facts: &mut Facts, event: InputEvent) -> bool {
    if facts.event_count >= MAX_EVENTS {
        facts.overflow = true;
        return true;
    }
    facts.events[facts.event_count] = event;
    facts.event_count += 1;
    if event.kind != 1 || event.down == 0 || event.unicode == 0 {
        return false;
    }
    for _ in 0..event.repeats {
        match event.unicode {
            8 => {
                facts.length = facts.length.saturating_sub(1);
            }
            13 => {
                if facts.length == INPUT_CAP {
                    facts.overflow = true;
                } else {
                    facts.input[facts.length] = b'\n';
                    facts.length += 1;
                }
                return true;
            }
            32..=126 => {
                if facts.length == INPUT_CAP {
                    facts.overflow = true;
                    return true;
                }
                facts.input[facts.length] = event.unicode as u8;
                facts.length += 1;
            }
            _ => {
                facts.invalid_input = true;
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
#[path = "consent_console.rs"]
mod console;
#[cfg(windows)]
impl Request {
    /// One exclusive interactive console capture. Run off the deadline path:
    /// output may block; the independent controller must supervise original
    /// deadlines and retain the prompt owner. Input polls do not renew time.
    pub fn capture_console(self, clock: &crate::monitor::windows_clock::WindowsClock) -> Record {
        console::capture(self, clock)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn a(n: u64) -> Anchor {
        Anchor {
            clock_token: [1; 16],
            qpc_before: n,
            qpc_after: n,
            interrupt_100ns: n / 100,
            unbiased_100ns: n / 100,
        }
    }
    fn digest(n: u8) -> Digest {
        Digest::of(Domain::RawArtifact, &[n])
    }
    fn request(stage: Stage) -> (Budget, Request) {
        let b = Budget::new(2 * 1024 * 1024).unwrap();
        let targets = match stage {
            Stage::StartingRead => [
                Some(LaunchTarget {
                    kind: Kind::Starting,
                    binding: digest(1),
                    declaration: digest(2),
                    image: digest(3),
                }),
                None,
            ],
            Stage::PilotSetup => [
                Some(LaunchTarget {
                    kind: Kind::Worker,
                    binding: digest(1),
                    declaration: digest(2),
                    image: digest(3),
                }),
                Some(LaunchTarget {
                    kind: Kind::Sampler,
                    binding: digest(1),
                    declaration: digest(4),
                    image: digest(5),
                }),
            ],
            Stage::Workload => [None, None],
        };
        let r = Request::freeze(
            stage,
            digest(1),
            targets,
            Time {
                frequency: 1_000_000_000,
                epoch: a(0),
                entry: a(100),
                issued: a(200),
                deadline: 1_000_000,
            },
            object([("approval_ref", Value::Text("not authority".into()))]),
            &b,
        )
        .unwrap();
        (b, r)
    }
    fn facts(r: &Request) -> Facts {
        let mut f = Facts {
            stdin_terminal: true,
            stdout_terminal: true,
            before: Some(a(300)),
            receipt: Some(a(400)),
            ..Default::default()
        };
        let line = format!("approve {}\n", r.digest.to_hex());
        f.input[..line.len()].copy_from_slice(line.as_bytes());
        f.length = line.len();
        f
    }
    // This private model intentionally cannot cross the public capture boundary.
    // It exercises attempt accounting without a terminal, process or OS clock.
    fn model(r: Request) -> Record {
        let f = facts(&r);
        let mut record = r.record_fixture(f);
        record.result = Ok(());
        record
    }
    #[test]
    fn exact_response_is_distinct_from_untrusted_fixture_authority() {
        let (_b, r) = request(Stage::StartingRead);
        let f = facts(&r);
        assert_eq!(assess(&r, &f), Ok(()));
        let record = r.record_fixture(f);
        assert_eq!(record.result(), Err(Fault::Untrusted));
        assert!(!record.interactive());
        assert_eq!(record.capture_ref(), None);
        #[cfg(windows)]
        {
            // Private console-source model only; no adapter or OS I/O is called.
            let (_b, r) = request(Stage::StartingRead);
            let f = facts(&r);
            let mut captured = Record::finish(r, f, Source::Console);
            let reference = captured.capture_ref();
            assert!(reference.is_some());
            captured.usage_fault = Some(Fault::Duplicate);
            assert_eq!(captured.result(), Err(Fault::Duplicate));
            assert_eq!(captured.capture_ref(), reference);
            captured.facts.stdin_terminal = false;
            assert!(!captured.interactive());
            assert_eq!(captured.capture_ref(), reference);
        }
    }
    #[test]
    fn stage_and_target_are_part_of_the_exact_request() {
        let (_b, read) = request(Stage::StartingRead);
        let (_b2, setup) = request(Stage::PilotSetup);
        let (_b3, work) = request(Stage::Workload);
        assert_ne!(read.digest(), setup.digest());
        assert_ne!(setup.digest(), work.digest());
        assert_eq!(assess(&setup, &facts(&read)), Err(Fault::Input));
        assert!(work.targets.iter().all(Option::is_none));
    }
    #[test]
    fn declaration_strings_booleans_partial_and_extra_responses_are_not_decisions() {
        let (_b, r) = request(Stage::StartingRead);
        for input in [
            b"yes\n".as_slice(),
            b"true\n",
            b"approve\n",
            b"not authority\n",
            b"",
            b"decline",
            b"decline now\n",
        ] {
            let mut f = facts(&r);
            f.input = [0; INPUT_CAP];
            f.input[..input.len()].copy_from_slice(input);
            f.length = input.len();
            assert_eq!(assess(&r, &f), Err(Fault::Input));
        }
        let mut f = facts(&r);
        f.input[..8].copy_from_slice(b"decline\n");
        f.length = 8;
        assert_eq!(assess(&r, &f), Err(Fault::Declined));
    }
    #[test]
    fn noninteractive_io_and_clock_failures_keep_their_separate_causes() {
        for mode in 0..6 {
            let (_b, r) = request(Stage::StartingRead);
            let mut f = facts(&r);
            let expected = match mode {
                0 => {
                    f.stdin_terminal = false;
                    Fault::NonInteractive
                }
                1 => {
                    f.stdout_terminal = false;
                    Fault::NonInteractive
                }
                2 => {
                    f.read_error = Some(IoFault {
                        kind: std::io::ErrorKind::BrokenPipe,
                        code: Some(109),
                    });
                    Fault::Io
                }
                3 => {
                    f.write_error = Some(IoFault {
                        kind: std::io::ErrorKind::BrokenPipe,
                        code: Some(232),
                    });
                    Fault::Io
                }
                4 => {
                    f.receipt = None;
                    Fault::Clock
                }
                _ => {
                    f.clock_error = Some(crate::workload::starting_export::ClockFault::ZeroToken);
                    Fault::Clock
                }
            };
            assert_eq!(assess(&r, &f), Err(expected));
            let record = r.record_fixture(f);
            assert_eq!(record.facts(), &f);
        }
    }
    #[test]
    fn deadline_equality_regression_suspend_and_overflow_cannot_be_approved() {
        let (_b, r) = request(Stage::StartingRead);
        for mode in 0..5 {
            let mut f = facts(&r);
            let expected = match mode {
                0 => {
                    f.receipt = Some(a(1_000_000));
                    Fault::Deadline
                }
                1 => {
                    f.receipt = Some(a(200));
                    Fault::Clock
                }
                2 => {
                    let mut end = a(400);
                    end.unbiased_100ns += 100_000;
                    f.receipt = Some(end);
                    Fault::Clock
                }
                3 => {
                    f.overflow = true;
                    Fault::Input
                }
                _ => {
                    f.length = INPUT_CAP + 1;
                    Fault::Input
                }
            };
            assert_eq!(assess(&r, &f), Err(expected));
        }
    }
    #[test]
    fn a_matching_role_is_consumed_once_and_duplicate_closes_all_successors() {
        let (_b, r) = request(Stage::PilotSetup);
        let targets = r.targets;
        let mut record = model(r);
        assert_eq!(
            record.take_launch(targets[0].unwrap(), Some(a(500)), None),
            Ok(())
        );
        assert_eq!(
            record.take_launch(targets[0].unwrap(), Some(a(600)), None),
            Err(Fault::Duplicate)
        );
        assert_eq!(
            record.take_launch(targets[1].unwrap(), Some(a(700)), None),
            Err(Fault::Duplicate)
        );
        assert_eq!(record.attempts()[0].unwrap().anchor, Some(a(500)));
        assert!(record.rejected_attempt().is_some());
    }
    #[test]
    fn each_setup_role_can_launch_once_but_workload_receipt_cannot_launch() {
        let (_b, r) = request(Stage::PilotSetup);
        let targets = r.targets;
        let mut record = model(r);
        for (i, target) in targets.into_iter().enumerate() {
            assert_eq!(
                record.take_launch(target.unwrap(), Some(a(500 + i as u64 * 100)), None),
                Ok(())
            );
        }
        assert_eq!(record.used(), [true, true]);
        let (_b, r) = request(Stage::Workload);
        let mut work = model(r);
        assert_eq!(
            work.take_launch(targets[0].unwrap(), Some(a(500)), None),
            Err(Fault::Binding)
        );
    }
    #[test]
    fn launch_expiry_or_wrong_target_retains_attempt_and_never_renews_consent() {
        for mode in 0..4 {
            let (_b, r) = request(Stage::StartingRead);
            let mut target = r.targets[0].unwrap();
            let mut record = model(r);
            let (now, expected) = match mode {
                0 => (Some(a(1_000_000)), Fault::Deadline),
                1 => {
                    target.image = digest(99);
                    (Some(a(500)), Fault::Binding)
                }
                2 => (None, Fault::Clock),
                _ => (Some(a(300)), Fault::Clock),
            };
            assert_eq!(record.take_launch(target, now, None), Err(expected));
            assert_eq!(record.result(), Err(expected));
            assert!(
                record.attempts().iter().any(Option::is_some)
                    || record.rejected_attempt().is_some()
            );
        }
    }
    #[test]
    fn decision_and_comparison_use_prepaid_storage_under_pressure() {
        let (b, r) = request(Stage::StartingRead);
        let f = facts(&r);
        let _pressure = b.reserve(2 * 1024 * 1024 - b.used()).unwrap();
        let used = b.used();
        assert_eq!(assess(&r, &f), Ok(()));
        let record = r.record_fixture(f);
        assert_eq!(record.result(), Err(Fault::Untrusted));
        assert_eq!(b.used(), used);
    }
    #[test]
    fn raw_key_events_keep_editing_and_repeats_bounded() {
        let mut f = Facts::default();
        let key = |unicode, repeats| InputEvent {
            kind: 1,
            down: 1,
            unicode,
            repeats,
            ..Default::default()
        };
        assert!(!input_event(&mut f, key(b'a'.into(), 2)));
        assert!(!input_event(&mut f, key(8, 1)));
        assert!(!input_event(&mut f, key(b'b'.into(), 1)));
        assert!(input_event(&mut f, key(13, 1)));
        assert_eq!(&f.input[..f.length], b"ab\n");
        assert_eq!(f.event_count, 4);
        assert_eq!(f.events[0].repeats, 2);
        let mut f = Facts::default();
        assert!(input_event(&mut f, key(b'x'.into(), u16::MAX)));
        assert!(f.overflow);
        assert_eq!(f.length, INPUT_CAP);
        let mut f = Facts::default();
        assert!(input_event(&mut f, key(0xD800, 1)));
        assert!(f.invalid_input);
        assert_eq!(f.events[0].unicode, 0xD800);
    }
    #[test]
    fn observed_timeout_or_clock_fault_stays_failed_after_a_later_valid_capture() {
        let (_b, r) = request(Stage::StartingRead);
        let mut f = facts(&r);
        f.deadline_expired = true;
        f.rejected_anchor = Some(a(1_000_000));
        assert_eq!(assess(&r, &f), Err(Fault::Deadline));
        f.deadline_expired = false;
        f.policy_error = Some(crate::monitor::clock::ClockError::Regression);
        assert_eq!(assess(&r, &f), Err(Fault::Clock));
    }
}
