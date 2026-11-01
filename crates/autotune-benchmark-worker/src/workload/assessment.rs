//! Parent-side audit of retained worker reports. No launch/native authority.
//! Callers retain each owned raw message and receipt, including every rejection.
use super::{
    dispatch::{Operation, Phase, Record},
    wire::{Message, WorkerBinding},
};
use crate::{
    monitor::{clock::Clock, Anchor},
    sampler::budget::{Budget, Reservation},
};
use autotune_fma_reference::profile::{Phase as PlanPhase, Profile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Binding,
    Sequence,
    Clock,
    Deadline,
    CallTrace,
    Pending,
    Check,
    Closed,
    Capacity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub warmup: u32,
    pub measured: u32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Warmup,
    AwaitMeasurement,
    Measured,
    Complete,
    Closed,
}

/// Bounded CPU audit, intended to run after raw evidence has been retained.
/// Passing means complete consistent *reports*, not independently replayed GPU
/// correctness, consent, coverage, process exit or baseline acceptance.
pub struct Audit {
    binding: WorkerBinding,
    source: Clock,
    receipts: Clock,
    ticks: Clock,
    latest_tick_ns: u64,
    last_source: Anchor,
    state: State,
    deadline_ns: u64,
    progress: Progress,
    fault: Option<Fault>,
    _allocation: Reservation,
}
impl Audit {
    /// The Start must already have been admitted by the parent safety/consent
    /// path. Constructing this report auditor supplies no such permission.
    pub fn new(binding: WorkerBinding, issued: Anchor, budget: &Budget) -> Result<Self, Fault> {
        Message::Start { issued }
            .validate(binding)
            .map_err(|_| Fault::Binding)?;
        let allocation = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let mut source = Clock::new(binding.monitor, binding.epoch).map_err(|_| Fault::Clock)?;
        let start_ns = source.observe(issued).map_err(|_| Fault::Clock)?;
        let mut receipts = Clock::new(binding.monitor, binding.epoch).map_err(|_| Fault::Clock)?;
        receipts.observe(issued).map_err(|_| Fault::Clock)?;
        let mut ticks = Clock::new(binding.monitor, binding.epoch).map_err(|_| Fault::Clock)?;
        ticks.observe(issued).map_err(|_| Fault::Clock)?;
        let deadline_ns = binding
            .plan
            .phase_deadline_ns(PlanPhase::Warmup, binding.origin_ns, start_ns)
            .map_err(|_| Fault::Deadline)?;
        Ok(Self {
            binding,
            source,
            receipts,
            ticks,
            latest_tick_ns: start_ns,
            last_source: issued,
            state: State::Warmup,
            deadline_ns,
            progress: Progress {
                warmup: 0,
                measured: 0,
            },
            fault: None,
            _allocation: allocation,
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ns
    }
    pub fn reports_complete(&self) -> bool {
        self.progress.warmup == self.binding.plan.warmup_dispatches
            && self.progress.measured == self.binding.plan.measured_dispatches
    }
    /// External stop/loss closes assessment; no late evidence reopens it.
    pub fn close(&mut self, fault: Fault) {
        self.fault.get_or_insert(fault);
        self.state = State::Closed;
    }
    fn fail<T>(&mut self, fault: Fault) -> Result<T, Fault> {
        self.close(fault);
        Err(fault)
    }
    fn receipt(&mut self, receipt: Option<Anchor>) -> Result<u64, Fault> {
        let Some(receipt) = receipt else {
            return Err(Fault::Clock);
        };
        self.receipts.observe(receipt).map_err(|_| Fault::Clock)
    }
    /// Independent controller ticks must continue even when no messages arrive.
    /// Before measurement entry is observed only its immutable cumulative bound
    /// is known. The entry receipt freezes its relative cap without rebasing.
    pub fn tick(&mut self, receipt: Option<Anchor>) -> Result<(), Fault> {
        if matches!(self.state, State::Closed | State::Complete) {
            return Err(Fault::Closed);
        }
        let now = match receipt.and_then(|a| self.ticks.observe(a).ok()) {
            Some(n) => n,
            None => return self.fail(Fault::Clock),
        };
        self.latest_tick_ns = now;
        if now >= self.deadline_ns {
            return self.fail(Fault::Deadline);
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        message: &Message,
        receipt: Option<Anchor>,
    ) -> Result<Progress, Fault> {
        if self.state == State::Closed {
            return Err(Fault::Closed);
        }
        if self.state == State::Complete {
            return self.fail(Fault::Sequence);
        }
        if message.validate(self.binding).is_err() {
            return self.fail(Fault::Binding);
        }
        let now = match self.receipt(receipt) {
            Ok(n) => n.max(self.latest_tick_ns),
            Err(f) => return self.fail(f),
        };
        if now >= self.deadline_ns {
            return self.fail(Fault::Deadline);
        }
        let receipt = receipt.expect("validated receipt");
        let result = match message {
            Message::PhaseEntered {
                phase: Phase::Measured,
                anchor,
            } => self.measurement(*anchor, receipt, now),
            Message::DispatchRecord(record) => self.record(record, receipt, now),
            _ => Err(Fault::Sequence),
        };
        match result {
            Ok(()) => Ok(self.progress),
            Err(f) => self.fail(f),
        }
    }
    fn source_anchor(&mut self, anchor: Anchor) -> Result<u64, Fault> {
        if anchor.qpc_before < self.last_source.qpc_after {
            return Err(Fault::Clock);
        }
        let ns = self.source.observe(anchor).map_err(|_| Fault::Clock)?;
        self.last_source = anchor;
        Ok(ns)
    }
    fn measurement(&mut self, anchor: Anchor, receipt: Anchor, now: u64) -> Result<(), Fault> {
        if self.state != State::AwaitMeasurement {
            return Err(Fault::Sequence);
        }
        if anchor.qpc_after > receipt.qpc_before {
            return Err(Fault::Clock);
        }
        let entered = self.source_anchor(anchor)?;
        self.deadline_ns = self
            .binding
            .plan
            .phase_deadline_ns(PlanPhase::Measured, self.binding.origin_ns, entered)
            .map_err(|_| Fault::Deadline)?;
        if now >= self.deadline_ns {
            return Err(Fault::Deadline);
        }
        self.state = State::Measured;
        Ok(())
    }
    fn record(&mut self, r: &Record, receipt: Anchor, now: u64) -> Result<(), Fault> {
        if !matches!(self.state, State::Warmup | State::Measured)
            || r.dispatch != self.progress.warmup + self.progress.measured
        {
            return Err(Fault::Sequence);
        }
        if r.pending {
            return Err(Fault::Pending);
        }
        if r.checked_lanes != r.lanes
            || r.readback_lanes != Some(r.lanes)
            || r.mismatches != 0
            || r.nonfinite != 0
            || r.first_failure.is_some()
            || r.input_sha256.is_none()
            || r.output_sha256.is_none()
        {
            return Err(Fault::Check);
        }
        self.call_order(r)?;
        let mut cycle_start = None;
        let mut submitted = None;
        let mut completion = None;
        let mut end = 0;
        for c in &r.calls {
            let before = c.before.ok_or(Fault::Clock)?;
            let after = c.after.ok_or(Fault::Clock)?;
            let start_ns = self.source_anchor(before)?;
            end = self.source_anchor(after)?;
            if cycle_start.is_none() {
                cycle_start = Some(start_ns);
            }
            // The first stream upload begins submit-to-completion accounting;
            // upload/sentinel time cannot become free pre-launch allowance.
            if c.operation == Operation::UploadA {
                submitted = Some(start_ns);
            }
            if c.operation == Operation::Query && c.status == Some(0) {
                completion = Some(end);
            }
            if end >= self.deadline_ns {
                return Err(Fault::Deadline);
            }
        }
        if self.last_source.qpc_after > receipt.qpc_before {
            return Err(Fault::Clock);
        }
        let limits = Profile::approved_design().limits;
        if completion
            .ok_or(Fault::CallTrace)?
            .saturating_sub(submitted.ok_or(Fault::CallTrace)?)
            >= limits.dispatch_completion_ns
            || end.saturating_sub(cycle_start.ok_or(Fault::CallTrace)?) >= limits.cycle_ns
        {
            return Err(Fault::Deadline);
        }
        if now >= self.deadline_ns {
            return Err(Fault::Deadline);
        }
        match self.state {
            State::Warmup => {
                self.progress.warmup += 1;
                if self.progress.warmup == self.binding.plan.warmup_dispatches {
                    self.state = State::AwaitMeasurement;
                    // Relative measurement entry is not yet observed. Do not
                    // synthesize/rebase it from this controller receipt.
                    self.deadline_ns = self.binding.origin_ns
                        + self.binding.plan.phase_caps_ns[..=PlanPhase::Measured as usize]
                            .iter()
                            .sum::<u64>();
                }
            }
            State::Measured => {
                self.progress.measured += 1;
                if self.progress.measured == self.binding.plan.measured_dispatches {
                    self.state = State::Complete;
                }
            }
            _ => return Err(Fault::Sequence),
        }
        Ok(())
    }
    fn call_order(&self, r: &Record) -> Result<(), Fault> {
        use Operation::*;
        let prefix = [
            Input,
            UploadA,
            UploadB,
            UploadX,
            Sentinel,
            UploadCompletion,
            Launch,
        ];
        if r.calls.len() < prefix.len() + 4 {
            return Err(Fault::CallTrace);
        }
        for (c, op) in r.calls.iter().zip(prefix) {
            if c.operation != op
                || (op == Input && !matches!(c.status, None | Some(0)))
                || (op != Input && c.status != Some(0))
            {
                return Err(Fault::CallTrace);
            }
        }
        let mut index = prefix.len();
        loop {
            let c = r.calls.get(index).ok_or(Fault::CallTrace)?;
            if c.operation != Query {
                return Err(Fault::CallTrace);
            }
            index += 1;
            match c.status {
                Some(0) => break,
                Some(600) => (), // CUDA_ERROR_NOT_READY, same ABI as cuda::query.
                None => return Err(Fault::Pending),
                _ => return Err(Fault::CallTrace),
            }
        }
        for op in [Readback, Check, Hash] {
            let c = r.calls.get(index).ok_or(Fault::CallTrace)?;
            if c.operation != op
                || (op == Readback && c.status != Some(0))
                || (op != Readback && !matches!(c.status, None | Some(0)))
            {
                return Err(Fault::CallTrace);
            }
            index += 1;
        }
        if index != r.calls.len() {
            return Err(Fault::CallTrace);
        }
        Ok(())
    }
}
