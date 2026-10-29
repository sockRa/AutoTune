//! Sampler-only derived envelope/raw export. Injected values never authenticate
//! provenance; only the separately authorized OS controller can supply live data.
use super::{
    budget::{Budget, Reservation},
    manifest::CaptureManifest,
    supervisor::{Fault, Input, Supervisor},
    wire::WireError,
    wire_model::{anchor_value, metric_name, object},
};
use crate::{
    evidence::*,
    monitor::{clock::Clock, controller::Action, Anchor},
};
use std::ops::Deref;

pub struct BoundEnvelope {
    envelope: Envelope,
    _allocation: Reservation,
    budget: Budget,
    estimate: usize,
    artifacts_accounted: bool,
}
impl Deref for BoundEnvelope {
    type Target = Envelope;
    fn deref(&self) -> &Envelope {
        &self.envelope
    }
}
pub struct BoundText {
    pub(super) text: String,
    pub(super) _allocation: Reservation,
    session_digest: Option<Digest>,
}
impl BoundText {
    pub fn copy(text: &str, budget: &Budget) -> Result<Self, WireError> {
        let allocation = budget
            .reserve(text.len())
            .map_err(|_| WireError::Capacity)?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(text.len())
            .map_err(|_| WireError::Capacity)?;
        owned.push_str(text);
        Ok(Self {
            text: owned,
            _allocation: allocation,
            session_digest: None,
        })
    }
    pub fn recharge(&mut self, budget: &Budget) -> Result<(), WireError> {
        if !self._allocation.belongs_to(budget) {
            self._allocation = budget
                .reserve(self.text.capacity())
                .map_err(|_| WireError::Capacity)?;
        }
        Ok(())
    }
    pub fn session_digest(&self) -> Option<Digest> {
        self.session_digest
    }
}
impl Deref for BoundText {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}
impl BoundEnvelope {
    /// Exact serialized bytes of manifest.json, profile.json and
    /// raw-events.ndjson. Session/index are excluded to avoid self-reference;
    /// capture-index.json separately accounts for the complete artifact set.
    pub fn account_artifacts(
        &mut self,
        manifest: &str,
        profile: &str,
        raw: &BoundText,
    ) -> Result<(), WireError> {
        self.artifacts_accounted = false;
        let observed_bytes = manifest
            .len()
            .checked_add(profile.len())
            .and_then(|n| n.checked_add(raw.len()))
            .ok_or(WireError::Capacity)? as u64;
        if observed_bytes > self.envelope.integrity.capacity_bytes {
            return Err(WireError::Capacity);
        }
        self.envelope.integrity.observed_bytes = observed_bytes;
        self.artifacts_accounted = true;
        Ok(())
    }
    pub fn encode(&self) -> Result<BoundText, WireError> {
        // A model can be inspected before artifacts are available, but the
        // export API must never publish its initial placeholder byte count.
        if !self.artifacts_accounted {
            return Err(WireError::Body);
        }
        let mut allocation = self
            .budget
            .reserve(self.estimate.checked_mul(2).ok_or(WireError::Capacity)?)
            .map_err(|_| WireError::Capacity)?;
        let text = encode_envelope(&self.envelope).map_err(|_| WireError::Body)?;
        let value = decode(text.as_bytes(), text.len()).map_err(|_| WireError::Body)?;
        let digest = value
            .as_object()
            .and_then(|o| o.get("digest"))
            .and_then(Value::as_text)
            .ok_or(WireError::Body)?;
        let session_digest = Digest::parse(digest).map_err(|_| WireError::Body)?;
        drop(value);
        allocation
            .shrink_to(text.capacity())
            .map_err(|_| WireError::Capacity)?;
        Ok(BoundText {
            text,
            _allocation: allocation,
            session_digest: Some(session_digest),
        })
    }
}
fn missing<T>(reason: &str) -> Obs<T> {
    Obs::Unavailable(Reason::new(reason).expect("fixed reason"))
}
fn time(s: &Supervisor, a: Anchor) -> Option<u64> {
    Clock::new(s.binding().monitor, s.origin())
        .ok()?
        .observe(a)
        .ok()
}
fn observed(s: &Supervisor, a: Anchor) -> Obs<u64> {
    time(s, a).map_or_else(|| missing("invalid_clock"), Obs::Known)
}
pub fn build_envelope(
    manifest: &CaptureManifest,
    s: &Supervisor,
    provenance: Provenance,
) -> Result<BoundEnvelope, WireError> {
    if manifest.binding(s.binding().monitor.qpc_frequency)? != s.binding() {
        return Err(WireError::Binding);
    }
    let estimate = s
        .samples()
        .count()
        .checked_add(1)
        .and_then(|n| n.checked_mul(4096))
        .and_then(|n| n.checked_add(16384))
        .ok_or(WireError::Capacity)?;
    let budget = s.failure_budget();
    let allocation = budget.reserve(estimate).map_err(|_| WireError::Capacity)?;
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let terminal = s.terminal();
    let stop_ns = terminal.map(|t| t.at_ns);
    let cutoff = stop_ns.unwrap_or(s.report().now_ns());
    let launch = s
        .launch_ns()
        .map_or_else(|| missing("not_observed"), Obs::Known);
    let ready = s
        .ready_ns()
        .map_or_else(|| missing("not_observed"), Obs::Known);
    let start = s
        .report()
        .events()
        .find_map(|event| {
            matches!(event.action, Action::Begin(Some(_)))
                .then_some(event.time.ok())
                .flatten()
        })
        .map_or_else(|| missing("not_observed"), Obs::Known);
    let mut telemetry = Vec::new();
    let mut raw_records = 0u64;
    let mut late = 0u64;
    let mut duplicate = 0u64;
    let mut invalid_timing = false;
    for sample in s.samples() {
        raw_records += 1;
        if time(s, sample.receipt).is_none_or(|ns| ns > cutoff) {
            late += 1;
        }
        let name = metric_name(sample.ticket.metric);
        if telemetry
            .iter()
            .any(|t: &Telemetry| t.metric == name && t.slot == sample.ticket.slot)
        {
            duplicate += 1;
            continue;
        }
        let timing = (
            time(s, sample.start),
            time(s, sample.end),
            time(s, sample.receipt),
        );
        let ordered = matches!(timing, (Some(a), Some(b), Some(c)) if sample.ticket.start_ns <= a && a <= b && b <= c)
            && s.issue_anchor(sample.ticket)
                .is_some_and(|a| a.qpc_after <= sample.start.qpc_before)
            && sample.start.qpc_after <= sample.end.qpc_before
            && sample.end.qpc_after <= sample.receipt.qpc_before;
        invalid_timing |= !ordered;
        let (mut value, vendor_code) = match &sample.raw.reading {
            autotune_probe::report::Reading::Available { value } => (
                value
                    .as_i64()
                    .map(TelemetryValue::Int)
                    .map_or_else(|| missing("invalid_value"), Obs::Known),
                missing("no_vendor_code"),
            ),
            autotune_probe::report::Reading::Unavailable { failure } => (
                missing("raw_unavailable"),
                failure
                    .vendor_status
                    .map(|n| Obs::Known(n.to_string()))
                    .unwrap_or_else(|| missing("no_vendor_code")),
            ),
        };
        if !ordered {
            value = missing("invalid_timing");
        }
        if super::convert(
            sample.ticket,
            time(s, sample.end).unwrap_or(0),
            time(s, sample.receipt).unwrap_or(0),
            sample.raw,
        )
        .is_err()
        {
            value = missing("invalid_raw");
        }
        telemetry.push(Telemetry {
            metric: name.into(),
            unit: match sample.ticket.metric.unit() {
                crate::monitor::Unit::DegC => "degC",
                crate::monitor::Unit::Milliwatt => "mW",
                crate::monitor::Unit::MHz => "MHz",
                crate::monitor::Unit::Percent => "%",
            }
            .into(),
            device: manifest.draft().raw_uuid.clone(),
            value,
            slot: sample.ticket.slot,
            query_start_ns: if ordered {
                observed(s, sample.start)
            } else {
                missing("invalid_timing")
            },
            query_end_ns: if ordered {
                observed(s, sample.end)
            } else {
                missing("invalid_timing")
            },
            receipt_ns: if ordered {
                observed(s, sample.receipt)
            } else {
                missing("invalid_timing")
            },
            internal_age: InternalAge::Unknown,
            vendor_code,
        });
    }
    let exit = s.exit_observation().map(|(a, o)| (Some(a), o.exit_code));
    let invalid = terminal
        .is_some_and(|t| !matches!(t.reason, Fault::Requested | Fault::Preflight))
        || duplicate > 0
        || invalid_timing
        || s.retention_overflow();
    let rejected_frames = s
        .traces()
        .iter()
        .filter(|t| matches!(t.input, Input::Rejected { .. }))
        .count() as u64;
    let origin = s.origin();
    let envelope = Envelope {
        provenance,
        identity: Identity {
            nonce: super::wire_model::hex(&s.binding().monitor.run),
            nvml_uuid: Obs::Known(manifest.draft().raw_uuid.clone()),
            cuda_uuid: missing("workload_disabled"),
            pci_bus_id: Obs::Known(manifest.draft().raw_pci.clone()),
            context_device: missing("workload_disabled"),
            boot: missing("not_observed"),
            uuid_match: MatchState::Unknown,
            pci_match: MatchState::Unknown,
        },
        artifacts: Artifacts {
            source_commit: manifest.draft().source_commit.clone(),
            worker_sha256: missing("workload_disabled"),
            lockfile_sha256: Obs::Known(
                Digest::parse(&super::wire_model::hex(&manifest.draft().lockfile))
                    .map_err(|_| WireError::Body)?,
            ),
            sampler_sha256: Obs::Known(
                Digest::parse(&super::wire_model::hex(&manifest.draft().sampler))
                    .map_err(|_| WireError::Body)?,
            ),
            ptx_sha256: missing("workload_disabled"),
            kernel_entry: "not_applicable".into(),
            generator_version: "not_applicable".into(),
            oracle_version: "not_applicable".into(),
        },
        manifest: Manifest {
            manifest_digest: Obs::Known(manifest.digest()),
            profile_digest: Obs::Known(manifest.profile_digest()),
            shape: missing("workload_disabled"),
            stop_policy: Obs::Known("e3-fma-qualification-v0.1".into()),
            approval_ref: Obs::Known(manifest.draft().approval_ref.clone()),
            seed: missing("workload_disabled"),
            control_writes: ControlWrites::None,
        },
        clocks: Clocks {
            qpc_frequency: s.binding().monitor.qpc_frequency,
            qpc_epoch_ticks: origin.qpc_before,
            interrupt_ticks: origin.interrupt_100ns,
            unbiased_ticks: Obs::Known(origin.unbiased_100ns),
            resolution_ns: crate::monitor::clock::ticks_to_ns(
                1,
                s.binding().monitor.qpc_frequency,
                true,
            )
            .map_err(|_| WireError::Body)?
            .max(100),
            utc_context: missing("not_observed"),
            discontinuities: u64::from(!s.report().current_clock_valid()),
        },
        lifecycle: Lifecycle {
            requested_ns: Obs::Known(0),
            launch_ns: launch,
            ready_ns: ready,
            start_ns: start,
            stop_requested_ns: stop_ns.map_or_else(|| missing("not_requested"), Obs::Known),
            exit_ns: exit
                .and_then(|(a, _)| a)
                .map_or_else(|| missing("not_observed"), |a| observed(s, a)),
            empty_job: if s.exit_confirmed() {
                EmptyJobLifecycle::ObservedEmpty
            } else {
                EmptyJobLifecycle::Unknown
            },
        },
        dispatches: Vec::new(),
        integrity: Integrity {
            capacity_bytes: p.serialized_bytes,
            observed_bytes: 0,
            failed: u64::from(invalid),
            late,
            rejected: duplicate + rejected_frames,
            overflow: u64::from(
                s.retention_overflow() || terminal.is_some_and(|t| t.reason == Fault::Capacity),
            ),
            expected_slots: (cutoff / p.cadence_ns + 1)
                .checked_mul(u64::from(p.metric_count))
                .ok_or(WireError::Capacity)?,
            observed_slots: telemetry.len() as u64,
            raw_records,
            writer_status: WriterStatus::Bounded,
        },
        telemetry,
        result: SessionResult {
            admission: Admission::Blocked,
            correctness: Correctness::NotChecked,
            worker_exit: missing("workload_disabled"),
            sampler_exit: exit
                .and_then(|(_, code)| code)
                .map_or_else(|| missing("not_observed"), |n| Obs::Known(i64::from(n))),
            empty_job: if s.exit_confirmed() {
                ResultEmptyJob::Observed
            } else {
                ResultEmptyJob::Unknown
            },
            native_completion: NativeCompletion::Unknown,
            coverage: Coverage::Incomplete,
            limitations: [
                "sampler_only",
                "workload_disabled",
                "sensor_age_unknown",
                "native_completion_unknown",
            ]
            .into_iter()
            .map(|s| Reason::new(s).expect("fixed reason"))
            .collect(),
            verdict: if invalid {
                Verdict::Invalid
            } else {
                Verdict::Blocked
            },
        },
    };
    Ok(BoundEnvelope {
        envelope,
        _allocation: allocation,
        budget,
        estimate,
        artifacts_accounted: false,
    })
}

