//! Bounded chunked delivery of one canonical starting-response export over the
//! owned ordinary lane. Queue handoff is not peer receipt, durability, native
//! execution, cleanup proof or an admission/qualification signal.
use super::{
    starting::Scope,
    starting_bootstrap::Declaration,
    starting_export::{Response, CAP},
    transport::SendFailure,
};
use crate::{
    evidence::{decode, Digest, Domain, Value},
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{exact, get, object, string, uint},
    },
};
/// One chunk header is far smaller than this; the bound keeps the raw data
/// payload predictable without trusting a peer-declared header length.
const HEADER_CAP: usize = 1024;
const ROLE: &str = "pilot_starting_response_delivery";
const KEYS: [&str; 10] = [
    "version",
    "role",
    "declaration",
    "scope",
    "digest",
    "index",
    "count",
    "total_len",
    "data_len",
    "offset",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Frame,
    Scope,
    Binding,
    Digest,
    Gap,
    Stopped,
}
fn frame_cap() -> usize {
    autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .frame_bytes as usize
}
fn data_cap() -> usize {
    frame_cap() - HEADER_CAP
}
pub fn chunk_count(total_len: usize) -> u32 {
    if total_len == 0 {
        return 0;
    }
    total_len.div_ceil(data_cap()) as u32
}
/// Fixed upper bound for the largest canonical response. Requested frames above
/// it are rejected before any body allocation.
pub fn max_chunks() -> u32 {
    chunk_count(CAP)
}
#[derive(Debug)]
pub struct Delivery {
    chunks: u32,
    bytes: usize,
    digest: Digest,
    drained: bool,
}
impl Delivery {
    pub fn chunks(&self) -> u32 {
        self.chunks
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    /// Observed after the queue handoffs; not a peer read or disk durability.
    pub fn drained(&self) -> bool {
        self.drained
    }
}
#[derive(Debug)]
pub struct DeliveryFault {
    index: u32,
    sent: u32,
    cause: SendFailure,
}
impl DeliveryFault {
    pub fn index(&self) -> u32 {
        self.index
    }
    pub fn sent(&self) -> u32 {
        self.sent
    }
    pub fn cause(&self) -> SendFailure {
        self.cause
    }
}
/// Child-side one-shot sender. A failure retains the failed index and stops; no
/// successor chunk follows and no frame is re-sent.
pub struct Chunks<'a> {
    canonical: &'a [u8],
    declaration: Digest,
    scope: Digest,
    digest: Digest,
    count: u32,
    _allocation: Reservation,
}
impl<'a> Chunks<'a> {
    /// The response itself bounds the canonical bytes/digest; this reserves the
    /// single reusable frame buffer before any chunk is encoded.
    pub fn new(response: &'a Response, budget: &Budget) -> Result<Self, Fault> {
        let canonical = response.canonical().as_bytes();
        if canonical.is_empty() || canonical.len() > CAP {
            return Err(Fault::Capacity);
        }
        let count = chunk_count(canonical.len());
        if count == 0 || count > max_chunks() {
            return Err(Fault::Capacity);
        }
        Ok(Self {
            canonical,
            declaration: response.declaration(),
            scope: response.scope_digest(),
            digest: response.digest(),
            count,
            _allocation: budget
                .reserve(frame_cap() + std::mem::size_of::<Self>())
                .map_err(|_| Fault::Capacity)?,
        })
    }
    pub fn count(&self) -> u32 {
        self.count
    }
    pub fn send(
        &self,
        mut queue: impl FnMut(&[u8]) -> Result<(), SendFailure>,
        drained: impl FnOnce() -> bool,
    ) -> Result<Delivery, DeliveryFault> {
        for (sent, index) in (0..self.count).enumerate() {
            let sent = sent as u32;
            let offset = index as usize * data_cap();
            let end = (offset + data_cap()).min(self.canonical.len());
            let data = &self.canonical[offset..end];
            let header = object([
                ("version", Value::Text("1".into())),
                ("role", Value::Text(ROLE.into())),
                ("declaration", Value::Text(self.declaration.to_hex())),
                ("scope", Value::Text(self.scope.to_hex())),
                ("digest", Value::Text(self.digest.to_hex())),
                ("index", Value::U64(index.into())),
                ("count", Value::U64(self.count.into())),
                ("total_len", Value::U64(self.canonical.len() as u64)),
                ("data_len", Value::U64(data.len() as u64)),
                ("offset", Value::U64(offset as u64)),
            ])
            .to_canonical();
            let total = 8usize
                .checked_add(header.len())
                .and_then(|n| n.checked_add(data.len()));
            let Some(total) = total.filter(|n| header.len() <= HEADER_CAP && *n <= frame_cap())
            else {
                return Err(DeliveryFault {
                    index,
                    sent,
                    cause: SendFailure::Capacity,
                });
            };
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(total).is_err() {
                return Err(DeliveryFault {
                    index,
                    sent,
                    cause: SendFailure::Capacity,
                });
            }
            bytes.extend_from_slice(&(total as u32).to_le_bytes());
            bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
            bytes.extend_from_slice(header.as_bytes());
            bytes.extend_from_slice(data);
            if let Err(cause) = queue(&bytes) {
                return Err(DeliveryFault { index, sent, cause });
            }
        }
        Ok(Delivery {
            chunks: self.count,
            bytes: self.canonical.len(),
            digest: self.digest,
            drained: drained(),
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Incomplete {
        received: u32,
        count: u32,
        bytes: usize,
    },
    Complete {
        bytes: usize,
        digest: Digest,
    },
    Faulted {
        received: u32,
        count: u32,
        bytes: usize,
        fault: Fault,
    },
}
struct Header {
    declaration: Digest,
    scope: Digest,
    digest: Digest,
    index: u32,
    count: u32,
    total_len: usize,
    data_len: usize,
    offset: usize,
}
fn header(value: &Value) -> Result<Header, Fault> {
    let o = exact(value, &KEYS).map_err(|_| Fault::Frame)?;
    if string(get(o, "version")).map_err(|_| Fault::Frame)? != "1"
        || string(get(o, "role")).map_err(|_| Fault::Frame)? != ROLE
    {
        return Err(Fault::Scope);
    }
    let index = u32::try_from(uint(get(o, "index")).map_err(|_| Fault::Frame)?)
        .map_err(|_| Fault::Frame)?;
    Ok(Header {
        declaration: Digest::parse(string(get(o, "declaration")).map_err(|_| Fault::Frame)?)
            .map_err(|_| Fault::Frame)?,
        scope: Digest::parse(string(get(o, "scope")).map_err(|_| Fault::Frame)?)
            .map_err(|_| Fault::Frame)?,
        digest: Digest::parse(string(get(o, "digest")).map_err(|_| Fault::Frame)?)
            .map_err(|_| Fault::Frame)?,
        index,
        count: u32::try_from(uint(get(o, "count")).map_err(|_| Fault::Frame)?)
            .map_err(|_| Fault::Frame)?,
        total_len: usize::try_from(uint(get(o, "total_len")).map_err(|_| Fault::Frame)?)
            .map_err(|_| Fault::Frame)?,
        data_len: usize::try_from(uint(get(o, "data_len")).map_err(|_| Fault::Frame)?)
            .map_err(|_| Fault::Frame)?,
        offset: usize::try_from(uint(get(o, "offset")).map_err(|_| Fault::Frame)?)
            .map_err(|_| Fault::Frame)?,
    })
}
fn split(bytes: &[u8]) -> Result<(Header, &[u8]), Fault> {
    if bytes.len() < 8 {
        return Err(Fault::Frame);
    }
    let total = u32::from_le_bytes(bytes[..4].try_into().map_err(|_| Fault::Frame)?) as usize;
    if total != bytes.len() || total > frame_cap() {
        return Err(Fault::Frame);
    }
    let header_len = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| Fault::Frame)?) as usize;
    if header_len == 0 || header_len > HEADER_CAP || 8 + header_len > bytes.len() {
        return Err(Fault::Frame);
    }
    let text = &bytes[8..8 + header_len];
    crate::sampler::wire::validate_complexity_limit(text, 32).map_err(|_| Fault::Capacity)?;
    let value = decode(text, HEADER_CAP).map_err(|_| Fault::Frame)?;
    Ok((header(&value)?, &bytes[8 + header_len..]))
}
/// Parent-side strict assembler. Storage for the largest response is reserved in
/// `new`; the first accepted chunk only allocates its declared bounded length.
/// A gap, duplicate, foreign declaration/scope, contradiction or digest
/// mismatch is sticky and leaves an explicit incomplete status.
pub struct Assembler {
    declaration: Digest,
    scope: Digest,
    digest: Option<Digest>,
    total_len: usize,
    count: u32,
    bytes: Vec<u8>,
    received: u32,
    fault: Option<Fault>,
    complete: bool,
    _allocation: Reservation,
}
impl Assembler {
    pub fn new(declaration: &Declaration, scope: &Scope, budget: &Budget) -> Result<Self, Fault> {
        if declaration.scope_digest() != scope.digest() || declaration.tag().as_bytes() == &[0; 32]
        {
            return Err(Fault::Scope);
        }
        Ok(Self {
            declaration: declaration.tag(),
            scope: scope.digest(),
            digest: None,
            total_len: 0,
            count: 0,
            bytes: Vec::new(),
            received: 0,
            fault: None,
            complete: false,
            _allocation: budget
                .reserve(CAP + std::mem::size_of::<Self>())
                .map_err(|_| Fault::Capacity)?,
        })
    }
    pub fn received(&self) -> u32 {
        self.received
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    /// Only a complete, digest-verified canonical export is exposed.
    pub fn canonical(&self) -> Option<&[u8]> {
        self.complete.then_some(self.bytes.as_slice())
    }
    pub fn status(&self) -> Status {
        match (self.complete, self.fault) {
            (true, _) => Status::Complete {
                bytes: self.bytes.len(),
                digest: self
                    .digest
                    .unwrap_or_else(|| Digest::of(Domain::RawArtifact, &[])),
            },
            (false, Some(fault)) => Status::Faulted {
                received: self.received,
                count: self.count,
                bytes: self.bytes.len(),
                fault,
            },
            (false, None) => Status::Incomplete {
                received: self.received,
                count: self.count,
                bytes: self.bytes.len(),
            },
        }
    }
    /// One accepted chunk. Failure is sticky; a later chunk cannot resume it.
    pub fn accept(&mut self, bytes: &[u8]) -> Result<(u32, bool), Fault> {
        if self.fault.is_some() || self.complete {
            return Err(self.fault.unwrap_or(Fault::Stopped));
        }
        let result = self.accept_inner(bytes);
        if let Err(f) = result {
            self.fault = Some(f);
        }
        result
    }
    fn accept_inner(&mut self, bytes: &[u8]) -> Result<(u32, bool), Fault> {
        let (h, data) = split(bytes)?;
        if h.declaration != self.declaration || h.scope != self.scope {
            return Err(Fault::Scope);
        }
        if data.len() != h.data_len {
            return Err(Fault::Frame);
        }
        match self.digest {
            None => {
                if h.index != 0
                    || h.count == 0
                    || h.count > max_chunks()
                    || h.total_len == 0
                    || h.total_len > CAP
                    || h.count != chunk_count(h.total_len)
                {
                    return Err(Fault::Frame);
                }
                let expected = data_cap().min(h.total_len);
                if h.data_len != expected || h.offset != 0 {
                    return Err(Fault::Frame);
                }
                self.bytes
                    .try_reserve_exact(h.total_len)
                    .map_err(|_| Fault::Capacity)?;
                self.digest = Some(h.digest);
                self.total_len = h.total_len;
                self.count = h.count;
            }
            Some(digest) => {
                if h.digest != digest
                    || h.count != self.count
                    || h.total_len != self.total_len
                    || h.index != self.received
                    || h.offset != h.index as usize * data_cap()
                {
                    return Err(Fault::Gap);
                }
                let remaining = self.total_len - h.offset;
                if h.data_len != data_cap().min(remaining) {
                    return Err(Fault::Frame);
                }
            }
        }
        self.bytes.extend_from_slice(data);
        self.received = self.received.checked_add(1).ok_or(Fault::Capacity)?;
        if self.received != self.count {
            return Ok((h.index, false));
        }
        if self.bytes.len() != self.total_len {
            return Err(Fault::Frame);
        }
        if std::str::from_utf8(&self.bytes).is_err() {
            return Err(Fault::Frame);
        }
        if Digest::of(Domain::RawArtifact, &self.bytes) != h.digest {
            return Err(Fault::Digest);
        }
        // Transfer consistency alone is not binding: the assembled canonical
        // must still name the parent's exact scope and declaration.
        verify_payload(&self.bytes, self.declaration, self.scope)?;
        self.complete = true;
        Ok((h.index, true))
    }
}
fn verify_payload(canonical: &[u8], declaration: Digest, scope: Digest) -> Result<(), Fault> {
    let value = decode(canonical, CAP).map_err(|_| Fault::Frame)?;
    let object = value.as_object().ok_or(Fault::Frame)?;
    let text = |key: &str| object.get(key).and_then(Value::as_text);
    if text("version") != Some("1")
        || text("scope") != Some("pilot_starting_settings_response")
        || text("request_digest") != Some(scope.to_hex().as_str())
        || text("declaration") != Some(declaration.to_hex().as_str())
    {
        return Err(Fault::Binding);
    }
    Ok(())
}
