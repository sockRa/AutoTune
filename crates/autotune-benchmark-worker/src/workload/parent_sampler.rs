//! Parent sampler receipts/scheduling. Reported facts are not authenticated peer
//! evidence, human consent, worker/native permission or baseline qualification.
use super::{
    bootstrap::EntryClock,
    gate::StopWindow,
    manifest::BoundRun,
    sampler_runtime::{route, Route},
    settings,
    settings_transport::{Frame, Parent},
    telemetry::{self, Receipt},
    transport::{Queue, RawReceived, SendFailure, Sender, SettingsReceived, WorkerReceived},
    wire::{Channel, Direction, EncodedFrame, Message, WorkerBinding},
};
#[cfg(windows)]
use crate::evidence::{Domain, Value};
use crate::{
    evidence::{Digest, Reason},
    monitor::{Anchor, Controller, Metric, Record, Status, Ticket},
    sampler::{
        budget::{Budget, Reservation},
        wire::{Lane, WireError},
    },
};
use autotune_probe::report::{Reading, Runtime, SymbolPresence};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Context,
    Capacity,
    Binding,
    Clock,
    SetupDeadline,
    ReentryDeadline,
    RunDeadline,
    Monitor,
    Protocol,
    Runtime,
    Environment,
    Settings,
    Simulated,
    Telemetry,
    Stopped,
    Transport,
    ShutdownDeadline,
    Supervision,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeGrantCommitments {
    pub setup: Digest,
    pub context: Digest,
    pub peers: Digest,
    pub consent: Digest,
    pub process: Digest,
    pub safety: Digest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeGrantAttempt {
    pub observations: [Option<super::parent_start::Observation>; 3],
    pub checks: [Option<Result<(), Fault>>; 3],
    pub codec_error: Option<WireError>,
    pub send_error: Option<SendFailure>,
    pub queued: bool,
    pub result: Result<(), Fault>,
}
/// Exact encoded NativeGrant bytes retained independently of queue ownership.
/// Queue success is not receipt, native execution, or durable evidence.
#[cfg(windows)]
pub struct NativeGrantWire {
    bytes: Vec<u8>,
    _allocation: Reservation,
}
#[cfg(windows)]
impl NativeGrantWire {
    fn copy(bytes: &[u8], budget: &Budget) -> Result<Self, Fault> {
        let size = bytes
            .len()
            .checked_add(std::mem::size_of::<Self>())
            .ok_or(Fault::Capacity)?;
        let allocation = budget.reserve(size).map_err(|_| Fault::Capacity)?;
        let mut retained = Vec::new();
        retained
            .try_reserve_exact(bytes.len())
            .map_err(|_| Fault::Capacity)?;
        retained.extend_from_slice(bytes);
        Ok(Self {
            bytes: retained,
            _allocation: allocation,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
#[derive(Default)]
pub struct Reentry {
    slot: Option<u64>,
    pair: [Option<Record>; 2],
    last: Option<u64>,
    consecutive: u32,
    latest: Option<[Record; 2]>,
}
impl Reentry {
    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }
    pub fn ready(&self) -> bool {
        self.consecutive
            >= autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .reentry_observations
    }
    fn fresh(&self, now: u64) -> bool {
        self.latest.is_some_and(|pair| {
            pair.iter().all(|r| {
                now.checked_sub(r.end_ns).is_some_and(|age| {
                    age < autotune_fma_reference::profile::Profile::approved_design()
                        .limits
                        .observation_gap_ns
                })
            })
        })
    }
    fn accept(&mut self, r: Record) {
        let i = match r.ticket.metric {
            Metric::Temperature => 0,
            Metric::GpuUtilization => 1,
            _ => return,
        };
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let ceiling = if i == 0 {
            p.reentry_temperature_degc
        } else {
            p.reentry_utilization_percent as i64
        };
        if r.status != Status::Available || r.value.is_none_or(|v| v > ceiling) {
            self.consecutive = 0;
            self.last = None;
            self.latest = None;
        }
        if self.slot != Some(r.ticket.slot) {
            self.slot = Some(r.ticket.slot);
            self.pair = [None; 2];
        }
        self.pair[i] = Some(r);
        if let [Some(t), Some(u)] = self.pair {
            let p = autotune_fma_reference::profile::Profile::approved_design().limits;
            let valid = t.status == Status::Available
                && u.status == Status::Available
                && t.value.is_some_and(|v| v <= p.reentry_temperature_degc)
                && u.value
                    .is_some_and(|v| v <= p.reentry_utilization_percent as i64);
            self.consecutive = if valid {
                if self
                    .last
                    .is_some_and(|s| s.checked_add(1) == Some(r.ticket.slot))
                {
                    self.consecutive.saturating_add(1)
                } else {
                    1
                }
            } else {
                0
            };
            self.last = Some(r.ticket.slot);
            self.latest = valid.then_some([t, u]);
            self.pair = [None; 2];
        }
    }
}
pub struct ParentSampler {
    binding: WorkerBinding,
    #[cfg(windows)]
    binding_digest: crate::evidence::Digest,
    expected_uuid: String,
    expected_pci: String,
    setup_entry: Anchor,
    setup_deadline: u64,
    now_ns: u64,
    history: EntryClock,
    monitor: Controller,
    tx: Channel,
    rx: Channel,
    control_tx: Channel,
    control_rx: Channel,
    settings: Parent,
    ready: Option<WorkerReceived>,
    runtime: Option<Runtime>,
    current: Option<SettingsReceived>,
    records: Vec<Receipt>,
    controls: Vec<WorkerReceived>,
    capacity: usize,
    rejected: Option<WorkerReceived>,
    pending: Option<(Ticket, Anchor)>,
    unsent: Option<EncodedFrame>,
    unsent_settings: Option<Frame>,
    unsent_stop: Option<EncodedFrame>,
    send_error: Option<SendFailure>,
    codec_error: Option<WireError>,
    fault: Option<Fault>,
    stop: Option<StopWindow>,
    stop_event: Option<Result<(), Option<i32>>>,
    reentry: Reentry,
    setup_done: bool,
    settings_issued: bool,
    stop_attempted: bool,
    finishing: bool,
    reported_finished: bool,
    miss_count: usize,
    starting_check: Option<(crate::evidence::Digest, Result<settings::Current, Fault>)>,
    environment_check: Option<(
        crate::evidence::Digest,
        Result<(), super::host_environment::Fault>,
    )>,
    context_check: Option<super::parent_context::Record>,
    origins: super::launch_origin::Chain,
    expected_origin: Option<super::launch_origin::Origin>,
    peer_check: Option<super::parent_peers::Record>,
    consent: Option<super::consent::Delivery>,
    process_frame: Option<super::process_state::Frame>,
    #[cfg(windows)]
    native_grant_process_frame: Option<super::process_state::Frame>,
    #[cfg(windows)]
    native_grant_observations: Option<(
        super::parent_start::Observation,
        Option<super::parent_start::Observation>,
    )>,
    #[cfg(windows)]
    native_grant_attempt: Option<NativeGrantAttempt>,
    #[cfg(windows)]
    native_grant_wire: Option<NativeGrantWire>,
    start_attempt: Option<super::parent_start::Attempt>,
    #[cfg(windows)]
    watch: Option<super::parent_watch_windows::Publisher>,
    watch_error: Option<super::parent_watch::Fault>,
    #[cfg(windows)]
    worker_start_tx: Channel,
    unsent_worker_start: Option<EncodedFrame>,
    #[cfg(windows)]
    unsent_native_grant: Option<EncodedFrame>,
    budget: Budget,
    _metadata: Reservation,
}
impl ParentSampler {
    /// Immutable binding used by the fixed parent export path.
    pub fn binding(&self) -> WorkerBinding {
        self.binding
    }
    pub fn watch_error(&self) -> Option<super::parent_watch::Fault> {
        self.watch_error
    }
    pub fn start_attempt(&self) -> Option<&super::parent_start::Attempt> {
        self.start_attempt.as_ref()
    }
    pub fn unsent_worker_start(&self) -> Option<&EncodedFrame> {
        self.unsent_worker_start.as_ref()
    }
    pub fn process_frame(&self) -> Option<&super::process_state::Frame> {
        self.process_frame.as_ref()
    }
    /// Fresh three-origin process observation reserved for a future one-shot
    /// native grant. It is separate from the historical Start decision frame.
    #[cfg(windows)]
    pub fn native_grant_process_frame(&self) -> Option<&super::process_state::Frame> {
        self.native_grant_process_frame.as_ref()
    }
    #[cfg(windows)]
    pub fn native_grant_attempt(&self) -> Option<&NativeGrantAttempt> {
        self.native_grant_attempt.as_ref()
    }
    #[cfg(windows)]
    pub fn native_grant_wire(&self) -> Option<&NativeGrantWire> {
        self.native_grant_wire.as_ref()
    }
    #[cfg(windows)]
    pub fn unsent_native_grant(&self) -> Option<&EncodedFrame> {
        self.unsent_native_grant.as_ref()
    }
    /// Canonical references to the exact parent-side records retained after
    /// fresh checks. A digest binds those records; it is not an attestation.
    #[cfg(windows)]
    pub fn native_grant_commitments(&self) -> Option<NativeGrantCommitments> {
        let _scratch = self.budget.reserve(64 * 1024).ok()?;
        self.derive_native_grant_commitments()
    }
    #[cfg(windows)]
    fn derive_native_grant_commitments(&self) -> Option<NativeGrantCommitments> {
        let attempt = self.start_attempt.as_ref()?;
        if !attempt.queued
            || attempt.result.is_err()
            || self.fault.is_some()
            || !self.context_open()
            || !self.watch_ready()
        {
            return None;
        }
        let (environment_digest, environment_result) = self.environment_check?;
        if environment_result.is_err() {
            return None;
        }
        let (starting_digest, starting_result) = self.starting_check?;
        let starting_current = starting_result.ok()?;
        let context = self.context_check.as_ref()?;
        if context.result.is_err() {
            return None;
        }
        let refs = context.references;
        if refs.host != Some(environment_digest) || refs.starting != Some(starting_digest) {
            return None;
        }
        let peers = self.peer_check.as_ref()?;
        if peers.result.is_err()
            || peers.context != Some(refs)
            || peers.starting_response != refs.starting_response
        {
            return None;
        }
        let consent = self.consent.as_ref()?;
        if consent.assessment.result.is_err()
            || consent.assessment.references != Some(refs)
            || consent.assessment.captures.iter().any(Option::is_none)
        {
            return None;
        }
        let process_frame = self.native_grant_process_frame.as_ref()?;
        let process = process_frame.native_grant_commitment()?;
        let (first, last) = self.native_grant_observations?;
        let last = last?;
        if !grant_observation_eligible(first, self.binding, self.setup_deadline)
            || !grant_observation_eligible(last, self.binding, self.setup_deadline)
            || process_frame.before != first.anchor
            || process_frame.after != last.anchor
        {
            return None;
        }
        let context_value = grant_object([
            ("references", grant_references_value(refs)),
            ("environment", grant_digest_value(environment_digest)),
            ("starting", grant_digest_value(starting_digest)),
            (
                "starting_current",
                grant_object([
                    (
                        "power_limit_mw",
                        Value::U64(starting_current.power_limit_mw.into()),
                    ),
                    (
                        "graphics_offset_p0_mhz",
                        Value::I64(starting_current.graphics_offset_p0_mhz.into()),
                    ),
                    (
                        "memory_offset_p0_mhz",
                        Value::I64(starting_current.memory_offset_p0_mhz.into()),
                    ),
                ]),
            ),
        ]);
        let peer_value = grant_object([
            ("context", context_value.clone()),
            (
                "starting_response",
                refs.starting_response
                    .map_or(Value::Null, grant_digest_value),
            ),
            (
                "peers",
                Value::Array(
                    peers
                        .peers
                        .map(|peer| {
                            let received = peer.received.ok()?;
                            Some(grant_object([
                                ("expected", grant_origin_value(peer.expected)),
                                ("received", grant_origin_value(received)),
                                ("cancelled", Value::Bool(peer.cancelled)),
                            ]))
                        })
                        .into_iter()
                        .collect::<Option<Vec<_>>>()?,
                ),
            ),
        ]);
        let captures = consent.assessment.captures.map(|capture| {
            capture.map(|capture| {
                grant_object([
                    ("request", grant_digest_value(capture.request)),
                    ("receipt", grant_anchor_value(capture.receipt)),
                ])
            })
        });
        let consent_value = grant_object([
            ("context", context_value.clone()),
            ("starting", captures[0].clone().unwrap_or(Value::Null)),
            ("setup", captures[1].clone().unwrap_or(Value::Null)),
            ("workload", captures[2].clone().unwrap_or(Value::Null)),
            (
                "setup_request",
                grant_digest_value(consent.decisions.setup.request().digest()),
            ),
            (
                "workload_request",
                grant_digest_value(consent.decisions.workload.request().digest()),
            ),
            ("now", grant_anchor_value(consent.assessment.now)),
        ]);
        let safety_value = Value::Array(
            [first, last]
                .into_iter()
                .map(grant_observation_value)
                .collect::<Option<Vec<_>>>()?,
        );
        Some(NativeGrantCommitments {
            setup: refs.worker_setup,
            context: grant_value_digest(context_value),
            peers: grant_value_digest(peer_value),
            consent: grant_value_digest(consent_value),
            process,
            safety: grant_value_digest(safety_value),
        })
    }
    /// Queue a separate NativeGrant exactly once on the worker's original
    /// ordinary-channel sequence. Every pre-queue boundary is fresh and bound
    /// to the original setup deadline; failed/uncertain frames remain owned.
    #[cfg(windows)]
    pub fn queue_native_grant<Q: Queue>(
        &mut self,
        clock: &crate::monitor::windows_clock::WindowsClock,
        out: &mut Sender<Q>,
    ) -> Result<(), Fault> {
        if self.native_grant_attempt.is_some() {
            self.worker_start_tx.poison();
            self.fail(Fault::Protocol, self.now_ns);
            return Err(Fault::Protocol);
        }
        let mut attempt = NativeGrantAttempt {
            observations: [None; 3],
            checks: [None; 3],
            codec_error: None,
            send_error: None,
            queued: false,
            result: Err(Fault::Context),
        };
        self.native_grant_attempt = Some(attempt);
        let result = (|| {
            let _scratch = self
                .budget
                .reserve(64 * 1024)
                .map_err(|_| Fault::Capacity)?;
            if self.unsent_worker_start.is_some() || self.unsent_native_grant.is_some() {
                return Err(Fault::Protocol);
            }
            let issued_observation = self.observe_start(clock);
            attempt.observations[0] = Some(issued_observation);
            let issued_check =
                grant_observation_check(issued_observation, self.binding, self.setup_deadline);
            attempt.checks[0] = Some(issued_check);
            issued_check?;
            let process_after = self
                .native_grant_process_frame
                .as_ref()
                .and_then(|frame| frame.after)
                .ok_or(Fault::Context)?;
            let process_age = self
                .binding
                .elapsed(issued_observation.anchor.ok_or(Fault::Clock)?)
                .ok()
                .and_then(|issued_ns| {
                    self.binding
                        .elapsed(process_after)
                        .ok()
                        .and_then(|process_ns| issued_ns.checked_sub(process_ns))
                })
                .ok_or(Fault::Clock)?;
            if process_age
                >= autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .observation_gap_ns
            {
                return Err(Fault::Context);
            }
            let mut commitments = self
                .derive_native_grant_commitments()
                .ok_or(Fault::Context)?;
            let issued_value = grant_observation_value(issued_observation).ok_or(Fault::Context)?;
            commitments.safety = grant_value_digest(grant_object([
                ("prior", grant_digest_value(commitments.safety)),
                ("issued", issued_value),
            ]));
            let issued = issued_observation.anchor.ok_or(Fault::Clock)?;
            let message = Message::NativeGrant {
                issued,
                setup: commitments.setup,
                context: commitments.context,
                peers: commitments.peers,
                consent: commitments.consent,
                process: commitments.process,
                safety: commitments.safety,
            };
            let frame = match self.worker_start_tx.encode(&message) {
                Ok(frame) => frame,
                Err(error) => {
                    attempt.codec_error = Some(error);
                    return Err(Fault::Transport);
                }
            };
            self.unsent_native_grant = Some(frame);
            let retained_wire = self
                .unsent_native_grant
                .as_ref()
                .expect("retained grant before send")
                .as_bytes();
            self.native_grant_wire = Some(NativeGrantWire::copy(retained_wire, &self.budget)?);
            let before_send = self.observe_start(clock);
            attempt.observations[1] = Some(before_send);
            let before_check =
                grant_observation_check(before_send, self.binding, self.setup_deadline);
            attempt.checks[1] = Some(before_check);
            before_check?;
            let issue_ns = self.binding.elapsed(issued).map_err(|_| Fault::Clock)?;
            let send_ns = self
                .binding
                .elapsed(before_send.anchor.ok_or(Fault::Clock)?)
                .map_err(|_| Fault::Clock)?;
            if send_ns.checked_sub(issue_ns).is_none_or(|age| {
                age >= autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .observation_gap_ns
            }) {
                return Err(Fault::Context);
            }
            let frame = self
                .unsent_native_grant
                .take()
                .expect("retained grant frame");
            match out.send_worker(frame) {
                Ok(()) => attempt.queued = true,
                Err(frame) => {
                    self.unsent_native_grant = Some(frame);
                    attempt.send_error =
                        Some(out.fault(Lane::Ordinary).unwrap_or(SendFailure::Queue));
                    self.worker_start_tx.poison();
                    return Err(Fault::Transport);
                }
            }
            self.notify_watch();
            let after_send = self.observe_start(clock);
            attempt.observations[2] = Some(after_send);
            let after_check =
                grant_observation_check(after_send, self.binding, self.setup_deadline);
            attempt.checks[2] = Some(after_check);
            after_check?;
            if self.watch_error.is_some() || self.fault.is_some() {
                return Err(Fault::Supervision);
            }
            Ok(())
        })();
        attempt.result = result;
        if result.is_err() {
            self.worker_start_tx.poison();
            let fault = result.expect_err("checked grant failure");
            let trigger = if fault == Fault::SetupDeadline {
                self.setup_deadline
            } else {
                self.now_ns
            };
            self.fail(fault, trigger);
        }
        self.native_grant_attempt = Some(attempt);
        result
    }
    /// Collect the three retained process owners and queue their matching
    /// grant in one ordered call. Process-handle queries can block: callers
    /// must run this on the retained collector lane, outside deadline control.
    #[cfg(windows)]
    pub fn collect_and_queue_native_grant<Q: Queue>(
        &mut self,
        input: super::process_state::Processes<'_>,
        clock: &crate::monitor::windows_clock::WindowsClock,
        out: &mut Sender<Q>,
    ) -> Result<(), Fault> {
        if self.observe_native_grant_processes(input, clock).is_err() {
            return Err(self.fault.unwrap_or(Fault::Context));
        }
        self.queue_native_grant(clock, out)
    }
    pub fn consent(&self) -> Option<&super::consent::Delivery> {
        self.consent.as_ref()
    }
    /// Retain the first setup/workload owners even on failure. A duplicate
    /// returns its original owners; no new heap allocation is needed to reject.
    #[allow(clippy::result_large_err)]
    pub fn accept_consent(
        &mut self,
        decisions: super::consent::Decisions,
        input: super::consent::ParentInputs<'_>,
    ) -> Result<(), super::consent::Rejected> {
        use super::consent::{self, ParentFault, Rejected};
        if self.consent.is_some() {
            input.setup.close_reference_path();
            self.fail(Fault::Protocol, self.now_ns);
            return Err(Rejected {
                fault: ParentFault::Duplicate,
                returned: Some(decisions),
            });
        }
        self.tick(Some(input.now));
        let assessment = consent::assess_parent(self, &decisions, &input);
        let result = assessment.result;
        self.consent = Some(consent::Delivery {
            decisions,
            assessment,
        });
        if let Err(fault) = result {
            input.setup.close_reference_path();
            self.fail(Fault::Context, self.now_ns);
            return Err(Rejected {
                fault,
                returned: None,
            });
        }
        Ok(())
    }
    pub(super) fn consent_window(&self) -> (Anchor, u64) {
        (self.setup_entry, self.setup_deadline)
    }
    pub fn peer_check(&self) -> Option<&super::parent_peers::Record> {
        self.peer_check.as_ref()
    }
    pub fn origin(&self) -> Result<super::launch_origin::Origin, super::launch_origin::Fault> {
        self.origins.result()
    }
    pub(super) fn context_binding(&self) -> WorkerBinding {
        self.binding
    }
    pub(super) fn context_open(&self) -> bool {
        self.fault.is_none() && !self.finishing && !self.reported_finished
    }
    pub fn context_check(&self) -> Option<&super::parent_context::Record> {
        self.context_check.as_ref()
    }
    /// One reported-reference check only. No peer/consent/safety authority or
    /// native admission follows; raw artifacts remain owned by the caller.
    pub fn check_context(
        &mut self,
        input: super::parent_context::Inputs<'_, '_>,
    ) -> Result<super::parent_context::Summary, super::parent_context::Rejection> {
        if self.context_check.is_some() {
            input.worker.close_reference_path();
            self.fail(Fault::Protocol, self.now_ns);
            return Err(super::parent_context::Rejection {
                part: super::parent_context::Part::Duplicate,
                cause: super::parent_context::Cause::Binding,
            });
        }
        let record = super::parent_context::screen(self, input);
        let result = record.result;
        self.context_check = Some(record);
        if result.is_err() {
            self.fail(Fault::Context, self.now_ns);
        }
        result
    }
    pub fn environment_check(
        &self,
    ) -> Option<(
        crate::evidence::Digest,
        Result<(), super::host_environment::Fault>,
    )> {
        self.environment_check
    }
    /// Retains the reported preflight artifact check. Success authenticates no
    /// peer, native reads or consent and cannot issue workload admission.
    pub fn check_environment(
        &mut self,
        environment: &super::host_environment::Reported,
        run: &BoundRun<'_>,
        epoch: Anchor,
    ) -> Result<(), Fault> {
        if self.environment_check.is_some() {
            self.fail(Fault::Protocol, self.now_ns);
            return Err(Fault::Protocol);
        }
        let result = if WorkerBinding::new(run, self.binding.monitor.qpc_frequency, epoch).ok()
            != Some(self.binding)
        {
            Err(super::host_environment::Fault::Binding)
        } else {
            environment
                .snapshot()
                .check_run(run, self.binding.monitor.qpc_frequency, epoch)
        };
        self.environment_check = Some((environment.snapshot().digest(), result));
        result.map_err(|_| {
            self.fail(Fault::Environment, self.now_ns);
            Fault::Environment
        })
    }
    pub fn starting_check(
        &self,
    ) -> Option<(crate::evidence::Digest, Result<settings::Current, Fault>)> {
        self.starting_check
    }
    /// A reference/equality check of reported artifacts, not a native permit.
    /// The caller retains the complete starting artifact and peer/consent proof.
    pub fn check_starting(
        &mut self,
        starting: &super::starting::ReportedSnapshot,
        run: &BoundRun<'_>,
        epoch: Anchor,
    ) -> Result<settings::Current, Fault> {
        if self.starting_check.is_some() {
            self.fail(Fault::Protocol, self.now_ns);
            return Err(Fault::Protocol);
        }
        let result = (|| {
            if WorkerBinding::new(run, self.binding.monitor.qpc_frequency, epoch).ok()
                != Some(self.binding)
            {
                return Err(Fault::Binding);
            }
            let expected = starting
                .check_run(run, self.binding.monitor.qpc_frequency, epoch)
                .map_err(|_| Fault::Binding)?;
            let current = self
                .current
                .as_ref()
                .and_then(|r| r.result().ok())
                .ok_or(Fault::Settings)?
                .capture()
                .current()
                .map_err(|_| Fault::Settings)?;
            if expected != current {
                return Err(Fault::Settings);
            }
            Ok(current)
        })();
        self.starting_check = Some((starting.snapshot().digest(), result));
        if let Err(e) = result {
            self.fail(e, self.now_ns);
        }
        result
    }
    pub fn ready(&self) -> Option<&WorkerReceived> {
        self.ready.as_ref()
    }
    pub fn runtime(&self) -> Option<&Runtime> {
        self.runtime.as_ref()
    }
    pub fn current(&self) -> Option<&SettingsReceived> {
        self.current.as_ref()
    }
    pub fn rejected(&self) -> Option<&WorkerReceived> {
        self.rejected.as_ref()
    }
    pub fn pending(&self) -> Option<(Ticket, Anchor)> {
        self.pending
    }
    pub fn unsent(&self) -> Option<&EncodedFrame> {
        self.unsent.as_ref()
    }
    pub fn unsent_settings(&self) -> Option<&Frame> {
        self.unsent_settings.as_ref()
    }
    pub fn unsent_stop(&self) -> Option<&EncodedFrame> {
        self.unsent_stop.as_ref()
    }
    pub fn send_error(&self) -> Option<SendFailure> {
        self.send_error
    }
    pub fn codec_error(&self) -> Option<WireError> {
        self.codec_error
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn stop(&self) -> Option<StopWindow> {
        self.stop
    }
    pub fn stop_event(&self) -> Option<Result<(), Option<i32>>> {
        self.stop_event
    }
    pub fn reported_finished(&self) -> bool {
        self.reported_finished
    }
    #[cfg(windows)]
    pub(super) fn is_finishing(&self) -> bool {
        self.finishing
    }
    /// The control pool must already be partitioned from this exact root. Native
    /// source/peer/runtime/environment/consent validation remains separate.
    pub fn new(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        setup_entry: Anchor,
        first: Anchor,
        budget: Budget,
        control_budget: Budget,
    ) -> Result<Self, Fault> {
        let check = control_budget.reserve(0).map_err(|_| Fault::Capacity)?;
        if !check.is_within(&budget) || check.belongs_to(&budget) {
            return Err(Fault::Binding);
        }
        drop(check);
        let binding = WorkerBinding::new(run, frequency, epoch).map_err(|_| Fault::Binding)?;
        #[cfg(windows)]
        let binding_digest = {
            let _codec = budget
                .reserve(64 * 1024 * 16)
                .map_err(|_| Fault::Capacity)?;
            binding.transport_digest()
        };
        let entry = binding.elapsed(setup_entry).map_err(|_| Fault::Clock)?;
        let n = binding.elapsed(first).map_err(|_| Fault::Clock)?;
        if entry < binding.origin_ns || n < entry || setup_entry.qpc_after > first.qpc_before {
            return Err(Fault::Clock);
        }
        let setup_deadline = entry
            .checked_add(binding.plan.phase_caps_ns[1])
            .ok_or(Fault::SetupDeadline)?
            .min(binding.setup_boundary());
        if n >= setup_deadline {
            return Err(Fault::SetupDeadline);
        }
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let capacity = ((binding.plan.envelope_ns / p.cadence_ns + 1) * 6) as usize;
        let bytes = capacity * std::mem::size_of::<Receipt>()
            + 4 * std::mem::size_of::<WorkerReceived>()
            + 64 * 1024
            + std::mem::size_of::<Self>();
        if bytes > p.metadata_bytes as usize {
            return Err(Fault::Capacity);
        }
        let metadata = budget.reserve(bytes).map_err(|_| Fault::Capacity)?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(capacity)
            .map_err(|_| Fault::Capacity)?;
        let mut controls = Vec::new();
        controls.try_reserve_exact(4).map_err(|_| Fault::Capacity)?;
        let history = EntryClock::with_capacity(
            frequency,
            epoch,
            first,
            (2 * binding.plan.envelope_ns / p.controller_tick_ns + capacity as u64 + 128) as usize,
            &budget,
        )
        .map_err(|_| Fault::Clock)?;
        let monitor = Controller::for_pilot_run(
            binding.monitor,
            epoch,
            first,
            binding.origin_ns,
            binding.run_end(),
            &budget,
        )
        .map_err(|_| Fault::Capacity)?;
        let mut result = Self {
            binding,
            #[cfg(windows)]
            binding_digest,
            expected_uuid: run.raw_uuid().into(),
            expected_pci: run.raw_pci().into(),
            setup_entry,
            setup_deadline,
            now_ns: n,
            history,
            monitor,
            tx: Channel::sampler(binding, Lane::Ordinary, Direction::ToWorker, budget.clone())
                .map_err(|_| Fault::Capacity)?,
            rx: Channel::sampler(
                binding,
                Lane::Ordinary,
                Direction::FromWorker,
                budget.clone(),
            )
            .map_err(|_| Fault::Capacity)?,
            control_tx: Channel::sampler(
                binding,
                Lane::Control,
                Direction::ToWorker,
                control_budget.clone(),
            )
            .map_err(|_| Fault::Capacity)?,
            control_rx: Channel::sampler(
                binding,
                Lane::Control,
                Direction::FromWorker,
                control_budget,
            )
            .map_err(|_| Fault::Capacity)?,
            settings: Parent::new(run, frequency, epoch, setup_entry, budget.clone())
                .map_err(|_| Fault::Settings)?,
            ready: None,
            runtime: None,
            current: None,
            records,
            controls,
            capacity,
            rejected: None,
            pending: None,
            unsent: None,
            unsent_settings: None,
            unsent_stop: None,
            send_error: None,
            codec_error: None,
            fault: None,
            stop: None,
            stop_event: None,
            reentry: Reentry::default(),
            setup_done: false,
            settings_issued: false,
            stop_attempted: false,
            finishing: false,
            reported_finished: false,
            miss_count: 0,
            starting_check: None,
            environment_check: None,
            context_check: None,
            origins: super::launch_origin::Chain::default(),
            expected_origin: None,
            peer_check: None,
            consent: None,
            process_frame: None,
            #[cfg(windows)]
            native_grant_process_frame: None,
            #[cfg(windows)]
            native_grant_observations: None,
            #[cfg(windows)]
            native_grant_attempt: None,
            #[cfg(windows)]
            native_grant_wire: None,
            start_attempt: None,
            #[cfg(windows)]
            watch: None,
            watch_error: None,
            #[cfg(windows)]
            worker_start_tx: Channel::new(
                binding,
                Lane::Ordinary,
                Direction::ToWorker,
                budget.clone(),
            )
            .map_err(|_| Fault::Capacity)?,
            unsent_worker_start: None,
            #[cfg(windows)]
            unsent_native_grant: None,
            budget,
            _metadata: metadata,
        };
        if result.monitor.report().stop().is_some() {
            result.fail(Fault::Monitor, result.monitor.report().now_ns());
        }
        Ok(result)
    }
    pub fn report(&self) -> &crate::monitor::Report {
        self.monitor.report()
    }
    pub fn records(&self) -> &[Receipt] {
        &self.records
    }
    pub fn controls(&self) -> &[WorkerReceived] {
        &self.controls
    }
    pub fn history(&self) -> &EntryClock {
        &self.history
    }
    pub fn reentry(&self) -> &Reentry {
        &self.reentry
    }
    pub fn native_admission_available(&self) -> bool {
        false
    }
    fn fail(&mut self, f: Fault, trigger: u64) {
        self.fault.get_or_insert(f);
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        self.stop.get_or_insert(StopWindow {
            trigger_ns: trigger,
            cooperative_ns: trigger
                .saturating_add(p.cooperative_stop_ns)
                .min(self.binding.run_end()),
            exit_ns: trigger
                .saturating_add(p.exit_observation_ns)
                .min(self.binding.run_end()),
        });
        #[cfg(windows)]
        if let (Some(watch), Some(window)) = (&self.watch, self.stop) {
            watch.stop(window);
        }
    }
    fn observe(&mut self, a: Option<Anchor>) -> Result<u64, Fault> {
        let n = self.history.observe(a).map_err(|_| Fault::Clock);
        let n = match n {
            Ok(n) => n,
            Err(f) => {
                self.fail(f, self.now_ns);
                return Err(f);
            }
        };
        self.now_ns = n;
        if n >= self.binding.run_end() {
            self.fail(Fault::RunDeadline, self.binding.run_end());
        } else if !self.setup_done && n >= self.setup_deadline {
            self.fail(Fault::SetupDeadline, self.setup_deadline);
        } else if !self.reentry.ready()
            && n >= self.binding.origin_ns + self.binding.plan.phase_caps_ns[0]
        {
            self.fail(
                Fault::ReentryDeadline,
                self.binding.origin_ns + self.binding.plan.phase_caps_ns[0],
            );
        }
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        if let Some((ticket, _)) = self.pending {
            let deadline = ticket
                .start_ns
                .saturating_add(p.query_bracket_ns)
                .saturating_add(p.transport_age_ns);
            if n > deadline {
                self.fail(Fault::Telemetry, deadline);
            }
        }
        if self.finishing && !self.reported_finished {
            if let Some(stop) = self.stop {
                if n >= stop.cooperative_ns {
                    self.fail(Fault::ShutdownDeadline, stop.cooperative_ns);
                }
            }
        }
        Ok(n)
    }
    fn monitor_fault(&mut self) {
        let report = self.monitor.report();
        if report
            .missed_from(self.miss_count)
            .any(|r| matches!(r.metric, Metric::Temperature | Metric::GpuUtilization))
        {
            self.reentry.consecutive = 0;
            self.reentry.last = None;
            self.reentry.latest = None;
            self.reentry.pair = [None; 2];
        }
        self.miss_count = report.capacity().retained_missed_ranges;
        if let Some(stop) = self.monitor.report().stop() {
            if self.finishing && stop.reason == crate::monitor::StopReason::Requested {
                return;
            }
            self.fail(Fault::Monitor, stop.at_ns);
        }
    }
    fn control_time_valid(&self, anchor: Anchor, receipt: Option<Anchor>) -> bool {
        let (Some(receipt), Some(stop)) = (receipt, self.stop) else {
            return false;
        };
        let Ok(mut clock) =
            crate::monitor::clock::Clock::new(self.binding.monitor, self.binding.epoch)
        else {
            return false;
        };
        let Ok(n) = clock.observe(anchor) else {
            return false;
        };
        let Ok(r) = clock.observe(receipt) else {
            return false;
        };
        anchor.qpc_after <= receipt.qpc_before && n >= stop.trigger_ns && r < stop.cooperative_ns
    }
    pub fn tick(&mut self, a: Option<Anchor>) {
        if self.observe(a).is_ok() {
            if let Some(a) = a {
                self.monitor.poll(a);
                self.monitor_fault();
            }
        }
        #[cfg(windows)]
        self.notify_watch();
    }
    /// No Start/permit is returned. This screen concerns reported sampler data;
    /// a later parent gate must prove peer/environment/consent and fresh timing.
    pub fn reported_screen(&mut self, a: Option<Anchor>) -> bool {
        self.tick(a);
        self.reported_ready()
    }
    fn reported_ready(&self) -> bool {
        self.fault.is_none()
            && !self.finishing
            && self.setup_done
            && self.reentry.ready()
            && self.reentry.fresh(self.now_ns)
            && self.monitor.report().fixture_compute_eligible()
            && !self.monitor.report().baseline_insufficient()[..5]
                .iter()
                .any(|v| *v)
    }
    pub fn issue_settings<Q: Queue>(
        &mut self,
        a: Anchor,
        out: &mut Sender<Q>,
    ) -> Result<(), Fault> {
        self.tick(Some(a));
        if self.fault.is_some() || self.runtime.is_none() {
            return Err(self.fault.unwrap_or(Fault::Protocol));
        }
        let frame = self.settings.issue(a).map_err(|_| {
            self.fail(Fault::Settings, self.now_ns);
            Fault::Settings
        })?;
        self.settings_issued = true;
        if let Err(frame) = out.send_settings(frame) {
            self.unsent_settings = Some(frame);
            self.send_error = out.fault(Lane::Ordinary);
            self.fail(Fault::Transport, self.now_ns);
            return Err(Fault::Transport);
        }
        Ok(())
    }
    pub fn next_query<Q: Queue>(&mut self, a: Anchor, out: &mut Sender<Q>) -> Result<bool, Fault> {
        self.observe(Some(a))?;
        if self.finishing {
            self.monitor.poll(a);
            self.monitor_fault();
            return self.fault.map_or(Ok(false), Err);
        }
        if self.fault.is_some() || self.runtime.is_none() {
            self.monitor.poll(a);
            self.monitor_fault();
            return self.fault.map_or(Ok(false), Err);
        }
        let Some(ticket) = self.monitor.begin(a) else {
            self.monitor_fault();
            return self.fault.map_or(Ok(false), Err);
        };
        self.pending = Some((ticket, a));
        let frame = self
            .tx
            .encode(&Message::SamplerQuery { ticket, issued: a })
            .map_err(|e| {
                self.codec_error = Some(e);
                self.fail(Fault::Transport, self.now_ns);
                Fault::Transport
            })?;
        if let Err(frame) = out.send_worker(frame) {
            self.unsent = Some(frame);
            self.send_error = out.fault(Lane::Ordinary);
            self.fail(Fault::Transport, self.now_ns);
            return Err(Fault::Transport);
        }
        Ok(true)
    }
    #[allow(clippy::result_large_err)]
    pub fn receive(&mut self, run: &BoundRun<'_>, raw: RawReceived) -> Result<(), RawReceived> {
        let result = self.receive_inner(run, raw);
        #[cfg(windows)]
        self.notify_watch();
        result
    }
    #[allow(clippy::result_large_err)] // Return the original prepaid packet owner.
    fn receive_inner(&mut self, run: &BoundRun<'_>, raw: RawReceived) -> Result<(), RawReceived> {
        self.origins.observe(raw.origin());
        if self
            .expected_origin
            .is_some_and(|o| raw.origin() != Some(o))
        {
            self.fail(Fault::Binding, self.now_ns);
            return Err(raw);
        }
        let receipt = raw.receipt();
        let _ = self.observe(receipt);
        if raw.lane() == Lane::Control {
            if self.controls.len() == 4 {
                self.fail(Fault::Capacity, self.now_ns);
                return Err(raw);
            }
            let received = raw.decode_worker(&mut self.control_rx);
            self.controls.push(received);
            let message = self
                .controls
                .last()
                .expect("retained control")
                .message()
                .cloned();
            if let Some(a) = receipt {
                self.monitor.poll(a);
                self.monitor_fault();
            }
            if self.finishing && self.fault.is_none() {
                match message {
                    Ok(Message::StopAck { anchor, .. })
                        if self.control_time_valid(anchor, receipt) => {}
                    Ok(Message::RoleFinished {
                        anchor,
                        pending: false,
                        cleanup_attempted: true,
                        cleanup_status: Some(0),
                    }) if !self.reported_finished && self.control_time_valid(anchor, receipt) => {
                        self.reported_finished = true
                    }
                    _ => self.fail(Fault::Protocol, self.now_ns),
                }
            } else {
                self.fail(Fault::Stopped, self.now_ns);
            }
            return Ok(());
        }
        match route(raw.packet().bytes(), &self.budget) {
            Ok(Route::Settings) => {
                if self.current.is_some() {
                    self.fail(Fault::Protocol, self.now_ns);
                    return Err(raw);
                }
                self.current = Some(raw.decode_settings(&mut self.settings, run));
                let valid = self
                    .current
                    .as_ref()
                    .and_then(|r| r.result().ok())
                    .is_some_and(|r| {
                        r.fault().is_none()
                            && r.capture().reported_source() == settings::Source::NativeGetterPath
                            && r.capture().current().is_ok()
                    });
                if !valid {
                    self.fail(Fault::Settings, self.now_ns);
                }
                self.setup_done = valid && self.runtime.is_some();
                if let Some(a) = receipt {
                    self.monitor.poll(a);
                    self.monitor_fault();
                }
                Ok(())
            }
            Ok(Route::Telemetry) => {
                if self.rejected.is_some() {
                    self.fail(Fault::Capacity, self.now_ns);
                    return Err(raw);
                }
                let received = raw.decode_worker(&mut self.rx);
                if received.binding() != Ok(self.binding) {
                    self.rejected = Some(received);
                    self.fail(Fault::Binding, self.now_ns);
                    return Ok(());
                }
                if matches!(received.message(), Ok(Message::SamplerReady { .. })) {
                    if self.ready.is_some() {
                        self.rejected = Some(received);
                        self.fail(Fault::Protocol, self.now_ns);
                        return Ok(());
                    }
                    self.ready = Some(received);
                    let valid = (|| {
                        let ready = self.ready.as_ref().expect("retained");
                        let (before, after, uuid, pci, runtime) = match ready.message() {
                            Ok(Message::SamplerReady {
                                before,
                                after,
                                raw_uuid,
                                raw_pci,
                                runtime,
                                ..
                            }) => (*before, *after, raw_uuid, raw_pci, runtime),
                            _ => return Err(Fault::Protocol),
                        };
                        if uuid != &self.expected_uuid
                            || pci != &self.expected_pci
                            || before.qpc_before < self.setup_entry.qpc_after
                            || self.binding.elapsed(before).map_err(|_| Fault::Clock)?
                                < self
                                    .binding
                                    .elapsed(self.setup_entry)
                                    .map_err(|_| Fault::Clock)?
                            || self.binding.elapsed(after).map_err(|_| Fault::Clock)?
                                >= self.setup_deadline
                            || receipt.is_none_or(|a| after.qpc_after > a.qpc_before)
                        {
                            return Err(Fault::Binding);
                        }
                        crate::sampler::wire::validate_complexity_limit(runtime.as_bytes(), 256)
                            .map_err(|_| Fault::Runtime)?;
                        let _codec = self
                            .budget
                            .reserve(64 * 1024 * 16)
                            .map_err(|_| Fault::Capacity)?;
                        let value: Runtime =
                            serde_json::from_str(runtime).map_err(|_| Fault::Runtime)?;
                        if serde_json::to_string(&value).ok().as_deref() != Some(runtime)
                            || !runtime_valid(&value)
                        {
                            return Err(Fault::Runtime);
                        }
                        Ok(value)
                    })();
                    match valid {
                        Ok(runtime) => self.runtime = Some(runtime),
                        Err(e) => self.fail(e, self.now_ns),
                    }
                    if let Some(a) = receipt {
                        self.monitor.poll(a);
                        self.monitor_fault();
                    }
                    return Ok(());
                }
                if self.records.len() == self.capacity {
                    self.rejected = Some(received);
                    self.fail(Fault::Capacity, self.now_ns);
                    return Ok(());
                }
                let pending = self.pending;
                let assessed = if let Some((ticket, issued)) = pending {
                    Receipt::assess(self.binding, ticket, issued, received)
                } else {
                    Receipt {
                        raw: received,
                        record: Err(telemetry::Error::Protocol),
                    }
                };
                self.records.push(assessed);
                let result = self.records.last().expect("retained").record;
                match (result, receipt) {
                    (Ok(record), Some(a)) => {
                        self.monitor.complete(a, record);
                        if self.monitor.report().pending().is_none() {
                            self.pending = None;
                        }
                        self.monitor_fault();
                        if self.fault.is_none() {
                            self.reentry.accept(record);
                        }
                    }
                    _ => {
                        self.fail(Fault::Telemetry, self.now_ns);
                        if let Some(a) = receipt {
                            self.monitor.request_stop(a);
                        }
                    }
                }
                Ok(())
            }
            Err(_) => {
                self.fail(Fault::Protocol, self.now_ns);
                Err(raw)
            }
        }
    }
    pub fn queue_stop<Q: Queue>(&mut self, out: &mut Sender<Q>) {
        let Some(w) = self.stop else {
            return;
        };
        if self.stop_attempted {
            return;
        }
        self.stop_attempted = true;
        let message = Message::Stop {
            reason: Reason::new("pilot-sampler-parent-stop").expect("fixed reason"),
            trigger_ns: w.trigger_ns,
            cooperative_ns: w.cooperative_ns,
            exit_ns: w.exit_ns,
        };
        match self.control_tx.encode(&message) {
            Ok(frame) => {
                if let Err(frame) = out.send_worker(frame) {
                    self.unsent_stop = Some(frame);
                    self.send_error = out.fault(Lane::Control);
                    self.fail(Fault::Transport, self.now_ns);
                }
            }
            Err(e) => {
                self.codec_error = Some(e);
                self.fail(Fault::Transport, self.now_ns);
            }
        }
    }
    /// Planned sampler shutdown after the outer controller has ended compute.
    /// Does not prove worker completion, process exit or native quiescence.
    pub fn finish<Q: Queue>(
        &mut self,
        a: Option<Anchor>,
        out: &mut Sender<Q>,
    ) -> Result<(), Fault> {
        let n = self.observe(a)?;
        if let Some(f) = self.fault {
            return Err(f);
        }
        if self.pending.is_some() {
            self.fail(Fault::Stopped, n);
            return Err(Fault::Stopped);
        }
        self.finishing = true;
        if let Some(a) = a {
            self.monitor.request_stop(a);
        }
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        self.stop.get_or_insert(StopWindow {
            trigger_ns: n,
            cooperative_ns: n
                .saturating_add(p.cooperative_stop_ns)
                .min(self.binding.run_end()),
            exit_ns: n
                .saturating_add(p.exit_observation_ns)
                .min(self.binding.run_end()),
        });
        #[cfg(windows)]
        self.notify_watch();
        self.queue_stop(out);
        self.fault.map_or(Ok(()), Err)
    }
}
#[cfg(windows)]
pub struct Poll {
    pub anchor: Option<Anchor>,
    pub next_anchor: Option<Anchor>,
    pub inputs_finished: Option<bool>,
    pub os_code: Option<i32>,
    pub transport_fault: bool,
    pub rejected: Option<RawReceived>,
}
#[cfg(windows)]
struct StartSource<'a, 'p, Q: Queue> {
    parent: &'a mut ParentSampler,
    input: super::process_state::Processes<'p>,
    clock: &'a crate::monitor::windows_clock::WindowsClock,
    out: &'a mut Sender<Q>,
}
#[cfg(windows)]
impl<Q: Queue> super::parent_start::Source for StartSource<'_, '_, Q> {
    fn observe(&mut self) -> super::parent_start::Observation {
        self.parent.observe_start(self.clock)
    }
    fn cached_process(&self) -> bool {
        self.parent.process_frame.is_some()
    }
    fn collect(&mut self) -> Result<(), super::process_state::Fault> {
        self.parent.observe_processes(
            super::process_state::Processes {
                starting: self.input.starting,
                sampler: self.input.sampler,
                worker: self.input.worker,
                setup: &mut *self.input.setup,
            },
            self.clock,
        )
    }
    fn encode(&mut self, issued: Anchor) -> Result<(), WireError> {
        let frame = self
            .parent
            .worker_start_tx
            .encode(&Message::Start { issued })?;
        self.parent.unsent_worker_start = Some(frame);
        Ok(())
    }
    fn send(&mut self) -> Result<(), SendFailure> {
        let frame = self
            .parent
            .unsent_worker_start
            .take()
            .expect("encoded Start");
        match self.out.send_worker(frame) {
            Ok(()) => Ok(()),
            Err(frame) => {
                self.parent.unsent_worker_start = Some(frame);
                Err(self.out.fault(Lane::Ordinary).unwrap_or(SendFailure::Queue))
            }
        }
    }
}
#[cfg(windows)]
impl ParentSampler {
    fn watch_window(&self) -> super::parent_watch::Window {
        super::parent_watch::Window {
            binding: self.binding,
            setup_deadline_ns: self.setup_deadline,
        }
    }
    fn watch_peers(&self) -> Option<[super::launch_origin::Origin; 2]> {
        self.peer_check
            .as_ref()
            .filter(|p| p.result.is_ok())
            .map(|p| [p.peers[1].expected, p.peers[2].expected])
    }
    /// Attach exactly one actual controller publisher. Caller retains Handle,
    /// report and operation threads through finalization; attachment/ready is
    /// not a native permit. Poll readiness before the one-shot Start attempt.
    pub fn attach_watch(
        &mut self,
        handle: &super::parent_watch_windows::Handle,
    ) -> Result<(), super::parent_watch::Fault> {
        if self.watch.is_some() || !self.context_open() {
            self.watch_error
                .get_or_insert(super::parent_watch::Fault::Binding);
            self.fail(Fault::Supervision, self.now_ns);
            return Err(super::parent_watch::Fault::Binding);
        }
        match handle.publisher(self.watch_window()) {
            Ok(publisher) => self.watch = Some(publisher),
            Err(f) => {
                self.watch_error.get_or_insert(f);
                self.fail(Fault::Supervision, self.now_ns);
                return Err(f);
            }
        }
        self.notify_watch();
        self.watch_error.map_or(Ok(()), Err)
    }
    pub fn watch_ready(&self) -> bool {
        self.watch_error.is_none()
            && self
                .watch
                .as_ref()
                .is_some_and(|w| w.ready(self.watch_window(), self.watch_peers()))
    }
    pub fn watch_status(&self) -> Option<super::parent_watch_publication::Status> {
        self.watch.as_ref().map(|w| w.status())
    }
    fn notify_watch(&mut self) {
        let facts = super::parent_watch_publication::Facts {
            window: self.watch_window(),
            anchor: self.history.raw().last().copied(),
            open: self.context_open(),
            stop: self.stop,
            planned_stop: self.finishing && self.fault.is_none(),
            start_ok: self
                .start_attempt
                .as_ref()
                .map(|a| a.queued && a.result.is_ok()),
            peers: self.watch_peers(),
            safety: *self.report().safety(),
        };
        if let Some(Err(f)) = self.watch.as_mut().map(|w| w.publish(facts)) {
            self.watch_error.get_or_insert(f);
            self.fail(Fault::Supervision, self.now_ns);
        }
    }
    fn observe_start(
        &mut self,
        clock: &crate::monitor::windows_clock::WindowsClock,
    ) -> super::parent_start::Observation {
        let mut capture_error = None;
        let anchor = clock
            .capture()
            .map_err(|e| capture_error = Some(e.into()))
            .ok();
        self.tick(anchor);
        super::parent_start::Observation {
            anchor,
            capture_error,
            clock_identity: (clock.qpc_frequency(), clock.clock_token()),
            parent_fault: self.fault,
            context: self.context_open()
                && self
                    .context_check
                    .as_ref()
                    .is_some_and(|c| c.result.is_ok())
                && self.peer_check.as_ref().is_some_and(|p| p.result.is_ok())
                && self
                    .consent
                    .as_ref()
                    .is_some_and(|d| d.assessment.result.is_ok()),
            reported_screen: self.reported_ready(),
            safety: *self.monitor.report().safety(),
            reentry: self.reentry.latest,
            consecutive: self.reentry.consecutive,
            watch_ready: self.watch_ready(),
        }
    }
    /// One Start intent on the worker's original parent-to-child sequence.
    /// Collects original process handles inside this attempt; an earlier frame
    /// is deliberately ineligible. The caller must retain process/collector
    /// owners off an independent original-deadline stop/termination path:
    /// OS reads can block. Queue acceptance is not receipt, an ongoing safety
    /// lease or native authority. No NativeAdmission is issued here.
    pub fn queue_start<Q: Queue>(
        &mut self,
        input: super::process_state::Processes<'_>,
        clock: &crate::monitor::windows_clock::WindowsClock,
        out: &mut Sender<Q>,
    ) -> Result<(), super::parent_start::Fault> {
        use super::parent_start::Fault as StartFault;
        if self.start_attempt.is_some() {
            input.setup.close_reference_path();
            self.worker_start_tx.poison();
            self.fail(Fault::Protocol, self.now_ns);
            return Err(StartFault::Duplicate);
        }
        let attempt = super::parent_start::run(
            self.binding,
            self.setup_deadline,
            &mut StartSource {
                parent: self,
                input: super::process_state::Processes {
                    starting: input.starting,
                    sampler: input.sampler,
                    worker: input.worker,
                    setup: &mut *input.setup,
                },
                clock,
                out,
            },
        );
        let mut result = attempt.result;
        self.start_attempt = Some(attempt);
        if result.is_ok() {
            self.notify_watch();
            if self.watch_error.is_some() || self.fault.is_some() {
                result = Err(StartFault::Supervision);
                self.start_attempt
                    .as_mut()
                    .expect("retained attempt")
                    .result = result;
            }
        }
        if result.is_err() {
            input.setup.close_reference_path();
            self.worker_start_tx.poison();
            match result {
                Err(StartFault::Deadline) => self.fail(Fault::SetupDeadline, self.setup_deadline),
                Err(StartFault::Clock) => self.fail(Fault::Clock, self.now_ns),
                Err(StartFault::Supervision) => self.fail(Fault::Supervision, self.now_ns),
                Err(StartFault::Send(_) | StartFault::Codec(_)) => {
                    self.fail(Fault::Transport, self.now_ns)
                }
                _ => self.fail(Fault::Context, self.now_ns),
            }
        }
        result
    }
    /// Concrete reads from the three original owners. OS queries may block;
    /// invoke off the independent deadline path and retain the collector owner.
    /// The retained result is historical, not a cached native/Start permit.
    pub fn observe_processes(
        &mut self,
        input: super::process_state::Processes<'_>,
        clock: &crate::monitor::windows_clock::WindowsClock,
    ) -> Result<(), super::process_state::Fault> {
        use super::process_state::{self, Fault as StateFault, Frame, Window};
        if self.process_frame.is_some() {
            input.setup.close_reference_path();
            self.fail(Fault::Protocol, self.now_ns);
            return Err(StateFault::Duplicate);
        }
        let mut before_error = None;
        let before = clock
            .capture()
            .map_err(|e| before_error = Some(e.into()))
            .ok();
        self.tick(before);
        let expected = [
            input.starting.origin(),
            input.sampler.origin(),
            input.worker.origin(),
        ];
        let context = self.context_open()
            && self
                .consent
                .as_ref()
                .is_some_and(|d| d.assessment.result.is_ok())
            && self
                .peer_check
                .as_ref()
                .is_some_and(|p| p.result.is_ok() && p.peers.map(|p| p.expected) == expected)
            && self.context_check.as_ref().is_some_and(|c| {
                c.result.is_ok() && c.references.worker_setup == input.setup.receipt().digest()
            })
            && input.setup.origin().ok() == Some(expected[2]);
        let window = Window {
            binding: self.binding.monitor(),
            epoch: self.binding.epoch,
            deadline_ns: self.setup_deadline,
        };
        let observed_clock = (clock.qpc_frequency(), clock.clock_token());
        let eligible = context
            && before_error.is_none()
            && observed_clock == (window.binding.qpc_frequency, window.epoch.clock_token);
        let snapshots = if eligible {
            [
                Some(input.starting.observe_state(window, clock)),
                Some(input.sampler.observe_state(window, clock)),
                Some(input.worker.observe_state(window, clock)),
            ]
        } else {
            [None; 3]
        };
        let mut after_error = None;
        let after = clock
            .capture()
            .map_err(|e| after_error = Some(e.into()))
            .ok();
        self.tick(after);
        let mut frame = Frame {
            window,
            observed_clock,
            before,
            after,
            before_error,
            after_error,
            snapshots,
            checks: [Err(StateFault::Missing); 3],
            clock_result: Err(StateFault::Clock),
            result: Err(StateFault::Context),
        };
        process_state::screen_frame(&mut frame, expected, context && self.context_open());
        let result = frame.result;
        self.process_frame = Some(frame);
        if result.is_err() {
            input.setup.close_reference_path();
            self.fail(Fault::Context, self.now_ns);
        }
        result
    }
    /// Collect the three original process owners again for a future native
    /// grant. The Start frame is historical and is never reused. OS queries
    /// run on this caller, outside the independent deadline controller.
    #[cfg(windows)]
    pub fn observe_native_grant_processes(
        &mut self,
        input: super::process_state::Processes<'_>,
        clock: &crate::monitor::windows_clock::WindowsClock,
    ) -> Result<(), super::process_state::Fault> {
        use super::process_state::{self, Fault as StateFault, Frame, Window};
        let start_ok = self
            .start_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.queued && attempt.result.is_ok());
        if self.native_grant_process_frame.is_some() {
            input.setup.close_reference_path();
            self.fail(Fault::Protocol, self.now_ns);
            return Err(StateFault::Duplicate);
        }
        if !start_ok {
            input.setup.close_reference_path();
            self.fail(Fault::Context, self.now_ns);
            return Err(StateFault::Context);
        }

        let first = self.observe_start(clock);
        let first_ok = first.capture_error.is_none()
            && first.clock_identity
                == (
                    self.binding.monitor.qpc_frequency,
                    self.binding.epoch.clock_token,
                )
            && first.anchor.is_some_and(|a| {
                self.binding
                    .elapsed(a)
                    .is_ok_and(|n| n < self.setup_deadline)
            })
            && first.parent_fault.is_none()
            && first.context
            && first.reported_screen
            && first.watch_ready;
        let expected = [
            input.starting.origin(),
            input.sampler.origin(),
            input.worker.origin(),
        ];
        let context = first_ok
            && self.context_open()
            && self
                .consent
                .as_ref()
                .is_some_and(|d| d.assessment.result.is_ok())
            && self
                .peer_check
                .as_ref()
                .is_some_and(|p| p.result.is_ok() && p.peers.map(|p| p.expected) == expected)
            && self.context_check.as_ref().is_some_and(|c| {
                c.result.is_ok() && c.references.worker_setup == input.setup.receipt().digest()
            })
            && input.setup.origin().ok() == Some(expected[2]);
        let window = Window {
            binding: self.binding.monitor(),
            epoch: self.binding.epoch,
            deadline_ns: self.setup_deadline,
        };
        let observed_clock = (clock.qpc_frequency(), clock.clock_token());
        let eligible =
            context && observed_clock == (window.binding.qpc_frequency, window.epoch.clock_token);
        let snapshots = if eligible {
            [
                Some(input.starting.observe_state(window, clock)),
                Some(input.sampler.observe_state(window, clock)),
                Some(input.worker.observe_state(window, clock)),
            ]
        } else {
            [None; 3]
        };
        let last = if eligible {
            Some(self.observe_start(clock))
        } else {
            None
        };
        self.native_grant_observations = Some((first, last));
        let last_ok = last.is_some_and(|observation| {
            observation.capture_error.is_none()
                && observation.clock_identity
                    == (
                        self.binding.monitor.qpc_frequency,
                        self.binding.epoch.clock_token,
                    )
                && observation.anchor.is_some_and(|a| {
                    self.binding
                        .elapsed(a)
                        .is_ok_and(|n| n < self.setup_deadline)
                })
                && observation.parent_fault.is_none()
                && observation.context
                && observation.reported_screen
                && observation.watch_ready
        });
        let mut frame = Frame {
            window,
            observed_clock,
            before: first.anchor,
            after: last.and_then(|observation| observation.anchor),
            before_error: first.capture_error,
            after_error: last.and_then(|observation| observation.capture_error),
            snapshots,
            checks: [Err(StateFault::Missing); 3],
            clock_result: Err(StateFault::Clock),
            result: Err(StateFault::Context),
        };
        process_state::screen_frame(&mut frame, expected, eligible && last_ok);
        let result = frame.result;
        self.native_grant_process_frame = Some(frame);
        if result.is_err() {
            input.setup.close_reference_path();
            let parent_fault = if self.watch_error.is_some() || !self.watch_ready() {
                Fault::Supervision
            } else if self.monitor.report().stop().is_some() {
                Fault::Monitor
            } else if self.now_ns >= self.setup_deadline {
                Fault::SetupDeadline
            } else if matches!(result, Err(StateFault::Clock)) {
                Fault::Clock
            } else {
                Fault::Context
            };
            self.fail(parent_fault, self.now_ns);
        }
        result
    }
    /// Compare retained local launch owners with every received artifact path.
    /// Caller keeps all process/session owners; success gives no native permit.
    pub fn check_processes(
        &mut self,
        input: super::parent_peers::Processes<'_>,
    ) -> Result<(), super::parent_peers::Fault> {
        use super::{
            launch_origin::Kind,
            parent_peers::{self, Peer},
        };
        if self.peer_check.is_some() {
            input.setup.close_reference_path();
            self.fail(Fault::Protocol, self.now_ns);
            return Err(parent_peers::Fault::Duplicate);
        }
        let context = if self.context_open() {
            self.context_check
                .as_ref()
                .and_then(|c| c.result.ok())
                .map(|c| c.references)
        } else {
            None
        };
        let starting_response = input
            .starting_session
            .canonical()
            .map(|b| crate::evidence::Digest::of(crate::evidence::Domain::RawArtifact, b));
        let peers = [
            Peer {
                expected: input.starting.origin(),
                received: input.starting_session.origin(),
                cancelled: input.starting.cancelled(),
            },
            Peer {
                expected: input.sampler.origin(),
                received: self.origin(),
                cancelled: input.sampler.cancelled(),
            },
            Peer {
                expected: input.worker.origin(),
                received: input.setup.origin(),
                cancelled: input.worker.cancelled(),
            },
        ];
        let mut record = parent_peers::assess(
            context,
            self.binding_digest,
            self.binding.images(),
            starting_response,
            peers,
        );
        // The setup object must be the one retained by the context check.
        if context.is_some_and(|c| c.worker_setup != input.setup.receipt().digest())
            || input.sampler.origin().kind() != Kind::Sampler
        {
            record.result = Err(parent_peers::Fault::Binding);
        }
        let result = record.result;
        self.peer_check = Some(record);
        if result.is_err() {
            input.setup.close_reference_path();
            self.fail(Fault::Binding, self.now_ns);
        } else {
            self.expected_origin = Some(input.sampler.origin());
        }
        result
    }
    pub fn for_process(
        run: &BoundRun<'_>,
        process: &super::windows_process::Process,
        setup_entry: Anchor,
        first: Anchor,
        budget: Budget,
    ) -> Result<Self, Fault> {
        let declaration = process.declaration();
        if declaration.role() != super::launch::Role::Sampler
            || WorkerBinding::new(
                run,
                declaration.binding().monitor().qpc_frequency,
                declaration.binding().epoch,
            )
            .ok()
                != Some(declaration.binding())
        {
            return Err(Fault::Binding);
        }
        let mut parent = Self::new(
            run,
            declaration.binding().monitor().qpc_frequency,
            declaration.binding().epoch,
            setup_entry,
            first,
            budget,
            process.reserved_control_budget(),
        )?;
        parent.expected_origin = Some(process.origin());
        Ok(parent)
    }
    /// Concrete retained process -> raw receipt/strict parent assessment -> next
    /// query/control handoff. Does not launch, wait for exit or grant compute.
    pub fn drive_process<Q: Queue>(
        &mut self,
        run: &BoundRun<'_>,
        process: &super::windows_process::Process,
        mut clock: impl FnMut() -> Option<Anchor>,
        out: &mut Sender<Q>,
    ) -> Poll {
        use super::windows_transport::Receive;
        if process.declaration().role() != super::launch::Role::Sampler
            || process.declaration().binding() != self.binding
            || self.expected_origin.is_some_and(|o| o != process.origin())
        {
            self.fail(Fault::Binding, self.now_ns);
            return Poll {
                anchor: None,
                next_anchor: None,
                inputs_finished: None,
                os_code: None,
                transport_fault: false,
                rejected: None,
            };
        }
        if process.cancelled() && !self.finishing {
            self.fail(Fault::Stopped, self.now_ns);
        }
        let mut result = if process.inputs_finished() {
            let anchor = clock();
            self.tick(anchor);
            if !(self.finishing && self.reported_finished) {
                self.fail(Fault::Transport, self.now_ns);
            }
            Poll {
                anchor,
                next_anchor: None,
                inputs_finished: Some(true),
                os_code: None,
                transport_fault: !(self.finishing && self.reported_finished),
                rejected: None,
            }
        } else {
            match process.receive(&mut clock) {
                Receive::Packet(raw) => {
                    let anchor = raw.receipt();
                    let rejected = self.receive(run, raw).err();
                    Poll {
                        anchor,
                        next_anchor: None,
                        inputs_finished: Some(false),
                        os_code: None,
                        transport_fault: false,
                        rejected,
                    }
                }
                Receive::Idle { anchor } => {
                    self.tick(anchor);
                    Poll {
                        anchor,
                        next_anchor: None,
                        inputs_finished: Some(false),
                        os_code: None,
                        transport_fault: false,
                        rejected: None,
                    }
                }
                Receive::Fault { code, anchor } => {
                    self.tick(anchor);
                    self.fail(Fault::Transport, self.now_ns);
                    Poll {
                        anchor,
                        next_anchor: None,
                        inputs_finished: Some(false),
                        os_code: code,
                        transport_fault: true,
                        rejected: None,
                    }
                }
            }
        };
        if self.fault.is_none()
            && !self.finishing
            && self.runtime.is_some()
            && !self.settings_issued
        {
            let anchor = clock();
            if let Some(a) = anchor {
                let _ = self.issue_settings(a, out);
            } else {
                self.tick(None);
            }
            result.next_anchor = anchor;
        }
        if self.fault.is_none()
            && !self.finishing
            && self.pending.is_none()
            && self.runtime.is_some()
        {
            let anchor = clock();
            if let Some(a) = anchor {
                let _ = self.next_query(a, out);
            } else {
                self.tick(None);
            }
            result.next_anchor = anchor;
        }
        if self.stop.is_some() {
            if self.stop_event.is_none() {
                self.stop_event = Some(process.request_stop().map_err(|e| e.raw_os_error()));
            }
            self.queue_stop(out);
        }
        result
    }
}
pub(super) fn runtime_valid(r: &Runtime) -> bool {
    let text = |f: &autotune_probe::report::Field| {
        f.unit == "text"
            && f.symbol_presence == SymbolPresence::Present
            && matches!(&f.reading,Reading::Available{value} if value.as_str().is_some_and(|s|!s.is_empty()&&s.len()<=255))
    };
    r.header_api_version == 13
        && r.library_path.as_ref().is_some_and(|p| !p.is_empty())
        && text(&r.driver_version)
        && text(&r.nvml_version)
        && r.cuda_driver_version.unit == "CUDA encoded integer"
        && r.cuda_driver_version.symbol_presence == SymbolPresence::Present
        && matches!(&r.cuda_driver_version.reading,Reading::Available{value} if value.as_i64().is_some_and(|n|n>0&&n<=i32::MAX as i64))
}

#[cfg(windows)]
fn grant_observation_eligible(
    observation: super::parent_start::Observation,
    binding: WorkerBinding,
    deadline: u64,
) -> bool {
    observation.capture_error.is_none()
        && observation.clock_identity
            == (binding.monitor().qpc_frequency, binding.epoch.clock_token)
        && observation.anchor.is_some_and(|anchor| {
            binding
                .elapsed(anchor)
                .is_ok_and(|elapsed| elapsed < deadline)
        })
        && observation.parent_fault.is_none()
        && observation.context
        && observation.reported_screen
        && observation.watch_ready
        && observation.safety.iter().all(Option::is_some)
        && observation.reentry.is_some()
}

#[cfg(windows)]
fn grant_observation_check(
    observation: super::parent_start::Observation,
    binding: WorkerBinding,
    deadline: u64,
) -> Result<(), Fault> {
    if observation.capture_error.is_some()
        || observation.clock_identity
            != (binding.monitor().qpc_frequency, binding.epoch.clock_token)
    {
        return Err(Fault::Clock);
    }
    let elapsed = observation
        .anchor
        .and_then(|anchor| binding.elapsed(anchor).ok())
        .ok_or(Fault::Clock)?;
    if elapsed >= deadline {
        return Err(Fault::SetupDeadline);
    }
    if observation.parent_fault.is_some() || !observation.watch_ready {
        return Err(Fault::Supervision);
    }
    if !observation.context {
        return Err(Fault::Context);
    }
    if !observation.reported_screen || !grant_observation_eligible(observation, binding, deadline) {
        return Err(Fault::ReentryDeadline);
    }
    Ok(())
}

#[cfg(windows)]
fn grant_observation_value(observation: super::parent_start::Observation) -> Option<Value> {
    if !observation.context
        || !observation.reported_screen
        || !observation.watch_ready
        || observation.capture_error.is_some()
        || observation.parent_fault.is_some()
    {
        return None;
    }
    let safety = observation
        .safety
        .map(|record| record.map(grant_monitor_record_value));
    let safety = safety.map(Option::unwrap);
    let reentry = observation.reentry?;
    Some(grant_object([
        ("anchor", grant_anchor_value(observation.anchor?)),
        ("clock_frequency", Value::U64(observation.clock_identity.0)),
        (
            "clock_token",
            Value::Text(grant_hex(&observation.clock_identity.1)),
        ),
        ("context", Value::Bool(observation.context)),
        ("reported_screen", Value::Bool(observation.reported_screen)),
        ("watch_ready", Value::Bool(observation.watch_ready)),
        (
            "consecutive_reentry",
            Value::U64(observation.consecutive.into()),
        ),
        ("safety", Value::Array(safety.into_iter().collect())),
        (
            "reentry",
            Value::Array(reentry.map(grant_monitor_record_value).into()),
        ),
    ]))
}

#[cfg(windows)]
fn grant_monitor_record_value(record: Record) -> Value {
    let ticket = record.ticket;
    let binding = ticket.binding;
    let metric = match ticket.metric {
        Metric::Temperature => "temperature",
        Metric::Power => "power",
        Metric::GraphicsClock => "graphics_clock",
        Metric::MemoryClock => "memory_clock",
        Metric::GpuUtilization => "gpu_utilization",
        Metric::MemoryUtilization => "memory_utilization",
    };
    let unit = match record.unit {
        crate::monitor::Unit::DegC => "deg_c",
        crate::monitor::Unit::Milliwatt => "milliwatt",
        crate::monitor::Unit::MHz => "mhz",
        crate::monitor::Unit::Percent => "percent",
    };
    let status = match record.status {
        Status::Available => Value::Text("available".into()),
        Status::Unsupported => Value::Text("unsupported".into()),
        Status::Unavailable => Value::Text("unavailable".into()),
        Status::Failed(code) => grant_object([("failed", Value::I64(code.into()))]),
    };
    grant_object([
        ("session", Value::Text(grant_hex(&binding.session))),
        ("run", Value::Text(grant_hex(&binding.run))),
        ("device_uuid", Value::Text(grant_hex(&binding.device.uuid))),
        ("device_domain", Value::U64(binding.device.domain.into())),
        ("device_bus", Value::U64(binding.device.bus.into())),
        ("device_number", Value::U64(binding.device.device.into())),
        ("clock_token", Value::Text(grant_hex(&binding.clock_token))),
        ("frequency", Value::U64(binding.qpc_frequency)),
        ("metric", Value::Text(metric.into())),
        ("slot", Value::U64(ticket.slot)),
        ("scheduled", Value::U64(ticket.scheduled_ns)),
        ("start", Value::U64(ticket.start_ns)),
        ("end", Value::U64(record.end_ns)),
        ("receipt", Value::U64(record.receipt_ns)),
        ("unit", Value::Text(unit.into())),
        ("status", status),
        ("value", record.value.map_or(Value::Null, Value::I64)),
    ])
}

#[cfg(windows)]
fn grant_references_value(refs: super::parent_context::References) -> Value {
    grant_object([
        ("manifest", grant_digest_value(refs.manifest)),
        ("scope", grant_digest_value(refs.scope)),
        ("host", refs.host.map_or(Value::Null, grant_digest_value)),
        (
            "starting",
            refs.starting.map_or(Value::Null, grant_digest_value),
        ),
        (
            "starting_response",
            refs.starting_response
                .map_or(Value::Null, grant_digest_value),
        ),
        ("worker_setup", grant_digest_value(refs.worker_setup)),
        (
            "current",
            refs.current.map_or(Value::Null, grant_digest_value),
        ),
        (
            "sampler_ready",
            refs.sampler_ready.map_or(Value::Null, grant_digest_value),
        ),
        (
            "current_response",
            refs.current_response
                .map_or(Value::Null, grant_digest_value),
        ),
    ])
}

#[cfg(windows)]
fn grant_origin_value(origin: super::launch_origin::Origin) -> Value {
    grant_object([
        (
            "kind",
            Value::Text(
                match origin.kind() {
                    super::launch_origin::Kind::Starting => "starting",
                    super::launch_origin::Kind::Sampler => "sampler",
                    super::launch_origin::Kind::Worker => "worker",
                }
                .into(),
            ),
        ),
        ("binding", grant_digest_value(origin.binding())),
        ("declaration", grant_digest_value(origin.declaration())),
        ("image", grant_digest_value(origin.image())),
        ("pid", Value::U64(origin.pid().into())),
        ("creation", Value::U64(origin.creation())),
        (
            "consent",
            origin.consent().map_or(Value::Null, |capture| {
                grant_object([
                    ("request", grant_digest_value(capture.request)),
                    ("receipt", grant_anchor_value(capture.receipt)),
                ])
            }),
        ),
    ])
}

#[cfg(windows)]
fn grant_anchor_value(anchor: Anchor) -> Value {
    grant_object([
        ("token", Value::Text(grant_hex(&anchor.clock_token))),
        ("before", Value::U64(anchor.qpc_before)),
        ("after", Value::U64(anchor.qpc_after)),
        ("interrupt", Value::U64(anchor.interrupt_100ns)),
        ("unbiased", Value::U64(anchor.unbiased_100ns)),
    ])
}

#[cfg(windows)]
fn grant_digest_value(digest: Digest) -> Value {
    Value::Text(digest.to_hex())
}

#[cfg(windows)]
fn grant_value_digest(value: Value) -> Digest {
    Digest::of_value(Domain::RawArtifact, &value)
}

#[cfg(windows)]
fn grant_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 15)] as char);
    }
    text
}

#[cfg(windows)]
fn grant_object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = crate::evidence::Object::new();
    for (key, value) in fields {
        object.insert(key, value).expect("fixed unique grant keys");
    }
    Value::Object(object)
}
