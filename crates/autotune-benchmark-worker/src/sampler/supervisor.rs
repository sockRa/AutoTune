//! Injected nonblocking sampler supervision. Effects request OS operations;
//! neither their emission nor child messages prove execution or termination.
use super::budget::{Budget, Reservation};
use super::wire::{Message, WireBinding};
use crate::{
    monitor::{clock::Clock, Anchor, ClockError, Controller, Report, Session, StopReason, Ticket},
    worker::DiscoveryBinding,
};

fn limits() -> autotune_fma_reference::profile::Limits {
    autotune_fma_reference::profile::Profile::approved_design().limits
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub raw_uuid: String,
    pub raw_pci: String,
    pub sampler_digest: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Requested,
    Monitor(StopReason),
    Protocol,
    Identity,
    Timing,
    ClockCapture,
    Transport,
    ChildFinished,
    ProcessLost,
    Capacity,
    Preflight,
    Resources,
}
impl Fault {
    fn name(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Monitor(_) => "monitor",
            Self::Protocol => "protocol",
            Self::Identity => "identity",
            Self::Timing => "timing",
            Self::ClockCapture => "clock_capture",
            Self::Transport => "transport",
            Self::ChildFinished => "child_finished",
            Self::ProcessLost => "process_lost",
            Self::Capacity => "capacity",
            Self::Preflight => "preflight",
            Self::Resources => "resources",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terminal {
    pub reason: Fault,
    pub at_ns: u64,
    pub cooperative_ns: u64,
    pub exit_ns: u64,
    pub pending: Option<Ticket>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessObservation {
    pub creation: u64,
    pub wait_status: u32,
    pub wait_error: Option<u32>,
    pub exit_code: Option<u32>,
    pub exit_error: Option<u32>,
    pub termination: Option<Result<(), u32>>,
    pub active_processes: Option<u32>,
    pub accounting_error: Option<u32>,
}
#[derive(Debug)]
// Fixed-size trace preallocation accounts for the entire inline message. A
// boxed variant would add per-event allocations outside that storage charge.
#[allow(clippy::large_enum_variant)]
pub enum Input {
    Tick,
    Launched {
        creation: u64,
    },
    LaunchFailure {
        stage: &'static str,
        error: u32,
        assigned: bool,
        creation: Option<u64>,
        process: ProcessObservation,
    },
    Message(Message),
    Stop,
    Fault(Fault),
    Process(ProcessObservation),
    ObservationExpired,
    Host {
        watchdog_100ns: u64,
        resources: super::runtime::ResourceObservation,
        sampler_resources: Option<super::runtime::ResourceObservation>,
        process: Option<ProcessObservation>,
    },
    Operation {
        operation: super::runtime::Operation,
        result: Result<(), u32>,
    },
    IoShutdown(super::runtime::IoObservation),
    WriterFailure(super::writer::Failure),
    WriterShutdown {
        cancellation: Result<(), u32>,
        finished: bool,
    },
    MetadataOverflow {
        attempted_bytes: u64,
    },
    Rejected {
        lane: super::wire::Lane,
        bytes: Vec<u8>,
        rejection: super::framing::Rejection,
    },
}
#[derive(Debug)]
// At most four stack-owned effects per advance; no per-message box allocation.
#[allow(clippy::large_enum_variant)]
pub enum Effect {
    Send(Message),
    SignalStop,
    Terminate,
    Finalize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    Binding,
    Clock(ClockError),
    Capacity,
}
pub struct Trace {
    pub receipt: Option<Anchor>,
    pub input: Input,
    _payload: Reservation,
}
pub struct Sample<'a> {
    pub ticket: Ticket,
    pub start: Anchor,
    pub end: Anchor,
    pub receipt: Anchor,
    pub raw: &'a autotune_probe::report::Field,
}

pub struct Supervisor {
    binding: WireBinding,
    origin: Anchor,
    selection: Selection,
    controller: Controller,
    child_clock: Clock,
    creation: Option<u64>,
    launch_ns: Option<u64>,
    ready_ns: Option<u64>,
    exit_observation: Option<(Anchor, ProcessObservation)>,
    ready: bool,
    terminal: Option<Terminal>,
    terminate_sent: bool,
    finalized: bool,
    exit_confirmed: bool,
    traces: Vec<Trace>,
    budget: Budget,
    _monitor: Reservation,
    _trace_storage: Reservation,
    writer_budget: Budget,
    failure_budget: Budget,
    metadata_used: usize,
    retention_overflow: bool,
}
impl Supervisor {
    pub fn new(
        binding: WireBinding,
        origin: Anchor,
        selection: Selection,
    ) -> Result<Self, InitError> {
        let parsed = DiscoveryBinding::parse(&selection.raw_uuid, &selection.raw_pci)
            .map_err(|_| InitError::Binding)?;
        if parsed.identity != binding.monitor.device
            || selection.sampler_digest == [0; 32]
            || binding.manifest_digest == [0; 32]
        {
            return Err(InitError::Binding);
        }
        let p = limits();
        let ordinary = p.evidence_ram_bytes - p.writer_queue_bytes - p.failure_snapshot_bytes;
        let budget = Budget::new(p.evidence_ram_bytes as usize).map_err(|_| InitError::Capacity)?;
        let writer = budget
            .partition(p.writer_queue_bytes as usize)
            .map_err(|_| InitError::Capacity)?;
        let failure = budget
            .partition(p.failure_snapshot_bytes as usize)
            .map_err(|_| InitError::Capacity)?;
        // Reserve the existing monitor's entire independently validated ceiling
        // BEFORE its fixed preallocation, then return unused accounted capacity.
        let mut monitor = budget
            .reserve(ordinary as usize)
            .map_err(|_| InitError::Capacity)?;
        let child_clock = Clock::new(binding.monitor, origin).map_err(InitError::Clock)?;
        let controller = Controller::new(binding.monitor, origin, Session::Pilot)
            .map_err(|_| InitError::Capacity)?;
        monitor
            .shrink_to(controller.report().capacity().allocated_bytes as usize)
            .map_err(|_| InitError::Capacity)?;
        // The sampler-only trace covers 10 s preflight plus the original 8 s
        // shutdown/finalization reserve, including per-tick process observations.
        let span = p
            .preflight_ns
            .checked_add(p.shutdown_reserve_ns)
            .ok_or(InitError::Capacity)?;
        let entries = (span / p.controller_tick_ns
            + span / p.cadence_ns * u64::from(p.metric_count) * 2
            + u64::from(p.control_queue_frames) * 8
            + 32)
            .min(p.event_capacity) as usize;
        let trace_storage = budget
            .reserve(
                entries
                    .checked_mul(std::mem::size_of::<Trace>())
                    .ok_or(InitError::Capacity)?,
            )
            .map_err(|_| InitError::Capacity)?;
        let mut traces = Vec::new();
        traces
            .try_reserve_exact(entries)
            .map_err(|_| InitError::Capacity)?;
        Ok(Self {
            binding,
            origin,
            selection,
            controller,
            child_clock,
            creation: None,
            launch_ns: None,
            ready_ns: None,
            exit_observation: None,
            ready: false,
            terminal: None,
            terminate_sent: false,
            finalized: false,
            exit_confirmed: false,
            traces,
            budget,
            _monitor: monitor,
            _trace_storage: trace_storage,
            writer_budget: writer,
            failure_budget: failure,
            // Fixed manifest/profile/selection and launch metadata ceiling.
            metadata_used: p.frame_bytes as usize,
            retention_overflow: false,
        })
    }
    pub fn report(&self) -> &Report {
        self.controller.report()
    }
    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminal.as_ref()
    }
    pub fn exit_confirmed(&self) -> bool {
        self.exit_confirmed
    }
    pub fn launch_ns(&self) -> Option<u64> {
        self.launch_ns
    }
    pub fn ready_ns(&self) -> Option<u64> {
        self.ready_ns
    }
    pub fn exit_observation(&self) -> Option<(Anchor, ProcessObservation)> {
        self.exit_observation
    }
    pub fn traces(&self) -> &[Trace] {
        &self.traces
    }
    pub fn binding(&self) -> WireBinding {
        self.binding
    }
    pub fn origin(&self) -> Anchor {
        self.origin
    }
    pub fn budget(&self) -> Budget {
        self.budget.clone()
    }
    pub fn writer_budget(&self) -> Budget {
        self.writer_budget.clone()
    }
    pub fn failure_budget(&self) -> Budget {
        self.failure_budget.clone()
    }
    pub fn retention_overflow(&self) -> bool {
        self.retention_overflow
    }
    pub fn samples(&self) -> impl Iterator<Item = Sample<'_>> {
        self.traces
            .iter()
            .filter_map(|t| match (&t.input, t.receipt) {
                (
                    Input::Message(Message::MetricRecord {
                        ticket,
                        start,
                        end,
                        raw,
                    }),
                    Some(receipt),
                ) => Some(Sample {
                    ticket: *ticket,
                    start: *start,
                    end: *end,
                    receipt,
                    raw,
                }),
                _ => None,
            })
    }
    pub fn issue_anchor(&self, ticket: Ticket) -> Option<Anchor> {
        self.report().events().find_map(|event| match event.action {
            crate::monitor::controller::Action::Begin(Some(t)) if t == ticket => Some(event.anchor),
            _ => None,
        })
    }
    fn stop(&mut self, reason: Fault, anchor: Option<Anchor>) {
        if self.terminal.is_some() {
            return;
        }
        let now = self.report().now_ns();
        let p = limits();
        self.terminal = Some(Terminal {
            reason,
            at_ns: now,
            cooperative_ns: now
                .saturating_add(p.cooperative_stop_ns)
                .min(self.report().session_deadline_ns()),
            exit_ns: now
                .saturating_add(p.exit_observation_ns)
                .min(self.report().session_deadline_ns()),
            pending: self.report().pending(),
        });
        if let Some(anchor) = anchor {
            self.controller.request_stop(anchor);
        }
    }
    fn handle(&mut self, anchor: Option<Anchor>, input: &Input) {
        match input {
            Input::Tick => {}
            Input::LaunchFailure { .. } => self.stop(Fault::Transport, anchor),
            Input::IoShutdown(_) => {}
            Input::WriterFailure(_) => self.stop(Fault::Transport, anchor),
            Input::WriterShutdown { .. } => {}
            Input::MetadataOverflow { .. } => self.stop(Fault::Capacity, anchor),
            Input::Rejected { .. } => self.stop(Fault::Transport, anchor),
            Input::Host {
                resources,
                sampler_resources,
                process,
                ..
            } => {
                if !resources.within_limits() {
                    self.stop(Fault::Resources, anchor);
                }
                if sampler_resources.is_some_and(|o| !o.within_sampler_limits()) {
                    self.stop(Fault::Resources, anchor);
                }
                if let Some(o) = process {
                    self.handle(anchor, &Input::Process(*o));
                }
            }
            Input::Operation { result, .. } => {
                if result.is_err() {
                    self.stop(Fault::Transport, anchor);
                }
            }
            Input::ObservationExpired => self.stop(Fault::Timing, anchor),
            Input::Launched { creation } => {
                if self.terminal.is_some() {
                    return;
                }
                if self.creation.is_some() || *creation == 0 {
                    self.stop(Fault::Protocol, anchor);
                } else {
                    self.creation = Some(*creation);
                    self.launch_ns = Some(self.report().now_ns());
                }
            }
            Input::Stop => self.stop(Fault::Requested, anchor),
            Input::Fault(reason) => self.stop(*reason, anchor),
            Input::Process(observation) => {
                if Some(observation.creation) != self.creation {
                    self.stop(Fault::Identity, anchor);
                    return;
                }
                if observation.wait_status == 0
                    && observation.wait_error.is_none()
                    && observation.exit_code.is_some()
                    && observation.exit_error.is_none()
                {
                    // Closure precedes job-empty interpretation. A direct exit
                    // observed during capture ends admission regardless of code.
                    self.stop(Fault::ProcessLost, anchor);
                    let in_window = anchor.is_some()
                        && self.report().current_clock_valid()
                        && !self.finalized
                        && self
                            .terminal
                            .is_some_and(|t| self.report().now_ns() < t.exit_ns);
                    if in_window
                        && observation.active_processes == Some(0)
                        && observation.accounting_error.is_none()
                    {
                        self.exit_confirmed = true;
                        if self.exit_observation.is_none() {
                            self.exit_observation = anchor.map(|a| (a, *observation));
                        }
                    }
                }
            }
            Input::Message(message) => self.handle_message(anchor, message),
        }
    }
    fn handle_message(&mut self, receipt: Option<Anchor>, message: &Message) {
        // Late messages remain raw supplemental evidence, never primary state
        // transitions or a freshness/exit/terminal upgrade.
        if self.terminal.is_some() {
            return;
        }
        let Some(receipt) = receipt else {
            return;
        };
        match message {
            Message::Ready {
                raw_uuid,
                raw_pci,
                sampler_digest,
                frequency,
                anchor,
                ..
            } => {
                if self.creation.is_none() || self.ready {
                    self.stop(Fault::Protocol, Some(receipt));
                    return;
                }
                if raw_uuid != &self.selection.raw_uuid
                    || raw_pci != &self.selection.raw_pci
                    || *sampler_digest != self.selection.sampler_digest
                    || *frequency != self.binding.monitor.qpc_frequency
                {
                    self.stop(Fault::Identity, Some(receipt));
                    return;
                }
                if anchor.qpc_after > receipt.qpc_before
                    || self.child_clock.observe(*anchor).is_err()
                {
                    self.stop(Fault::Timing, Some(receipt));
                    return;
                }
                self.ready = true;
                self.ready_ns = Some(self.report().now_ns());
            }
            Message::MetricRecord {
                ticket,
                start,
                end,
                raw,
            } => {
                if !self.ready || self.report().pending() != Some(*ticket) {
                    self.stop(Fault::Protocol, Some(receipt));
                    return;
                }
                let start_ns = self.child_clock.observe(*start);
                let end_ns = self.child_clock.observe(*end);
                match (start_ns, end_ns) {
                    (Ok(s), Ok(e))
                        if ticket.start_ns <= s
                            && self
                                .issue_anchor(*ticket)
                                .is_some_and(|a| a.qpc_after <= start.qpc_before)
                            && start.qpc_after <= end.qpc_before
                            && s <= e
                            && end.qpc_after <= receipt.qpc_before
                            && e <= self.report().now_ns() =>
                    {
                        match super::convert(*ticket, e, self.report().now_ns(), raw) {
                            Ok(record) => self.controller.complete(receipt, record),
                            Err(_) => self.stop(Fault::Protocol, Some(receipt)),
                        }
                    }
                    _ => self.stop(Fault::Timing, Some(receipt)),
                }
            }
            Message::StopAck { anchor, .. } => {
                if self.child_clock.observe(*anchor).is_err() {
                    self.stop(Fault::Timing, Some(receipt));
                } else {
                    self.stop(Fault::Protocol, Some(receipt));
                } // No unsolicited stop acknowledgment.
            }
            Message::RoleFinished { .. } => self.stop(Fault::ChildFinished, Some(receipt)),
            _ => self.stop(Fault::Protocol, Some(receipt)),
        }
    }
    fn retain(
        &mut self,
        receipt: Option<Anchor>,
        input: Input,
        provided: Option<Reservation>,
    ) -> bool {
        if matches!(input, Input::Tick) {
            return true;
        } // Controller already retains every tick.
        let payload = match &input {
            Input::Message(m) => match m.text_bound() {
                Ok(n) => n,
                Err(_) => return false,
            },
            Input::Rejected { bytes, .. } => bytes.capacity(),
            _ => 0,
        };
        let metadata = matches!(input, Input::Message(_) | Input::Rejected { .. });
        if metadata
            && self
                .metadata_used
                .checked_add(payload)
                .is_none_or(|n| n > limits().metadata_bytes as usize)
        {
            if !self.retention_overflow && self.traces.len() < self.traces.capacity() {
                self.traces.push(Trace {
                    receipt,
                    input: Input::MetadataOverflow {
                        attempted_bytes: payload as u64,
                    },
                    _payload: self.failure_budget.reserve(0).expect("zero reservation"),
                });
            }
            self.retention_overflow = true;
            return false;
        }
        if payload > limits().frame_bytes as usize
            || self.traces.len() == self.traces.capacity()
            || self.traces.len() as u64 + self.report().capacity().retained_controls as u64
                >= limits().event_capacity
        {
            return false;
        }
        let reservation = match provided {
            Some(mut charge) if charge.is_within(&self.budget) && charge.bytes() >= payload => {
                if charge.shrink_to(payload).is_err() {
                    return false;
                }
                charge
            }
            _ => match self.budget.reserve(payload) {
                Ok(charge) => charge,
                Err(_) => return false,
            },
        };
        self.traces.push(Trace {
            receipt,
            input,
            _payload: reservation,
        });
        if metadata {
            self.metadata_used += payload;
        }
        true
    }
    pub fn advance(&mut self, anchor: Option<Anchor>, input: Input) -> Vec<Effect> {
        self.advance_inner(anchor, input, None)
    }
    pub(crate) fn advance_reserved(
        &mut self,
        anchor: Option<Anchor>,
        input: Input,
        provided: Reservation,
    ) -> Vec<Effect> {
        self.advance_inner(anchor, input, Some(provided))
    }
    fn advance_inner(
        &mut self,
        anchor: Option<Anchor>,
        input: Input,
        provided: Option<Reservation>,
    ) -> Vec<Effect> {
        let observation_expired = matches!(input, Input::ObservationExpired);
        let had_terminal = self.terminal.is_some();
        if let Some(a) = anchor {
            self.controller.poll(a);
            if let Some(stop) = self.report().stop() {
                self.stop(Fault::Monitor(stop.reason), Some(a));
            }
            if self.report().now_ns() >= limits().preflight_ns {
                self.stop(Fault::Preflight, Some(a));
            }
        } else {
            self.stop(Fault::ClockCapture, None);
        }
        self.handle(anchor, &input);
        if !self.retain(anchor, input, provided) {
            self.retention_overflow = true;
            self.stop(Fault::Capacity, anchor);
        }
        if let Some(stop) = self.report().stop() {
            self.stop(Fault::Monitor(stop.reason), anchor);
        }
        let mut effects = Vec::with_capacity(4);
        if let Some(terminal) = self.terminal {
            if !had_terminal {
                effects.push(Effect::SignalStop);
                effects.push(Effect::Send(Message::Stop {
                    reason: terminal.reason.name().into(),
                    trigger_ns: terminal.at_ns,
                    cooperative_ns: terminal.cooperative_ns,
                    exit_ns: terminal.exit_ns,
                }));
            }
            if !self.exit_confirmed
                && !self.terminate_sent
                && (anchor.is_none()
                    || !self.report().current_clock_valid()
                    || self.report().now_ns() >= terminal.cooperative_ns)
            {
                effects.push(Effect::Terminate);
                self.terminate_sent = true;
            }
            if !self.finalized
                && (self.exit_confirmed
                    || observation_expired
                    || self.report().now_ns() >= terminal.exit_ns)
            {
                effects.push(Effect::Finalize);
                self.finalized = true;
            }
        } else if self.ready {
            if let Some(a) = anchor {
                if let Some(ticket) = self.controller.begin(a) {
                    effects.push(Effect::Send(Message::Query(ticket)));
                }
            }
        }
        effects
    }
}
