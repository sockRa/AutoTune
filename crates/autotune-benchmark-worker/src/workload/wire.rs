//! Worker-role canonical framing, separate from the unchanged sampler schema.
//! Frames bind declarations and observations, never human consent or GPU proof.
use super::manifest::BoundRun;
use crate::{
    evidence::{decode, Digest, Domain, Reason, Value},
    monitor::{clock::Clock, Anchor, Binding},
    sampler::{
        budget::{Budget, Reservation},
        wire::{Lane, WireError},
        wire_model::{anchor_value, exact, get, hex, object, string, uint, unhex},
    },
    worker::Identity,
};
use autotune_fma_reference::profile::{Profile, RunPlan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ToWorker,
    FromWorker,
}
impl Direction {
    fn name(self) -> &'static str {
        match self {
            Self::ToWorker => "to_worker",
            Self::FromWorker => "from_worker",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerBinding {
    pub(super) monitor: Binding,
    pub(super) epoch: Anchor,
    pub(super) origin_ns: u64,
    pub(super) plan: RunPlan,
    manifest: Digest,
    profile: Digest,
    worker: Digest,
    sampler: Digest,
}
impl WorkerBinding {
    pub fn new(run: &BoundRun<'_>, frequency: u64, epoch: Anchor) -> Result<Self, WireError> {
        let monitor = run.binding(frequency).map_err(|_| WireError::Binding)?;
        Clock::new(monitor, epoch).map_err(|_| WireError::Binding)?;
        Ok(Self {
            monitor,
            epoch,
            origin_ns: run.origin_ns(),
            plan: run.plan(),
            manifest: run.manifest_digest(),
            profile: run.profile_digest(),
            worker: run.worker_digest(),
            sampler: run.sampler_digest(),
        })
    }
    pub fn monitor(self) -> Binding {
        self.monitor
    }
    #[cfg(windows)]
    pub(super) fn images(self) -> [Digest; 2] {
        [self.sampler, self.worker]
    }
    // Call only while holding codec storage: the canonical binding is allocated.
    pub(super) fn transport_digest(self) -> Digest {
        Digest::of(Domain::RawArtifact, self.value().to_canonical().as_bytes())
    }
    pub(super) fn elapsed(self, a: Anchor) -> Result<u64, WireError> {
        Clock::new(self.monitor, self.epoch)
            .map_err(|_| WireError::Binding)?
            .observe(a)
            .map_err(|_| WireError::Body)
    }
    pub(super) fn setup_boundary(self) -> u64 {
        self.origin_ns + self.plan.phase_caps_ns[0] + self.plan.phase_caps_ns[1]
    }
    pub(super) fn run_end(self) -> u64 {
        self.origin_ns + self.plan.envelope_ns
    }
    pub(super) fn value(self) -> Value {
        let b = self.monitor;
        object([
            ("session", Value::Text(hex(&b.session))),
            ("run", Value::Text(hex(&b.run))),
            ("device", identity_value(b.device)),
            ("manifest", Value::Text(self.manifest.to_hex())),
            ("profile", Value::Text(self.profile.to_hex())),
            ("worker", Value::Text(self.worker.to_hex())),
            ("sampler", Value::Text(self.sampler.to_hex())),
            ("clock_token", Value::Text(hex(&b.clock_token))),
            ("frequency", Value::U64(b.qpc_frequency)),
            ("epoch", anchor_value(self.epoch)),
            ("origin_ns", Value::U64(self.origin_ns)),
            (
                "plan",
                object([
                    ("seed", Value::U64(self.plan.seed)),
                    ("lanes", Value::U64(self.plan.shape.lanes().into())),
                    ("steps", Value::U64(self.plan.shape.steps().value().into())),
                    ("warmup", Value::U64(self.plan.warmup_dispatches.into())),
                    ("measured", Value::U64(self.plan.measured_dispatches.into())),
                    ("envelope_ns", Value::U64(self.plan.envelope_ns)),
                    (
                        "phase_caps_ns",
                        Value::Array(self.plan.phase_caps_ns.map(Value::U64).into()),
                    ),
                ]),
            ),
        ])
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    SamplerQuery {
        ticket: crate::monitor::Ticket,
        issued: Anchor,
    },
    SamplerRecord {
        ticket: crate::monitor::Ticket,
        before: Option<Anchor>,
        after: Option<Anchor>,
        raw: Option<autotune_probe::report::Field>,
        native: bool,
    },
    SamplerReady {
        before: Anchor,
        after: Anchor,
        raw_uuid: String,
        raw_pci: String,
        sampler_digest: Digest,
        runtime: String,
    },
    NativeFailure {
        stage: Reason,
        before: Option<Anchor>,
        after: Option<Anchor>,
        reason: autotune_probe::report::Reason,
        vendor_status: Option<i32>,
        detail: String,
    },
    PhaseEntered {
        phase: super::dispatch::Phase,
        anchor: Anchor,
    },
    DispatchRecord(super::dispatch::Record),
    FailureChunk(super::dispatch::FailureChunk),
    SetupChunk(super::worker_setup_delivery::Chunk),
    Bootstrap {
        manifest: String,
    },
    Ready {
        anchor: Anchor,
        identity: Identity,
        context: Identity,
        worker_digest: Digest,
        driver_version: i32,
        compute_major: i32,
        compute_minor: i32,
        prepared_lanes: u32,
    },
    Start {
        issued: Anchor,
    },
    /// A distinct parent authorization after Start. The opaque evidence digests
    /// bind the parent decision inputs; they do not prove those inputs' truth.
    NativeGrant {
        issued: Anchor,
        setup: Digest,
        context: Digest,
        peers: Digest,
        consent: Digest,
        process: Digest,
        safety: Digest,
    },
    Stop {
        reason: Reason,
        trigger_ns: u64,
        cooperative_ns: u64,
        exit_ns: u64,
    },
    StopAck {
        anchor: Anchor,
        pending: bool,
    },
    RoleFinished {
        anchor: Anchor,
        pending: bool,
        cleanup_attempted: bool,
        cleanup_status: Option<i32>,
    },
}
impl Message {
    fn route(&self) -> (Lane, Direction, &'static str) {
        match self {
            Self::SamplerQuery { .. } => (Lane::Ordinary, Direction::ToWorker, "sampler_query"),
            Self::SamplerRecord { .. } => (Lane::Ordinary, Direction::FromWorker, "sampler_record"),
            Self::SamplerReady { .. } => (Lane::Ordinary, Direction::FromWorker, "sampler_ready"),
            Self::NativeFailure { .. } => (Lane::Control, Direction::FromWorker, "native_failure"),
            Self::PhaseEntered { .. } => (Lane::Ordinary, Direction::FromWorker, "phase_entered"),
            Self::DispatchRecord(_) => (Lane::Ordinary, Direction::FromWorker, "dispatch_record"),
            Self::FailureChunk(_) => (Lane::Ordinary, Direction::FromWorker, "failure_chunk"),
            Self::SetupChunk(_) => (Lane::Ordinary, Direction::FromWorker, "setup_chunk"),
            Self::Bootstrap { .. } => (Lane::Ordinary, Direction::ToWorker, "bootstrap"),
            Self::Ready { .. } => (Lane::Ordinary, Direction::FromWorker, "ready"),
            Self::Start { .. } => (Lane::Ordinary, Direction::ToWorker, "start"),
            Self::NativeGrant { .. } => (Lane::Ordinary, Direction::ToWorker, "native_grant"),
            Self::Stop { .. } => (Lane::Control, Direction::ToWorker, "stop"),
            Self::StopAck { .. } => (Lane::Control, Direction::FromWorker, "stop_ack"),
            Self::RoleFinished { .. } => (Lane::Control, Direction::FromWorker, "role_finished"),
        }
    }
    fn payload_bytes(&self) -> usize {
        match self {
            Self::SamplerRecord { raw, .. } => {
                raw.as_ref().map_or(0, super::telemetry::field_bytes)
            }
            Self::SamplerReady {
                raw_uuid,
                raw_pci,
                runtime,
                ..
            } => raw_uuid.capacity() + raw_pci.capacity() + runtime.capacity(),
            Self::NativeFailure { stage, detail, .. } => stage.as_str().len() + detail.capacity(),
            Self::DispatchRecord(r) => r.allocation_bytes(),
            Self::FailureChunk(c) => c.allocation_bytes(),
            Self::SetupChunk(c) => c.allocation_bytes(),
            Self::Bootstrap { manifest } => manifest.capacity(),
            Self::Stop { reason, .. } => reason.as_str().len(),
            _ => 0,
        }
    }
    pub(super) fn validate(&self, b: WorkerBinding) -> Result<(), WireError> {
        match self {
            Self::SetupChunk(c) => c.validate()?,
            Self::SamplerQuery { ticket, issued } => {
                super::telemetry::validate_ticket(*ticket, b)?;
                if b.elapsed(*issued)? != ticket.start_ns {
                    return Err(WireError::Body);
                }
            }
            Self::SamplerRecord { ticket, raw, .. } => {
                super::telemetry::validate_ticket(*ticket, b)?;
                if raw.as_ref().is_some_and(|f| !super::settings::bounded(f)) {
                    return Err(WireError::Capacity);
                }
            }
            Self::SamplerReady {
                before,
                after,
                raw_uuid,
                raw_pci,
                sampler_digest,
                runtime,
            } => {
                let mut c = Clock::new(b.monitor, b.epoch).map_err(|_| WireError::Binding)?;
                let start = c.observe(*before).map_err(|_| WireError::Body)?;
                let end = c.observe(*after).map_err(|_| WireError::Body)?;
                if before.qpc_after > after.qpc_before
                    || start < b.origin_ns
                    || end >= b.setup_boundary()
                    || *sampler_digest != b.sampler
                    || raw_uuid.capacity() > 128
                    || raw_pci.capacity() > 128
                    || runtime.capacity() > 16384
                    || !matches!(crate::worker::DiscoveryBinding::parse(raw_uuid,raw_pci),Ok(i) if i.identity==b.monitor.device)
                {
                    return Err(WireError::Binding);
                }
            }
            Self::NativeFailure { detail, .. } => {
                if detail.capacity() > 1024 {
                    return Err(WireError::Capacity);
                }
            }
            Self::PhaseEntered { anchor, .. } => {
                if anchor.clock_token != b.monitor.clock_token {
                    return Err(WireError::Binding);
                }
            }
            Self::DispatchRecord(r) => r.validate(b)?,
            Self::FailureChunk(c) => c.validate(b)?,
            Self::Bootstrap { manifest } => {
                if manifest.capacity() > 16384
                    || Digest::of(Domain::Manifest, manifest.as_bytes()) != b.manifest
                {
                    return Err(WireError::Body);
                }
            }
            Self::Ready {
                anchor,
                identity,
                context,
                worker_digest,
                driver_version,
                compute_major,
                compute_minor,
                prepared_lanes,
            } => {
                b.elapsed(*anchor)?;
                if *identity != b.monitor.device
                    || *context != *identity
                    || *worker_digest != b.worker
                    || *driver_version <= 0
                    || (*compute_major, *compute_minor) != (7, 5)
                    || *prepared_lanes != b.plan.shape.lanes()
                {
                    return Err(WireError::Binding);
                }
            }
            Self::Start { issued } => {
                let ns = b.elapsed(*issued)?;
                if ns < b.origin_ns || ns >= b.setup_boundary() {
                    return Err(WireError::Body);
                }
            }
            Self::NativeGrant { issued, .. } => {
                let ns = b.elapsed(*issued)?;
                if ns < b.origin_ns || ns >= b.setup_boundary() {
                    return Err(WireError::Body);
                }
            }
            Self::Stop {
                trigger_ns,
                cooperative_ns,
                exit_ns,
                ..
            } => {
                let p = Profile::approved_design().limits;
                if *cooperative_ns
                    != trigger_ns
                        .saturating_add(p.cooperative_stop_ns)
                        .min(b.run_end())
                    || *exit_ns
                        != trigger_ns
                            .saturating_add(p.exit_observation_ns)
                            .min(b.run_end())
                {
                    return Err(WireError::Body);
                }
            }
            Self::StopAck { anchor, .. } => {
                b.elapsed(*anchor)?;
            }
            Self::RoleFinished {
                anchor,
                cleanup_attempted,
                cleanup_status,
                ..
            } => {
                b.elapsed(*anchor)?;
                if !cleanup_attempted && cleanup_status.is_some() {
                    return Err(WireError::Body);
                }
            }
        }
        Ok(())
    }
    fn body(&self) -> Value {
        match self {
            Self::SetupChunk(c) => c.value(),
            Self::SamplerQuery { ticket, issued } => object([
                ("ticket", super::telemetry::ticket_value(*ticket)),
                ("issued", super::settings::raw_anchor(*issued)),
            ]),
            Self::SamplerRecord {
                ticket,
                before,
                after,
                raw,
                native,
            } => object([
                ("ticket", super::telemetry::ticket_value(*ticket)),
                (
                    "before",
                    before.map_or(Value::Null, super::settings::raw_anchor),
                ),
                (
                    "after",
                    after.map_or(Value::Null, super::settings::raw_anchor),
                ),
                (
                    "raw",
                    raw.as_ref().map_or(Value::Null, super::settings::raw_value),
                ),
                ("native", Value::Bool(*native)),
            ]),
            Self::SamplerReady {
                before,
                after,
                raw_uuid,
                raw_pci,
                sampler_digest,
                runtime,
            } => object([
                ("before", super::settings::raw_anchor(*before)),
                ("after", super::settings::raw_anchor(*after)),
                ("raw_uuid", Value::Text(raw_uuid.clone())),
                ("raw_pci", Value::Text(raw_pci.clone())),
                ("sampler_digest", Value::Text(sampler_digest.to_hex())),
                ("runtime", Value::Text(runtime.clone())),
            ]),
            Self::NativeFailure {
                stage,
                before,
                after,
                reason,
                vendor_status,
                detail,
            } => object([
                ("stage", Value::Text(stage.as_str().into())),
                (
                    "before",
                    before.map_or(Value::Null, super::settings::raw_anchor),
                ),
                (
                    "after",
                    after.map_or(Value::Null, super::settings::raw_anchor),
                ),
                (
                    "reason",
                    Value::Text(
                        serde_json::to_value(reason)
                            .expect("closed reason")
                            .as_str()
                            .expect("reason text")
                            .into(),
                    ),
                ),
                (
                    "vendor_status",
                    vendor_status.map_or(Value::Null, |n| Value::I64(n.into())),
                ),
                ("detail", Value::Text(detail.clone())),
            ]),
            Self::PhaseEntered { phase, anchor } => object([
                ("phase", Value::Text(phase.name().into())),
                ("anchor", anchor_value(*anchor)),
            ]),
            Self::DispatchRecord(r) => r.value(),
            Self::FailureChunk(c) => c.value(),
            Self::Bootstrap { manifest } => object([("manifest", Value::Text(manifest.clone()))]),
            Self::Ready {
                anchor,
                identity,
                context,
                worker_digest,
                driver_version,
                compute_major,
                compute_minor,
                prepared_lanes,
            } => object([
                ("anchor", anchor_value(*anchor)),
                ("identity", identity_value(*identity)),
                ("context", identity_value(*context)),
                ("worker_digest", Value::Text(worker_digest.to_hex())),
                ("driver_version", Value::I64((*driver_version).into())),
                ("compute_major", Value::I64((*compute_major).into())),
                ("compute_minor", Value::I64((*compute_minor).into())),
                ("prepared_lanes", Value::U64((*prepared_lanes).into())),
            ]),
            Self::Start { issued } => object([("issued", anchor_value(*issued))]),
            Self::NativeGrant {
                issued,
                setup,
                context,
                peers,
                consent,
                process,
                safety,
            } => object([
                ("issued", anchor_value(*issued)),
                ("setup", Value::Text(setup.to_hex())),
                ("context", Value::Text(context.to_hex())),
                ("peers", Value::Text(peers.to_hex())),
                ("consent", Value::Text(consent.to_hex())),
                ("process", Value::Text(process.to_hex())),
                ("safety", Value::Text(safety.to_hex())),
            ]),
            Self::Stop {
                reason,
                trigger_ns,
                cooperative_ns,
                exit_ns,
            } => object([
                ("reason", Value::Text(reason.as_str().into())),
                ("trigger_ns", Value::U64(*trigger_ns)),
                ("cooperative_ns", Value::U64(*cooperative_ns)),
                ("exit_ns", Value::U64(*exit_ns)),
            ]),
            Self::StopAck { anchor, pending } => object([
                ("anchor", anchor_value(*anchor)),
                ("pending", Value::Bool(*pending)),
            ]),
            Self::RoleFinished {
                anchor,
                pending,
                cleanup_attempted,
                cleanup_status,
            } => object([
                ("anchor", anchor_value(*anchor)),
                ("pending", Value::Bool(*pending)),
                ("cleanup_attempted", Value::Bool(*cleanup_attempted)),
                (
                    "cleanup_status",
                    cleanup_status.map_or(Value::Null, |n| Value::I64(n.into())),
                ),
            ]),
        }
    }
    fn parse(kind: &str, v: &Value, b: WorkerBinding) -> Result<Self, WireError> {
        let i32_value = |v: &Value| {
            i32::try_from(v.as_i64().map_err(|_| WireError::Body)?).map_err(|_| WireError::Body)
        };
        let message = match kind {
            "sampler_query" => {
                let o = exact(v, &["ticket", "issued"])?;
                Self::SamplerQuery {
                    ticket: super::telemetry::parse_ticket(get(o, "ticket"), b)?,
                    issued: super::settings::parse_anchor(get(o, "issued"))
                        .map_err(|_| WireError::Body)?
                        .ok_or(WireError::Body)?,
                }
            }
            "sampler_record" => {
                let o = exact(v, &["ticket", "before", "after", "raw", "native"])?;
                Self::SamplerRecord {
                    ticket: super::telemetry::parse_ticket(get(o, "ticket"), b)?,
                    before: super::settings::parse_anchor(get(o, "before"))
                        .map_err(|_| WireError::Body)?,
                    after: super::settings::parse_anchor(get(o, "after"))
                        .map_err(|_| WireError::Body)?,
                    raw: match get(o, "raw") {
                        Value::Null => None,
                        v => {
                            let f = super::settings::parse_field(v).map_err(|_| WireError::Body)?;
                            if super::settings::raw_value(&f).to_canonical() != v.to_canonical() {
                                return Err(WireError::Canonical);
                            }
                            Some(f)
                        }
                    },
                    native: match get(o, "native") {
                        Value::Bool(v) => *v,
                        _ => return Err(WireError::Body),
                    },
                }
            }
            "sampler_ready" => {
                let o = exact(
                    v,
                    &[
                        "before",
                        "after",
                        "raw_uuid",
                        "raw_pci",
                        "sampler_digest",
                        "runtime",
                    ],
                )?;
                Self::SamplerReady {
                    before: super::settings::parse_anchor(get(o, "before"))
                        .map_err(|_| WireError::Body)?
                        .ok_or(WireError::Body)?,
                    after: super::settings::parse_anchor(get(o, "after"))
                        .map_err(|_| WireError::Body)?
                        .ok_or(WireError::Body)?,
                    raw_uuid: string(get(o, "raw_uuid"))?.into(),
                    raw_pci: string(get(o, "raw_pci"))?.into(),
                    sampler_digest: Digest::parse(string(get(o, "sampler_digest"))?)
                        .map_err(|_| WireError::Body)?,
                    runtime: string(get(o, "runtime"))?.into(),
                }
            }
            "native_failure" => {
                let o = exact(
                    v,
                    &[
                        "stage",
                        "before",
                        "after",
                        "reason",
                        "vendor_status",
                        "detail",
                    ],
                )?;
                Self::NativeFailure {
                    stage: Reason::new(string(get(o, "stage"))?).map_err(|_| WireError::Body)?,
                    before: super::settings::parse_anchor(get(o, "before"))
                        .map_err(|_| WireError::Body)?,
                    after: super::settings::parse_anchor(get(o, "after"))
                        .map_err(|_| WireError::Body)?,
                    reason: serde_json::from_value(serde_json::Value::String(
                        string(get(o, "reason"))?.into(),
                    ))
                    .map_err(|_| WireError::Body)?,
                    vendor_status: match get(o, "vendor_status") {
                        Value::Null => None,
                        n => Some(i32_value(n)?),
                    },
                    detail: string(get(o, "detail"))?.into(),
                }
            }
            "phase_entered" => {
                let o = exact(v, &["phase", "anchor"])?;
                Self::PhaseEntered {
                    phase: super::dispatch::Phase::parse(get(o, "phase"))?,
                    anchor: parse_anchor(get(o, "anchor"), b)?,
                }
            }
            "dispatch_record" => Self::DispatchRecord(super::dispatch::Record::parse(v, b)?),
            "failure_chunk" => Self::FailureChunk(super::dispatch::FailureChunk::parse(v)?),
            "setup_chunk" => Self::SetupChunk(super::worker_setup_delivery::Chunk::parse(v)?),
            "bootstrap" => {
                let o = exact(v, &["manifest"])?;
                Self::Bootstrap {
                    manifest: string(get(o, "manifest"))?.into(),
                }
            }
            "ready" => {
                let o = exact(
                    v,
                    &[
                        "anchor",
                        "identity",
                        "context",
                        "worker_digest",
                        "driver_version",
                        "compute_major",
                        "compute_minor",
                        "prepared_lanes",
                    ],
                )?;
                Self::Ready {
                    anchor: parse_anchor(get(o, "anchor"), b)?,
                    identity: parse_identity(get(o, "identity"))?,
                    context: parse_identity(get(o, "context"))?,
                    worker_digest: Digest::parse(string(get(o, "worker_digest"))?)
                        .map_err(|_| WireError::Body)?,
                    driver_version: i32_value(get(o, "driver_version"))?,
                    compute_major: i32_value(get(o, "compute_major"))?,
                    compute_minor: i32_value(get(o, "compute_minor"))?,
                    prepared_lanes: uint(get(o, "prepared_lanes"))?
                        .try_into()
                        .map_err(|_| WireError::Body)?,
                }
            }
            "start" => {
                let o = exact(v, &["issued"])?;
                Self::Start {
                    issued: parse_anchor(get(o, "issued"), b)?,
                }
            }
            "native_grant" => {
                let o = exact(
                    v,
                    &[
                        "issued", "setup", "context", "peers", "consent", "process", "safety",
                    ],
                )?;
                Self::NativeGrant {
                    issued: parse_anchor(get(o, "issued"), b)?,
                    setup: Digest::parse(string(get(o, "setup"))?).map_err(|_| WireError::Body)?,
                    context: Digest::parse(string(get(o, "context"))?)
                        .map_err(|_| WireError::Body)?,
                    peers: Digest::parse(string(get(o, "peers"))?).map_err(|_| WireError::Body)?,
                    consent: Digest::parse(string(get(o, "consent"))?)
                        .map_err(|_| WireError::Body)?,
                    process: Digest::parse(string(get(o, "process"))?)
                        .map_err(|_| WireError::Body)?,
                    safety: Digest::parse(string(get(o, "safety"))?)
                        .map_err(|_| WireError::Body)?,
                }
            }
            "stop" => {
                let o = exact(v, &["reason", "trigger_ns", "cooperative_ns", "exit_ns"])?;
                Self::Stop {
                    reason: Reason::new(string(get(o, "reason"))?).map_err(|_| WireError::Body)?,
                    trigger_ns: uint(get(o, "trigger_ns"))?,
                    cooperative_ns: uint(get(o, "cooperative_ns"))?,
                    exit_ns: uint(get(o, "exit_ns"))?,
                }
            }
            "stop_ack" => {
                let o = exact(v, &["anchor", "pending"])?;
                Self::StopAck {
                    anchor: parse_anchor(get(o, "anchor"), b)?,
                    pending: boolean(get(o, "pending"))?,
                }
            }
            "role_finished" => {
                let o = exact(
                    v,
                    &["anchor", "pending", "cleanup_attempted", "cleanup_status"],
                )?;
                Self::RoleFinished {
                    anchor: parse_anchor(get(o, "anchor"), b)?,
                    pending: boolean(get(o, "pending"))?,
                    cleanup_attempted: boolean(get(o, "cleanup_attempted"))?,
                    cleanup_status: match get(o, "cleanup_status") {
                        Value::Null => None,
                        v => Some(i32_value(v)?),
                    },
                }
            }
            _ => return Err(WireError::Body),
        };
        message.validate(b)?;
        Ok(message)
    }
}
pub struct EncodedFrame {
    bytes: Vec<u8>,
    pub(super) binding: Digest,
    pub(super) lane: Lane,
    pub(super) direction: Direction,
    _allocation: Reservation,
}
impl EncodedFrame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
pub struct OwnedMessage {
    message: Message,
    binding: WorkerBinding,
    _allocation: Reservation,
}
impl OwnedMessage {
    pub fn binding(&self) -> WorkerBinding {
        self.binding
    }
    pub fn message(&self) -> &Message {
        &self.message
    }
}
pub struct Channel {
    binding: WorkerBinding,
    lane: Lane,
    direction: Direction,
    next: u64,
    poisoned: bool,
    budget: Budget,
    _metadata: Reservation,
    sampler: bool,
}
impl Channel {
    pub fn sampler(
        binding: WorkerBinding,
        lane: Lane,
        direction: Direction,
        budget: Budget,
    ) -> Result<Self, WireError> {
        let mut result = Self::new(binding, lane, direction, budget)?;
        result.sampler = true;
        Ok(result)
    }
    fn role(&self) -> &'static str {
        if self.sampler {
            "pilot_sampler"
        } else {
            "worker"
        }
    }
    fn role_message(&self, m: &Message) -> Result<(), WireError> {
        let shared = matches!(
            m,
            Message::Stop { .. }
                | Message::StopAck { .. }
                | Message::RoleFinished { .. }
                | Message::NativeFailure { .. }
        );
        let specific = matches!(
            m,
            Message::SamplerReady { .. }
                | Message::SamplerQuery { .. }
                | Message::SamplerRecord { .. }
        );
        if shared || self.sampler == specific {
            Ok(())
        } else {
            Err(WireError::Binding)
        }
    }
    pub(super) fn lane(&self) -> Lane {
        self.lane
    }
    /// Encode an owned observed trace only on its original execution binding.
    pub fn encode_traced(
        &mut self,
        trace: &super::trace::TracedRecord,
    ) -> Result<EncodedFrame, WireError> {
        if self.poisoned {
            return Err(WireError::Poisoned);
        }
        if trace.binding != self.binding {
            self.poison();
            return Err(WireError::Binding);
        }
        self.encode(&trace.message)
    }
    pub fn new(
        binding: WorkerBinding,
        lane: Lane,
        direction: Direction,
        budget: Budget,
    ) -> Result<Self, WireError> {
        let metadata = budget.reserve(4096).map_err(|_| WireError::Capacity)?;
        Ok(Self {
            sampler: false,
            binding,
            lane,
            direction,
            next: 0,
            poisoned: false,
            budget,
            _metadata: metadata,
        })
    }
    /// External framing/transport failures must close the lane too.
    pub fn poison(&mut self) {
        self.poisoned = true;
    }
    fn finish<T>(&mut self, result: Result<T, WireError>) -> Result<T, WireError> {
        match result {
            Ok(value) => {
                self.next = match self.next.checked_add(1) {
                    Some(n) => n,
                    None => {
                        self.poison();
                        return Err(WireError::Sequence);
                    }
                };
                Ok(value)
            }
            Err(e) => {
                self.poison();
                Err(e)
            }
        }
    }
    pub fn encode(&mut self, message: &Message) -> Result<EncodedFrame, WireError> {
        if self.poisoned {
            return Err(WireError::Poisoned);
        }
        let result = self.encode_inner(message);
        self.finish(result)
    }
    fn encode_inner(&self, message: &Message) -> Result<EncodedFrame, WireError> {
        self.role_message(message)?;
        let (lane, direction, kind) = message.route();
        if lane != self.lane || direction != self.direction {
            return Err(WireError::Lane);
        }
        message.validate(self.binding)?;
        let cap = Profile::approved_design().limits.frame_bytes as usize;
        let _codec = self
            .budget
            .reserve(cap * 16)
            .map_err(|_| WireError::Capacity)?;
        let mut allocation = self
            .budget
            .reserve(cap + std::mem::size_of::<EncodedFrame>())
            .map_err(|_| WireError::Capacity)?;
        let value = object([
            ("version", Value::Text("1".into())),
            ("role", Value::Text(self.role().into())),
            ("lane", Value::Text(lane_name(lane).into())),
            ("direction", Value::Text(direction.name().into())),
            ("sequence", Value::U64(self.next)),
            ("binding", self.binding.value()),
            ("kind", Value::Text(kind.into())),
            ("body", message.body()),
        ]);
        let encoded = value.to_canonical();
        crate::sampler::wire::validate_complexity_limit(encoded.as_bytes(), 4096)?;
        let len = encoded.len().checked_add(4).ok_or(WireError::Length)?;
        if len > cap {
            return Err(WireError::Length);
        }
        let mut bytes = Vec::with_capacity(len);
        bytes.extend_from_slice(&(len as u32).to_le_bytes());
        bytes.extend_from_slice(encoded.as_bytes());
        allocation
            .shrink_to(bytes.capacity() + std::mem::size_of::<EncodedFrame>())
            .map_err(|_| WireError::Capacity)?;
        Ok(EncodedFrame {
            bytes,
            binding: self.binding.transport_digest(),
            lane: self.lane,
            direction: self.direction,
            _allocation: allocation,
        })
    }
    pub fn decode(&mut self, frame: &[u8]) -> Result<OwnedMessage, WireError> {
        if self.poisoned {
            return Err(WireError::Poisoned);
        }
        let result = self.decode_inner(frame);
        self.finish(result)
    }
    fn decode_inner(&self, frame: &[u8]) -> Result<OwnedMessage, WireError> {
        let prefix = frame
            .get(..4)
            .ok_or(WireError::Length)?
            .try_into()
            .map_err(|_| WireError::Length)?;
        if crate::sampler::wire::Channel::checked_length(prefix)? != frame.len() {
            return Err(WireError::Length);
        }
        crate::sampler::wire::validate_complexity_limit(&frame[4..], 4096)?;
        let cap = Profile::approved_design().limits.frame_bytes as usize;
        let mut allocation = self
            .budget
            .reserve(cap * 16)
            .map_err(|_| WireError::Capacity)?;
        let value = decode(&frame[4..], cap - 4).map_err(|_| WireError::Canonical)?;
        let o = exact(
            &value,
            &[
                "version",
                "role",
                "lane",
                "direction",
                "sequence",
                "binding",
                "kind",
                "body",
            ],
        )?;
        if string(get(o, "version"))? != "1"
            || string(get(o, "role"))? != self.role()
            || get(o, "binding").to_canonical() != self.binding.value().to_canonical()
        {
            return Err(WireError::Binding);
        }
        if string(get(o, "lane"))? != lane_name(self.lane)
            || string(get(o, "direction"))? != self.direction.name()
        {
            return Err(WireError::Lane);
        }
        if uint(get(o, "sequence"))? != self.next {
            return Err(WireError::Sequence);
        }
        let message = Message::parse(string(get(o, "kind"))?, get(o, "body"), self.binding)?;
        self.role_message(&message)?;
        let (lane, direction, _) = message.route();
        if lane != self.lane || direction != self.direction {
            return Err(WireError::Lane);
        }
        drop(value);
        allocation
            .shrink_to(std::mem::size_of::<OwnedMessage>() + message.payload_bytes())
            .map_err(|_| WireError::Capacity)?;
        Ok(OwnedMessage {
            message,
            binding: self.binding,
            _allocation: allocation,
        })
    }
}
fn lane_name(l: Lane) -> &'static str {
    match l {
        Lane::Ordinary => "ordinary",
        Lane::Control => "control",
    }
}
fn identity_value(i: Identity) -> Value {
    object([
        ("uuid", Value::Text(hex(&i.uuid))),
        ("domain", Value::U64(i.domain.into())),
        ("bus", Value::U64(i.bus.into())),
        ("device", Value::U64(i.device.into())),
    ])
}
fn parse_identity(v: &Value) -> Result<Identity, WireError> {
    let o = exact(v, &["uuid", "domain", "bus", "device"])?;
    let i = Identity {
        uuid: unhex(get(o, "uuid"))?,
        domain: uint(get(o, "domain"))?
            .try_into()
            .map_err(|_| WireError::Body)?,
        bus: uint(get(o, "bus"))?
            .try_into()
            .map_err(|_| WireError::Body)?,
        device: uint(get(o, "device"))?
            .try_into()
            .map_err(|_| WireError::Body)?,
    };
    if !i.valid() {
        return Err(WireError::Binding);
    }
    Ok(i)
}
pub(super) fn parse_anchor(v: &Value, b: WorkerBinding) -> Result<Anchor, WireError> {
    let o = exact(
        v,
        &[
            "qpc_before",
            "qpc_after",
            "interrupt_100ns",
            "unbiased_100ns",
        ],
    )?;
    Ok(Anchor {
        clock_token: b.monitor.clock_token,
        qpc_before: uint(get(o, "qpc_before"))?,
        qpc_after: uint(get(o, "qpc_after"))?,
        interrupt_100ns: uint(get(o, "interrupt_100ns"))?,
        unbiased_100ns: uint(get(o, "unbiased_100ns"))?,
    })
}
fn boolean(v: &Value) -> Result<bool, WireError> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err(WireError::Body),
    }
}
