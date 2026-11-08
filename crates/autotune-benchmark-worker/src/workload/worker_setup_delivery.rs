//! Worker setup bytes on the existing ordinary sequence, followed by Ready.
//! Delivery and strict reading are not native acceptance, consent or admission.
use super::{
    bootstrap::EntryClock,
    transport::{RawReceived, WorkerReceived},
    wire::{Channel, Direction, Message, WorkerBinding},
    worker_setup::{self, Artifact, Receipt},
};
use crate::{
    evidence::{Digest, Domain, Value},
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        wire::Lane,
        wire::WireError,
        wire_model::{exact, get, hex, object, string, uint},
    },
};
pub const CHUNK_BYTES: usize = 16 * 1024;
pub const MAX_CHUNKS: u32 = (worker_setup::CAP / CHUNK_BYTES) as u32;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    digest: Digest,
    index: u32,
    count: u32,
    total: u32,
    data: String,
}
impl Chunk {
    pub fn index(&self) -> u32 {
        self.index
    }
    pub fn count(&self) -> u32 {
        self.count
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    pub fn total_bytes(&self) -> u32 {
        self.total
    }
    pub(crate) fn allocation_bytes(&self) -> usize {
        self.data.capacity()
    }
    pub(crate) fn validate(&self) -> Result<(), WireError> {
        let total = self.total as usize;
        if total == 0
            || total > worker_setup::CAP
            || self.count != total.div_ceil(CHUNK_BYTES) as u32
            || self.index >= self.count
        {
            return Err(WireError::Body);
        }
        let size = CHUNK_BYTES.min(total - self.index as usize * CHUNK_BYTES);
        if self.data.len() != size * 2
            || self.data.capacity() > CHUNK_BYTES * 2
            || !self
                .data
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(WireError::Body);
        }
        Ok(())
    }
    pub(crate) fn value(&self) -> Value {
        object([
            ("digest", Value::Text(self.digest.to_hex())),
            ("index", Value::U64(self.index.into())),
            ("count", Value::U64(self.count.into())),
            ("total", Value::U64(self.total.into())),
            ("data", Value::Text(self.data.clone())),
        ])
    }
    pub(crate) fn parse(v: &Value) -> Result<Self, WireError> {
        let o = exact(v, &["digest", "index", "count", "total", "data"])?;
        let data = string(get(o, "data"))?;
        if data.len() > CHUNK_BYTES * 2 {
            return Err(WireError::Capacity);
        }
        let chunk = Self {
            digest: Digest::parse(string(get(o, "digest"))?).map_err(|_| WireError::Body)?,
            index: uint(get(o, "index"))?
                .try_into()
                .map_err(|_| WireError::Body)?,
            count: uint(get(o, "count"))?
                .try_into()
                .map_err(|_| WireError::Body)?,
            total: uint(get(o, "total"))?
                .try_into()
                .map_err(|_| WireError::Body)?,
            data: data.into(),
        };
        chunk.validate()?;
        Ok(chunk)
    }
}
pub struct Chunks<'a> {
    artifact: &'a Artifact,
    _allocation: Reservation,
}
impl<'a> Chunks<'a> {
    pub fn new(artifact: &'a Artifact, budget: &Budget) -> Result<Self, Fault> {
        let allocation = budget
            .reserve(CHUNK_BYTES * 4 + std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        Ok(Self {
            artifact,
            _allocation: allocation,
        })
    }
    pub fn count(&self) -> u32 {
        self.artifact.canonical().len().div_ceil(CHUNK_BYTES) as u32
    }
    /// One model at a time; caller holds the prepaid scratch until it is sent.
    pub fn message(&self, index: u32) -> Result<Message, Fault> {
        if index >= self.count() {
            return Err(Fault::Protocol);
        }
        let bytes = self.artifact.canonical().as_bytes();
        let start = index as usize * CHUNK_BYTES;
        let end = (start + CHUNK_BYTES).min(bytes.len());
        Ok(Message::SetupChunk(Chunk {
            digest: self.artifact.digest(),
            index,
            count: self.count(),
            total: bytes.len() as u32,
            data: hex(&bytes[start..end]),
        }))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Binding,
    Clock,
    Deadline,
    Stopped,
    Protocol,
    Wire(WireError),
    Digest,
    Payload(worker_setup::Fault),
    Transport,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub receipt: Option<Anchor>,
    pub frame_digest: Digest,
    pub index: Option<u32>,
    pub origin: Option<super::launch_origin::Origin>,
}
pub struct Receiver {
    binding: WorkerBinding,
    deadline_raw: u64,
    decoder: Channel,
    history: EntryClock,
    bytes: Vec<u8>,
    digest: Option<Digest>,
    total: u32,
    count: u32,
    received: u32,
    receipt: Option<Receipt>,
    chunks: Vec<WorkerReceived>,
    ready: Option<WorkerReceived>,
    rejected: Option<WorkerReceived>,
    arrivals: Vec<Arrival>,
    fault: Option<Fault>,
    transport: Option<(Option<i32>, Option<Anchor>)>,
    budget: Budget,
    _packet_storage: Reservation,
    _allocation: Reservation,
}
impl Receiver {
    /// Parent supplies its once-frozen original raw deadline, never a new grace.
    pub fn new(
        binding: WorkerBinding,
        first: Anchor,
        deadline_raw: u64,
        budget: Budget,
    ) -> Result<Self, Fault> {
        let base = binding.epoch.interrupt_100ns;
        if deadline_raw <= base.saturating_add(binding.origin_ns / 100)
            || deadline_raw > base.saturating_add(binding.setup_boundary() / 100)
            || deadline_raw == u64::MAX
        {
            return Err(Fault::Binding);
        }
        if first.interrupt_100ns >= deadline_raw {
            return Err(Fault::Deadline);
        }
        let allocation = budget
            .reserve(
                worker_setup::CAP
                    + (MAX_CHUNKS as usize + 1) * std::mem::size_of::<Arrival>()
                    + std::mem::size_of::<ReadyDelivery>(),
            )
            .map_err(|_| Fault::Capacity)?;
        let packet_storage = budget
            .reserve(
                (MAX_CHUNKS as usize + 1)
                    .checked_mul(std::mem::size_of::<WorkerReceived>())
                    .ok_or(Fault::Capacity)?,
            )
            .map_err(|_| Fault::Capacity)?;
        let history = EntryClock::new(binding.monitor.qpc_frequency, binding.epoch, first, &budget)
            .map_err(|e| {
                if e == super::bootstrap::Error::Capacity {
                    Fault::Capacity
                } else {
                    Fault::Clock
                }
            })?;
        let decoder = Channel::new(
            binding,
            Lane::Ordinary,
            Direction::FromWorker,
            budget.clone(),
        )
        .map_err(Fault::Wire)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(worker_setup::CAP)
            .map_err(|_| Fault::Capacity)?;
        let mut arrivals = Vec::new();
        arrivals
            .try_reserve_exact(MAX_CHUNKS as usize + 1)
            .map_err(|_| Fault::Capacity)?;
        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(MAX_CHUNKS as usize)
            .map_err(|_| Fault::Capacity)?;
        Ok(Self {
            binding,
            deadline_raw,
            decoder,
            history,
            bytes,
            digest: None,
            total: 0,
            count: 0,
            received: 0,
            receipt: None,
            chunks,
            ready: None,
            rejected: None,
            arrivals,
            fault: None,
            transport: None,
            budget,
            _packet_storage: packet_storage,
            _allocation: allocation,
        })
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn received(&self) -> u32 {
        self.received
    }
    pub fn payload(&self) -> &[u8] {
        &self.bytes
    }
    pub fn receipt(&self) -> Option<&Receipt> {
        self.receipt.as_ref()
    }
    pub fn ready(&self) -> Option<&WorkerReceived> {
        self.ready.as_ref()
    }
    pub fn rejected(&self) -> Option<&WorkerReceived> {
        self.rejected.as_ref()
    }
    pub fn arrivals(&self) -> &[Arrival] {
        &self.arrivals
    }
    pub fn history(&self) -> &EntryClock {
        &self.history
    }
    pub fn transport(&self) -> Option<(Option<i32>, Option<Anchor>)> {
        self.transport
    }
    fn fail(&mut self, f: Fault) -> Fault {
        self.fault.get_or_insert(f);
        self.decoder.poison();
        self.fault.expect("sticky fault")
    }
    pub fn tick(&mut self, a: Option<Anchor>) -> Result<(), Fault> {
        if let Some(f) = self.fault {
            return Err(f);
        }
        self.history.observe(a).map_err(|e| {
            self.fail(if e == super::bootstrap::Error::Capacity {
                Fault::Capacity
            } else {
                Fault::Clock
            })
        })?;
        if a.is_none_or(|a| a.interrupt_100ns >= self.deadline_raw) {
            return Err(self.fail(Fault::Deadline));
        }
        Ok(())
    }
    pub fn stop(&mut self) {
        self.fail(Fault::Stopped);
    }
    pub fn transport_failed(&mut self, code: Option<i32>, a: Option<Anchor>) {
        if self.fault.is_none() {
            self.transport = Some((code, a));
            self.fail(Fault::Transport);
        }
    }
    /// Consume one owned raw packet, retaining the first rejected owner. No retry
    /// or reconnect can resume the poisoned decoder or replace the first cause.
    pub fn accept(&mut self, raw: RawReceived) -> Result<(), Fault> {
        if let Some(f) = self.fault {
            return Err(f);
        }
        let packet = raw.decode_worker(&mut self.decoder);
        let result = self.accept_packet(&packet);
        if let Err(f) = result {
            self.rejected = Some(packet);
            return Err(self.fail(f));
        }
        if matches!(packet.message(), Ok(Message::Ready { .. })) {
            self.ready = Some(packet)
        } else {
            self.chunks.push(packet)
        }
        Ok(())
    }
    fn accept_packet(&mut self, packet: &WorkerReceived) -> Result<(), Fault> {
        if self.ready.is_some() {
            return Err(Fault::Protocol);
        }
        let receipt = packet.raw().receipt();
        self.tick(receipt)?;
        let message = packet.message().map_err(Fault::Wire)?;
        if packet.binding().map_err(Fault::Wire)? != self.binding {
            return Err(Fault::Binding);
        }
        let index = if let Message::SetupChunk(c) = message {
            Some(c.index)
        } else {
            None
        };
        if self.arrivals.len() == MAX_CHUNKS as usize + 1 {
            return Err(Fault::Capacity);
        }
        self.arrivals.push(Arrival {
            receipt,
            frame_digest: Digest::of(Domain::RawArtifact, packet.raw().packet().bytes()),
            index,
            origin: packet.raw().origin(),
        });
        match message {
            Message::SetupChunk(c) => {
                if self.receipt.is_some() || c.index != self.received {
                    return Err(Fault::Protocol);
                }
                if self.digest.is_none() {
                    if c.index != 0 {
                        return Err(Fault::Protocol);
                    }
                    self.digest = Some(c.digest);
                    self.total = c.total;
                    self.count = c.count;
                }
                if self.digest != Some(c.digest) || self.total != c.total || self.count != c.count {
                    return Err(Fault::Protocol);
                }
                for pair in c.data.as_bytes().as_chunks::<2>().0 {
                    let n = |v: u8| if v <= b'9' { v - b'0' } else { v - b'a' + 10 };
                    self.bytes.push(n(pair[0]) * 16 + n(pair[1]));
                }
                self.received += 1;
                if self.received == self.count {
                    if self.bytes.len() != self.total as usize
                        || Digest::of(Domain::RawArtifact, &self.bytes) != c.digest
                    {
                        return Err(Fault::Digest);
                    }
                    self.receipt = Some(
                        Receipt::read(
                            &self.bytes,
                            self.binding,
                            self.deadline_raw,
                            c.digest,
                            &self.budget,
                        )
                        .map_err(Fault::Payload)?,
                    );
                }
                Ok(())
            }
            Message::Ready { .. } => {
                if self.receipt.is_none() {
                    Err(Fault::Protocol)
                } else {
                    Ok(())
                }
            }
            _ => Err(Fault::Protocol),
        }
    }
    #[allow(clippy::result_large_err)] // Return the original partial transport owner without allocating an error path.
    pub fn into_ready(self) -> Result<ReadyDelivery, Self> {
        if self.fault.is_none() && self.ready.is_some() && self.receipt.is_some() {
            Ok(ReadyDelivery {
                receiver: self,
                assessment: None,
            })
        } else {
            Err(self)
        }
    }
}
/// A delivery handoff only. The same decoder continues its original sequence;
/// caller still owes setup acceptance, peer/consent/safety and native admission.
pub struct ReadyDelivery {
    receiver: Receiver,
    assessment: Option<
        Result<super::worker_setup_assessment::Summary, super::worker_setup_assessment::Rejection>,
    >,
}
impl ReadyDelivery {
    /// Every setup chunk and Ready must have the same local launcher stamp.
    pub fn origin(&self) -> Result<super::launch_origin::Origin, super::launch_origin::Fault> {
        let mut chain = super::launch_origin::Chain::default();
        for arrival in self.arrivals() {
            chain.observe(arrival.origin);
        }
        chain.result()
    }
    pub(crate) fn close_reference_path(&mut self) {
        self.receiver.decoder.poison();
    }
    /// One retained assessment of reported assertions. A rejection poisons the
    /// ordinary decoder; independent stop/control and native authority remain
    /// the parent's responsibility. Calling again returns the original result.
    pub fn assess_setup(
        &mut self,
    ) -> Result<super::worker_setup_assessment::Summary, super::worker_setup_assessment::Rejection>
    {
        if let Some(result) = self.assessment {
            return result;
        }
        let result = super::worker_setup_assessment::screen(
            self.receipt(),
            self.receiver.binding,
            self.ready().message().expect("decoded Ready"),
            &self.receiver.arrivals,
        );
        if result.is_err() {
            self.receiver.decoder.poison();
        }
        self.assessment = Some(result);
        result
    }
    pub fn assessment(
        &self,
    ) -> Option<
        Result<super::worker_setup_assessment::Summary, super::worker_setup_assessment::Rejection>,
    > {
        self.assessment
    }
    pub fn receipt(&self) -> &Receipt {
        self.receiver.receipt.as_ref().expect("verified delivery")
    }
    pub fn ready(&self) -> &WorkerReceived {
        self.receiver.ready.as_ref().expect("Ready owner")
    }
    /// Exact raw setup packet owners in wire sequence, followed by Ready.
    pub fn packets(&self) -> impl Iterator<Item = &WorkerReceived> {
        self.receiver
            .chunks
            .iter()
            .chain(self.receiver.ready.iter())
    }
    pub fn decoder(&mut self) -> &mut Channel {
        &mut self.receiver.decoder
    }
    pub fn history(&self) -> &EntryClock {
        &self.receiver.history
    }
    pub fn arrivals(&self) -> &[Arrival] {
        &self.receiver.arrivals
    }
}