/// Bounded offline collection of the raw trace. The fixed controller uses this
/// only after admission has closed; storage runs on its separate writer lane.
pub fn raw_events(s: &Supervisor) -> Result<BoundText, WireError> {
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let _temporary = s
        .failure_budget()
        .reserve(p.frame_bytes as usize * 16)
        .map_err(|_| WireError::Capacity)?;
    let mut output = RawBuilder::new(s.writer_budget())?;
    for (i, trace) in s.traces().iter().enumerate() {
        let (kind, body) = match &trace.input {
            Input::Message(message) => (message.kind(), message.body(s.binding())?),
            Input::Tick => ("tick", Value::Null),
            Input::LaunchFailure {
                stage,
                error,
                assigned,
                creation,
                process,
            } => (
                "launch_failure",
                object([
                    ("stage", Value::Text((*stage).into())),
                    ("error", Value::U64((*error).into())),
                    ("assigned", Value::Bool(*assigned)),
                    ("creation", creation.map_or(Value::Null, Value::U64)),
                    ("process", process_value(*process)),
                ]),
            ),
            Input::Stop => ("stop", Value::Null),
            Input::ObservationExpired => ("observation_expired", Value::Null),
            Input::Fault(f) => ("fault", Value::Text(format!("{f:?}"))),
            Input::MetadataOverflow { attempted_bytes } => (
                "metadata_overflow",
                object([("attempted_bytes", Value::U64(*attempted_bytes))]),
            ),
            Input::IoShutdown(o) => (
                "io_shutdown",
                object([
                    ("lane", Value::Text(format!("{:?}", o.lane))),
                    ("reader", Value::Bool(o.reader)),
                    ("finished", Value::Bool(o.finished)),
                    (
                        "cancellation",
                        match o.cancellation {
                            Ok(()) => Value::Text("requested".into()),
                            Err(code) => Value::U64(code.into()),
                        },
                    ),
                ]),
            ),
            Input::WriterFailure(failure) => (
                "writer_failure",
                object([
                    ("operation", Value::Text(failure.operation.into())),
                    (
                        "os_code",
                        failure
                            .os_code
                            .map_or(Value::Null, |n| Value::I64(n.into())),
                    ),
                    ("kind", Value::Text(format!("{:?}", failure.kind))),
                ]),
            ),
            Input::WriterShutdown {
                cancellation,
                finished,
            } => (
                "writer_shutdown",
                object([
                    (
                        "cancellation",
                        match cancellation {
                            Ok(()) => Value::Text("requested".into()),
                            Err(code) => Value::U64((*code).into()),
                        },
                    ),
                    ("finished", Value::Bool(*finished)),
                ]),
            ),
            Input::Rejected {
                lane,
                bytes,
                rejection,
            } => (
                "rejected_frame",
                object([
                    ("lane", Value::Text(format!("{lane:?}"))),
                    ("phase", Value::Text(rejection.phase.name().into())),
                    ("fault", Value::Text(format!("{:?}", rejection.fault))),
                    (
                        "os_code",
                        match rejection.fault {
                            super::framing::ReadFault::Os(code) => Value::U64(code.into()),
                            _ => Value::Null,
                        },
                    ),
                    ("reported", Value::U64(rejection.reported)),
                    ("queue_overflow", Value::Bool(rejection.queue_overflow)),
                    ("received_length", Value::U64(bytes.len() as u64)),
                    ("bytes_hex", Value::Text(super::wire_model::hex(bytes))),
                ]),
            ),
            Input::Operation { operation, result } => (
                "operation",
                object([
                    ("operation", Value::Text(operation.name().into())),
                    (
                        "result",
                        match result {
                            Ok(()) => Value::Text("returned".into()),
                            Err(code) => Value::U64(u64::from(*code)),
                        },
                    ),
                ]),
            ),
            Input::Host {
                watchdog_100ns,
                resources,
                sampler_resources,
                process,
            } => (
                "host_observation",
                object([
                    ("watchdog_100ns", Value::U64(*watchdog_100ns)),
                    (
                        "sampler_resources",
                        sampler_resources.map_or(Value::Null, resource_value),
                    ),
                    (
                        "resources",
                        object([
                            (
                                "commit_bytes",
                                resources.commit_bytes.map_or(Value::Null, Value::U64),
                            ),
                            (
                                "user_100ns",
                                resources.user_100ns.map_or(Value::Null, Value::U64),
                            ),
                            (
                                "kernel_100ns",
                                resources.kernel_100ns.map_or(Value::Null, Value::U64),
                            ),
                            (
                                "memory_error",
                                resources
                                    .memory_error
                                    .map_or(Value::Null, |n| Value::U64(n.into())),
                            ),
                            (
                                "time_error",
                                resources
                                    .time_error
                                    .map_or(Value::Null, |n| Value::U64(n.into())),
                            ),
                        ]),
                    ),
                    ("process", process.map_or(Value::Null, process_value)),
                ]),
            ),
            Input::Launched { creation } => {
                ("launched", object([("creation", Value::U64(*creation))]))
            }
            Input::Process(o) => (
                "process_observation",
                object([
                    ("creation", Value::U64(o.creation)),
                    ("wait_status", Value::U64(o.wait_status.into())),
                    (
                        "wait_error",
                        o.wait_error.map_or(Value::Null, |n| Value::U64(n.into())),
                    ),
                    (
                        "exit_code",
                        o.exit_code.map_or(Value::Null, |n| Value::U64(n.into())),
                    ),
                    (
                        "exit_error",
                        o.exit_error.map_or(Value::Null, |n| Value::U64(n.into())),
                    ),
                    (
                        "termination",
                        match o.termination {
                            None => Value::Null,
                            Some(Ok(())) => Value::Text("requested".into()),
                            Some(Err(n)) => Value::U64(n.into()),
                        },
                    ),
                    (
                        "active_processes",
                        o.active_processes
                            .map_or(Value::Null, |n| Value::U64(n.into())),
                    ),
                    (
                        "accounting_error",
                        o.accounting_error
                            .map_or(Value::Null, |n| Value::U64(n.into())),
                    ),
                ]),
            ),
        };
        let line = object([
            ("stream", Value::Text("supervisor".into())),
            ("sequence", Value::U64(i as u64)),
            ("receipt", trace.receipt.map_or(Value::Null, anchor_value)),
            ("kind", Value::Text(kind.into())),
            ("body", body),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    // Independent monitor order is explicit: no fabricated total physical order
    // between same-anchor controller actions and child/control-thread events.
    for (i, event) in s.report().events().enumerate() {
        let action = match event.action {
            Action::Initial => "initial",
            Action::Poll => "poll",
            Action::Begin(_) => "begin",
            Action::Complete(_) => "complete",
            Action::RequestStop => "request_stop",
        };
        let pending = match event.action {
            Action::Begin(Some(ticket)) => super::wire::Message::Query(ticket).body(s.binding())?,
            Action::Complete(record) => object([
                (
                    "ticket",
                    super::wire::Message::Query(record.ticket).body(s.binding())?,
                ),
                ("end_ns", Value::U64(record.end_ns)),
                ("receipt_ns", Value::U64(record.receipt_ns)),
                ("value", record.value.map_or(Value::Null, Value::I64)),
                ("status", Value::Text(format!("{:?}", record.status))),
            ]),
            _ => Value::Null,
        };
        let missed = event
            .missed
            .into_iter()
            .flatten()
            .map(|m| {
                object([
                    ("metric", Value::Text(metric_name(m.metric).into())),
                    ("first", Value::U64(m.first)),
                    ("last", Value::U64(m.last)),
                ])
            })
            .collect();
        let line = object([
            ("stream", Value::Text("monitor".into())),
            ("sequence", Value::U64(i as u64)),
            ("anchor", anchor_value(event.anchor)),
            (
                "elapsed_ns",
                event
                    .time
                    .map_or_else(|e| Value::Text(format!("{e:?}")), Value::U64),
            ),
            ("kind", Value::Text(action.into())),
            ("body", pending),
            ("missed", Value::Array(missed)),
            (
                "record_error",
                event
                    .record_error
                    .map_or(Value::Null, |e| Value::Text(format!("{e:?}"))),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    Ok(output.finish())
}
struct RawBuilder {
    text: String,
    storage: Reservation,
    budget: Budget,
}
impl RawBuilder {
    fn new(budget: Budget) -> Result<Self, WireError> {
        let storage = budget.reserve(0).map_err(|_| WireError::Capacity)?;
        Ok(Self {
            text: String::new(),
            storage,
            budget,
        })
    }
    fn push_line(&mut self, line: &str) -> Result<(), WireError> {
        let needed = self
            .text
            .len()
            .checked_add(line.len())
            .and_then(|n| n.checked_add(1))
            .ok_or(WireError::Capacity)?;
        if needed > self.text.capacity() {
            let next = needed.max(
                self.text
                    .capacity()
                    .checked_mul(2)
                    .ok_or(WireError::Capacity)?,
            );
            // Both old and new allocations are charged during reallocation.
            // Only after reserve/allocate succeeds is the old charge released.
            let storage = self.budget.reserve(next).map_err(|_| WireError::Capacity)?;
            self.text
                .try_reserve_exact(next - self.text.len())
                .map_err(|_| WireError::Capacity)?;
            self.storage = storage;
        }
        self.text.push_str(line);
        self.text.push('\n');
        Ok(())
    }
    fn finish(self) -> BoundText {
        BoundText {
            text: self.text,
            _allocation: self.storage,
            session_digest: None,
        }
    }
}
fn process_value(o: super::supervisor::ProcessObservation) -> Value {
    object([
        ("creation", Value::U64(o.creation)),
        ("wait_status", Value::U64(o.wait_status.into())),
        (
            "wait_error",
            o.wait_error.map_or(Value::Null, |n| Value::U64(n.into())),
        ),
        (
            "exit_code",
            o.exit_code.map_or(Value::Null, |n| Value::U64(n.into())),
        ),
        (
            "exit_error",
            o.exit_error.map_or(Value::Null, |n| Value::U64(n.into())),
        ),
        (
            "termination",
            match o.termination {
                None => Value::Null,
                Some(Ok(())) => Value::Text("requested".into()),
                Some(Err(n)) => Value::U64(n.into()),
            },
        ),
        (
            "active_processes",
            o.active_processes
                .map_or(Value::Null, |n| Value::U64(n.into())),
        ),
        (
            "accounting_error",
            o.accounting_error
                .map_or(Value::Null, |n| Value::U64(n.into())),
        ),
    ])
}
fn resource_value(o: super::runtime::ResourceObservation) -> Value {
    object([
        (
            "commit_bytes",
            o.commit_bytes.map_or(Value::Null, Value::U64),
        ),
        ("user_100ns", o.user_100ns.map_or(Value::Null, Value::U64)),
        (
            "kernel_100ns",
            o.kernel_100ns.map_or(Value::Null, Value::U64),
        ),
        (
            "memory_error",
            o.memory_error.map_or(Value::Null, |n| Value::U64(n.into())),
        ),
        (
            "time_error",
            o.time_error.map_or(Value::Null, |n| Value::U64(n.into())),
        ),
    ])
}
