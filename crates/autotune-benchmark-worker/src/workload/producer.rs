//! Bound serial producer, prepared host reference and actual observed-driver seam.
//! No executable/native admission issuer. Injected fixtures never qualify tuning.
use super::{
    dispatch::{FailureChunk, FirstFailure, Operation, Phase, Record, CHUNK_LANES},
    gate::{StartObservation, StopWindow},
    teardown,
    trace::{CallTrace, Fault as TraceFault, Observer, TracedRecord},
    wire::{Channel, Direction, EncodedFrame, Message, WorkerBinding},
};
use crate::{
    evidence::Digest,
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        wire::Lane,
    },
    worker::{Fault as NativeFault, Identity, Provenance, Query},
};
use autotune_fma_reference::{
    input::{DispatchIndex, Recipe, Shape},
    profile::{Phase as PlanPhase, Profile},
    reference::PreparedReference,
};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use std::{cell::RefCell, rc::Rc};

pub struct Inputs<'a> {
    pub a: &'a [f32],
    pub b: &'a [f32],
    pub x: &'a [f32],
    pub sentinel: &'a [f32],
    pub steps: u32,
}
/// All native methods must use the observer and stop on its failure. Metadata
/// accessors do no driver I/O. The real implementation delegates to CudaBackend.
pub trait Driver {
    fn provenance(&self) -> Provenance;
    fn identity(&self) -> Identity;
    fn shape(&self) -> Shape;
    fn submit(
        &mut self,
        input: Inputs<'_>,
        observer: &mut impl Observer,
    ) -> Result<(), NativeFault>;
    fn query(&mut self, observer: &mut impl Observer) -> Result<Query, NativeFault>;
    fn readback(
        &mut self,
        output: &mut [f32],
        observer: &mut impl Observer,
    ) -> Result<usize, NativeFault>;
    fn close(&mut self, observer: &mut impl teardown::Observer) -> Result<(), NativeFault>;
}
/// Immediate bounded queue handoff. Success means accepted ownership, not peer
/// receipt/durability. Failure must return the exact frame, never retry/reconnect.
pub trait FrameSink {
    fn send(&mut self, frame: EncodedFrame) -> Result<(), EncodedFrame>;
}

