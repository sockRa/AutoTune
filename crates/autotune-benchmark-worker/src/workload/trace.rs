//! Raw per-call observations for the actual CUDA adapter. No native admission.
use super::{
    dispatch::{Call, Operation, Phase, Record},
    wire::{Message, WorkerBinding},
};
use crate::{
    monitor::{clock::Clock, Anchor},
    sampler::budget::{Budget, Reservation},
    worker::Fault as BackendFault,
};
use autotune_fma_reference::profile::{Phase as PlanPhase, Profile};
use std::sync::Arc;

/// Adapter must stop immediately on observer failure and keep native uncertainty
/// sticky. A callback may inspect clock/control; it must never call the driver.
pub trait Observer {
    fn before(&mut self, operation: Operation) -> Result<(), BackendFault>;
    fn after(&mut self, operation: Operation, status: i32) -> Result<(), BackendFault>;
}
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
pub(crate) struct Unobserved;
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
impl Observer for Unobserved {
    fn before(&mut self, _: Operation) -> Result<(), BackendFault> {
        Ok(())
    }
    fn after(&mut self, _: Operation, _: i32) -> Result<(), BackendFault> {
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Binding,
    Clock,
    Deadline,
    Stopped,
    Capacity,
    Protocol,
    Native { operation: Operation, status: i32 },
}
const CALLS: usize = 64;

/// Owns raw calls and their charge. Construction computes the approved original
/// phase bound; the producer must pass the same frozen entry for every dispatch.
/// Source callbacks are injected; constructing a trace cannot load any driver.
pub struct CallTrace<C, S> {
    binding: WorkerBinding,
    phase: Phase,
    dispatch: u32,
    clock: C,
    stop: S,
    validator: Clock,
    last: Anchor,
    calls: Vec<Call>,
    active: bool,
    pending: bool,
    fault: Option<Fault>,
    unmatched: Option<Call>,
    deadline_ns: u64,
    cycle_start: Option<u64>,
    stream_start: Option<u64>,
    _allocation: Reservation,
    control: Option<Arc<super::control::Shared>>,
    native_active: bool,
}
impl<C: FnMut() -> Option<Anchor>, S: FnMut() -> bool> CallTrace<C, S> {
    pub fn new(
        binding: WorkerBinding,
        dispatch: u32,
        phase: Phase,
        first_entry: Anchor,
        clock: C,
        stop: S,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        if dispatch >= binding.plan.warmup_dispatches + binding.plan.measured_dispatches
            || (dispatch < binding.plan.warmup_dispatches) != (phase == Phase::Warmup)
        {
            return Err(Fault::Binding);
        }
        let allocation = budget
            .reserve(std::mem::size_of::<(Self, Record)>() + CALLS * std::mem::size_of::<Call>())
            .map_err(|_| Fault::Capacity)?;
        let mut validator = Clock::new(binding.monitor, binding.epoch).map_err(|_| Fault::Clock)?;
        let entered = validator.observe(first_entry).map_err(|_| Fault::Clock)?;
        let plan_phase = match phase {
            Phase::Warmup => PlanPhase::Warmup,
            Phase::Measured => PlanPhase::Measured,
        };
        let deadline_ns = binding
            .plan
            .phase_deadline_ns(plan_phase, binding.origin_ns, entered)
            .map_err(|_| Fault::Deadline)?;
        Ok(Self {
            binding,
            phase,
            dispatch,
            clock,
            stop,
            validator,
            last: first_entry,
            calls: Vec::with_capacity(CALLS),
            active: false,
            pending: false,
            fault: None,
            unmatched: None,
            deadline_ns,
            cycle_start: None,
            stream_start: None,
            _allocation: allocation,
            control: None,
            native_active: false,
        })
    }
    /// Exact serial host-call bookkeeping. Does not grant workload admission;
    /// the stop callback must enforce original drain windows independently.
    pub fn bind_control(&mut self, shared: Arc<super::control::Shared>) -> Result<(), Fault> {
        if self.control.is_some() || !self.calls.is_empty() || !shared.matches(self.binding) {
            shared.close();
            return Err(Fault::Binding);
        }
        self.control = Some(shared);
        Ok(())
    }
    pub fn calls(&self) -> &[Call] {
        &self.calls
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn pending(&self) -> bool {
        self.pending
    }
    pub fn unmatched(&self) -> Option<&Call> {
        self.unmatched.as_ref()
    }
    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ns
    }
    fn fail(&mut self, f: Fault) -> BackendFault {
        self.fault.get_or_insert(f);
        if let Some(shared) = &self.control {
            if f == Fault::Stopped {
                shared.close();
            } else {
                shared.close_at(
                    self.binding,
                    if f == Fault::Clock {
                        None
                    } else {
                        self.calls.last().and_then(|c| c.after)
                    },
                );
            }
        }
        match self.fault.expect("first fault") {
            Fault::Native { operation, status } => BackendFault {
                operation: native_name(operation),
                raw_status: status,
            },
            _ => BackendFault {
                operation: "worker call observation",
                raw_status: -2,
            },
        }
    }
    fn capture(&mut self) -> (Option<Anchor>, Result<u64, Fault>) {
        let a = (self.clock)();
        let value = a.ok_or(Fault::Clock).and_then(|a| {
            if a.qpc_before < self.last.qpc_after {
                return Err(Fault::Clock);
            }
            let ns = self.validator.observe(a).map_err(|_| Fault::Clock)?;
            self.last = a;
            Ok(ns)
        });
        (a, value)
    }
    fn check(&self, now: u64) -> Result<(), Fault> {
        if self.control.as_ref().is_some_and(|s| {
            self.binding
                .epoch
                .interrupt_100ns
                .checked_add(now / 100)
                .is_none_or(|raw| raw >= s.cooperative_raw())
        }) {
            return Err(Fault::Stopped);
        }
        let limits = Profile::approved_design().limits;
        if now >= self.deadline_ns
            || self
                .cycle_start
                .is_some_and(|n| now.saturating_sub(n) >= limits.cycle_ns)
            || self
                .stream_start
                .is_some_and(|n| now.saturating_sub(n) >= limits.dispatch_completion_ns)
        {
            return Err(Fault::Deadline);
        }
        Ok(())
    }
    fn begin(&mut self, op: Operation) -> Result<(), BackendFault> {
        if let Some(f) = self.fault {
            return Err(self.fail(f));
        }
        if self.active {
            return Err(self.fail(Fault::Protocol));
        }
        if self.calls.len() == CALLS {
            return Err(self.fail(Fault::Capacity));
        }
        self.calls.push(Call {
            operation: op,
            before: None,
            after: None,
            status: None,
        });
        let (anchor, now) = self.capture();
        self.calls.last_mut().expect("new call").before = anchor;
        let now = now.map_err(|f| self.fail(f))?;
        if op == Operation::Input {
            self.cycle_start.get_or_insert(now);
        }
        if op == Operation::UploadA {
            self.stream_start.get_or_insert(now);
        }
        self.check(now).map_err(|f| self.fail(f))?;
        if (self.stop)() {
            return Err(self.fail(Fault::Stopped));
        }
        self.active = true;
        if matches!(
            op,
            Operation::UploadA
                | Operation::UploadB
                | Operation::UploadX
                | Operation::Sentinel
                | Operation::UploadCompletion
                | Operation::Launch
                | Operation::Query
                | Operation::Readback
        ) {
            if let Some(shared) = &self.control {
                let entered = if shared.admission().stopped() {
                    if matches!(op, Operation::Query | Operation::Readback)
                        && matches!(shared.stop_window(self.binding), Ok(Some(window)) if now < window.cooperative_ns)
                    {
                        shared.admission().begin_drain()
                    } else {
                        return Err(self.fail(Fault::Stopped));
                    }
                } else {
                    shared.admission().begin()
                };
                if entered.is_err() {
                    return Err(self.fail(Fault::Protocol));
                }
                self.native_active = true;
            }
            self.pending = true;
        }
        Ok(())
    }
    fn finish(&mut self, op: Operation, status: Option<i32>) -> Result<(), BackendFault> {
        if !self.active || self.calls.last().is_none_or(|c| c.operation != op) {
            let (after, _) = self.capture();
            self.unmatched.get_or_insert(Call {
                operation: op,
                before: None,
                after,
                status,
            });
            return Err(self.fail(Fault::Protocol));
        }
        // Store the raw return before attempting the post-call clock.
        self.calls.last_mut().expect("active call").status = status;
        if self.native_active {
            if let Some(shared) = &self.control {
                shared.admission().complete();
            }
            self.native_active = false;
        }
        if status == Some(0)
            && matches!(
                op,
                Operation::UploadCompletion | Operation::Query | Operation::Readback
            )
        {
            self.pending = false;
        }
        let native_failure =
            status.is_some_and(|n| n != 0 && !(op == Operation::Query && n == 600));
        if native_failure {
            self.fault.get_or_insert(Fault::Native {
                operation: op,
                status: status.expect("native return"),
            });
        }
        let (after, now) = self.capture();
        self.calls.last_mut().expect("active call").after = after;
        self.active = false;
        let now = match now {
            Ok(n) => n,
            Err(f) => return Err(self.fail(f)),
        };
        // Returned completion ends the stream interval; readback/check/hash are
        // still charged against the complete cycle and original phase ceiling.
        let checked = self.check(now);
        if status == Some(0) && op == Operation::Query {
            self.stream_start = None;
        }
        checked.map_err(|f| self.fail(f))?;
        if let Some(f) = self.fault {
            return Err(self.fail(f));
        }
        if (self.stop)() {
            return Err(self.fail(Fault::Stopped));
        }
        Ok(())
    }
    pub fn before_host(&mut self, op: Operation) -> Result<(), BackendFault> {
        if !matches!(op, Operation::Input | Operation::Check | Operation::Hash) {
            return Err(self.fail(Fault::Protocol));
        }
        self.begin(op)
    }
    pub fn after_host(&mut self, op: Operation) -> Result<(), BackendFault> {
        self.finish(op, None)
    }
    /// Transfer calls and their reservation into an immutable original-bound
    /// owner. Incompatible summaries retain both raw sources on rejection.
    pub fn bind(self, mut summary: Record) -> Result<TracedRecord, Box<(Self, Record)>> {
        if !summary.calls.is_empty()
            || summary.dispatch != self.dispatch
            || summary.phase != self.phase
            || summary.pending != self.pending
            || summary.validate(self.binding).is_err()
        {
            return Err(Box::new((self, summary)));
        }
        summary.calls = self.calls;
        Ok(TracedRecord {
            binding: self.binding,
            message: Message::DispatchRecord(summary),
            fault: self.fault,
            unmatched: self.unmatched,
            _allocation: self._allocation,
        })
    }
}
impl<C: FnMut() -> Option<Anchor>, S: FnMut() -> bool> Observer for CallTrace<C, S> {
    fn before(&mut self, op: Operation) -> Result<(), BackendFault> {
        self.begin(op)
    }
    fn after(&mut self, op: Operation, status: i32) -> Result<(), BackendFault> {
        self.finish(op, Some(status))
    }
}
pub struct TracedRecord {
    pub(super) binding: WorkerBinding,
    pub(super) message: Message,
    fault: Option<Fault>,
    unmatched: Option<Call>,
    _allocation: Reservation,
}
impl TracedRecord {
    pub fn record(&self) -> &Record {
        let Message::DispatchRecord(record) = &self.message else {
            unreachable!("fixed trace variant")
        };
        record
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn unmatched(&self) -> Option<&Call> {
        self.unmatched.as_ref()
    }
}
fn native_name(op: Operation) -> &'static str {
    match op {
        Operation::UploadA | Operation::UploadB | Operation::UploadX | Operation::Sentinel => {
            "cuMemcpyHtoD_v2"
        }
        Operation::UploadCompletion => "cuStreamQuery(upload completion)",
        Operation::Launch => "cuLaunchKernel",
        Operation::Query => "cuStreamQuery",
        Operation::Readback => "cuMemcpyDtoH_v2",
        _ => "worker host stage",
    }
}
