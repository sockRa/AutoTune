//! Actual setup observer bound to original worker clock/control. No loader,
//! executable or workload admission issuer; callbacks never call a driver.
use super::{control::Shared, wire::WorkerBinding};
use crate::{
    cuda::{SetupObserver, SetupOperation, SetupReturn},
    monitor::{clock::Clock, Anchor},
    sampler::budget::{Budget, Reservation},
    worker::Fault as NativeFault,
};
use std::sync::Arc;

// Adapter permits at most 64 devices, two calls each, five attributes for the
// unique match, three initial calls and ten context/module/buffer/stream calls.
const CAP: usize = 3 + 64 * 2 + 5 + 10;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Binding,
    Capacity,
    Clock,
    Deadline,
    Stopped,
    Protocol,
    Native(i32),
}
pub struct Entry {
    pub operation: SetupOperation,
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    pub attempted: bool,
    pub raw: Option<SetupReturn>,
}
pub struct Ledger {
    binding: WorkerBinding,
    shared: Arc<Shared>,
    clock: Clock,
    last: Anchor,
    deadline_raw: u64,
    entries: Vec<Entry>,
    pending: Option<usize>,
    fault: Option<Fault>,
    overflow: Option<(SetupOperation, Option<Anchor>)>,
    rejected_return: Option<(SetupOperation, SetupReturn, Option<Anchor>)>,
    _allocation: Reservation,
}
impl Ledger {
    /// Requires the control state's one-shot original bootstrap setup deadline.
    /// Setup completion is still the caller's Ready/receipt step, not this log.
    pub fn new(
        binding: WorkerBinding,
        shared: Arc<Shared>,
        first: Anchor,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let (deadline, completed) = shared.setup_observations();
        if !shared.matches(binding) || deadline.is_none() || completed.is_some() {
            shared.close();
            return Err(Fault::Binding);
        }
        let result = (|| {
            let mut clock =
                Clock::new(binding.monitor(), binding.epoch).map_err(|_| Fault::Clock)?;
            if first.qpc_before < binding.epoch.qpc_after {
                return Err(Fault::Clock);
            }
            clock.observe(first).map_err(|_| Fault::Clock)?;
            let deadline_raw = deadline.expect("checked original deadline");
            if first.interrupt_100ns >= deadline_raw {
                return Err(Fault::Deadline);
            }
            if shared.admission().stopped() {
                return Err(Fault::Stopped);
            }
            let allocation = budget
                .reserve(std::mem::size_of::<Self>() + CAP * std::mem::size_of::<Entry>())
                .map_err(|_| Fault::Capacity)?;
            let mut entries = Vec::new();
            entries
                .try_reserve_exact(CAP)
                .map_err(|_| Fault::Capacity)?;
            Ok((clock, deadline_raw, allocation, entries))
        })();
        let (clock, deadline_raw, allocation, entries) = match result {
            Ok(v) => v,
            Err(f) => {
                if f == Fault::Deadline {
                    shared.expire_setup(binding);
                } else {
                    shared.close_at(binding, None);
                }
                return Err(f);
            }
        };
        Ok(Self {
            binding,
            shared,
            clock,
            last: first,
            deadline_raw,
            entries,
            pending: None,
            fault: None,
            overflow: None,
            rejected_return: None,
            _allocation: allocation,
        })
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn overflow(&self) -> Option<(SetupOperation, Option<Anchor>)> {
        self.overflow
    }
    pub fn rejected_return(&self) -> Option<(SetupOperation, SetupReturn, Option<Anchor>)> {
        self.rejected_return
    }
    pub fn observer<F: FnMut() -> Option<Anchor>>(&mut self, clock: F) -> Observer<'_, F> {
        Observer {
            ledger: self,
            clock,
        }
    }
    fn fail(&mut self, fault: Fault, anchor: Option<Anchor>) -> NativeFault {
        self.fault.get_or_insert(fault);
        if fault == Fault::Deadline {
            self.shared.expire_setup(self.binding);
        } else {
            self.shared.close_at(
                self.binding,
                if fault == Fault::Clock { None } else { anchor },
            );
        }
        NativeFault {
            operation: "worker CUDA setup observation",
            raw_status: match self.fault {
                Some(Fault::Native(s)) => s,
                _ => -2,
            },
        }
    }
    fn check(&mut self, anchor: Option<Anchor>) -> Result<(), Fault> {
        let a = anchor.ok_or(Fault::Clock)?;
        if a.qpc_before < self.last.qpc_after {
            return Err(Fault::Clock);
        }
        self.clock.observe(a).map_err(|_| Fault::Clock)?;
        self.last = a;
        if a.interrupt_100ns >= self.deadline_raw {
            return Err(Fault::Deadline);
        }
        if self.shared.admission().stopped() {
            return Err(Fault::Stopped);
        }
        if self.shared.setup_observations().1.is_some() {
            return Err(Fault::Protocol);
        }
        Ok(())
    }
}
pub struct Observer<'a, F> {
    ledger: &'a mut Ledger,
    clock: F,
}
impl<F> Drop for Observer<'_, F> {
    fn drop(&mut self) {
        // A clock callback may unwind after a returned result was retained.
        // Close admission without changing an unresolved pending native call.
        if std::thread::panicking() {
            self.ledger.shared.close_at(self.ledger.binding, None);
        }
    }
}
impl<F: FnMut() -> Option<Anchor>> SetupObserver for Observer<'_, F> {
    fn before(&mut self, operation: SetupOperation) -> Result<(), NativeFault> {
        let a = (self.clock)();
        if self.ledger.entries.len() == CAP {
            self.ledger.overflow.get_or_insert((operation, a));
            return Err(self.ledger.fail(Fault::Capacity, a));
        }
        let index = self.ledger.entries.len();
        self.ledger.entries.push(Entry {
            operation,
            before: a,
            after: None,
            attempted: false,
            raw: None,
        });
        if self.ledger.pending.is_some() {
            return Err(self.ledger.fail(Fault::Protocol, a));
        }
        if let Some(f) = self.ledger.fault {
            return Err(self.ledger.fail(f, a));
        }
        if let Err(f) = self.ledger.check(a) {
            return Err(self.ledger.fail(f, a));
        }
        if self.ledger.shared.admission().begin().is_err() {
            return Err(self.ledger.fail(Fault::Stopped, a));
        }
        self.ledger.entries[index].attempted = true;
        self.ledger.pending = Some(index);
        Ok(())
    }
    fn returned(&mut self, operation: SetupOperation, raw: SetupReturn) -> Result<(), NativeFault> {
        let Some(index) = self.ledger.pending.filter(|i| {
            self.ledger.entries[*i].operation == operation && self.ledger.entries[*i].raw.is_none()
        }) else {
            if self.ledger.rejected_return.is_none() {
                self.ledger.rejected_return = Some((operation, raw, None));
                let a = (self.clock)();
                self.ledger.rejected_return = Some((operation, raw, a));
            }
            return Err(self.ledger.fail(Fault::Protocol, None));
        };
        // Keep the actual return before ANY post-clock callback can fail/panic.
        self.ledger.entries[index].raw = Some(raw);
        self.ledger.pending = None;
        self.ledger.shared.admission().complete();
        let a = (self.clock)();
        self.ledger.entries[index].after = a;
        let checked = self.ledger.check(a);
        if let Err(f) = checked {
            return Err(self.ledger.fail(f, a));
        }
        if raw.status != 0 {
            return Err(self.ledger.fail(Fault::Native(raw.status), a));
        }
        if let Some(f) = self.ledger.fault {
            return Err(self.ledger.fail(f, a));
        }
        Ok(())
    }
}
