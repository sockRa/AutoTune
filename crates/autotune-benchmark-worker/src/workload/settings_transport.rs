//! Single current-settings exchange for a bound pilot run. Separate from the
//! unchanged readonly sampler protocol; no native mode or admission authority.
use super::{
    manifest::BoundRun,
    settings::{self, Block, Capture, ReportedCapture},
    wire::WorkerBinding,
};
use crate::{
    evidence::{decode, Digest, Domain, Value},
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{exact, get, object, string},
    },
};
const CAP: usize = 64 * 1024;

pub struct Frame {
    bytes: Vec<u8>,
    _allocation: Reservation,
}
impl Frame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
fn encode(budget: &Budget, value: impl FnOnce() -> Value) -> Result<Frame, Block> {
    let _codec = budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
    let allocation = budget
        .reserve(CAP + std::mem::size_of::<Frame>())
        .map_err(|_| Block::Capacity)?;
    let text = value().to_canonical();
    let len = text
        .len()
        .checked_add(4)
        .filter(|n| *n <= CAP)
        .ok_or(Block::Capacity)?;
    let mut bytes = Vec::with_capacity(len);
    bytes.extend_from_slice(&(len as u32).to_le_bytes());
    bytes.extend_from_slice(text.as_bytes());
    Ok(Frame {
        bytes,
        _allocation: allocation,
    })
}
fn read(bytes: &[u8]) -> Result<Value, Block> {
    let prefix = bytes.get(..4).ok_or(Block::Scope)?;
    let length = u32::from_le_bytes(prefix.try_into().map_err(|_| Block::Scope)?) as usize;
    if length != bytes.len() || !(4..=CAP).contains(&length) {
        return Err(Block::Scope);
    }
    crate::sampler::wire::validate_complexity_limit(&bytes[4..], 256)
        .map_err(|_| Block::Capacity)?;
    decode(&bytes[4..], CAP - 4).map_err(|_| Block::Scope)
}
fn header(binding: WorkerBinding, kind: &'static str, body: Value) -> Value {
    object([
        ("version", Value::Text("1".into())),
        ("role", Value::Text("pilot_settings_sampler".into())),
        ("sequence", Value::U64(0)),
        ("binding", binding.value()),
        ("kind", Value::Text(kind.into())),
        ("body", body),
    ])
}
fn body<'a>(v: &'a Value, binding: WorkerBinding, kind: &str) -> Result<&'a Value, Block> {
    let o = exact(
        v,
        &["version", "role", "sequence", "binding", "kind", "body"],
    )
    .map_err(|_| Block::Scope)?;
    if string(get(o, "version")).ok() != Some("1")
        || string(get(o, "role")).ok() != Some("pilot_settings_sampler")
        || get(o, "sequence").as_u64().ok() != Some(0)
        || get(o, "binding").to_canonical() != binding.value().to_canonical()
        || string(get(o, "kind")).ok() != Some(kind)
    {
        return Err(Block::Scope);
    }
    Ok(get(o, "body"))
}
fn digest(bytes: &[u8]) -> Digest {
    Digest::of(Domain::RawArtifact, bytes)
}
fn observe(clock: &mut Clock, last: &mut Anchor, a: Anchor) -> Result<u64, Block> {
    if a.qpc_before < last.qpc_after {
        return Err(Block::Clock);
    }
    let n = clock.observe(a).map_err(|_| Block::Clock)?;
    *last = a;
    Ok(n)
}

