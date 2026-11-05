//! Original-deadline child finalization evidence, never OS exit/GPU proof.
use super::{
    bootstrap::EntryClock, control::Shared, producer::Cleanup, teardown, wire::WorkerBinding,
};
use crate::{
    monitor::{
        clock::{Clock, ClockError},
        Anchor,
    },
    sampler::{
        budget::{Budget, Reservation},
        runtime::IoObservation,
        wire::Lane,
    },
};
use std::sync::Arc;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Clock,
    Capacity,
    Deadline,
    Protocol,
    Cleanup,
    Control,
    Io,
}
/// Continuous validation across entry, setup, producer and finalization. Valid
/// raw anchors belong to the respective stage ledgers; the first bad raw anchor
/// is retained here before the downstream callback receives None.
pub struct ClockChain {
    validator: Clock,
    last: Anchor,
    fault: Option<ClockError>,
    bad: Option<Anchor>,
    missing: bool,
}
impl ClockChain {
    pub fn new(binding: WorkerBinding, first: Anchor) -> Result<Self, Fault> {
        let mut validator =
            Clock::new(binding.monitor(), binding.epoch).map_err(|_| Fault::Clock)?;
        if first.qpc_before < binding.epoch.qpc_after {
            return Err(Fault::Clock);
        }
        validator.observe(first).map_err(|_| Fault::Clock)?;
        Ok(Self {
            validator,
            last: first,
            fault: None,
            bad: None,
            missing: false,
        })
    }
    pub fn observe(&mut self, raw: Option<Anchor>) -> Option<Anchor> {
        if self.fault.is_some() || self.missing {
            return None;
        }
        let Some(a) = raw else {
            self.missing = true;
            return None;
        };
        let result = if a.qpc_before < self.last.qpc_after {
            Err(ClockError::Order)
        } else {
            self.validator.observe(a)
        };
        match result {
            Ok(_) => {
                self.last = a;
                Some(a)
            }
            Err(f) => {
                self.fault = Some(f);
                self.bad = Some(a);
                None
            }
        }
    }
    pub fn fault(&self) -> Option<ClockError> {
        self.fault
    }
    pub fn bad(&self) -> Option<Anchor> {
        self.bad
    }
    pub fn missing(&self) -> bool {
        self.missing
    }
    pub fn last(&self) -> Anchor {
        self.last
    }
}
pub struct State {
    binding: WorkerBinding,
    shared: Arc<Shared>,
    original_deadline_ns: u64,
    pub history: EntryClock,
    pub fault: Option<Fault>,
    pub cleanup: Option<Cleanup>,
    pub teardown_fault: Option<teardown::Fault>,
    cleanup_recorded: bool,
    pub completion: Option<Result<(), super::control::Fault>>,
    pub output_drained: bool,
    pub control_finished: bool,
    pub drain_receipt: Option<Anchor>,
    pub io_shutdown: Option<[IoObservation; 4]>,
    pub io_completion: Option<[bool; 4]>,
    pub io_receipt: Option<Anchor>,
    _metadata: Reservation,
}
impl State {
    pub fn binding(&self) -> WorkerBinding {
        self.binding
    }
    /// deadline_ns must be the already-frozen producer exit deadline or the
    /// independently published original stop exit window, never now + grace.
    pub fn new(
        binding: WorkerBinding,
        shared: Arc<Shared>,
        first: Anchor,
        deadline_ns: u64,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        if !shared.matches(binding) || deadline_ns > binding.run_end() {
            return Err(Fault::Protocol);
        }
        let history = EntryClock::new(
            binding.monitor().qpc_frequency,
            binding.epoch,
            first,
            budget,
        )
        .map_err(|e| {
            if e == super::bootstrap::Error::Capacity {
                Fault::Capacity
            } else {
                Fault::Clock
            }
        })?;
        let metadata = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let state = Self {
            binding,
            shared,
            original_deadline_ns: deadline_ns,
            history,
            fault: None,
            cleanup: None,
            teardown_fault: None,
            cleanup_recorded: false,
            completion: None,
            output_drained: false,
            control_finished: false,
            drain_receipt: None,
            io_shutdown: None,
            io_completion: None,
            io_receipt: None,
            _metadata: metadata,
        };
        if binding.elapsed(first).map_err(|_| Fault::Clock)? >= deadline_ns
            || first.interrupt_100ns >= state.shared.cooperative_raw()
        {
            return Err(Fault::Deadline);
        }
        Ok(state)
    }
    pub fn original_deadline_ns(&self) -> u64 {
        self.original_deadline_ns
    }
    pub fn fail(&mut self, f: Fault) {
        self.fault.get_or_insert(f);
    }
    fn within(&mut self, a: Option<Anchor>) -> bool {
        match self.history.observe(a) {
            Err(e) => {
                self.fail(if e == super::bootstrap::Error::Capacity {
                    Fault::Capacity
                } else {
                    Fault::Clock
                });
                false
            }
            Ok(n) => {
                if n >= self.original_deadline_ns
                    || a.is_none_or(|v| v.interrupt_100ns >= self.shared.cooperative_raw())
                {
                    self.fail(Fault::Deadline);
                    false
                } else {
                    true
                }
            }
        }
    }
    pub fn record_cleanup(&mut self, cleanup: Option<Cleanup>, fault: Option<teardown::Fault>) {
        if self.cleanup_recorded {
            self.fail(Fault::Protocol);
            return;
        }
        self.cleanup_recorded = true;
        self.cleanup = cleanup;
        self.teardown_fault = fault;
        if fault.is_some()
            || !cleanup.is_some_and(|c| {
                c.attempted && c.before.is_some() && c.after.is_some() && c.result == Some(Ok(()))
            })
        {
            self.fail(Fault::Cleanup);
        }
        if let Some(c) = cleanup {
            let _ = self.within(c.before);
            let _ = self.within(c.after);
        }
    }
    pub fn completion_queued(&mut self, result: Result<(), super::control::Fault>) {
        if self.completion.is_some() {
            self.fail(Fault::Protocol);
            return;
        }
        self.completion = Some(result);
        if !self.cleanup_recorded {
            self.fail(Fault::Protocol);
        }
        if result.is_err() {
            self.fail(Fault::Control);
        }
    }
    /// Poll booleans before taking this receipt clock. Queue drain is local
    /// ownership completion, not peer delivery, durability or process exit.
    pub fn drain(&mut self, a: Option<Anchor>, output: bool, control: bool) -> bool {
        self.output_drained = output;
        self.control_finished = control;
        self.drain_receipt = a;
        self.within(a) && output && control
    }
    pub fn io_started(&mut self, raw: [IoObservation; 4], a: Option<Anchor>) {
        if self.io_shutdown.is_some() {
            self.fail(Fault::Protocol);
            return;
        }
        self.io_shutdown = Some(raw);
        self.io_receipt = a;
        if !self.output_drained || !self.control_finished {
            self.fail(Fault::Io);
        }
        if raw.iter().enumerate().any(|(i, o)| {
            o.reader != (i < 2)
                || o.lane
                    != if i % 2 == 0 {
                        Lane::Ordinary
                    } else {
                        Lane::Control
                    }
        }) {
            self.fail(Fault::Protocol);
        }
        let _ = self.within(a);
    }
    /// Thread completion is observed separately from cancellation's raw result.
    /// A failed cancellation can never be reported as causing completion.
    pub fn io_poll(&mut self, raw: [bool; 4], a: Option<Anchor>) -> bool {
        self.io_completion = Some(raw);
        self.io_receipt = a;
        if self.io_shutdown.is_none() {
            self.fail(Fault::Protocol);
            return false;
        }
        self.within(a) && raw.iter().all(|done| *done)
    }
    pub fn complete(&self) -> bool {
        self.fault.is_none()
            && self.shared.admission().stopped()
            && !self.shared.admission().pending()
            && self.cleanup_recorded
            && self.completion == Some(Ok(()))
            && self.output_drained
            && self.control_finished
            && self.io_shutdown.is_some()
            && self.io_completion == Some([true; 4])
    }
}
