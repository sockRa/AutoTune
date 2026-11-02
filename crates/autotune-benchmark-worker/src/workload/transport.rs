//! Owned handoffs used by Windows pipes and deterministic byte-stream fixtures.
use super::{
    manifest::BoundRun,
    producer::FrameSink,
    settings::Block,
    settings_transport::{self, Parent, Receipt},
    wire::{Channel, Direction, EncodedFrame, Message, OwnedMessage, WorkerBinding},
};
use crate::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        framing::Packet,
        wire::{Lane, WireError},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendFailure {
    Closed,
    Binding,
    Capacity,
    Queue,
    Os(u32),
}
/// The implementation reserves destination storage BEFORE copying any bytes.
/// Success means queue ownership, never pipe completion/peer receipt/durability.
pub trait Queue {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure>;
}
/// Destination ownership for a bounded queue copy. The producer keeps its
/// original frame until this charged copy has actually entered the queue.
pub struct QueuedBytes {
    bytes: Vec<u8>,
    _allocation: Reservation,
}
impl QueuedBytes {
    pub fn copy(bytes: &[u8], budget: &Budget) -> Result<Self, SendFailure> {
        if !(4..=64 * 1024).contains(&bytes.len()) {
            return Err(SendFailure::Capacity);
        }
        let allocation = budget
            .reserve(bytes.len() + std::mem::size_of::<Self>())
            .map_err(|_| SendFailure::Capacity)?;
        let mut copy = Vec::new();
        copy.try_reserve_exact(bytes.len())
            .map_err(|_| SendFailure::Capacity)?;
        copy.extend_from_slice(bytes);
        Ok(Self {
            bytes: copy,
            _allocation: allocation,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    #[cfg(windows)]
    pub(crate) fn into_parts(self) -> (Vec<u8>, Reservation) {
        (self.bytes, self._allocation)
    }
}
pub struct Sender<Q> {
    queue: Q,
    binding: Digest,
    direction: Direction,
    faults: [Option<SendFailure>; 2],
    _metadata: Reservation,
}
fn index(lane: Lane) -> usize {
    usize::from(lane == Lane::Control)
}
impl<Q: Queue> Sender<Q> {
    pub fn new(
        queue: Q,
        binding: WorkerBinding,
        direction: Direction,
        budget: &Budget,
    ) -> Result<Self, SendFailure> {
        let _codec = budget
            .reserve(64 * 1024 * 16)
            .map_err(|_| SendFailure::Capacity)?;
        Ok(Self {
            queue,
            binding: binding.transport_digest(),
            direction,
            faults: [None; 2],
            _metadata: budget
                .reserve(std::mem::size_of::<Self>())
                .map_err(|_| SendFailure::Capacity)?,
        })
    }
    pub fn fault(&self, lane: Lane) -> Option<SendFailure> {
        self.faults[index(lane)]
    }
    fn handoff(
        &mut self,
        lane: Lane,
        binding: Digest,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<(), SendFailure> {
        if self.faults[index(lane)].is_some() {
            return Err(SendFailure::Closed);
        }
        let result = if binding != self.binding || direction != self.direction {
            Err(SendFailure::Binding)
        } else {
            self.queue.try_send(lane, bytes)
        };
        if let Err(f) = result {
            self.faults[index(lane)] = Some(f);
        }
        result
    }
    /// Failure returns the exact original owner; callers retain it as evidence.
    pub fn send_worker(&mut self, frame: EncodedFrame) -> Result<(), EncodedFrame> {
        match self.handoff(frame.lane, frame.binding, frame.direction, frame.as_bytes()) {
            Ok(()) => Ok(()),
            Err(_) => Err(frame),
        }
    }
    pub fn send_settings(
        &mut self,
        frame: settings_transport::Frame,
    ) -> Result<(), settings_transport::Frame> {
        let direction = if frame.response {
            Direction::FromWorker
        } else {
            Direction::ToWorker
        };
        match self.handoff(Lane::Ordinary, frame.binding, direction, frame.as_bytes()) {
            Ok(()) => Ok(()),
            Err(_) => Err(frame),
        }
    }
}
impl<Q: Queue> FrameSink for Sender<Q> {
    fn send(&mut self, frame: EncodedFrame) -> Result<(), EncodedFrame> {
        self.send_worker(frame)
    }
}

/// Includes original framing/write rejection, bytes and RAM ownership even if
/// parsing fails. The receipt clock is captured after the nonblocking receive.
pub struct RawReceived {
    lane: Lane,
    packet: Packet,
    receipt: Option<Anchor>,
}
impl RawReceived {
    pub fn new(lane: Lane, packet: Packet, receipt: Option<Anchor>) -> Self {
        Self {
            lane,
            packet,
            receipt,
        }
    }
    pub fn lane(&self) -> Lane {
        self.lane
    }
    pub fn packet(&self) -> &Packet {
        &self.packet
    }
    pub fn receipt(&self) -> Option<Anchor> {
        self.receipt
    }
    pub fn decode_worker(self, decoder: &mut Channel) -> WorkerReceived {
        let message = if self.packet.rejection().is_some() {
            decoder.poison();
            Err(WireError::Body)
        } else if self.lane != decoder.lane() {
            decoder.poison();
            Err(WireError::Lane)
        } else {
            decoder.decode(self.packet.bytes())
        };
        if self.receipt.is_none() {
            decoder.poison();
        }
        WorkerReceived { raw: self, message }
    }
    pub fn decode_settings(self, parent: &mut Parent, run: &BoundRun<'_>) -> SettingsReceived {
        let result = if self.packet.rejection().is_some() || self.lane != Lane::Ordinary {
            parent.reject_transport(Block::Scope);
            Err(Block::Scope)
        } else if let Some(receipt) = self.receipt {
            parent.receive(run, self.packet.bytes(), receipt)
        } else {
            parent.reject_transport(Block::Clock);
            Err(Block::Clock)
        };
        SettingsReceived { raw: self, result }
    }
}
pub struct WorkerReceived {
    raw: RawReceived,
    message: Result<OwnedMessage, WireError>,
}
impl WorkerReceived {
    pub fn raw(&self) -> &RawReceived {
        &self.raw
    }
    pub fn message(&self) -> Result<&Message, WireError> {
        self.message
            .as_ref()
            .map(OwnedMessage::message)
            .map_err(|e| *e)
    }
}
pub struct SettingsReceived {
    raw: RawReceived,
    result: Result<Receipt, Block>,
}
impl SettingsReceived {
    pub fn raw(&self) -> &RawReceived {
        &self.raw
    }
    pub fn result(&self) -> Result<&Receipt, Block> {
        self.result.as_ref().map_err(|e| *e)
    }
}