/// Child-side one-shot parser. Failure, replay or response construction closes
/// it. Accepting bytes is not permission to load NVML or execute the getters.
pub struct Child {
    binding: WorkerBinding,
    budget: Budget,
    used: bool,
    _metadata: Reservation,
}
pub struct Request {
    binding: WorkerBinding,
    issued: Anchor,
    deadline_ns: u64,
    digest: Digest,
    budget: Budget,
    responded: bool,
    collected: bool,
    _metadata: Reservation,
}
impl Child {
    pub fn new(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        budget: Budget,
    ) -> Result<Self, Block> {
        Ok(Self {
            binding: WorkerBinding::new(run, frequency, epoch).map_err(|_| Block::Clock)?,
            _metadata: budget
                .reserve(std::mem::size_of::<Self>())
                .map_err(|_| Block::Capacity)?,
            budget,
            used: false,
        })
    }
    pub fn accept(&mut self, bytes: &[u8]) -> Result<Request, Block> {
        if self.used {
            return Err(Block::Stopped);
        }
        self.used = true;
        let _codec = self.budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        let value = read(bytes)?;
        let o = exact(
            body(&value, self.binding, "request")?,
            &["setup_entry", "deadline_ns", "issued"],
        )
        .map_err(|_| Block::Scope)?;
        let setup_entry = settings::parse_anchor(get(o, "setup_entry"))?.ok_or(Block::Clock)?;
        let issued = settings::parse_anchor(get(o, "issued"))?.ok_or(Block::Clock)?;
        let mut clock =
            Clock::new(self.binding.monitor, self.binding.epoch).map_err(|_| Block::Clock)?;
        let mut last = self.binding.epoch;
        let entry = observe(&mut clock, &mut last, setup_entry)?;
        let now = observe(&mut clock, &mut last, issued)?;
        let deadline_ns = entry
            .checked_add(self.binding.plan.phase_caps_ns[1])
            .ok_or(Block::Phase)?
            .min(self.binding.setup_boundary());
        if entry < self.binding.origin_ns
            || now >= deadline_ns
            || get(o, "deadline_ns").as_u64().ok() != Some(deadline_ns)
        {
            return Err(Block::Phase);
        }
        Ok(Request {
            binding: self.binding,
            issued,
            deadline_ns,
            digest: digest(bytes),
            budget: self.budget.clone(),
            responded: false,
            collected: false,
            _metadata: self
                .budget
                .reserve(std::mem::size_of::<Request>())
                .map_err(|_| Block::Capacity)?,
        })
    }
}
impl Request {
    pub fn issued(&self) -> Anchor {
        self.issued
    }
    /// Actual selected collector seam for a separately authorized, contained
    /// sampler. At most one collection attempt per accepted request. This API
    /// neither loads the runtime nor authorizes a native capture by itself.
    pub fn collect_native(
        &mut self,
        run: &BoundRun<'_>,
        sampler: &mut autotune_probe::nvml::NvmlSampler,
        clock: impl FnMut() -> Option<Anchor>,
        stop: impl FnMut() -> bool,
    ) -> Result<Capture, Block> {
        if self.collected || self.responded {
            return Err(Block::Stopped);
        }
        self.collected = true;
        if WorkerBinding::new(run, self.binding.monitor.qpc_frequency, self.binding.epoch)
            .map_err(|_| Block::Clock)?
            != self.binding
        {
            return Err(Block::Scope);
        }
        Capture::collect_native_scoped(
            run,
            self.binding,
            sampler,
            (self.issued, self.deadline_ns),
            (clock, stop),
            &self.budget,
        )
    }
    /// Borrow the collector's retained artifact. Failure leaves it owned by the
    /// caller; even an uncertain handoff cannot authorize response regeneration.
    pub fn respond(&mut self, capture: &Capture) -> Result<Frame, Block> {
        if self.responded {
            return Err(Block::Stopped);
        }
        self.responded = true;
        if capture.binding != self.binding {
            return Err(Block::Scope);
        }
        encode(&self.budget, || {
            header(
                self.binding,
                "response",
                object([
                    ("request_digest", Value::Text(self.digest.to_hex())),
                    ("capture_digest", Value::Text(capture.digest().to_hex())),
                    ("capture", Value::Text(capture.canonical().into())),
                ]),
            )
        })
    }
}

