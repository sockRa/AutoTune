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

#[cfg(windows)]
pub struct ParentTelemetryProjection {
    pub telemetry: Vec<Telemetry>,
    pub expected_slots: u64,
    pub observed_slots: u64,
    pub raw_records: u64,
    pub late: u64,
    pub rejected: u64,
    pub duplicates: u64,
    pub invalid_timing: bool,
    _allocation: Reservation,
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

/// Build a workload-session v1 projection from one frozen pilot run plus its
/// sampler and worker owners. Any fact that cannot be represented faithfully
/// remains in raw NDJSON and makes this summary explicitly incomplete.
#[derive(Clone, Copy)]
#[cfg(windows)]
enum PilotSamplerInput<'a> {
    Supervisor(&'a Supervisor),
    Parent(&'a crate::workload::parent_sampler::ParentSampler),
}

#[cfg(windows)]
struct PilotSamplerProjection {
    nvml_uuid: Option<String>,
    nvml_pci: Option<String>,
    identity_conflict: bool,
    sample_count: usize,
    event_count: usize,
    audit_failed: bool,
    retention_overflow: bool,
    current_clock_valid: bool,
    origin: Anchor,
    sampler_exit: Obs<i64>,
    expected_slots: u64,
    observed_slots: u64,
    late: u64,
    rejected: u64,
    telemetry_incomplete: bool,
    telemetry: Vec<Telemetry>,
    _telemetry_allocation: Option<Reservation>,
}

#[cfg(windows)]
fn project_pilot_sampler(
    source: PilotSamplerInput<'_>,
    manifest: &crate::workload::manifest::PilotManifest,
    budget: &Budget,
    limits: autotune_fma_reference::profile::Limits,
) -> Result<PilotSamplerProjection, WireError> {
    use crate::workload::wire::Message;

    match source {
        PilotSamplerInput::Supervisor(s) => {
            let mut identity = None;
            let mut identity_conflict = false;
            for trace in s.traces() {
                if let Input::Message(super::wire::Message::Ready {
                    raw_uuid, raw_pci, ..
                }) = &trace.input
                {
                    let observed = (raw_uuid.clone(), raw_pci.clone());
                    if identity.as_ref().is_some_and(|prior| prior != &observed) {
                        identity_conflict = true;
                    } else {
                        identity.get_or_insert(observed);
                    }
                }
            }
            let terminal = s.terminal();
            let cutoff_ns = terminal.map_or_else(|| s.report().now_ns(), |t| t.at_ns);
            let expected_slots = (cutoff_ns / limits.cadence_ns + 1)
                .checked_mul(u64::from(limits.metric_count))
                .ok_or(WireError::Capacity)?;
            let event_count = s
                .traces()
                .len()
                .checked_add(s.report().events().count())
                .ok_or(WireError::Capacity)?;
            let late = s
                .samples()
                .filter(|sample| time(s, sample.receipt).is_none_or(|receipt| receipt > cutoff_ns))
                .count() as u64;
            let rejected = s
                .traces()
                .iter()
                .filter(|trace| matches!(trace.input, Input::Rejected { .. }))
                .count() as u64;
            let sampler_exit = s
                .exit_observation()
                .and_then(|(_, observation)| observation.exit_code)
                .map_or_else(
                    || missing("sampler_exit_unobserved"),
                    |code| Obs::Known(i64::from(code)),
                );
            Ok(PilotSamplerProjection {
                nvml_uuid: identity
                    .as_ref()
                    .and_then(|pair| (!identity_conflict).then(|| pair.0.clone())),
                nvml_pci: identity
                    .as_ref()
                    .and_then(|pair| (!identity_conflict).then(|| pair.1.clone())),
                identity_conflict,
                sample_count: s.samples().count(),
                event_count,
                audit_failed: terminal
                    .is_some_and(|t| !matches!(t.reason, Fault::Requested | Fault::Preflight)),
                retention_overflow: s.retention_overflow(),
                current_clock_valid: s.report().current_clock_valid(),
                origin: s.origin(),
                sampler_exit,
                expected_slots,
                observed_slots: 0,
                late,
                rejected,
                telemetry_incomplete: expected_slots > 0,
                telemetry: Vec::new(),
                _telemetry_allocation: None,
            })
        }
        PilotSamplerInput::Parent(parent) => {
            let binding = parent.binding();
            let ready_identity = parent.ready().and_then(|ready| match ready.message() {
                Ok(Message::SamplerReady {
                    raw_uuid, raw_pci, ..
                }) => Some((raw_uuid.clone(), raw_pci.clone())),
                _ => None,
            });
            let identity_conflict = parent.rejected().is_some_and(|rejected| {
                matches!(rejected.message(), Ok(Message::SamplerReady { .. }))
                    && rejected
                        .message()
                        .ok()
                        .is_some_and(|message| match message {
                            Message::SamplerReady {
                                raw_uuid, raw_pci, ..
                            } => ready_identity.as_ref().is_some_and(|identity| {
                                identity != &(raw_uuid.clone(), raw_pci.clone())
                            }),
                            _ => false,
                        })
            });
            let report = parent.report();
            let stop = report.stop();
            let event_count = parent
                .records()
                .len()
                .checked_add(parent.controls().len())
                .and_then(|n| n.checked_add(usize::from(parent.current().is_some())))
                .and_then(|n| n.checked_add(usize::from(parent.ready().is_some())))
                .and_then(|n| n.checked_add(usize::from(parent.rejected().is_some())))
                .and_then(|n| n.checked_add(parent.history().raw().len()))
                .and_then(|n| n.checked_add(report.events().count()))
                .and_then(|n| n.checked_add(1))
                .ok_or(WireError::Capacity)?;
            let projected =
                project_parent_telemetry(parent, &manifest.manifest_draft().raw_uuid, budget)?;
            let ParentTelemetryProjection {
                telemetry,
                expected_slots,
                observed_slots,
                raw_records: _,
                late,
                rejected,
                duplicates,
                invalid_timing,
                _allocation,
            } = projected;
            let telemetry_allocation = _allocation;
            let history_incomplete = parent.history().overflow().is_some()
                || parent.history().missing()
                || parent.history().clock_error().is_some();
            let current_ok = parent
                .current()
                .is_some_and(|current| current.result().is_ok());
            let sampler_exit = missing("sampler_exit_unobserved");
            Ok(PilotSamplerProjection {
                nvml_uuid: if identity_conflict {
                    None
                } else {
                    ready_identity.as_ref().map(|identity| identity.0.clone())
                },
                nvml_pci: if identity_conflict {
                    None
                } else {
                    ready_identity.as_ref().map(|identity| identity.1.clone())
                },
                identity_conflict,
                sample_count: parent.records().len(),
                event_count,
                audit_failed: parent.fault().is_some()
                    || stop
                        .is_some_and(|stop| stop.reason != crate::monitor::StopReason::Requested)
                    || !parent.reported_finished()
                    || !current_ok
                    || parent.ready().is_none()
                    || history_incomplete
                    || duplicates > 0
                    || invalid_timing
                    || rejected > 0,
                retention_overflow: parent.history().overflow().is_some(),
                current_clock_valid: report.current_clock_valid(),
                origin: binding.epoch(),
                sampler_exit,
                expected_slots,
                observed_slots,
                late,
                rejected,
                telemetry_incomplete: observed_slots != expected_slots
                    || duplicates > 0
                    || invalid_timing
                    || rejected > 0,
                telemetry,
                _telemetry_allocation: Some(telemetry_allocation),
            })
        }
    }
}

#[cfg(windows)]
pub fn build_pilot_envelope(
    manifest: &crate::workload::manifest::PilotManifest,
    s: &Supervisor,
    setup: &crate::workload::worker_setup_delivery::ReadyDelivery,
    worker: &crate::workload::parent_worker_channel::WorkerReportEvidence,
    provenance: Provenance,
) -> Result<BoundEnvelope, WireError> {
    let budget = s.failure_budget();
    build_pilot_envelope_from_source(
        manifest,
        PilotSamplerInput::Supervisor(s),
        setup,
        worker,
        provenance,
        budget,
    )
}

#[cfg(windows)]
pub fn build_pilot_envelope_from_parent(
    manifest: &crate::workload::manifest::PilotManifest,
    parent: &crate::workload::parent_sampler::ParentSampler,
    setup: &crate::workload::worker_setup_delivery::ReadyDelivery,
    worker: &crate::workload::parent_worker_channel::WorkerReportEvidence,
    provenance: Provenance,
    budget: &Budget,
) -> Result<BoundEnvelope, WireError> {
    build_pilot_envelope_from_source(
        manifest,
        PilotSamplerInput::Parent(parent),
        setup,
        worker,
        provenance,
        budget.clone(),
    )
}

#[cfg(windows)]
fn build_pilot_envelope_from_source(
    manifest: &crate::workload::manifest::PilotManifest,
    source: PilotSamplerInput<'_>,
    setup: &crate::workload::worker_setup_delivery::ReadyDelivery,
    worker: &crate::workload::parent_worker_channel::WorkerReportEvidence,
    provenance: Provenance,
    budget: Budget,
) -> Result<BoundEnvelope, WireError> {
    let binding = worker.binding();
    let monitor = binding.monitor();
    let source_binding = match source {
        PilotSamplerInput::Supervisor(s) => {
            s.binding().monitor == monitor
                && s.binding().manifest_digest == *manifest.digest().as_bytes()
        }
        PilotSamplerInput::Parent(parent) => {
            parent.binding().monitor() == monitor
                && parent.binding().manifest_digest() == manifest.digest()
        }
    };
    if binding.manifest_digest() != manifest.digest()
        || binding.profile_digest() != manifest.profile_digest()
        || binding.worker_digest() != manifest.manifest_draft().artifacts.worker
        || binding.sampler_digest() != manifest.manifest_draft().artifacts.sampler
        || setup.receipt().binding() != binding
        || setup.origin().ok() != Some(worker.origin())
        || !source_binding
    {
        return Err(WireError::Binding);
    }
    let provenance = if setup.receipt().reported().simulated {
        Provenance::SimulatedFixture
    } else {
        provenance
    };
    let run_index = manifest
        .manifest_draft()
        .runs
        .iter()
        .position(|run| *run == monitor.run)
        .ok_or(WireError::Binding)?;
    let run = manifest
        .bind_run(run_index, binding.origin_ns())
        .map_err(|_| WireError::Binding)?;
    if run
        .binding(monitor.qpc_frequency)
        .map_err(|_| WireError::Binding)?
        != monitor
    {
        return Err(WireError::Binding);
    }

    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let sampler = project_pilot_sampler(source, manifest, &budget, p)?;
    let PilotSamplerProjection {
        nvml_uuid,
        nvml_pci,
        identity_conflict: sampler_identity_conflict,
        sample_count,
        event_count,
        audit_failed: sampler_audit_failed,
        retention_overflow: sampler_retention_overflow,
        current_clock_valid,
        origin,
        sampler_exit,
        expected_slots,
        observed_slots,
        late,
        rejected: sampler_rejected,
        telemetry_incomplete,
        telemetry,
        _telemetry_allocation: telemetry_allocation,
    } = sampler;
    let _telemetry_allocation = telemetry_allocation;
    let ready = match setup.ready().message().map_err(|_| WireError::Body)? {
        crate::workload::wire::Message::Ready {
            identity, context, ..
        } => Some((*identity, *context)),
        _ => None,
    };
    let (cuda_identity, context_identity) = ready.ok_or(WireError::Binding)?;
    let cuda_uuid = uuid_text(cuda_identity.uuid);
    let declared_identity = crate::worker::DiscoveryBinding::parse(
        &manifest.manifest_draft().raw_uuid,
        &manifest.manifest_draft().raw_pci,
    )
    .map_err(|_| WireError::Binding)?
    .identity;
    let uuid_match = match nvml_uuid {
        Some(ref raw) => {
            match crate::worker::DiscoveryBinding::parse(raw, &manifest.manifest_draft().raw_pci) {
                Ok(nvml) if nvml.identity.uuid == cuda_identity.uuid => MatchState::Match,
                Ok(_) => MatchState::Mismatch,
                Err(_) => MatchState::Unknown,
            }
        }
        None => MatchState::Unknown,
    };
    let setup_accepted = matches!(setup.assessment(), Some(Ok(_)));
    let context_device = setup
        .assessment()
        .and_then(Result::ok)
        .map(|summary| summary.selected_device.to_string())
        .map_or_else(|| missing("setup_not_accepted"), Obs::Known);

    let plan = binding.run_plan();
    let expected_dispatches = plan
        .warmup_dispatches
        .checked_add(plan.measured_dispatches)
        .ok_or(WireError::Capacity)? as usize;
    let estimate = expected_dispatches
        .checked_add(sample_count)
        .and_then(|n| n.checked_add(worker.result_packets().len()))
        .and_then(|n| n.checked_mul(4096))
        .and_then(|n| n.checked_add(32768))
        .ok_or(WireError::Capacity)?;
    let allocation = budget.reserve(estimate).map_err(|_| WireError::Capacity)?;
    let mut dispatches = Vec::new();
    dispatches
        .try_reserve_exact(expected_dispatches)
        .map_err(|_| WireError::Capacity)?;
    let mut omitted_dispatches = 0u64;
    let mut raw_mismatch = false;
    let mut projection_open = true;
    for received in worker.result_packets() {
        let message = match received.message() {
            Ok(message) => message,
            Err(_) => {
                omitted_dispatches = omitted_dispatches.saturating_add(1);
                projection_open = false;
                continue;
            }
        };
        let crate::workload::wire::Message::DispatchRecord(record) = message else {
            continue;
        };
        raw_mismatch |= record.mismatches > 0 || record.nonfinite > 0;
        if !projection_open {
            omitted_dispatches = omitted_dispatches.saturating_add(1);
            continue;
        }
        if record.dispatch as usize != dispatches.len() {
            omitted_dispatches = omitted_dispatches.saturating_add(1);
            projection_open = false;
            continue;
        }
        let query_status = record
            .calls
            .iter()
            .rev()
            .find(|call| call.operation == crate::workload::dispatch::Operation::Query)
            .and_then(|call| call.status);
        let (Some(input_digest), Some(return_code)) = (record.input_sha256, query_status) else {
            omitted_dispatches = omitted_dispatches.saturating_add(1);
            projection_open = false;
            continue;
        };
        let full_failure_retained = worker
            .failure_vector()
            .is_some_and(|vector| vector.complete() && vector.dispatch() == record.dispatch);
        let (retention, output_digest) =
            if (record.mismatches > 0 || record.nonfinite > 0) && full_failure_retained {
                (
                    Retention::FullFailure,
                    record
                        .output_sha256
                        .map_or_else(|| missing("digest_unavailable"), Obs::Known),
                )
            } else if let Some(digest) = record.output_sha256 {
                (Retention::DigestOnly, Obs::Known(digest))
            } else {
                omitted_dispatches = omitted_dispatches.saturating_add(1);
                projection_open = false;
                continue;
            };
        let elapsed = |anchor: Option<Anchor>| {
            anchor
                .and_then(|anchor| {
                    Clock::new(monitor, binding.epoch())
                        .ok()?
                        .observe(anchor)
                        .ok()
                })
                .map_or_else(|| missing("invalid_or_missing_clock"), Obs::Known)
        };
        let call_time = |operation, after: bool| {
            let mut calls = record
                .calls
                .iter()
                .filter(|call| call.operation == operation);
            let call = if operation == crate::workload::dispatch::Operation::Query {
                calls.next_back()
            } else {
                calls.next()
            };
            call.map(|call| elapsed(if after { call.after } else { call.before }))
                .unwrap_or_else(|| missing("call_not_reported"))
        };
        dispatches.push(Dispatch {
            j: u64::from(record.dispatch),
            role: match record.phase {
                crate::workload::dispatch::Phase::Warmup => Role::Warmup,
                crate::workload::dispatch::Phase::Measured => Role::Measured,
            },
            shape: format!("{}x{}", record.lanes, record.steps),
            input_digest,
            submit_ns: call_time(crate::workload::dispatch::Operation::UploadA, false),
            query_ns: call_time(crate::workload::dispatch::Operation::Query, true),
            readback_ns: call_time(crate::workload::dispatch::Operation::Readback, true),
            check_ns: call_time(crate::workload::dispatch::Operation::Hash, true),
            return_code: i64::from(return_code),
            checked_lanes: u64::from(record.checked_lanes),
            mismatch: u64::from(record.mismatches),
            nonfinite: u64::from(record.nonfinite),
            work: record.verified_fma,
            output_digest,
            retention,
        });
    }
    let total_events = event_count
        .checked_add(worker.receive_events().len())
        .and_then(|n| n.checked_add(1))
        .ok_or(WireError::Capacity)?;
    if total_events as u64 > p.event_capacity {
        return Err(WireError::Capacity);
    }
    let audit_failed = worker.assessment_fault().is_some()
        || worker.control_audit().is_none_or(|audit| {
            audit.fault().is_some() || audit.native_failure() || !audit.role_finished()
        })
        || worker.overflowed()
        || sampler_retention_overflow
        || sampler_audit_failed;
    let reports_complete = worker.reports_complete()
        && dispatches.len() == expected_dispatches
        && omitted_dispatches == 0;
    let rejected = omitted_dispatches.saturating_add(sampler_rejected);
    let mut limitations = vec![
        Reason::new("worker_exit_unobserved").expect("fixed reason"),
        Reason::new("native_completion_unknown").expect("fixed reason"),
        Reason::new("telemetry_details_in_raw_stream").expect("fixed reason"),
        Reason::new("pci_function_unobserved").expect("fixed reason"),
        Reason::new("gpu_correctness_not_independently_replayed").expect("fixed reason"),
    ];
    if !setup_accepted {
        limitations.push(Reason::new("setup_not_accepted").expect("fixed reason"));
    }
    if !reports_complete {
        limitations.push(Reason::new("dispatch_projection_incomplete").expect("fixed reason"));
    }
    if sampler_identity_conflict {
        limitations.push(Reason::new("sampler_identity_conflict").expect("fixed reason"));
    }
    if telemetry_incomplete {
        limitations.push(Reason::new("telemetry_summary_incomplete").expect("fixed reason"));
    }
    if uuid_match != MatchState::Match
        || declared_identity != cuda_identity
        || context_identity != cuda_identity
    {
        limitations.push(Reason::new("device_identity_not_fully_matched").expect("fixed reason"));
    }
    let envelope = Envelope {
        provenance,
        identity: Identity {
            nonce: super::wire_model::hex(&monitor.run),
            nvml_uuid: nvml_uuid.map_or_else(
                || missing("nvml_identity_unobserved"),
                |v| Obs::Known(v.to_owned()),
            ),
            cuda_uuid: if cuda_identity.valid() {
                Obs::Known(cuda_uuid)
            } else {
                missing("cuda_identity_invalid")
            },
            pci_bus_id: nvml_pci.map_or_else(
                || missing("nvml_pci_unobserved"),
                |v| Obs::Known(v.to_owned()),
            ),
            context_device,
            boot: missing("boot_context_unobserved"),
            uuid_match,
            pci_match: MatchState::Unknown,
        },
        artifacts: Artifacts {
            source_commit: manifest.manifest_draft().source_commit.clone(),
            worker_sha256: Obs::Known(manifest.manifest_draft().artifacts.worker),
            lockfile_sha256: Obs::Known(manifest.manifest_draft().artifacts.lockfile),
            sampler_sha256: Obs::Known(manifest.manifest_draft().artifacts.sampler),
            ptx_sha256: Obs::Known(manifest.manifest_draft().artifacts.ptx),
            kernel_entry: "autotune_fma".into(),
            generator_version: autotune_fma_reference::input::GENERATOR_ID.into(),
            oracle_version: "autotune-fma-reference-0.1.0".into(),
        },
        manifest: Manifest {
            manifest_digest: Obs::Known(manifest.digest()),
            profile_digest: Obs::Known(manifest.profile_digest()),
            shape: Obs::Known(format!(
                "{}x{}",
                plan.shape.lanes(),
                plan.shape.steps().value()
            )),
            stop_policy: Obs::Known("e3-fma-qualification-v0.1".into()),
            approval_ref: Obs::Known(manifest.manifest_draft().approval_ref.clone()),
            seed: Obs::Known(plan.seed),
            control_writes: ControlWrites::None,
        },
        clocks: Clocks {
            qpc_frequency: monitor.qpc_frequency,
            qpc_epoch_ticks: origin.qpc_before,
            interrupt_ticks: origin.interrupt_100ns,
            unbiased_ticks: Obs::Known(origin.unbiased_100ns),
            resolution_ns: crate::monitor::clock::ticks_to_ns(1, monitor.qpc_frequency, true)
                .map_err(|_| WireError::Body)?
                .max(100),
            utc_context: missing("utc_context_unobserved"),
            discontinuities: u64::from(!current_clock_valid),
        },
        lifecycle: Lifecycle {
            requested_ns: missing("parent_request_time_unavailable"),
            launch_ns: missing("role_launch_time_unavailable"),
            ready_ns: setup.assessment().and_then(Result::ok).map_or_else(
                || missing("setup_ready_not_accepted"),
                |summary| elapsed_at(monitor, binding.epoch(), Some(summary.ready)),
            ),
            start_ns: missing("start_issue_time_unavailable"),
            stop_requested_ns: missing("parent_stop_issue_time_unavailable"),
            exit_ns: missing("worker_process_exit_unobserved"),
            empty_job: EmptyJobLifecycle::Unknown,
        },
        dispatches,
        telemetry,
        integrity: Integrity {
            capacity_bytes: p.serialized_bytes,
            observed_bytes: 0,
            failed: u64::from(audit_failed),
            late,
            rejected,
            overflow: u64::from(worker.overflowed() || sampler_retention_overflow),
            expected_slots,
            observed_slots,
            raw_records: total_events as u64,
            writer_status: WriterStatus::Bounded,
        },
        result: SessionResult {
            admission: Admission::NotQualified,
            correctness: Correctness::NotChecked,
            worker_exit: missing("worker_process_exit_unobserved"),
            sampler_exit,
            empty_job: ResultEmptyJob::Unknown,
            native_completion: NativeCompletion::Unknown,
            coverage: Coverage::Incomplete,
            limitations,
            verdict: if audit_failed || raw_mismatch {
                Verdict::Invalid
            } else {
                Verdict::Inconclusive
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

#[cfg(windows)]
fn uuid_text(bytes: [u8; 16]) -> String {
    format!(
        "GPU-{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

#[cfg(windows)]
fn elapsed_at(monitor: crate::monitor::Binding, epoch: Anchor, anchor: Option<Anchor>) -> Obs<u64> {
    anchor
        .and_then(|anchor| Clock::new(monitor, epoch).ok()?.observe(anchor).ok())
        .map_or_else(|| missing("invalid_or_missing_clock"), Obs::Known)
}

/// Bounded offline collection of the raw trace. The fixed controller uses this
/// only after admission has closed; storage runs on its separate writer lane.
pub fn raw_events(s: &Supervisor) -> Result<BoundText, WireError> {
    raw_events_inner(s, |_| Ok(()))
}

/// Compose a candidate live-workload NDJSON stream from sampler/controller and
/// owned worker packets. This requires a workload-capable envelope/archive;
/// the readonly-sampler index must not be used for this stream. Cross-stream
/// order is recorded per owner, without inventing a total order.
#[cfg(windows)]
pub fn raw_events_with_worker(
    s: &Supervisor,
    setup: &crate::workload::worker_setup_delivery::ReadyDelivery,
    worker: &crate::workload::parent_worker_channel::WorkerReportEvidence,
) -> Result<BoundText, WireError> {
    raw_events_inner(s, |output| {
        if worker.overflowed() {
            return Err(WireError::Capacity);
        }
        let binding = worker.binding();
        if binding.monitor() != s.binding().monitor
            || binding.manifest_digest().as_bytes() != &s.binding().manifest_digest
        {
            return Err(WireError::Binding);
        }
        // Only the sampler/controller and worker poll streams are available
        // here; the eventual workload manifest must add other role event owners.
        let total_events = s
            .traces()
            .len()
            .checked_add(s.report().events().count())
            .and_then(|n| n.checked_add(worker.receive_events().len()))
            .and_then(|n| n.checked_add(1))
            .ok_or(WireError::Capacity)?;
        if total_events as u64
            > autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .event_capacity
        {
            return Err(WireError::Capacity);
        }
        append_worker_events(output, setup, worker)
    })
}

/// Compose raw evidence retained by the fixed workload ParentSampler. Packet
/// bytes and their independently parsed parent-side result are emitted together;
/// the report, receipt anchors and clock history remain separate owner streams.
/// This is a raw artifact adapter only and does not establish qualification.
#[cfg(windows)]
pub fn raw_events_with_parent_sampler(
    parent: &crate::workload::parent_sampler::ParentSampler,
    budget: &Budget,
) -> Result<BoundText, WireError> {
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let report_events = parent.report().events().count();
    let total_events = parent
        .records()
        .len()
        .checked_add(parent.controls().len())
        .and_then(|n| n.checked_add(usize::from(parent.current().is_some())))
        .and_then(|n| n.checked_add(usize::from(parent.ready().is_some())))
        .and_then(|n| n.checked_add(usize::from(parent.rejected().is_some())))
        .and_then(|n| n.checked_add(parent.history().raw().len()))
        .and_then(|n| n.checked_add(report_events))
        .and_then(|n| n.checked_add(1))
        .ok_or(WireError::Capacity)?;
    if total_events as u64 > p.event_capacity {
        return Err(WireError::Capacity);
    }
    let _temporary = budget
        .reserve(p.frame_bytes as usize * 16)
        .map_err(|_| WireError::Capacity)?;
    let mut output = RawBuilder::new(budget.clone())?;
    let binding = parent.binding();
    for (sequence, receipt) in parent.records().iter().enumerate() {
        let line = object([
            ("stream", Value::Text("parent_sampler_receipt".into())),
            ("sequence", Value::U64(sequence as u64)),
            (
                "receipt",
                receipt
                    .raw
                    .raw()
                    .receipt()
                    .map_or(Value::Null, anchor_value),
            ),
            ("kind", Value::Text("sampler_record".into())),
            (
                "body",
                object([
                    ("packet", worker_packet_value(&receipt.raw)),
                    (
                        "assessment",
                        receipt.record.as_ref().map_or_else(
                            |error| Value::Text(format!("rejected:{error:?}")),
                            |record| Value::Text(format!("accepted:{record:?}")),
                        ),
                    ),
                ]),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    for (sequence, packet) in parent.controls().iter().enumerate() {
        let line = object([
            ("stream", Value::Text("parent_sampler_control".into())),
            ("sequence", Value::U64(sequence as u64)),
            (
                "receipt",
                packet.raw().receipt().map_or(Value::Null, anchor_value),
            ),
            ("kind", Value::Text("control_packet".into())),
            (
                "body",
                object([
                    ("packet", worker_packet_value(packet)),
                    (
                        "decoded",
                        packet.message().map_or_else(
                            |error| Value::Text(format!("error:{error:?}")),
                            |message| {
                                Value::Text(format!("ok:{:?}", std::mem::discriminant(message)))
                            },
                        ),
                    ),
                ]),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    for (kind, packet) in [("ready", parent.ready()), ("rejected", parent.rejected())] {
        if let Some(packet) = packet {
            let line = object([
                (
                    "stream",
                    Value::Text("parent_sampler_retained_packet".into()),
                ),
                ("sequence", Value::U64(0)),
                (
                    "receipt",
                    packet.raw().receipt().map_or(Value::Null, anchor_value),
                ),
                ("kind", Value::Text(kind.into())),
                (
                    "body",
                    object([
                        ("packet", worker_packet_value(packet)),
                        (
                            "decoded",
                            packet.message().map_or_else(
                                |error| Value::Text(format!("error:{error:?}")),
                                |message| {
                                    Value::Text(format!("ok:{:?}", std::mem::discriminant(message)))
                                },
                            ),
                        ),
                    ]),
                ),
            ])
            .to_canonical();
            output.push_line(&line)?;
        }
    }
    if let Some(current) = parent.current() {
        let raw = current.raw();
        let bytes = raw.packet().bytes();
        let assessment = current.result().map_or_else(
            |fault| object([("fault", Value::Text(format!("{fault:?}")))]),
            |receipt| {
                object([
                    ("issued", Value::Text(format!("{:?}", receipt.issued()))),
                    ("received", Value::Text(format!("{:?}", receipt.received()))),
                    (
                        "capture",
                        Value::Text(receipt.capture().canonical().to_owned()),
                    ),
                ])
            },
        );
        let line = object([
            ("stream", Value::Text("parent_settings".into())),
            ("sequence", Value::U64(0)),
            ("receipt", raw.receipt().map_or(Value::Null, anchor_value)),
            ("kind", Value::Text("current_settings".into())),
            (
                "body",
                object([
                    ("lane", Value::Text(format!("{:?}", raw.lane()))),
                    (
                        "origin",
                        raw.origin().map_or(Value::Null, |origin| {
                            object([
                                ("kind", Value::Text(format!("{:?}", origin.kind()))),
                                ("binding", Value::Text(origin.binding().to_hex())),
                                ("declaration", Value::Text(origin.declaration().to_hex())),
                                ("image", Value::Text(origin.image().to_hex())),
                                ("pid", Value::U64(origin.pid().into())),
                                ("creation", Value::U64(origin.creation())),
                            ])
                        }),
                    ),
                    ("received_length", Value::U64(bytes.len() as u64)),
                    (
                        "packet_digest",
                        Value::Text(
                            crate::evidence::Digest::of(
                                crate::evidence::Domain::RawArtifact,
                                bytes,
                            )
                            .to_hex(),
                        ),
                    ),
                    ("bytes_hex", Value::Text(super::wire_model::hex(bytes))),
                    (
                        "rejection",
                        raw.packet()
                            .rejection()
                            .map_or(Value::Null, |value| Value::Text(format!("{value:?}"))),
                    ),
                    ("assessment", assessment),
                ]),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    for (sequence, anchor) in parent.history().raw().iter().enumerate() {
        let line = object([
            ("stream", Value::Text("parent_clock_history".into())),
            ("sequence", Value::U64(sequence as u64)),
            ("receipt", anchor_value(*anchor)),
            ("kind", Value::Text("clock_anchor".into())),
            (
                "body",
                object([(
                    "manifest_digest",
                    Value::Text(binding.manifest_digest().to_hex()),
                )]),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    for (sequence, event) in parent.report().events().enumerate() {
        let line = object([
            ("stream", Value::Text("parent_monitor".into())),
            ("sequence", Value::U64(sequence as u64)),
            ("receipt", anchor_value(event.anchor)),
            ("kind", Value::Text("monitor_event".into())),
            (
                "body",
                object([
                    ("action", Value::Text(format!("{:?}", event.action))),
                    (
                        "time",
                        event.time.map_or_else(
                            |error| Value::Text(format!("error:{error:?}")),
                            Value::U64,
                        ),
                    ),
                    (
                        "record_error",
                        event
                            .record_error
                            .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
                    ),
                    ("missed", Value::Text(format!("{:?}", event.missed))),
                ]),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    let line = object([
        ("stream", Value::Text("parent_sampler_assessment".into())),
        ("sequence", Value::U64(0)),
        ("receipt", Value::Null),
        ("kind", Value::Text("summary".into())),
        (
            "body",
            object([
                (
                    "manifest_digest",
                    Value::Text(binding.manifest_digest().to_hex()),
                ),
                (
                    "clock_overflow",
                    parent
                        .history()
                        .overflow()
                        .map_or(Value::Null, anchor_value),
                ),
                ("clock_missing", Value::Bool(parent.history().missing())),
                (
                    "clock_error",
                    Value::Text(format!("{:?}", parent.history().clock_error())),
                ),
                ("fault", Value::Text(format!("{:?}", parent.fault()))),
                ("reported_finished", Value::Bool(parent.reported_finished())),
                (
                    "current_clock_valid",
                    Value::Bool(parent.report().current_clock_valid()),
                ),
                ("event_count", Value::U64(total_events as u64)),
            ]),
        ),
    ])
    .to_canonical();
    output.push_line(&line)?;
    Ok(output.finish())
}

/// Combine the fixed parent's retained sampler records with the worker's
/// setup/result/control poll ledger in one bounded workload raw stream.
#[cfg(windows)]
pub fn raw_events_with_parent_sampler_and_worker(
    parent: &crate::workload::parent_sampler::ParentSampler,
    setup: &crate::workload::worker_setup_delivery::ReadyDelivery,
    worker: &crate::workload::parent_worker_channel::WorkerReportEvidence,
    budget: &Budget,
) -> Result<BoundText, WireError> {
    let binding = worker.binding();
    if parent.binding() != binding
        || setup.receipt().binding() != binding
        || setup.origin().ok() != Some(worker.origin())
    {
        return Err(WireError::Binding);
    }
    if worker.overflowed() {
        return Err(WireError::Capacity);
    }
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let parent_events = parent
        .records()
        .len()
        .checked_add(parent.controls().len())
        .and_then(|n| n.checked_add(usize::from(parent.current().is_some())))
        .and_then(|n| n.checked_add(usize::from(parent.ready().is_some())))
        .and_then(|n| n.checked_add(usize::from(parent.rejected().is_some())))
        .and_then(|n| n.checked_add(parent.history().raw().len()))
        .and_then(|n| n.checked_add(parent.report().events().count()))
        .and_then(|n| n.checked_add(1))
        .ok_or(WireError::Capacity)?;
    let total_events = parent_events
        .checked_add(worker.receive_events().len())
        .and_then(|n| n.checked_add(1))
        .ok_or(WireError::Capacity)?;
    if total_events as u64 > p.event_capacity {
        return Err(WireError::Capacity);
    }
    let raw = raw_events_with_parent_sampler(parent, budget)?;
    let _temporary = budget
        .reserve(p.frame_bytes as usize * 16)
        .map_err(|_| WireError::Capacity)?;
    let BoundText {
        text,
        _allocation: storage,
        ..
    } = raw;
    let mut output = RawBuilder {
        text,
        storage,
        budget: budget.clone(),
    };
    append_worker_events(&mut output, setup, worker)?;
    Ok(output.finish())
}

/// Append canonical retained start, consent-request, environment and
/// worker-setup artifacts owned by the finalized pilot. These are artifact rows rather
/// than lifecycle events; their payloads remain byte-exact after hex encoding.
/// Each row labels source ownership only and does not assert that reported
/// contents are true or that a workload was qualified.
#[cfg(windows)]
pub fn append_pilot_owner_artifacts<E>(
    raw: BoundText,
    owners: &crate::workload::session_finalizer::PilotOwners<E>,
    budget: &Budget,
) -> Result<BoundText, WireError> {
    let starting_handoff = owners.starting_handoff.session.canonical();
    let sources: [(&str, Option<&[u8]>); 5] = [
        ("starting_handoff", starting_handoff),
        (
            "consent_request",
            Some(owners.starting_consent.request().canonical().as_bytes()),
        ),
        (
            "starting_read",
            Some(owners.starting_read.canonical().as_bytes()),
        ),
        (
            "host_environment",
            Some(owners.host_environment.snapshot().canonical().as_bytes()),
        ),
        (
            "worker_setup",
            Some(
                owners
                    .worker_setup_delivery
                    .receipt()
                    .canonical()
                    .as_bytes(),
            ),
        ),
    ];
    let source_bytes = sources
        .iter()
        .try_fold(0usize, |total, (_, bytes)| {
            total.checked_add(bytes.map_or(0, <[u8]>::len))
        })
        .ok_or(WireError::Capacity)?;
    // The temporary Value tree, hex payloads and canonical lines coexist while
    // RawBuilder charges the growing output buffer separately.
    let scratch_bytes = source_bytes
        .checked_mul(4)
        .and_then(|n| n.checked_add(5 * 4096))
        .and_then(|n| n.checked_add(crate::workload::consent::CAP * 16))
        .ok_or(WireError::Capacity)?;
    let _scratch = budget
        .reserve(scratch_bytes)
        .map_err(|_| WireError::Capacity)?;
    let consent_capture = owners.starting_consent.raw_evidence_value().to_canonical();
    let BoundText {
        text,
        _allocation: storage,
        ..
    } = raw;
    let mut output = RawBuilder {
        text,
        storage,
        budget: budget.clone(),
    };
    for (owner, source) in sources {
        let bytes = source.unwrap_or_default();
        let line = object([
            ("kind", Value::Text("retained_owner_artifact".into())),
            ("owner", Value::Text(owner.into())),
            ("available", Value::Bool(source.is_some())),
            (
                "digest",
                if source.is_none() {
                    Value::Null
                } else {
                    Value::Text(Digest::of(Domain::RawArtifact, bytes).to_hex())
                },
            ),
            (
                "bytes_hex",
                if source.is_none() {
                    Value::Null
                } else {
                    Value::Text(super::wire_model::hex(bytes))
                },
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    let consent_bytes = consent_capture.as_bytes();
    let consent_line = object([
        ("kind", Value::Text("retained_owner_artifact".into())),
        ("owner", Value::Text("consent_capture".into())),
        ("available", Value::Bool(true)),
        (
            "digest",
            Value::Text(Digest::of(Domain::RawArtifact, consent_bytes).to_hex()),
        ),
        (
            "bytes_hex",
            Value::Text(super::wire_model::hex(consent_bytes)),
        ),
    ])
    .to_canonical();
    output.push_line(&consent_line)?;
    Ok(output.finish())
}

/// Append retained Start, NativeGrant re-entry and normal-retirement process
/// frames. The rows preserve OS calls, partial results, anchors and pending
/// request state; they do not claim anything beyond those observed handles.
#[cfg(windows)]
pub fn append_pilot_process_artifacts(
    raw: BoundText,
    parent: &crate::workload::parent_sampler::ParentSampler,
    retirement: Option<&crate::workload::parent_watch_retirement::Frame>,
    budget: &Budget,
) -> Result<BoundText, WireError> {
    // These fixed-size models have six calls each and three process snapshots.
    // Reserve before constructing their temporary Value trees/encoded forms.
    let _scratch = budget
        .reserve(crate::workload::consent::CAP * 16)
        .map_err(|_| WireError::Capacity)?;
    let frames = [
        (
            "start_process_observation",
            parent
                .process_frame()
                .map(crate::workload::process_state::Frame::raw_evidence_value),
        ),
        (
            "native_grant_process_observation",
            parent
                .native_grant_process_frame()
                .map(crate::workload::process_state::Frame::raw_evidence_value),
        ),
        (
            "normal_retirement_process_observation",
            retirement.map(crate::workload::parent_watch_retirement::Frame::raw_evidence_value),
        ),
    ];
    let BoundText {
        text,
        _allocation: storage,
        ..
    } = raw;
    let mut output = RawBuilder {
        text,
        storage,
        budget: budget.clone(),
    };
    for (owner, value) in frames {
        let bytes = value.map(|value| value.to_canonical());
        let line = object([
            ("kind", Value::Text("retained_owner_artifact".into())),
            ("owner", Value::Text(owner.into())),
            ("available", Value::Bool(bytes.is_some())),
            (
                "digest",
                bytes.as_ref().map_or(Value::Null, |bytes| {
                    Value::Text(Digest::of(Domain::RawArtifact, bytes.as_bytes()).to_hex())
                }),
            ),
            (
                "bytes_hex",
                bytes.as_ref().map_or(Value::Null, |bytes| {
                    Value::Text(super::wire_model::hex(bytes.as_bytes()))
                }),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    let grant_wire = parent.native_grant_wire().map(|wire| wire.bytes());
    output.push_line(&owner_bytes_line("native_grant_wire", grant_wire)?)?;
    let grant_attempt = parent
        .native_grant_attempt()
        .map(native_grant_attempt_value)
        .map(|value| value.to_canonical());
    output.push_line(&owner_bytes_line(
        "native_grant_attempt",
        grant_attempt.as_deref().map(str::as_bytes),
    )?)?;
    Ok(output.finish())
}

#[cfg(windows)]
fn owner_bytes_line(owner: &str, bytes: Option<&[u8]>) -> Result<String, WireError> {
    let line = object([
        ("kind", Value::Text("retained_owner_artifact".into())),
        ("owner", Value::Text(owner.into())),
        ("available", Value::Bool(bytes.is_some())),
        (
            "digest",
            bytes.map_or(Value::Null, |bytes| {
                Value::Text(Digest::of(Domain::RawArtifact, bytes).to_hex())
            }),
        ),
        (
            "bytes_hex",
            bytes.map_or(Value::Null, |bytes| {
                Value::Text(super::wire_model::hex(bytes))
            }),
        ),
    ])
    .to_canonical();
    Ok(line)
}

#[cfg(windows)]
fn parent_start_observation_value(
    observation: crate::workload::parent_start::Observation,
) -> Value {
    object([
        (
            "anchor",
            observation.anchor.map_or(Value::Null, anchor_value),
        ),
        (
            "capture_error",
            observation
                .capture_error
                .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
        ),
        (
            "clock_identity",
            object([
                ("frequency", Value::U64(observation.clock_identity.0)),
                (
                    "token",
                    Value::Text(super::wire_model::hex(&observation.clock_identity.1)),
                ),
            ]),
        ),
        (
            "parent_fault",
            observation
                .parent_fault
                .map_or(Value::Null, |fault| Value::Text(format!("{fault:?}"))),
        ),
        ("context", Value::Bool(observation.context)),
        ("reported_screen", Value::Bool(observation.reported_screen)),
        (
            "safety",
            Value::Array(
                observation
                    .safety
                    .map(|record| record.map_or(Value::Null, parent_record_value))
                    .into(),
            ),
        ),
        (
            "reentry",
            observation.reentry.map_or(Value::Null, |records| {
                Value::Array(records.map(parent_record_value).into())
            }),
        ),
        ("consecutive", Value::U64(observation.consecutive.into())),
        ("watch_ready", Value::Bool(observation.watch_ready)),
    ])
}

#[cfg(windows)]
fn parent_record_value(record: crate::monitor::Record) -> Value {
    let binding = record.ticket.binding;
    object([
        (
            "session",
            Value::Text(super::wire_model::hex(&binding.session)),
        ),
        ("run", Value::Text(super::wire_model::hex(&binding.run))),
        (
            "device_uuid",
            Value::Text(super::wire_model::hex(&binding.device.uuid)),
        ),
        ("device_domain", Value::U64(binding.device.domain.into())),
        ("device_bus", Value::U64(binding.device.bus.into())),
        ("device_number", Value::U64(binding.device.device.into())),
        (
            "clock_token",
            Value::Text(super::wire_model::hex(&binding.clock_token)),
        ),
        ("frequency", Value::U64(binding.qpc_frequency)),
        (
            "metric",
            Value::Text(metric_name(record.ticket.metric).into()),
        ),
        ("slot", Value::U64(record.ticket.slot)),
        ("scheduled_ns", Value::U64(record.ticket.scheduled_ns)),
        ("start_ns", Value::U64(record.ticket.start_ns)),
        ("end_ns", Value::U64(record.end_ns)),
        ("receipt_ns", Value::U64(record.receipt_ns)),
        ("unit", Value::Text(format!("{:?}", record.unit))),
        ("status", Value::Text(format!("{:?}", record.status))),
        ("value", record.value.map_or(Value::Null, Value::I64)),
    ])
}

#[cfg(windows)]
fn native_grant_attempt_value(
    attempt: &crate::workload::parent_sampler::NativeGrantAttempt,
) -> Value {
    object([
        (
            "observations",
            Value::Array(
                attempt
                    .observations
                    .map(|observation| {
                        observation.map_or(Value::Null, parent_start_observation_value)
                    })
                    .into(),
            ),
        ),
        (
            "checks",
            Value::Array(
                attempt
                    .checks
                    .map(|check| {
                        check.map_or(Value::Null, |check| {
                            Value::Text(match check {
                                Ok(()) => "ok".into(),
                                Err(error) => format!("error:{error:?}"),
                            })
                        })
                    })
                    .into(),
            ),
        ),
        (
            "codec_error",
            attempt
                .codec_error
                .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
        ),
        (
            "send_error",
            attempt
                .send_error
                .map_or(Value::Null, |error| Value::Text(format!("{error:?}"))),
        ),
        ("queued", Value::Bool(attempt.queued)),
        (
            "result",
            Value::Text(match attempt.result {
                Ok(()) => "ok".into(),
                Err(error) => format!("error:{error:?}"),
            }),
        ),
    ])
}

/// Derive typed telemetry only from actual receipts retained by the fixed
/// workload ParentSampler. Raw packets remain the audit source for omitted or
/// rejected rows; this projection never promotes a rejected reading.
#[cfg(windows)]
pub fn project_parent_telemetry(
    parent: &crate::workload::parent_sampler::ParentSampler,
    device: &str,
    budget: &Budget,
) -> Result<ParentTelemetryProjection, WireError> {
    use crate::monitor::Unit;
    use crate::workload::wire::Message;

    let binding = parent.binding();
    let monitor = binding.monitor();
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let cutoff = parent
        .report()
        .stop()
        .map_or_else(|| parent.report().now_ns(), |stop| stop.at_ns);
    let expected_slots = (cutoff / p.cadence_ns + 1)
        .checked_mul(u64::from(p.metric_count))
        .ok_or(WireError::Capacity)?;
    let estimate = parent
        .records()
        .len()
        .checked_mul(1024)
        .and_then(|n| n.checked_add(4096))
        .ok_or(WireError::Capacity)?;
    let allocation = budget.reserve(estimate).map_err(|_| WireError::Capacity)?;
    let mut telemetry = Vec::new();
    telemetry
        .try_reserve_exact(parent.records().len())
        .map_err(|_| WireError::Capacity)?;
    let mut raw_records = 0u64;
    let mut late = 0u64;
    let mut rejected = 0u64;
    let mut duplicates = 0u64;
    let mut invalid_timing = false;

    for receipt in parent.records() {
        raw_records = raw_records.saturating_add(1);
        let (ticket, before, after, raw) = match receipt.raw.message() {
            Ok(Message::SamplerRecord {
                ticket,
                before,
                after,
                raw: Some(raw),
                ..
            }) => (*ticket, *before, *after, raw),
            _ => {
                rejected = rejected.saturating_add(1);
                continue;
            }
        };
        if ticket.binding != monitor {
            rejected = rejected.saturating_add(1);
            continue;
        }
        let name = metric_name(ticket.metric);
        if telemetry
            .iter()
            .any(|row: &Telemetry| row.metric == name && row.slot == ticket.slot)
        {
            duplicates = duplicates.saturating_add(1);
            continue;
        }
        let issue_anchor = parent.report().events().find_map(|event| {
            matches!(event.action, Action::Begin(Some(actual)) if actual == ticket)
                .then_some(event.anchor)
        });
        let mut clock = Clock::new(monitor, binding.epoch()).map_err(|_| WireError::Binding)?;
        let issued_ns = issue_anchor.and_then(|anchor| clock.observe(anchor).ok());
        let start_ns = issued_ns.and_then(|_| before.and_then(|a| clock.observe(a).ok()));
        let end_ns = start_ns.and_then(|_| after.and_then(|a| clock.observe(a).ok()));
        let receipt_anchor = receipt.raw.raw().receipt();
        let receipt_ns = end_ns.and_then(|_| receipt_anchor.and_then(|a| clock.observe(a).ok()));
        let ordered = matches!((issued_ns, start_ns, end_ns, receipt_ns), (Some(issue), Some(start), Some(end), Some(receipt_ns)) if ticket.start_ns == issue && ticket.start_ns <= start && start <= end && end <= receipt_ns)
            && before.is_some_and(|start| {
                after.is_some_and(|end| {
                    start.qpc_after <= end.qpc_before
                        && receipt_anchor.is_some_and(|anchor| end.qpc_after <= anchor.qpc_before)
                })
            });
        invalid_timing |= !ordered;
        if receipt_ns.is_none_or(|ns| ns > cutoff) {
            late = late.saturating_add(1);
        }

        let assessed = receipt.record.as_ref().ok();
        if assessed.is_none_or(|record| record.ticket != ticket) {
            rejected = rejected.saturating_add(1);
        }
        let (value, vendor_code) = match &raw.reading {
            autotune_probe::report::Reading::Available { .. } => {
                let value = if ordered {
                    assessed.map_or_else(
                        || missing("assessment_rejected"),
                        |record| {
                            if record.status == crate::monitor::Status::Available {
                                record.value.map_or_else(
                                    || missing("invalid_value"),
                                    |n| Obs::Known(TelemetryValue::Int(n)),
                                )
                            } else {
                                missing("raw_unavailable")
                            }
                        },
                    )
                } else {
                    missing("invalid_timing")
                };
                (value, missing("no_vendor_code"))
            }
            autotune_probe::report::Reading::Unavailable { failure } => (
                missing("raw_unavailable"),
                failure
                    .vendor_status
                    .map(|code| Obs::Known(code.to_string()))
                    .unwrap_or_else(|| missing("no_vendor_code")),
            ),
        };
        let unit = match ticket.metric.unit() {
            Unit::DegC => "degC",
            Unit::Milliwatt => "mW",
            Unit::MHz => "MHz",
            Unit::Percent => "%",
        };
        telemetry.push(Telemetry {
            metric: name.into(),
            unit: unit.into(),
            device: device.into(),
            value,
            slot: ticket.slot,
            query_start_ns: if ordered {
                start_ns.map_or_else(|| missing("invalid_timing"), Obs::Known)
            } else {
                missing("invalid_timing")
            },
            query_end_ns: if ordered {
                end_ns.map_or_else(|| missing("invalid_timing"), Obs::Known)
            } else {
                missing("invalid_timing")
            },
            receipt_ns: if ordered {
                receipt_ns.map_or_else(|| missing("invalid_timing"), Obs::Known)
            } else {
                missing("invalid_timing")
            },
            internal_age: InternalAge::Unknown,
            vendor_code,
        });
    }
    let observed_slots = telemetry.len() as u64;
    Ok(ParentTelemetryProjection {
        telemetry,
        expected_slots,
        observed_slots,
        raw_records,
        late,
        rejected,
        duplicates,
        invalid_timing,
        _allocation: allocation,
    })
}

fn raw_events_inner<F>(s: &Supervisor, append: F) -> Result<BoundText, WireError>
where
    F: FnOnce(&mut RawBuilder) -> Result<(), WireError>,
{
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
    append(&mut output)?;
    Ok(output.finish())
}

#[cfg(windows)]
fn append_worker_events(
    output: &mut RawBuilder,
    setup: &crate::workload::worker_setup_delivery::ReadyDelivery,
    worker: &crate::workload::parent_worker_channel::WorkerReportEvidence,
) -> Result<(), WireError> {
    let binding = worker.binding();
    if setup.receipt().binding() != binding || setup.origin().ok() != Some(worker.origin()) {
        return Err(WireError::Binding);
    }
    let setup_packet_count = setup.packets().count();
    let mut setup_packets = setup.packets();
    let mut result_packets = worker.result_packets().iter();
    let mut control_packets = worker.control_packets().iter();
    let mut ordinary_packet_index = 0usize;
    for event in worker.receive_events() {
        let packet = if event.kind != crate::workload::parent_worker_channel::ReceiveKind::Packet {
            if event.packet_digest.is_some() {
                return Err(WireError::Body);
            }
            None
        } else {
            let packet = match event.lane {
                crate::sampler::wire::Lane::Ordinary => {
                    let packet = if ordinary_packet_index < setup_packet_count {
                        setup_packets.next()
                    } else {
                        result_packets.next()
                    }
                    .ok_or(WireError::Capacity)?;
                    ordinary_packet_index += 1;
                    packet
                }
                crate::sampler::wire::Lane::Control => {
                    control_packets.next().ok_or(WireError::Capacity)?
                }
            };
            if event.packet_digest.is_none() {
                return Err(WireError::Body);
            }
            Some(packet)
        };
        let packet_matches = packet.zip(event.packet_digest).map(|(packet, digest)| {
            crate::evidence::Digest::of(
                crate::evidence::Domain::RawArtifact,
                packet.raw().packet().bytes(),
            ) == digest
        });
        let line = object([
            ("stream", Value::Text("worker_poll".into())),
            ("sequence", Value::U64(event.ordinal.into())),
            ("receipt", event.anchor.map_or(Value::Null, anchor_value)),
            ("kind", Value::Text(format!("{:?}", event.kind))),
            (
                "body",
                object([
                    ("lane", Value::Text(format!("{:?}", event.lane))),
                    ("packet", packet.map_or(Value::Null, worker_packet_value)),
                    (
                        "packet_matches_digest",
                        packet_matches.map_or(Value::Null, Value::Bool),
                    ),
                    (
                        "packet_digest",
                        event
                            .packet_digest
                            .map_or(Value::Null, |digest| Value::Text(digest.to_hex())),
                    ),
                    (
                        "os_code",
                        event
                            .code
                            .map_or(Value::Null, |code| Value::I64(code.into())),
                    ),
                ]),
            ),
        ])
        .to_canonical();
        output.push_line(&line)?;
    }
    if setup_packets.next().is_some()
        || result_packets.next().is_some()
        || control_packets.next().is_some()
    {
        return Err(WireError::Capacity);
    }
    let progress = worker.progress();
    let vector = worker.failure_vector();
    let control = worker.control_audit();
    let line = object([
        ("stream", Value::Text("worker_assessment".into())),
        ("sequence", Value::U64(0)),
        ("receipt", Value::Null),
        ("kind", Value::Text("summary".into())),
        (
            "body",
            object([
                (
                    "progress_warmup",
                    progress.map_or(Value::Null, |p| Value::U64(p.warmup.into())),
                ),
                (
                    "progress_measured",
                    progress.map_or(Value::Null, |p| Value::U64(p.measured.into())),
                ),
                ("reports_complete", Value::Bool(worker.reports_complete())),
                (
                    "assessment_fault",
                    worker
                        .assessment_fault()
                        .map_or(Value::Null, |f| Value::Text(format!("{f:?}"))),
                ),
                ("receive_overflow", Value::Bool(worker.overflowed())),
                (
                    "control_role_finished",
                    control.map_or(Value::Null, |audit| Value::Bool(audit.role_finished())),
                ),
                (
                    "control_native_failure",
                    control.map_or(Value::Null, |audit| Value::Bool(audit.native_failure())),
                ),
                (
                    "control_fault",
                    control
                        .and_then(|audit| audit.fault())
                        .map_or(Value::Null, |fault| Value::Text(format!("{fault:?}"))),
                ),
                (
                    "failure_vector_complete",
                    vector.map_or(Value::Null, |v| Value::Bool(v.complete())),
                ),
                (
                    "failure_vector_failed",
                    vector.map_or(Value::Null, |v| Value::Bool(v.failed())),
                ),
                (
                    "failure_vector_lanes",
                    vector.map_or(Value::Null, |v| Value::U64(v.bits().len() as u64)),
                ),
            ]),
        ),
    ])
    .to_canonical();
    output.push_line(&line)?;
    Ok(())
}

#[cfg(windows)]
fn worker_packet_value(packet: &crate::workload::transport::WorkerReceived) -> Value {
    let raw = packet.raw();
    let bytes = raw.packet().bytes();
    let packet_digest = crate::evidence::Digest::of(crate::evidence::Domain::RawArtifact, bytes);
    object([
        ("lane", Value::Text(format!("{:?}", raw.lane()))),
        (
            "origin",
            raw.origin().map_or(Value::Null, |origin| {
                object([
                    ("kind", Value::Text(format!("{:?}", origin.kind()))),
                    ("binding", Value::Text(origin.binding().to_hex())),
                    ("declaration", Value::Text(origin.declaration().to_hex())),
                    ("image", Value::Text(origin.image().to_hex())),
                    ("pid", Value::U64(origin.pid().into())),
                    ("creation", Value::U64(origin.creation())),
                ])
            }),
        ),
        ("received_length", Value::U64(bytes.len() as u64)),
        ("packet_digest", Value::Text(packet_digest.to_hex())),
        ("bytes_hex", Value::Text(super::wire_model::hex(bytes))),
        (
            "rejection",
            raw.packet().rejection().map_or(Value::Null, |rejection| {
                Value::Text(format!("{rejection:?}"))
            }),
        ),
        ("decode_ok", Value::Bool(packet.message().is_ok())),
    ])
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
