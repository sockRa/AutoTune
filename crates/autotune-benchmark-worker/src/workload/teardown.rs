//! Per-call teardown evidence. Callbacks use the producer's continuous clock and
//! frozen exit/control bounds; this module neither loads nor calls a driver.
use crate::{monitor::Anchor, sampler::child::NativeAdmission, worker::Fault as NativeFault};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Stream,
    Buffer(u8),
    Module,
    Context,
    Library,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Return {
    Cuda(i32),
    Library {
        released: bool,
        error_code: Option<i32>,
    },
}
/// Stop immediately on rejection. Retain the native return before post-clock
/// observation; hooks perform no driver calls or implicit cleanup.
pub trait Observer {
    fn before(&mut self, operation: Operation) -> Result<(), NativeFault>;
    fn returned(&mut self, operation: Operation, raw: Return) -> Result<(), NativeFault>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Clock,
    Deadline,
    Protocol,
    Native(i32),
    Library(Option<i32>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub operation: Operation,
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    pub attempted: bool,
    pub raw: Option<Return>,
}
#[derive(Clone, Copy)]
pub struct Check {
    pub anchor: Option<Anchor>,
    pub fault: Option<Fault>,
}
/// Eight inline entries are accounted by Producer::prepare before any work.
/// A pending call is never cleared by a mismatched or missing return.
#[derive(Default)]
pub struct Ledger {
    entries: [Option<Entry>; 8],
    len: usize,
    pending: Option<usize>,
    fault: Option<Fault>,
    overflow: Option<(Operation, Option<Anchor>)>,
    rejected_return: Option<(Operation, Return, Option<Anchor>)>,
}
impl Ledger {
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries[..self.len].iter().flatten()
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn overflow(&self) -> Option<(Operation, Option<Anchor>)> {
        self.overflow
    }
    pub fn rejected_return(&self) -> Option<(Operation, Return, Option<Anchor>)> {
        self.rejected_return
    }
    pub fn observer<F: FnMut() -> Check>(&mut self, check: F) -> Checked<'_, F> {
        Checked {
            ledger: self,
            check,
            gate: None,
        }
    }
    pub(super) fn controlled<'a, F: FnMut() -> Check>(
        &'a mut self,
        check: F,
        gate: Option<&'a NativeAdmission>,
    ) -> Checked<'a, F> {
        Checked {
            ledger: self,
            check,
            gate,
        }
    }
    fn fail(&mut self, fault: Fault) -> NativeFault {
        self.fault.get_or_insert(fault);
        NativeFault {
            operation: "worker teardown observation",
            raw_status: match self.fault {
                Some(Fault::Native(s)) => s,
                _ => -2,
            },
        }
    }
    fn validate(&mut self, check: Check) -> Result<(), NativeFault> {
        if let Some(f) = check.fault {
            return Err(self.fail(f));
        }
        if check.anchor.is_none() {
            return Err(self.fail(Fault::Clock));
        }
        if let Some(f) = self.fault {
            return Err(self.fail(f));
        }
        Ok(())
    }
    /// Actual close result remains separate in Cleanup. A reported success must
    /// include seven returned CUDA releases and optionally one library release.
    pub(super) fn finish(&mut self, success: bool, check: Check) {
        let _ = self.validate(check);
        if success
            && (self.pending.is_some()
                || self.len < 7
                || self.entries().any(|e| !e.attempted || e.raw.is_none()))
        {
            self.fail(Fault::Protocol);
        }
    }
}
pub struct Checked<'a, F> {
    ledger: &'a mut Ledger,
    check: F,
    gate: Option<&'a NativeAdmission>,
}
impl<F> Drop for Checked<'_, F> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.ledger.fault.get_or_insert(Fault::Protocol);
            if let Some(gate) = self.gate {
                gate.stop();
            }
        }
    }
}
impl<F: FnMut() -> Check> Observer for Checked<'_, F> {
    fn before(&mut self, operation: Operation) -> Result<(), NativeFault> {
        let check = (self.check)();
        let i = self.ledger.len;
        if i == 8 {
            self.ledger
                .overflow
                .get_or_insert((operation, check.anchor));
            return Err(self.ledger.fail(Fault::Protocol));
        }
        self.ledger.entries[i] = Some(Entry {
            operation,
            before: check.anchor,
            after: None,
            attempted: false,
            raw: None,
        });
        self.ledger.len += 1;
        let expected = match i {
            0 => Operation::Stream,
            1..=4 => Operation::Buffer((i - 1) as u8),
            5 => Operation::Module,
            6 => Operation::Context,
            _ => Operation::Library,
        };
        if self.ledger.pending.is_some() || operation != expected {
            return Err(self.ledger.fail(Fault::Protocol));
        }
        self.ledger.validate(check)?;
        if self.gate.is_some_and(|gate| gate.begin_cleanup().is_err()) {
            return Err(self.ledger.fail(Fault::Protocol));
        }
        self.ledger.entries[i]
            .as_mut()
            .expect("retained entry")
            .attempted = true;
        self.ledger.pending = Some(i);
        Ok(())
    }
    fn returned(&mut self, operation: Operation, raw: Return) -> Result<(), NativeFault> {
        let Some(i) = self.ledger.pending.filter(|i| {
            self.ledger.entries[*i]
                .as_ref()
                .is_some_and(|e| e.operation == operation && e.raw.is_none())
        }) else {
            if self.ledger.rejected_return.is_none() {
                self.ledger.rejected_return = Some((operation, raw, None));
                let check = (self.check)();
                self.ledger.rejected_return = Some((operation, raw, check.anchor));
            }
            return Err(self.ledger.fail(Fault::Protocol));
        };
        self.ledger.entries[i].as_mut().expect("retained entry").raw = Some(raw);
        self.ledger.pending = None;
        if let Some(gate) = self.gate {
            gate.complete();
        }
        let check = (self.check)();
        self.ledger.entries[i]
            .as_mut()
            .expect("retained entry")
            .after = check.anchor;
        self.ledger.validate(check)?;
        match (operation, raw) {
            (
                Operation::Library,
                Return::Library {
                    released: true,
                    error_code: None,
                },
            ) => Ok(()),
            (
                Operation::Library,
                Return::Library {
                    released: false,
                    error_code,
                },
            ) => Err(self.ledger.fail(Fault::Library(error_code))),
            (Operation::Library, _) | (_, Return::Library { .. }) => {
                Err(self.ledger.fail(Fault::Protocol))
            }
            (_, Return::Cuda(0)) => Ok(()),
            (_, Return::Cuda(status)) => Err(self.ledger.fail(Fault::Native(status))),
        }
    }
}