/// Parent result retains raw frame, all raw capture contradictions and local
/// anchors independently of validity. A readable native-path report is no peer,
/// runtime, consent, safety/re-entry, freshness-at-Start or baseline proof.
pub struct Receipt {
    issued: Anchor,
    received: Anchor,
    capture: ReportedCapture,
    frame: Frame,
    fault: Option<Block>,
}
impl Receipt {
    pub fn issued(&self) -> Anchor {
        self.issued
    }
    pub fn received(&self) -> Anchor {
        self.received
    }
    pub fn capture(&self) -> &ReportedCapture {
        &self.capture
    }
    pub fn frame(&self) -> &[u8] {
        self.frame.as_bytes()
    }
    pub fn fault(&self) -> Option<Block> {
        self.fault
    }
}
pub struct Parent {
    binding: WorkerBinding,
    setup_entry: Anchor,
    deadline_ns: u64,
    clock: Clock,
    last: Anchor,
    pending: Option<(Anchor, Digest)>,
    issued: bool,
    received: bool,
    fault: Option<Block>,
    budget: Budget,
    _metadata: Reservation,
}
impl Parent {
    /// Setup entry is a caller-supplied observation, not proof of re-entry.
    /// Freeze the approved relative 10 s cap and cumulative setup boundary.
    pub fn new(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        setup_entry: Anchor,
        budget: Budget,
    ) -> Result<Self, Block> {
        let binding = WorkerBinding::new(run, frequency, epoch).map_err(|_| Block::Clock)?;
        let mut clock = Clock::new(binding.monitor, epoch).map_err(|_| Block::Clock)?;
        let mut last = epoch;
        let entered = observe(&mut clock, &mut last, setup_entry)?;
        if entered < binding.origin_ns || entered >= binding.setup_boundary() {
            return Err(Block::Phase);
        }
        let deadline_ns = entered
            .checked_add(binding.plan.phase_caps_ns[1])
            .ok_or(Block::Phase)?
            .min(binding.setup_boundary());
        Ok(Self {
            binding,
            setup_entry,
            deadline_ns,
            clock,
            last,
            pending: None,
            issued: false,
            received: false,
            fault: None,
            _metadata: budget
                .reserve(std::mem::size_of::<Self>())
                .map_err(|_| Block::Capacity)?,
            budget,
        })
    }
    pub fn fault(&self) -> Option<Block> {
        self.fault
    }
    pub fn stop(&mut self) {
        self.fault.get_or_insert(Block::Stopped);
    }
    /// Independent parent supervision. Does not wait for an NVML return.
    pub fn tick(&mut self, anchor: Anchor) -> Result<(), Block> {
        let result = observe(&mut self.clock, &mut self.last, anchor).and_then(|n| {
            if n >= self.deadline_ns {
                Err(Block::Phase)
            } else {
                Ok(())
            }
        });
        if let Err(f) = result {
            self.fault.get_or_insert(f);
        }
        self.fault.map_or(Ok(()), Err)
    }
    pub fn issue(&mut self, issued: Anchor) -> Result<Frame, Block> {
        if self.issued {
            self.fault.get_or_insert(Block::Scope);
            return Err(Block::Scope);
        }
        self.issued = true;
        self.tick(issued)?;
        match encode(&self.budget, || {
            header(
                self.binding,
                "request",
                object([
                    ("setup_entry", settings::raw_anchor(self.setup_entry)),
                    ("deadline_ns", Value::U64(self.deadline_ns)),
                    ("issued", settings::raw_anchor(issued)),
                ]),
            )
        }) {
            Ok(frame) => {
                self.pending = Some((issued, digest(frame.as_bytes())));
                Ok(frame)
            }
            Err(f) => {
                self.fault.get_or_insert(f);
                Err(f)
            }
        }
    }
    /// Late/stopped, structurally valid responses are retained with a sticky
    /// fault. Structural rejection leaves the borrowed bytes caller-owned.
    pub fn receive(
        &mut self,
        run: &BoundRun<'_>,
        bytes: &[u8],
        received: Anchor,
    ) -> Result<Receipt, Block> {
        let result = self.receive_inner(run, bytes, received);
        if let Err(f) = result {
            self.fault.get_or_insert(f);
        }
        result
    }
    fn receive_inner(
        &mut self,
        run: &BoundRun<'_>,
        bytes: &[u8],
        received: Anchor,
    ) -> Result<Receipt, Block> {
        if self.received {
            return Err(Block::Scope);
        }
        self.received = true;
        let (issued, request_digest) = self.pending.ok_or(Block::Scope)?;
        if WorkerBinding::new(run, self.binding.monitor.qpc_frequency, self.binding.epoch)
            .map_err(|_| Block::Clock)?
            != self.binding
        {
            return Err(Block::Scope);
        }
        let _ = self.tick(received);
        let _codec = self.budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        let allocation = self
            .budget
            .reserve(CAP + std::mem::size_of::<Receipt>())
            .map_err(|_| Block::Capacity)?;
        let value = read(bytes)?;
        let o = exact(
            body(&value, self.binding, "response")?,
            &["request_digest", "capture_digest", "capture"],
        )
        .map_err(|_| Block::Scope)?;
        if string(get(o, "request_digest")).ok() != Some(request_digest.to_hex().as_str()) {
            return Err(Block::Scope);
        }
        let capture_digest =
            Digest::parse(string(get(o, "capture_digest")).map_err(|_| Block::Scope)?)
                .map_err(|_| Block::Scope)?;
        let capture = ReportedCapture::parse(
            run,
            self.binding.monitor.qpc_frequency,
            self.binding.epoch,
            string(get(o, "capture"))
                .map_err(|_| Block::Scope)?
                .as_bytes(),
            capture_digest,
            &self.budget,
        )?;
        let mut clock =
            Clock::new(self.binding.monitor, self.binding.epoch).map_err(|_| Block::Clock)?;
        let mut last = self.binding.epoch;
        let chronology = (|| {
            observe(&mut clock, &mut last, issued)?;
            for o in &capture.draft().observations {
                for anchor in [o.before, o.after].into_iter().flatten() {
                    if observe(&mut clock, &mut last, anchor)? >= self.deadline_ns {
                        return Err(Block::Phase);
                    }
                }
            }
            observe(&mut clock, &mut last, received)?;
            capture.current()?;
            Ok::<(), Block>(())
        })();
        if let Err(f) = chronology {
            self.fault.get_or_insert(f);
        }
        Ok(Receipt {
            issued,
            received,
            capture,
            frame: Frame {
                bytes: bytes.to_vec(),
                _allocation: allocation,
            },
            fault: self.fault,
        })
    }
}