/// There is deliberately no issuer/constructor until actual parent admission
/// validates exact consent, required facts, safety, re-entry and supervision.
pub struct NativeAdmission {
    binding: WorkerBinding,
}
/// Preserve the original native owner and callback owners on every recoverable
/// preparation failure. No speculative close/retry is performed on this path.
pub struct PrepareFailure<B, C, S> {
    pub fault: Fault,
    pub driver: B,
    pub clock: C,
    pub stop: S,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Admission,
    Binding,
    Capacity,
    Clock,
    Deadline,
    Protocol,
    Native(NativeFault),
    Incorrect,
    Length,
    Transport,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    ReportsComplete,
    Stopped,
    Fault(Fault),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopObservation {
    pub anchor: Anchor,
    pub elapsed_ns: u64,
    pub window: StopWindow,
}
#[derive(Clone, Copy, Default)]
pub struct StopHistory {
    pub entries: [Option<StopObservation>; 8],
    pub overflow: Option<StopObservation>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockFailure {
    pub dispatch: u32,
    pub anchor: Option<Anchor>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cleanup {
    pub attempted: bool,
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    pub result: Option<Result<(), NativeFault>>,
}

struct Context<C, S> {
    clock: C,
    stop: S,
    validator: Clock,
    last: Anchor,
    now: u64,
    dispatch: u32,
    failure: Option<ClockFailure>,
    requested: Option<StopObservation>,
    control_failure: Option<StopWindow>,
    run_end: u64,
    drain: bool,
    binding: WorkerBinding,
    shared: Option<Arc<super::control::Shared>>,
    shared_unknown: bool,
    shared_invalid: bool,
    control_history: StopHistory,
}
impl<C: FnMut() -> Option<Anchor>, S: FnMut() -> Option<StopWindow>> Context<C, S> {
    fn capture(&mut self) -> Option<Anchor> {
        if self.failure.is_some() {
            return None;
        }
        let raw = (self.clock)();
        self.observe(raw)
    }
    fn observe(&mut self, raw: Option<Anchor>) -> Option<Anchor> {
        if self.failure.is_some() {
            return None;
        }
        let value = raw
            .filter(|a| a.qpc_before >= self.last.qpc_after)
            .and_then(|a| self.validator.observe(a).ok());
        match value {
            Some(now) => {
                self.last = raw.expect("validated capture");
                self.now = now;
                // Poll original control deadlines even if the call trace will
                // reject this timestamp before its next observer callback.
                self.control();
                raw
            }
            None => {
                self.failure = Some(ClockFailure {
                    dispatch: self.dispatch,
                    anchor: raw,
                });
                None
            }
        }
    }
    fn control(&mut self) -> bool {
        if self.control_failure.is_none() && self.control_history.overflow.is_none() {
            if let Some(window) = (self.stop)() {
                self.observe_window(window);
            }
            if let Some(shared) = &self.shared {
                let window = shared.stop_window(self.binding);
                let closed = shared.admission().stopped();
                self.shared_unknown = closed && matches!(window, Ok(None));
                self.shared_invalid |= window.is_err();
                if let Ok(Some(window)) = window {
                    self.observe_window(window);
                }
            }
        }
        self.control_failure.is_some()
            || self.control_history.overflow.is_some()
            || self.shared_unknown
            || self.shared_invalid
            || self
                .shared
                .as_ref()
                .is_some_and(|s| self.last.interrupt_100ns >= s.cooperative_raw())
            || self
                .requested
                .is_some_and(|r| !self.drain || self.now >= r.window.cooperative_ns)
    }
    fn observe_window(&mut self, window: StopWindow) {
        let p = Profile::approved_design().limits;
        if window.trigger_ns > self.now
            || window.cooperative_ns
                != window
                    .trigger_ns
                    .saturating_add(p.cooperative_stop_ns)
                    .min(self.run_end)
            || window.exit_ns
                != window
                    .trigger_ns
                    .saturating_add(p.exit_observation_ns)
                    .min(self.run_end)
        {
            self.control_failure = Some(window);
        } else {
            let observation = StopObservation {
                anchor: self.last,
                elapsed_ns: self.now,
                window,
            };
            if self
                .requested
                .is_none_or(|r| window.trigger_ns < r.window.trigger_ns)
            {
                if let Some(slot) = self
                    .control_history
                    .entries
                    .iter_mut()
                    .find(|e| e.is_none())
                {
                    *slot = Some(observation);
                    self.requested = Some(observation);
                } else {
                    self.control_history.overflow.get_or_insert(observation);
                }
            }
        }
    }
}
type RunTrace = CallTrace<Box<dyn FnMut() -> Option<Anchor>>, Box<dyn FnMut() -> bool>>;
struct Snapshot {
    dispatch: u32,
    output: Vec<f32>,
    sha256: Option<Digest>,
    offset: usize,
    _allocation: Reservation,
}

pub struct Producer<B, C, S> {
    binding: WorkerBinding,
    driver: B,
    context: Rc<RefCell<Context<C, S>>>,
    budget: Budget,
    failure_budget: Budget,
    prepared: PreparedReference,
    x: Vec<f32>,
    output: Vec<f32>,
    sentinel: Vec<f32>,
    _working: Reservation,
    channel: Channel,
    started: bool,
    in_flight: bool,
    native_idle: bool,
    dispatch: u32,
    phase: Phase,
    entry: Anchor,
    phase_deadline: u64,
    exit_deadline: Option<u64>,
    end: Option<End>,
    end_trigger: Option<u64>,
    trace: Option<RunTrace>,
    summary: Option<Record>,
    last_record: Option<TracedRecord>,
    bind_error: Option<Box<(RunTrace, Record)>>,
    failed_frame: Option<EncodedFrame>,
    phase_entry: Option<Message>,
    snapshot: Option<Snapshot>,
    readback_length: Option<usize>,
    transport_failed: bool,
    export_fault: Option<Fault>,
    cleanup: Option<Cleanup>,
    teardown: teardown::Ledger,
    control: Option<Arc<super::control::Shared>>,
    reports_sent: u32,
}
impl<
        B: Driver,
        C: FnMut() -> Option<Anchor> + 'static,
        S: FnMut() -> Option<StopWindow> + 'static,
    > Producer<B, C, S>
{
    /// CPU-only preparation; backend is already separately created. Accounts
    /// owned host buffers, not opaque driver allocations or an OS commit limit.
    pub fn prepare(
        binding: WorkerBinding,
        driver: B,
        clock: C,
        stop: S,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        Self::prepare_retained(binding, driver, clock, stop, budget).map_err(|f| f.fault)
    }
    #[allow(clippy::result_large_err)] // Return original owners without post-failure allocation.
    pub fn prepare_retained(
        binding: WorkerBinding,
        driver: B,
        clock: C,
        stop: S,
        budget: &Budget,
    ) -> Result<Self, PrepareFailure<B, C, S>> {
        let lanes = binding.plan.shape.lanes() as usize;
        let resources = (|| {
            if driver.identity() != binding.monitor.device || driver.shape() != binding.plan.shape {
                return Err(Fault::Binding);
            }
            let working = budget
                .reserve(
                    lanes * 28
                        + std::mem::size_of::<Self>()
                        + std::mem::size_of::<Context<C, S>>()
                        + 4096,
                )
                .map_err(|_| Fault::Capacity)?;
            let failure_budget = budget
                .partition(Profile::approved_design().limits.failure_snapshot_bytes as usize)
                .map_err(|_| Fault::Capacity)?;
            let validator = Clock::new(binding.monitor, binding.epoch).map_err(|_| Fault::Clock)?;
            let channel = Channel::new(
                binding,
                Lane::Ordinary,
                Direction::FromWorker,
                budget.clone(),
            )
            .map_err(|_| Fault::Capacity)?;
            Ok((working, failure_budget, validator, channel))
        })();
        let (working, failure_budget, validator, channel) = match resources {
            Ok(v) => v,
            Err(fault) => {
                return Err(PrepareFailure {
                    fault,
                    driver,
                    clock,
                    stop,
                })
            }
        };
        let context = Rc::new(RefCell::new(Context {
            clock,
            stop,
            validator,
            last: binding.epoch,
            now: 0,
            dispatch: 0,
            failure: None,
            requested: None,
            control_failure: None,
            run_end: binding.run_end(),
            drain: false,
            binding,
            shared: None,
            shared_unknown: false,
            shared_invalid: false,
            control_history: StopHistory::default(),
        }));
        let prepared = PreparedReference::new(Recipe::new(binding.plan.seed, binding.plan.shape));
        Ok(Self {
            binding,
            driver,
            context,
            budget: budget.clone(),
            failure_budget,
            prepared,
            x: vec![0.0; lanes],
            output: vec![f32::NAN; lanes],
            sentinel: vec![f32::NAN; lanes],
            _working: working,
            channel,
            started: false,
            in_flight: false,
            native_idle: true,
            dispatch: 0,
            phase: Phase::Warmup,
            entry: binding.epoch,
            phase_deadline: 0,
            exit_deadline: None,
            end: None,
            end_trigger: None,
            trace: None,
            summary: None,
            last_record: None,
            bind_error: None,
            failed_frame: None,
            phase_entry: None,
            snapshot: None,
            readback_length: None,
            transport_failed: false,
            export_fault: None,
            cleanup: None,
            teardown: teardown::Ledger::default(),
            control: None,
            reports_sent: 0,
        })
    }
    pub fn start_fixture(&mut self, start: StartObservation) -> Result<(), Fault> {
        if self.driver.provenance() != Provenance::SimulatedFixture {
            self.end.get_or_insert(End::Fault(Fault::Admission));
            return Err(Fault::Admission);
        }
        self.start(start)
    }
    /// Attach the exact independent control state before Start. This grants no
    /// workload admission and cannot replace an existing control binding.
    pub fn bind_control(&mut self, shared: Arc<super::control::Shared>) -> Result<(), Fault> {
        if self.started || self.control.is_some() || !shared.matches(self.binding) {
            shared.close();
            if let Some(existing) = &self.control {
                existing.close();
            }
            return Err(Fault::Binding);
        }
        self.control = Some(shared);
        self.context.borrow_mut().shared = self.control.clone();
        Ok(())
    }
    pub fn start_native(
        &mut self,
        start: StartObservation,
        admission: NativeAdmission,
    ) -> Result<(), Fault> {
        if admission.binding != self.binding
            || self.driver.provenance() != Provenance::Native
            || self.control.as_ref().is_none_or(|shared| {
                shared.admission().stopped()
                    || shared.admission().pending()
                    || shared.setup_observations().1.is_none()
            })
        {
            self.end.get_or_insert(End::Fault(Fault::Admission));
            return Err(Fault::Admission);
        }
        self.start(start)
    }
    fn start(&mut self, start: StartObservation) -> Result<(), Fault> {
        if self.started || self.end.is_some() || start.binding != self.binding {
            self.end.get_or_insert(End::Fault(Fault::Binding));
            return Err(Fault::Binding);
        }
        let mut c = self.context.borrow_mut();
        // Issued is a peer observation already checked by Gate. It can precede
        // our post-Ready queue receipt; validate against the original epoch
        // without rewinding this worker's continuous observation clock.
        self.binding
            .elapsed(start.issued)
            .map_err(|_| Fault::Clock)?;
        if start.issued.qpc_after > start.receipt.qpc_before
            || (start.receipt != c.last && start.receipt.qpc_before < c.last.qpc_after)
        {
            return Err(Fault::Clock);
        }
        if start.receipt != c.last {
            c.now = c
                .validator
                .observe(start.receipt)
                .map_err(|_| Fault::Clock)?;
        }
        c.last = start.receipt;
        self.entry = start.issued;
        let entered = self.binding.elapsed(self.entry).map_err(|_| Fault::Clock)?;
        self.phase_deadline = self
            .binding
            .plan
            .phase_deadline_ns(PlanPhase::Warmup, self.binding.origin_ns, entered)
            .map_err(|_| Fault::Deadline)?;
        self.started = true;
        Ok(())
    }
    pub fn end(&self) -> Option<End> {
        self.end
    }
    pub fn provenance(&self) -> Provenance {
        self.driver.provenance()
    }
    pub(super) fn identity(&self) -> Identity {
        self.driver.identity()
    }
    pub(super) fn inherit_setup(&mut self, anchor: Anchor) -> Result<(), Fault> {
        let mut c = self.context.borrow_mut();
        if self.started || c.failure.is_some() || anchor.qpc_before < c.last.qpc_after {
            return Err(Fault::Clock);
        }
        c.now = c.validator.observe(anchor).map_err(|_| Fault::Clock)?;
        c.last = anchor;
        Ok(())
    }
    pub(super) fn latest_observation(&self) -> Option<Anchor> {
        let c = self.context.borrow();
        c.failure.map_or(Some(c.last), |f| f.anchor)
    }
    #[cfg(windows)]
    pub(super) fn poll_clock(&mut self) -> Option<Anchor> {
        let mut c = self.context.borrow_mut();
        if c.failure.is_some() {
            None
        } else {
            (c.clock)()
        }
    }
    pub(super) fn observe_poll(&mut self, anchor: Option<Anchor>) -> Result<u64, Fault> {
        let mut c = self.context.borrow_mut();
        c.observe(anchor).ok_or(Fault::Clock)?;
        if c.now >= c.run_end {
            return Err(Fault::Deadline);
        }
        Ok(c.now)
    }
    pub fn reports_sent(&self) -> u32 {
        self.reports_sent
    }
    pub fn rejected_binding(&self) -> Option<(&Record, &[super::dispatch::Call])> {
        self.bind_error.as_ref().map(|raw| (&raw.1, raw.0.calls()))
    }
    pub fn last_record(&self) -> Option<&TracedRecord> {
        self.last_record.as_ref()
    }
    pub fn failed_frame(&self) -> Option<&EncodedFrame> {
        self.failed_frame.as_ref()
    }
    pub fn phase_entry(&self) -> Option<&Message> {
        self.phase_entry.as_ref()
    }
    pub fn clock_failure(&self) -> Option<ClockFailure> {
        self.context.borrow().failure
    }
    pub fn stop_observation(&self) -> Option<StopObservation> {
        self.context.borrow().requested
    }
    pub fn stop_history(&self) -> StopHistory {
        self.context.borrow().control_history
    }
    pub fn control_failure(&self) -> Option<StopWindow> {
        self.context.borrow().control_failure
    }
    pub fn exit_deadline_ns(&self) -> Option<u64> {
        self.exit_deadline
    }
    pub fn readback_length(&self) -> Option<usize> {
        self.readback_length
    }
    pub fn transport_failed(&self) -> bool {
        self.transport_failed
    }
    pub fn export_fault(&self) -> Option<Fault> {
        self.export_fault
    }
    pub fn retained_output(&self) -> &[f32] {
        self.snapshot
            .as_ref()
            .map_or(self.output.as_slice(), |s| s.output.as_slice())
    }
    pub fn failure_export_complete(&self) -> bool {
        self.export_fault.is_none()
            && !self.transport_failed
            && self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.offset == s.output.len())
    }
    pub fn failure_words_handed_off(&self) -> usize {
        self.snapshot.as_ref().map_or(0, |s| s.offset)
    }
    pub fn cleanup_observation(&self) -> Option<Cleanup> {
        self.cleanup
    }
    pub fn teardown_ledger(&self) -> &teardown::Ledger {
        &self.teardown
    }
    pub(super) fn setup_observation(&mut self, deadline: u64) -> Result<Anchor, Fault> {
        if self.started || self.end.is_some() || deadline > self.binding.setup_boundary() {
            return Err(Fault::Protocol);
        }
        self.guard(deadline)?;
        let c = self.context.borrow();
        if c.requested.is_some()
            || self
                .control
                .as_ref()
                .is_some_and(|s| s.admission().stopped())
        {
            return Err(Fault::Admission);
        }
        Ok(c.last)
    }
    pub(super) fn publish_ready(
        &mut self,
        message: &Message,
        sink: &mut impl FrameSink,
    ) -> Result<(), Fault> {
        let frame = self.channel.encode(message).map_err(|_| Fault::Protocol)?;
        self.send(frame, sink)
    }
    fn guard(&mut self, deadline: u64) -> Result<(), Fault> {
        let mut c = self.context.borrow_mut();
        c.capture().ok_or(Fault::Clock)?;
        c.control();
        if c.control_failure.is_some() {
            return Err(Fault::Protocol);
        }
        if c.control_history.overflow.is_some() {
            return Err(Fault::Capacity);
        }
        if c.shared_unknown || c.shared_invalid {
            return Err(Fault::Admission);
        }
        if self.end.is_some() {
            if let Some(request) = c.requested {
                self.exit_deadline = Some(
                    self.exit_deadline
                        .map_or(request.window.exit_ns, |n| n.min(request.window.exit_ns)),
                );
                self.end_trigger = Some(self.end_trigger.map_or(request.window.trigger_ns, |n| {
                    n.min(request.window.trigger_ns)
                }));
            }
        }
        let deadline = c
            .requested
            .map_or(deadline, |r| deadline.min(r.window.exit_ns));
        if c.now >= deadline {
            return Err(Fault::Deadline);
        }
        Ok(())
    }
    fn terminal(&mut self, end: End) {
        self.end.get_or_insert(end);
        let c = self.context.borrow();
        let first = c
            .requested
            .map_or(c.now, |r| r.window.trigger_ns.min(c.now));
        let trigger = *self.end_trigger.get_or_insert(first);
        let deadline = first
            .min(trigger)
            .saturating_add(Profile::approved_design().limits.exit_observation_ns)
            .min(self.binding.run_end());
        let deadline = c
            .requested
            .map_or(deadline, |r| deadline.min(r.window.exit_ns));
        self.exit_deadline = Some(
            self.exit_deadline
                .map_or(deadline, |prior| prior.min(deadline)),
        );
    }
    fn send(&mut self, frame: EncodedFrame, sink: &mut impl FrameSink) -> Result<(), Fault> {
        if self.transport_failed {
            return Err(Fault::Transport);
        }
        match sink.send(frame) {
            Ok(()) => Ok(()),
            Err(frame) => {
                self.failed_frame = Some(frame);
                self.transport_failed = true;
                self.channel.poison();
                Err(Fault::Transport)
            }
        }
    }
    fn emit_record(&mut self, sink: &mut impl FrameSink) {
        let Some(trace) = self.trace.take() else {
            return;
        };
        let mut summary = self.summary.take().expect("active summary");
        summary.pending = trace.pending();
        summary.verified_fma = if !summary.pending
            && summary.checked_lanes == summary.lanes
            && summary.readback_lanes == Some(summary.lanes)
            && summary.mismatches == 0
            && summary.input_sha256.is_some()
            && summary.output_sha256.is_some()
        {
            u64::from(summary.lanes) * u64::from(summary.steps)
        } else {
            0
        };
        match trace.bind(summary) {
            Ok(record) => self.last_record = Some(record),
            Err(raw) => {
                self.bind_error = Some(raw);
                self.terminal(End::Fault(Fault::Protocol));
                return;
            }
        }
        match self
            .channel
            .encode_traced(self.last_record.as_ref().expect("bound record"))
        {
            Ok(frame) => {
                if let Err(f) = self.send(frame, sink) {
                    self.terminal(End::Fault(f));
                } else {
                    self.reports_sent += 1;
                }
            }
            Err(_) => self.terminal(End::Fault(Fault::Capacity)),
        }
    }
    fn failed(&mut self, error: NativeFault, sink: &mut impl FrameSink) {
        self.native_idle = false;
        let end = match self.trace.as_ref().and_then(|t| t.fault()) {
            Some(TraceFault::Stopped) => {
                if self.context.borrow().control_history.overflow.is_some() {
                    End::Fault(Fault::Capacity)
                } else if self.context.borrow().shared_unknown
                    || self.context.borrow().shared_invalid
                {
                    End::Fault(Fault::Admission)
                } else if self.context.borrow().control_failure.is_some() {
                    End::Fault(Fault::Protocol)
                } else {
                    End::Stopped
                }
            }
            Some(TraceFault::Clock) => End::Fault(Fault::Clock),
            Some(TraceFault::Deadline) => End::Fault(Fault::Deadline),
            Some(TraceFault::Capacity) => End::Fault(Fault::Capacity),
            Some(TraceFault::Binding | TraceFault::Protocol) => End::Fault(Fault::Protocol),
            _ => End::Fault(Fault::Native(error)),
        };
        self.terminal(end);
        self.emit_record(sink);
    }
    fn new_trace(&mut self) -> Result<(), Fault> {
        self.context.borrow_mut().dispatch = self.dispatch;
        let clock = self.context.clone();
        let stop = self.context.clone();
        let clock: Box<dyn FnMut() -> Option<Anchor>> =
            Box::new(move || clock.borrow_mut().capture());
        let stop: Box<dyn FnMut() -> bool> = Box::new(move || stop.borrow_mut().control());
        let mut trace = CallTrace::new(
            self.binding,
            self.dispatch,
            self.phase,
            self.entry,
            clock,
            stop,
            &self.budget,
        )
        .map_err(|fault| match fault {
            TraceFault::Binding => Fault::Binding,
            TraceFault::Clock => Fault::Clock,
            TraceFault::Deadline => Fault::Deadline,
            TraceFault::Capacity => Fault::Capacity,
            _ => Fault::Protocol,
        })?;
        if let Some(shared) = &self.control {
            trace
                .bind_control(shared.clone())
                .map_err(|_| Fault::Binding)?;
        }
        self.trace = Some(trace);
        let p = self.binding.plan;
        self.summary = Some(Record {
            dispatch: self.dispatch,
            phase: self.phase,
            seed: p.seed,
            lanes: p.shape.lanes(),
            steps: p.shape.steps().value(),
            calls: Vec::new(),
            pending: false,
            readback_lanes: None,
            checked_lanes: 0,
            mismatches: 0,
            nonfinite: 0,
            first_failure: None,
            verified_fma: 0,
            input_sha256: None,
            output_sha256: None,
        });
        Ok(())
    }
    /// At most one submit or one query/completion/readback/check sequence. No
    /// successor dispatch is hidden in a call. Independent parent deadlines
    /// remain necessary for a blocked driver/sink; these callbacks cannot cancel it.
    pub fn advance(&mut self, sink: &mut impl FrameSink) {
        if self.end.is_some() {
            return;
        }
        if !self.started {
            self.terminal(End::Fault(Fault::Admission));
            return;
        }
        if !self.in_flight {
            self.context.borrow_mut().drain = false;
            let entering =
                self.dispatch == self.binding.plan.warmup_dispatches && self.phase == Phase::Warmup;
            let deadline = if entering {
                self.binding.origin_ns
                    + self.binding.plan.phase_caps_ns[..=PlanPhase::Measured as usize]
                        .iter()
                        .sum::<u64>()
            } else {
                self.phase_deadline
            };
            if let Err(f) = self.guard(deadline) {
                self.terminal(End::Fault(f));
                return;
            }
            if self.context.borrow().requested.is_some() {
                self.terminal(End::Stopped);
                return;
            }
            if self.dispatch == self.binding.plan.warmup_dispatches && self.phase == Phase::Warmup {
                self.phase = Phase::Measured;
                let c = self.context.borrow();
                self.entry = c.last;
                self.phase_deadline = match self.binding.plan.phase_deadline_ns(
                    PlanPhase::Measured,
                    self.binding.origin_ns,
                    c.now,
                ) {
                    Ok(n) => n,
                    Err(_) => {
                        drop(c);
                        self.terminal(End::Fault(Fault::Deadline));
                        return;
                    }
                };
                drop(c);
                self.phase_entry = Some(Message::PhaseEntered {
                    phase: Phase::Measured,
                    anchor: self.entry,
                });
                match self
                    .channel
                    .encode(self.phase_entry.as_ref().expect("entry"))
                {
                    Ok(frame) => {
                        if let Err(f) = self.send(frame, sink) {
                            self.terminal(End::Fault(f));
                            return;
                        }
                    }
                    Err(_) => {
                        self.terminal(End::Fault(Fault::Capacity));
                        return;
                    }
                }
            }
            if let Err(f) = self.new_trace() {
                self.terminal(End::Fault(f));
                return;
            }
            let trace = self.trace.as_mut().expect("new trace");
            if let Err(e) = trace.before_host(Operation::Input) {
                self.failed(e, sink);
                return;
            }
            let j = DispatchIndex::new(self.dispatch).expect("bound count");
            self.prepared
                .fill_x(j, &mut self.x)
                .expect("fixed input length");
            self.output.fill(f32::NAN);
            if let Err(e) = trace.after_host(Operation::Input) {
                self.failed(e, sink);
                return;
            }
            self.native_idle = false;
            let result = self.driver.submit(
                Inputs {
                    a: self.prepared.a(),
                    b: self.prepared.b(),
                    x: &self.x,
                    sentinel: &self.sentinel,
                    steps: self.binding.plan.shape.steps().value(),
                },
                trace,
            );
            if let Err(e) = result {
                self.failed(e, sink);
                return;
            }
            self.in_flight = true;
            return;
        }
        // Once submitted, stop closes successors but permits finite query/readback
        // and host checking under the original <=100/250 ms/phase ceilings.
        self.context.borrow_mut().drain = true;
        let trace = self.trace.as_mut().expect("in-flight trace");
        match self.driver.query(trace) {
            Ok(Query::Pending) => return,
            Ok(Query::Complete) => (),
            Err(e) => {
                self.failed(e, sink);
                return;
            }
        }
        let len = match self.driver.readback(&mut self.output, trace) {
            Ok(n) => n,
            Err(e) => {
                self.failed(e, sink);
                return;
            }
        };
        self.readback_length = Some(len);
        self.summary.as_mut().expect("summary").readback_lanes = u32::try_from(len).ok();
        self.native_idle = true;
        self.in_flight = false;
        if len != self.output.len() {
            self.terminal(End::Fault(Fault::Length));
            self.emit_record(sink);
            return;
        }
        if let Err(e) = trace.before_host(Operation::Check) {
            self.failed(e, sink);
            return;
        }
        let check = self
            .prepared
            .check(
                DispatchIndex::new(self.dispatch).expect("bound index"),
                &self.output,
            )
            .expect("fixed output length");
        let summary = self.summary.as_mut().expect("summary");
        summary.checked_lanes = check.checked_lanes();
        summary.mismatches = check.mismatches();
        summary.nonfinite = check.nonfinite();
        summary.first_failure = check.first_failure().map(|f| FirstFailure {
            lane: f.lane_index,
            expected: f.expected_integer,
            actual_bits: f.actual_bits,
            nonfinite: matches!(
                f.kind,
                autotune_fma_reference::reference::FailureKind::NonFinite
            ),
        });
        if !check.matches_reference() && self.end.is_none() {
            self.end = Some(End::Fault(Fault::Incorrect));
            let c = self.context.borrow();
            self.end_trigger = Some(
                c.requested
                    .map_or(c.now, |r| r.window.trigger_ns.min(c.now)),
            );
        }
        if let Err(e) = trace.after_host(Operation::Check) {
            if !check.matches_reference() {
                self.terminal(End::Fault(Fault::Incorrect));
            }
            self.failed(e, sink);
            self.snapshot();
            return;
        }
        if let Err(e) = trace.before_host(Operation::Hash) {
            self.failed(e, sink);
            self.snapshot();
            return;
        }
        let summary = self.summary.as_mut().expect("summary");
        summary.input_sha256 = Some(hash(&[self.prepared.a(), self.prepared.b(), &self.x]));
        summary.output_sha256 = Some(hash(&[&self.output]));
        if let Err(e) = trace.after_host(Operation::Hash) {
            if !check.matches_reference() {
                self.terminal(End::Fault(Fault::Incorrect));
            }
            self.failed(e, sink);
            self.snapshot();
            return;
        }
        if !check.matches_reference() {
            self.terminal(End::Fault(Fault::Incorrect));
        }
        self.emit_record(sink);
        if self.end.is_some() {
            self.snapshot();
            return;
        }
        if let Err(f) = self.guard(self.phase_deadline) {
            self.terminal(End::Fault(f));
            return;
        }
        if self.context.borrow().requested.is_some() {
            self.terminal(End::Stopped);
            return;
        }
        self.dispatch += 1;
        if self.dispatch
            == self.binding.plan.warmup_dispatches + self.binding.plan.measured_dispatches
        {
            let now = self.context.borrow().now;
            self.exit_deadline = self
                .binding
                .plan
                .phase_deadline_ns(PlanPhase::Exit, self.binding.origin_ns, now)
                .ok();
            self.terminal(End::ReportsComplete);
        } else {
            self.last_record = None;
        }
    }
    fn snapshot(&mut self) {
        if self.snapshot.is_some() {
            return;
        }
        let Some(record) = self.last_record.as_ref().map(TracedRecord::record) else {
            return;
        };
        if record.mismatches == 0 || record.readback_lanes != Some(record.lanes) {
            return;
        }
        let bytes = self.output.capacity() * 4;
        let allocation = match self
            .failure_budget
            .reserve(bytes + std::mem::size_of::<Snapshot>())
        {
            Ok(a) => a,
            Err(_) => {
                self.export_fault = Some(Fault::Capacity);
                return;
            }
        };
        self.snapshot = Some(Snapshot {
            dispatch: record.dispatch,
            output: std::mem::take(&mut self.output),
            sha256: record.output_sha256,
            offset: 0,
            _allocation: allocation,
        });
        let remaining = self._working.bytes() - bytes;
        self._working
            .shrink_to(remaining)
            .expect("transfer existing output charge");
    }
    /// One bounded first-failure frame; no native calls and no automatic retry.
    /// Missing digest/clock/transport evidence retains the original output.
    pub fn export_failure_step(&mut self, sink: &mut impl FrameSink) -> Result<bool, Fault> {
        if self.transport_failed {
            return Err(Fault::Transport);
        }
        if let Some(fault) = self.export_fault {
            return Err(fault);
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Err(Fault::Protocol);
        };
        if snapshot.sha256.is_none() {
            self.export_fault = Some(Fault::Protocol);
            return Err(Fault::Protocol);
        }
        if snapshot.offset == snapshot.output.len() {
            return Ok(true);
        }
        if let Err(f) = self.guard(self.exit_deadline.unwrap_or(self.binding.run_end())) {
            self.export_fault = Some(f);
            return Err(f);
        }
        let snapshot = self.snapshot.as_ref().expect("snapshot");
        let end = (snapshot.offset + CHUNK_LANES).min(snapshot.output.len());
        let _allocation = match self.failure_budget.reserve((end - snapshot.offset) * 4) {
            Ok(a) => a,
            Err(_) => {
                self.export_fault = Some(Fault::Capacity);
                return Err(Fault::Capacity);
            }
        };
        let message = Message::FailureChunk(FailureChunk {
            dispatch: snapshot.dispatch,
            offset: snapshot.offset as u32,
            total_lanes: snapshot.output.len() as u32,
            bits: snapshot.output[snapshot.offset..end]
                .iter()
                .map(|v| v.to_bits())
                .collect(),
        });
        let frame = match self.channel.encode(&message) {
            Ok(f) => f,
            Err(_) => {
                self.export_fault = Some(Fault::Capacity);
                return Err(Fault::Capacity);
            }
        };
        if let Err(f) = self.send(frame, sink) {
            self.export_fault = Some(f);
            return Err(f);
        }
        self.snapshot.as_mut().expect("snapshot").offset = end;
        if let Err(f) = self.guard(self.exit_deadline.unwrap_or(self.binding.run_end())) {
            self.export_fault = Some(f);
            return Err(f);
        }
        Ok(end == self.snapshot.as_ref().expect("snapshot").output.len())
    }
    /// Explicit one-shot cleanup only after known returned idle and terminal
    /// compute. Every return/clock uncertainty is retained; never proof of exit.
    pub fn cleanup(&mut self) {
        if self.cleanup.is_some() || !self.started || self.end.is_none() {
            return;
        }
        let mut observation = Cleanup {
            attempted: false,
            before: None,
            after: None,
            result: None,
        };
        if !self.started || self.end.is_none() || !self.native_idle {
            self.cleanup = Some(observation);
            return;
        }
        if self
            .guard(self.exit_deadline.unwrap_or(self.binding.run_end()))
            .is_err()
        {
            self.cleanup = Some(observation);
            return;
        }
        if self
            .context
            .borrow()
            .requested
            .is_some_and(|r| self.context.borrow().now >= r.window.cooperative_ns)
        {
            self.cleanup = Some(observation);
            return;
        }
        observation.before = Some(self.context.borrow().last);
        observation.attempted = true;
        self.native_idle = false;
        self.cleanup = Some(observation);
        let deadline = self.exit_deadline.unwrap_or(self.binding.run_end());
        let context = self.context.clone();
        let control = self.control.clone();
        // Terminal compute already forbids successors. Close the serial lane
        // after checking original control, before any explicit resource release.
        if let Some(shared) = &control {
            shared.close();
        }
        let mut check = || {
            let mut c = context.borrow_mut();
            let anchor = c.capture();
            let fault = if anchor.is_none() {
                Some(teardown::Fault::Clock)
            } else if c.control_failure.is_some() {
                Some(teardown::Fault::Protocol)
            } else if c.now >= deadline
                || control.as_ref().is_some_and(|shared| {
                    anchor.is_none_or(|a| a.interrupt_100ns >= shared.cooperative_raw())
                })
                || c.requested
                    .is_some_and(|r| c.now >= r.window.cooperative_ns || c.now >= r.window.exit_ns)
            {
                Some(teardown::Fault::Deadline)
            } else {
                None
            };
            teardown::Check {
                anchor: anchor.or(c.failure.and_then(|f| f.anchor)),
                fault,
            }
        };
        let result = self.driver.close(&mut self.teardown.controlled(
            &mut check,
            control.as_ref().map(|shared| shared.admission()),
        ));
        self.cleanup
            .as_mut()
            .expect("cleanup entry retained")
            .result = Some(result);
        let final_check = check();
        self.cleanup.as_mut().expect("cleanup entry retained").after = final_check.anchor;
        self.teardown.finish(result.is_ok(), final_check);
        if let Some(request) = self.context.borrow().requested {
            self.exit_deadline = Some(deadline.min(request.window.exit_ns));
            self.end_trigger = Some(self.end_trigger.map_or(request.window.trigger_ns, |n| {
                n.min(request.window.trigger_ns)
            }));
        }
    }
}
fn hash(slices: &[&[f32]]) -> Digest {
    let mut hash = Sha256::new();
    for slice in slices {
        for n in *slice {
            hash.update(n.to_bits().to_le_bytes());
        }
    }
    Digest::parse(&format!("{:x}", hash.finalize())).expect("SHA256 hex")
}
