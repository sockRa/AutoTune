//! Parent-side bounded reception of one starting-response export. The strict
//! assembler is driven under the original epoch/deadline and a sticky stop;
//! receipt is not peer identity, native admission, durability or qualification.
use super::{
    starting::Scope,
    starting_bootstrap::Declaration,
    starting_delivery::{Assembler, Fault as Assembly, Status},
};
use crate::sampler::budget::Budget;
/// Parent-side reception fault, kept separate from the assembler's own bounded
/// chunk faults so supervision failures stay distinguishable after completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Assembly(Assembly),
    Clock,
    Deadline,
    Stopped,
}
/// One-shot receiver for the single starting response of an original session.
/// A stop, deadline, clock or assembly fault is sticky; no later chunk resumes
/// reception and no payload is exposed unless every chunk verified.
pub struct Receiver {
    assembler: Assembler,
    epoch_ns: u64,
    deadline_ns: u64,
    last_ns: u64,
    stopped: bool,
    fault: Option<Fault>,
}
impl Receiver {
    /// Freeze the original-epoch deadline before any chunk is accepted. A
    /// deadline that does not follow the epoch is rejected without allocation.
    pub fn new(
        declaration: &Declaration,
        scope: &Scope,
        epoch_ns: u64,
        deadline_ns: u64,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        if deadline_ns <= epoch_ns {
            return Err(Fault::Deadline);
        }
        Ok(Self {
            assembler: Assembler::new(declaration, scope, budget).map_err(Fault::Assembly)?,
            epoch_ns,
            deadline_ns,
            last_ns: epoch_ns,
            stopped: false,
            fault: None,
        })
    }
    pub fn epoch_ns(&self) -> u64 {
        self.epoch_ns
    }
    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ns
    }
    pub fn received(&self) -> u32 {
        self.assembler.received()
    }
    pub fn complete(&self) -> bool {
        self.fault.is_none() && self.assembler.complete()
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
            .or_else(|| self.assembler.fault().map(Fault::Assembly))
    }
    pub fn status(&self) -> Status {
        self.assembler.status()
    }
    /// Explicit stop closes reception for every successor chunk. The assembler
    /// state already received is retained and remains incomplete. A stop after
    /// a verified completion is redundant and cannot withdraw that payload.
    pub fn stop(&mut self) {
        if self.complete() {
            return;
        }
        self.stopped = true;
        self.fault.get_or_insert(Fault::Stopped);
    }
    /// Only a fully verified payload bound to the parent's declaration/scope is
    /// exposed, and only while no supervision fault was recorded.
    pub fn canonical(&self) -> Option<&[u8]> {
        if self.fault.is_some() {
            return None;
        }
        self.assembler.canonical()
    }
    /// One bounded chunk with the parent's observed receive clock. Rejection
    /// leaves the borrowed bytes caller-owned; the first fault is retained.
    pub fn accept(&mut self, now_ns: u64, bytes: &[u8]) -> Result<Status, Fault> {
        if let Some(fault) = self.fault() {
            return Err(fault);
        }
        if self.assembler.complete() {
            // A stray chunk after verification cannot withdraw or replace the
            // completed payload; the rejection is reported without a new fault.
            return Err(Fault::Assembly(Assembly::Stopped));
        }
        if self.stopped {
            return Err(Fault::Stopped);
        }
        if now_ns < self.last_ns {
            self.fault = Some(Fault::Clock);
            return Err(Fault::Clock);
        }
        self.last_ns = now_ns;
        if now_ns >= self.deadline_ns {
            self.fault = Some(Fault::Deadline);
            return Err(Fault::Deadline);
        }
        match self.assembler.accept(bytes) {
            Ok(_) => Ok(self.assembler.status()),
            Err(fault) => {
                self.fault = Some(Fault::Assembly(fault));
                Err(Fault::Assembly(fault))
            }
        }
    }
}
