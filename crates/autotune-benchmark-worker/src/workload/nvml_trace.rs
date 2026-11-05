//! Bounded per-ABI startup/runtime observations; not native execution authority.
use crate::{
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        child::NativeAdmission,
    },
};
use autotune_probe::nvml::{SamplerCall, SamplerObserver, SamplerReturn};
use autotune_probe::runtime_file::{Stage, MAX_CHECKS};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileCheck {
    pub stage: Stage,
    pub anchor: Option<Anchor>,
}
const CAP: usize = 1 + 1 + 1024 * 3 + 3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Clock,
    Deadline,
    Stopped,
    Protocol,
    Native(i32),
}
pub struct Entry {
    pub call: SamplerCall,
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    pub attempted: bool,
    pub raw: Option<SamplerReturn>,
}
pub struct Ledger {
    clock: Clock,
    last: Anchor,
    deadline_ns: u64,
    entries: Vec<Entry>,
    pending: Option<usize>,
    fault: Option<Fault>,
    overflow: Option<(SamplerCall, Option<Anchor>)>,
    rejected_return: Option<(SamplerCall, SamplerReturn, Option<Anchor>)>,
    _allocation: Reservation,
    file_checks: Vec<FileCheck>,
    file_overflow: Option<FileCheck>,
}
impl Ledger {
    pub fn new(
        frequency: u64,
        epoch: Anchor,
        first: Anchor,
        deadline_ns: u64,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let mut clock =
            Clock::temporal(frequency, epoch.clock_token, epoch).map_err(|_| Fault::Clock)?;
        if first.qpc_before < epoch.qpc_after {
            return Err(Fault::Clock);
        }
        let n = clock.observe(first).map_err(|_| Fault::Clock)?;
        if n >= deadline_ns {
            return Err(Fault::Deadline);
        }
        let allocation = budget
            .reserve(
                std::mem::size_of::<Self>()
                    + CAP * std::mem::size_of::<Entry>()
                    + MAX_CHECKS * std::mem::size_of::<FileCheck>()
                    + 512 * 1024,
            )
            .map_err(|_| Fault::Capacity)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(CAP)
            .map_err(|_| Fault::Capacity)?;
        let mut file_checks = Vec::new();
        file_checks
            .try_reserve_exact(MAX_CHECKS)
            .map_err(|_| Fault::Capacity)?;
        Ok(Self {
            clock,
            last: first,
            deadline_ns,
            entries,
            pending: None,
            fault: None,
            overflow: None,
            rejected_return: None,
            _allocation: allocation,
            file_checks,
            file_overflow: None,
        })
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn file_checks(&self) -> &[FileCheck] {
        &self.file_checks
    }
    pub fn file_overflow(&self) -> Option<FileCheck> {
        self.file_overflow
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn overflow(&self) -> Option<(SamplerCall, Option<Anchor>)> {
        self.overflow
    }
    pub fn rejected_return(&self) -> Option<(SamplerCall, SamplerReturn, Option<Anchor>)> {
        self.rejected_return
    }
    pub fn observer<'a, F: FnMut() -> Option<Anchor>>(
        &'a mut self,
        gate: &'a NativeAdmission,
        clock: F,
    ) -> Observer<'a, F> {
        Observer {
            ledger: self,
            gate,
            clock,
        }
    }
    fn fail(&mut self, f: Fault, gate: &NativeAdmission) {
        self.fault.get_or_insert(f);
        gate.stop();
    }
    fn check(&mut self, a: Option<Anchor>) -> Result<(), Fault> {
        let a = a.ok_or(Fault::Clock)?;
        if a.qpc_before < self.last.qpc_after {
            return Err(Fault::Clock);
        }
        let n = self.clock.observe(a).map_err(|_| Fault::Clock)?;
        self.last = a;
        if n >= self.deadline_ns {
            return Err(Fault::Deadline);
        }
        Ok(())
    }
}
pub struct Observer<'a, F> {
    ledger: &'a mut Ledger,
    gate: &'a NativeAdmission,
    clock: F,
}
impl<F> Drop for Observer<'_, F> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.ledger.fault.get_or_insert(Fault::Protocol);
            self.gate.stop();
        }
    }
}
impl<F: FnMut() -> Option<Anchor>> SamplerObserver for Observer<'_, F> {
    fn file_check(&mut self, stage: Stage) -> bool {
        let anchor = (self.clock)();
        let raw = FileCheck { stage, anchor };
        if self.ledger.file_checks.len() == MAX_CHECKS {
            self.ledger.file_overflow.get_or_insert(raw);
            self.ledger.fail(Fault::Capacity, self.gate);
            return false;
        }
        self.ledger.file_checks.push(raw);
        let result = if self.ledger.pending.is_some() {
            Err(Fault::Protocol)
        } else if let Some(f) = self.ledger.fault {
            Err(f)
        } else {
            self.ledger.check(anchor).and_then(|()| {
                if self.gate.stopped() {
                    Err(Fault::Stopped)
                } else {
                    Ok(())
                }
            })
        };
        if let Err(f) = result {
            self.ledger.fail(f, self.gate);
            return false;
        }
        true
    }
    fn before(&mut self, call: SamplerCall) -> bool {
        let a = (self.clock)();
        if self.ledger.entries.len() == CAP {
            self.ledger.overflow.get_or_insert((call, a));
            self.ledger.fail(Fault::Capacity, self.gate);
            return false;
        }
        let index = self.ledger.entries.len();
        self.ledger.entries.push(Entry {
            call,
            before: a,
            after: None,
            attempted: false,
            raw: None,
        });
        let result = if self.ledger.pending.is_some() {
            Err(Fault::Protocol)
        } else if let Some(f) = self.ledger.fault {
            Err(f)
        } else {
            self.ledger.check(a).and_then(|()| {
                if self.gate.stopped() {
                    Err(Fault::Stopped)
                } else {
                    Ok(())
                }
            })
        };
        if let Err(f) = result {
            self.ledger.fail(f, self.gate);
            return false;
        }
        self.ledger.entries[index].attempted = true;
        self.ledger.pending = Some(index);
        true
    }
    fn returned(&mut self, call: SamplerCall, raw: SamplerReturn) -> bool {
        let index = self.ledger.pending;
        if !index.is_some_and(|i| {
            self.ledger.entries[i].call == call && self.ledger.entries[i].raw.is_none()
        }) {
            if self.ledger.rejected_return.is_none() {
                self.ledger.rejected_return = Some((call, raw, None));
                let a = (self.clock)();
                self.ledger.rejected_return = Some((call, raw, a));
            }
            self.ledger.fail(Fault::Protocol, self.gate);
            return false;
        }
        let i = index.expect("checked pending index");
        // Raw return owned BEFORE the potentially failing post-clock callback.
        self.ledger.entries[i].raw = Some(raw);
        if raw.status != 0 {
            self.ledger.fail(Fault::Native(raw.status), self.gate);
        }
        let a = (self.clock)();
        self.ledger.entries[i].after = a;
        self.ledger.pending = None;
        let checked = self.ledger.check(a);
        if let Err(f) = checked {
            self.ledger.fail(f, self.gate);
        }
        if self.gate.stopped() && self.ledger.fault.is_none() {
            self.ledger.fail(Fault::Stopped, self.gate);
        }
        self.ledger.fault.is_none()
    }
}
