//! Serial pre-manifest native-call trace, composed with independent control.
//! Raw results must be owned by the caller before recording a post-call clock.
use super::{
    bootstrap::EntryClock, sampler_runtime::Call, starting::Scope, starting_control::Shared,
};
use crate::{
    evidence::Digest,
    monitor::Anchor,
    sampler::budget::{Budget, Reservation},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Clock,
    Deadline,
    Control,
    Protocol,
    Runtime,
    Read,
    Cleanup,
    Drain,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Open,
    Describe,
    Read,
    Cleanup,
}
pub struct Trace {
    scope: Digest,
    pub history: EntryClock,
    pub calls: [Call; 4],
    pub fault: Option<Fault>,
    pub cleanup_status: Option<i32>,
    pub stop_trigger: Option<Anchor>,
    pub drain_receipt: Option<Anchor>,
    pub output_drained: bool,
    pub control_finished: bool,
    _metadata: Reservation,
}
impl Trace {
    pub fn new(scope: &Scope, first: Anchor, budget: &Budget) -> Result<Self, Fault> {
        let d = scope.draft();
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let mut clock = crate::monitor::clock::Clock::temporal(d.frequency, d.clock_token, d.epoch)
            .map_err(|_| Fault::Clock)?;
        if first.qpc_before < d.epoch.qpc_after {
            return Err(Fault::Clock);
        }
        if clock.observe(first).map_err(|_| Fault::Clock)? >= p.preflight_ns {
            return Err(Fault::Deadline);
        }
        let metadata = budget
            .reserve(512 * 1024 + std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let history = EntryClock::with_capacity(
            d.frequency,
            d.epoch,
            first,
            ((p.preflight_ns + p.exit_observation_ns) / p.controller_tick_ns * 4 + 128) as usize,
            budget,
        )
        .map_err(|_| Fault::Clock)?;
        Ok(Self {
            scope: scope.digest(),
            history,
            calls: std::array::from_fn(|_| Call::default()),
            fault: None,
            cleanup_status: None,
            stop_trigger: None,
            drain_receipt: None,
            output_drained: false,
            control_finished: false,
            _metadata: metadata,
        })
    }
    pub fn fail(&mut self, f: Fault, shared: &Shared) {
        self.fault.get_or_insert(f);
        shared.close();
    }
    pub fn observe(&mut self, a: Option<Anchor>, shared: &Shared) -> Result<u64, Fault> {
        if shared.scope_digest() != self.scope {
            self.fail(Fault::Protocol, shared);
            return Err(Fault::Protocol);
        }
        let result = self.history.observe(a).map_err(|e| {
            if e == super::bootstrap::Error::Capacity {
                Fault::Capacity
            } else {
                Fault::Clock
            }
        });
        match result {
            Ok(n) => {
                let p = autotune_fma_reference::profile::Profile::approved_design().limits;
                if n >= p.preflight_ns && shared.window().is_none() {
                    let _ = shared.request_stop(a);
                }
                Ok(n)
            }
            Err(f) => {
                self.fail(f, shared);
                shared.fail(if f == Fault::Clock {
                    super::starting_control::Fault::Clock
                } else {
                    super::starting_control::Fault::Capacity
                });
                Err(f)
            }
        }
    }
    pub fn preflight(&mut self, a: Option<Anchor>, shared: &Shared) -> bool {
        match self.observe(a, shared) {
            Ok(n)
                if n < autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .preflight_ns
                    && !shared.admission().stopped()
                    && self.fault.is_none() =>
            {
                true
            }
            Ok(_) => {
                self.fail(Fault::Deadline, shared);
                false
            }
            Err(_) => false,
        }
    }
    pub fn begin(&mut self, op: Operation, a: Option<Anchor>, shared: &Shared) -> bool {
        let index = op as usize;
        if self.calls[index].requested {
            self.fail(Fault::Protocol, shared);
            return false;
        }
        self.calls[index].requested = true;
        self.calls[index].before = a;
        let predecessor = match op {
            Operation::Open => true,
            Operation::Describe => self.calls[0].returned,
            Operation::Read => self.calls[1].returned,
            Operation::Cleanup => true,
        };
        let valid = predecessor
            && if op == Operation::Cleanup {
                self.observe(a, shared).is_ok()
                    && a.is_some_and(|a| shared.cleanup_allowed(a))
                    && shared.admission().begin_cleanup().is_ok()
            } else {
                self.preflight(a, shared)
                    && if op == Operation::Read {
                        // Snapshot::collect_native owns its own begin/return pairing.
                        !shared.admission().pending()
                    } else {
                        shared.admission().begin().is_ok()
                    }
            };
        if !valid {
            self.fail(
                if op == Operation::Cleanup {
                    Fault::Cleanup
                } else {
                    Fault::Control
                },
                shared,
            );
            return false;
        }
        self.calls[index].attempted = true;
        true
    }
    pub fn returned(&mut self, op: Operation, a: Option<Anchor>, shared: &Shared) {
        let index = op as usize;
        if !self.calls[index].attempted || self.calls[index].returned {
            self.fail(Fault::Protocol, shared);
            return;
        }
        self.calls[index].returned = true;
        self.calls[index].after = a;
        if op != Operation::Read {
            shared.admission().complete();
        }
        if let Ok(n) = self.observe(a, shared) {
            if op != Operation::Cleanup
                && n >= autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .preflight_ns
            {
                self.fail(Fault::Deadline, shared);
            }
            if matches!(op, Operation::Open | Operation::Describe) && shared.admission().stopped() {
                self.fail(Fault::Control, shared);
            }
        }
    }
    pub fn cleanup_returned(&mut self, a: Option<Anchor>, status: Option<i32>, shared: &Shared) {
        if !self.calls[3].attempted || self.calls[3].returned {
            self.fail(Fault::Protocol, shared);
            return;
        }
        self.cleanup_status = status;
        self.returned(Operation::Cleanup, a, shared);
        if status != Some(0) || a.is_none_or(|a| a.interrupt_100ns >= shared.cooperative_raw()) {
            self.fail(Fault::Cleanup, shared);
        }
    }
    /// Capture after polling booleans. Queue drain is not receipt or durability.
    pub fn drain(
        &mut self,
        a: Option<Anchor>,
        output: bool,
        control: bool,
        shared: &Shared,
    ) -> bool {
        self.output_drained = output;
        self.control_finished = control;
        self.drain_receipt = a;
        if self.observe(a, shared).is_err()
            || a.is_none_or(|a| a.interrupt_100ns >= shared.cooperative_raw())
        {
            self.fail(Fault::Drain, shared);
            return false;
        }
        output && control
    }
}
