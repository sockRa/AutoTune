//! One bounded byte-stream frame with lossless prefix/partial-body retention.
//! The Windows reader uses this same implementation; no platform call runs here.
use super::{
    budget::{Budget, BudgetError, Reservation},
    wire::{Channel, WireError},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Prefix,
    Body,
    Decode,
    Queue,
    Write,
}
impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Body => "body",
            Self::Decode => "decode",
            Self::Queue => "queue",
            Self::Write => "write",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadFault {
    Eof,
    Os(u32),
    InvalidCount,
    Length,
    Capacity,
    Allocation,
    Queue,
    Stopped,
    Wire(WireError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejection {
    pub fault: ReadFault,
    pub phase: Phase,
    pub reported: u64,
    pub queue_overflow: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadStep {
    pub reported: usize,
    pub error: Option<u32>,
}
pub trait Source {
    fn read(&mut self, output: &mut [u8]) -> ReadStep;
}
pub trait Sink {
    fn write(&mut self, input: &[u8]) -> ReadStep;
}
/// Preserve partial-write counts even when the OS reports failure alongside
/// nonzero bytes. A failed send is never retried as a new protocol message.
pub fn write_packet(sink: &mut impl Sink, bytes: &[u8]) -> Result<(), Rejection> {
    let mut sent = 0;
    while sent < bytes.len() {
        let step = sink.write(&bytes[sent..]);
        let fault = if step.reported > bytes.len() - sent {
            Some(ReadFault::InvalidCount)
        } else {
            sent += step.reported;
            step.error
                .map(ReadFault::Os)
                .or_else(|| (step.reported == 0).then_some(ReadFault::Eof))
        };
        if let Some(fault) = fault {
            return Err(Rejection {
                fault,
                phase: Phase::Write,
                reported: if fault == ReadFault::InvalidCount {
                    step.reported as u64
                } else {
                    sent as u64
                },
                queue_overflow: false,
            });
        }
    }
    Ok(())
}
pub struct Packet {
    bytes: Vec<u8>,
    charge: Reservation,
    rejection: Option<Rejection>,
}
impl Packet {
    #[cfg(windows)]
    pub(super) fn from_parts(
        bytes: Vec<u8>,
        charge: Reservation,
        rejection: Option<Rejection>,
    ) -> Self {
        Self {
            bytes,
            charge,
            rejection,
        }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn rejection(&self) -> Option<Rejection> {
        self.rejection
    }
    pub fn into_parts(self) -> (Vec<u8>, Reservation, Option<Rejection>) {
        (self.bytes, self.charge, self.rejection)
    }
}
pub struct Framer {
    budget: Budget,
    prefix: Budget,
    failed: bool,
}
impl Framer {
    pub fn new(budget: Budget) -> Result<Self, BudgetError> {
        let prefix = budget.partition(4)?;
        Ok(Self {
            budget,
            prefix,
            failed: false,
        })
    }
    pub fn read(&mut self, source: &mut impl Source) -> Packet {
        if self.failed {
            return Packet {
                bytes: Vec::new(),
                charge: self.prefix.reserve(0).expect("zero charge"),
                rejection: Some(Rejection {
                    fault: ReadFault::Stopped,
                    phase: Phase::Prefix,
                    reported: 0,
                    queue_overflow: false,
                }),
            };
        }
        let mut charge = match self.prefix.reserve(4) {
            Ok(c) => c,
            Err(_) => {
                return self.reject(
                    Vec::new(),
                    self.prefix.reserve(0).expect("zero charge"),
                    ReadFault::Capacity,
                    Phase::Prefix,
                    0,
                )
            }
        };
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(4).is_err() {
            return self.reject(bytes, charge, ReadFault::Allocation, Phase::Prefix, 0);
        }
        bytes.resize(4, 0);
        if let Err((fault, filled, reported)) = fill(source, &mut bytes) {
            bytes.truncate(filled);
            return self.reject(bytes, charge, fault, Phase::Prefix, reported);
        }
        let length =
            match Channel::checked_length(bytes[..4].try_into().expect("four prefix bytes")) {
                Ok(n) => n,
                Err(_) => return self.reject(bytes, charge, ReadFault::Length, Phase::Prefix, 4),
            };
        let next = match self.budget.reserve(length) {
            Ok(c) => c,
            Err(_) => return self.reject(bytes, charge, ReadFault::Capacity, Phase::Body, 0),
        };
        if bytes.try_reserve_exact(length - 4).is_err() {
            return self.reject(bytes, charge, ReadFault::Allocation, Phase::Body, 0);
        }
        charge = next;
        bytes.resize(length, 0);
        if let Err((fault, filled, reported)) = fill(source, &mut bytes[4..]) {
            bytes.truncate(4 + filled);
            return self.reject(bytes, charge, fault, Phase::Body, reported);
        }
        Packet {
            bytes,
            charge,
            rejection: None,
        }
    }
    fn reject(
        &mut self,
        bytes: Vec<u8>,
        charge: Reservation,
        fault: ReadFault,
        phase: Phase,
        reported: u64,
    ) -> Packet {
        self.failed = true;
        Packet {
            bytes,
            charge,
            rejection: Some(Rejection {
                fault,
                phase,
                reported,
                queue_overflow: false,
            }),
        }
    }
}
fn fill(source: &mut impl Source, buffer: &mut [u8]) -> Result<(), (ReadFault, usize, u64)> {
    let mut filled = 0;
    while filled < buffer.len() {
        let remaining = buffer.len() - filled;
        let step = source.read(&mut buffer[filled..]);
        if step.reported > remaining {
            return Err((ReadFault::InvalidCount, filled, step.reported as u64));
        }
        filled += step.reported;
        if let Some(error) = step.error {
            return Err((ReadFault::Os(error), filled, step.reported as u64));
        }
        if step.reported == 0 {
            return Err((ReadFault::Eof, filled, 0));
        }
    }
    Ok(())
}

/// One reserved producer-fault slot per reader. The control path only try-locks;
/// a full normal queue cannot evict the first rejected packet.
pub struct FaultSlot<T>(std::sync::Mutex<Option<T>>);
impl<T> Default for FaultSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> FaultSlot<T> {
    pub fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }
    pub fn store(&self, value: T) -> Result<(), T> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(value);
        }
        *slot = Some(value);
        Ok(())
    }
    pub fn take(&self) -> Result<Option<T>, std::sync::TryLockError<()>> {
        match self.0.try_lock() {
            Ok(mut slot) => Ok(slot.take()),
            Err(std::sync::TryLockError::WouldBlock) => Err(std::sync::TryLockError::WouldBlock),
            Err(std::sync::TryLockError::Poisoned(e)) => Ok(e.into_inner().take()),
        }
    }
}
