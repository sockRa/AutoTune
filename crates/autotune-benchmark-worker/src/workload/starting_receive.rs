//! Parent-side bounded reception of one starting-response export. The strict
//! assembler is driven under the original epoch/deadline and a sticky stop;
//! receipt is not peer identity, native admission, durability or qualification.
use super::{
    starting::Scope,
    starting_bootstrap::Declaration,
    starting_delivery::{Assembler, Fault as Assembly, Status},
};
use crate::sampler::budget::Budget;
#[cfg(windows)]
use {
    super::{
        transport::RawReceived,
        windows_transport::{InheritedTransport, Receive},
    },
    crate::{monitor::Anchor, sampler::wire::Lane},
};
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
    /// Advance the receive clock without a chunk. The deadline is enforced on
    /// idle polls too, so a silent peer cannot keep reception open forever.
    pub fn tick(&mut self, now_ns: u64) -> Result<(), Fault> {
        if let Some(fault) = self.fault() {
            return Err(fault);
        }
        if self.assembler.complete() {
            return Ok(());
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
        Ok(())
    }
    /// A missing parent clock observation is retained as a clock fault; it can
    /// never be treated as an on-time or on-deadline observation.
    pub fn clock_lost(&mut self) {
        if !self.complete() {
            self.fault.get_or_insert(Fault::Clock);
        }
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

/// One concrete inherited-pipe poll result. Supervisory facts are retained
/// separately from the receiver's sticky fault so the OS cause is not lost.
#[cfg(windows)]
pub struct Poll {
    pub anchor: Option<Anchor>,
    pub transport_fault: bool,
    pub os_code: Option<i32>,
    pub idle: bool,
    pub finished: bool,
    /// A rejected packet stays caller-owned; it never replaces accepted bytes.
    pub rejected: Option<RawReceived>,
}
/// Drive one nonblocking owned-pipe poll into the strict receiver. The observed
/// anchor is converted to the receiver's elapsed-ns domain before any deadline
/// check, so a raw QPC reading can never be compared against an elapsed bound.
/// Idle polls still advance the original-epoch deadline; a closed or faulty peer
/// closes reception instead of waiting without a bound.
#[cfg(windows)]
pub fn poll_native(
    receiver: &mut Receiver,
    transport: &InheritedTransport,
    mut clock: impl FnMut() -> Option<Anchor>,
    mut elapsed: impl FnMut(Anchor) -> Option<u64>,
) -> Poll {
    if transport.inputs_finished() && !receiver.complete() {
        receiver.stop();
    }
    match transport.receive(&mut clock) {
        Receive::Packet(raw) => {
            let anchor = raw.receipt();
            let accepted = if raw.lane() != Lane::Ordinary || raw.packet().rejection().is_some() {
                receiver.stop();
                Err(())
            } else {
                match anchor.and_then(&mut elapsed) {
                    Some(now) => receiver
                        .accept(now, raw.packet().bytes())
                        .map(|_| ())
                        .map_err(|_| ()),
                    None => {
                        receiver.clock_lost();
                        Err(())
                    }
                }
            };
            let rejected = accepted.err().map(|_| raw);
            Poll {
                anchor,
                transport_fault: false,
                os_code: None,
                idle: false,
                finished: false,
                rejected,
            }
        }
        Receive::Idle { anchor } => {
            match anchor.and_then(&mut elapsed) {
                Some(now) => {
                    let _ = receiver.tick(now);
                }
                None => receiver.clock_lost(),
            }
            Poll {
                anchor,
                transport_fault: false,
                os_code: None,
                idle: true,
                finished: transport.inputs_finished() && !receiver.complete(),
                rejected: None,
            }
        }
        Receive::Fault { code, anchor } => {
            receiver.stop();
            Poll {
                anchor,
                transport_fault: true,
                os_code: code,
                idle: false,
                finished: false,
                rejected: None,
            }
        }
    }
}
