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
use crate::{
    evidence::Reason,
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
    budget: Budget,
    _metadata: Reservation,
}
impl ParentSampler {
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
    }
    /// No Start/permit is returned. This screen concerns reported sampler data;
    /// a later parent gate must prove peer/environment/consent and fresh timing.
    pub fn reported_screen(&mut self, a: Option<Anchor>) -> bool {
        self.tick(a);
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
impl ParentSampler {
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
