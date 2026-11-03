//! Distinct pre-manifest sampler launch/bootstrap. No pilot run or native permit.
//! The caller retains received packets/actual clocks on rejection, validates live
//! handles and the locked image, and independently supervises startup and reads.
use super::{
    launch::{decimal, valid_handles, Role},
    settings::{parse_anchor, raw_anchor},
    starting::Scope,
    transport::{Queue, SendFailure},
};
use crate::{
    evidence::{decode, Digest, Domain, Value},
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        wire::Lane,
        wire_model::{exact, get, hex, object, string, unhex},
    },
};
const CAP: usize = 64 * 1024;
pub const FLAG: &str = "--pilot-starting-v1";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Arguments,
    Binding,
    Frame,
    Clock,
    Deadline,
    Capacity,
}
pub struct Declaration {
    scope: Digest,
    image: Digest,
    frequency: u64,
    epoch: Anchor,
    entry: Anchor,
    tag: Digest,
    _allocation: Reservation,
}
impl Declaration {
    pub fn new(scope: &Scope, entry: Anchor, budget: &Budget) -> Result<Self, Error> {
        let d = scope.draft();
        let mut clock =
            Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| Error::Clock)?;
        let n = clock.observe(entry).map_err(|_| Error::Clock)?;
        if entry.qpc_before < d.epoch.qpc_after {
            return Err(Error::Clock);
        }
        if n >= deadline_ns() {
            return Err(Error::Deadline);
        }
        let _codec = budget.reserve(CAP * 16).map_err(|_| Error::Capacity)?;
        let allocation = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        let tag = Digest::of(
            Domain::RawArtifact,
            object([
                ("version", Value::Text("1".into())),
                ("role", Value::Text(FLAG.into())),
                ("scope", Value::Text(scope.digest().to_hex())),
                ("entry", raw_anchor(entry)),
            ])
            .to_canonical()
            .as_bytes(),
        );
        Ok(Self {
            scope: scope.digest(),
            image: d.artifacts.sampler,
            frequency: d.frequency,
            epoch: d.epoch,
            entry,
            tag,
            _allocation: allocation,
        })
    }
    pub fn image_name(&self) -> &'static str {
        Role::Sampler.image_name()
    }
    pub fn image_digest(&self) -> Digest {
        self.image
    }
    pub fn entry(&self) -> Anchor {
        self.entry
    }
    pub fn scope_digest(&self) -> Digest {
        self.scope
    }
    pub fn tag(&self) -> Digest {
        self.tag
    }
    /// Caller reserves command storage first. Handles are tokens, never adopted here.
    pub fn arguments(&self, handles: [usize; 5]) -> Result<Vec<String>, Error> {
        valid_handles(handles).map_err(|_| Error::Arguments)?;
        Ok(vec![
            FLAG.into(),
            handles[0].to_string(),
            handles[1].to_string(),
            handles[2].to_string(),
            handles[3].to_string(),
            handles[4].to_string(),
            self.scope.to_hex(),
            self.frequency.to_string(),
            hex(&self.epoch.clock_token),
            self.epoch.qpc_before.to_string(),
            self.epoch.qpc_after.to_string(),
            self.epoch.interrupt_100ns.to_string(),
            self.epoch.unbiased_100ns.to_string(),
            self.tag.to_hex(),
        ])
    }
}
pub struct Arguments {
    handles: [usize; 5],
    scope: Digest,
    frequency: u64,
    epoch: Anchor,
    tag: Digest,
}
impl Arguments {
    /// Excludes argv[0]. Deliberately rejects worker/pilot manifest argument modes.
    pub fn parse(args: &[String]) -> Result<Self, Error> {
        if args.len() != 14 || args.iter().any(|s| s.len() > 128) || args[0] != FLAG {
            return Err(Error::Arguments);
        }
        let n = |i: usize| decimal(&args[i]).map_err(|_| Error::Arguments);
        let mut handles = [0; 5];
        for (i, h) in handles.iter_mut().enumerate() {
            *h = usize::try_from(n(i + 1)?).map_err(|_| Error::Arguments)?;
        }
        valid_handles(handles).map_err(|_| Error::Arguments)?;
        let epoch = Anchor {
            clock_token: unhex(&Value::Text(args[8].clone())).map_err(|_| Error::Arguments)?,
            qpc_before: n(9)?,
            qpc_after: n(10)?,
            interrupt_100ns: n(11)?,
            unbiased_100ns: n(12)?,
        };
        let frequency = n(7)?;
        Clock::temporal(frequency, epoch.clock_token, epoch).map_err(|_| Error::Clock)?;
        Ok(Self {
            handles,
            scope: Digest::parse(&args[6]).map_err(|_| Error::Arguments)?,
            frequency,
            epoch,
            tag: Digest::parse(&args[13]).map_err(|_| Error::Arguments)?,
        })
    }
    pub fn handles(&self) -> [usize; 5] {
        self.handles
    }
    pub fn frequency(&self) -> u64 {
        self.frequency
    }
    pub fn epoch(&self) -> Anchor {
        self.epoch
    }
    /// Bound the pre-bootstrap wait by the ORIGINAL session epoch, even before
    /// the request arrives. This validates actual first entry without a run ID.
    pub fn preflight_end_100ns(&self, first: Anchor) -> Result<u64, Error> {
        let mut clock = Clock::temporal(self.frequency, self.epoch.clock_token, self.epoch)
            .map_err(|_| Error::Clock)?;
        if first.qpc_before < self.epoch.qpc_after {
            return Err(Error::Clock);
        }
        if clock.observe(first).map_err(|_| Error::Clock)? >= deadline_ns() {
            return Err(Error::Deadline);
        }
        self.epoch
            .interrupt_100ns
            .checked_add(deadline_ns() / 100)
            .ok_or(Error::Deadline)
    }
}
pub struct Frame {
    bytes: Vec<u8>,
    tag: Digest,
    _allocation: Reservation,
}
impl Frame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn encode(
        declaration: &Declaration,
        scope: &Scope,
        budget: &Budget,
    ) -> Result<Self, Error> {
        if scope.digest() != declaration.scope {
            return Err(Error::Binding);
        }
        let _codec = budget.reserve(CAP * 16).map_err(|_| Error::Capacity)?;
        let allocation = budget
            .reserve(CAP + std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        let text = object([
            ("version", Value::Text("1".into())),
            ("role", Value::Text("pilot_starting_bootstrap".into())),
            ("declaration", Value::Text(declaration.tag.to_hex())),
            ("entry", raw_anchor(declaration.entry)),
            ("request", Value::Text(scope.canonical().into())),
        ])
        .to_canonical();
        let len = text
            .len()
            .checked_add(4)
            .filter(|n| *n <= CAP)
            .ok_or(Error::Capacity)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| Error::Capacity)?;
        bytes.extend_from_slice(&(len as u32).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
        Ok(Self {
            bytes,
            tag: declaration.tag,
            _allocation: allocation,
        })
    }
}
pub struct Bound {
    scope: Scope,
    declaration: Declaration,
    first: Anchor,
    receipt: Anchor,
    _allocation: Reservation,
}
impl Bound {
    /// first/receipt must be actual observations from the child, retained by its
    /// caller even when this returns Err. Binding a report never authorizes NVML.
    pub fn parse(
        args: &Arguments,
        bytes: &[u8],
        first: Anchor,
        receipt: Anchor,
        budget: &Budget,
    ) -> Result<Self, Error> {
        if !(4..=CAP).contains(&bytes.len())
            || u32::from_le_bytes(bytes[..4].try_into().map_err(|_| Error::Frame)?) as usize
                != bytes.len()
        {
            return Err(Error::Frame);
        }
        let _codec = budget.reserve(CAP * 16).map_err(|_| Error::Capacity)?;
        let allocation = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        crate::sampler::wire::validate_complexity_limit(&bytes[4..], 256)
            .map_err(|_| Error::Capacity)?;
        let v = decode(&bytes[4..], CAP - 4).map_err(|_| Error::Frame)?;
        let o = exact(&v, &["version", "role", "declaration", "entry", "request"])
            .map_err(|_| Error::Frame)?;
        if string(get(o, "version")).ok() != Some("1")
            || string(get(o, "role")).ok() != Some("pilot_starting_bootstrap")
        {
            return Err(Error::Binding);
        }
        let scope = Scope::parse(
            string(get(o, "request"))
                .map_err(|_| Error::Frame)?
                .as_bytes(),
            budget,
        )
        .map_err(|_| Error::Binding)?;
        if scope.digest() != args.scope
            || scope.draft().frequency != args.frequency
            || scope.draft().epoch != args.epoch
        {
            return Err(Error::Binding);
        }
        let entry = parse_anchor(get(o, "entry"))
            .map_err(|_| Error::Clock)?
            .ok_or(Error::Clock)?;
        let declaration = Declaration::new(&scope, entry, budget)?;
        if declaration.tag != args.tag
            || string(get(o, "declaration")).ok() != Some(declaration.tag.to_hex().as_str())
        {
            return Err(Error::Binding);
        }
        let mut clock = Clock::temporal(args.frequency, args.epoch.clock_token, args.epoch)
            .map_err(|_| Error::Clock)?;
        let mut last = args.epoch;
        for a in [entry, first, receipt] {
            if a.qpc_before < last.qpc_after {
                return Err(Error::Clock);
            }
            if clock.observe(a).map_err(|_| Error::Clock)? >= deadline_ns() {
                return Err(Error::Deadline);
            }
            last = a;
        }
        if Frame::encode(&declaration, &scope, budget)?.as_bytes() != bytes {
            return Err(Error::Frame);
        }
        Ok(Self {
            scope,
            declaration,
            first,
            receipt,
            _allocation: allocation,
        })
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn declaration(&self) -> &Declaration {
        &self.declaration
    }
    pub fn first_entry(&self) -> Anchor {
        self.first
    }
    pub fn receipt(&self) -> Anchor {
        self.receipt
    }
    /// Absolute elapsed time from the original epoch, never receipt + 10 seconds.
    pub fn deadline_ns(&self) -> u64 {
        deadline_ns()
    }
}
fn deadline_ns() -> u64 {
    autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .preflight_ns
}
/// One parent-to-child bootstrap handoff, with an exact original frame returned
/// on every failure. Cannot send pilot/worker frames or renew a failed handoff.
pub struct Sender<Q> {
    queue: Q,
    tag: Digest,
    attempted: bool,
    fault: Option<SendFailure>,
    _allocation: Reservation,
}
impl<Q: Queue> Sender<Q> {
    pub fn new(queue: Q, declaration: &Declaration, budget: &Budget) -> Result<Self, Error> {
        Ok(Self {
            queue,
            tag: declaration.tag,
            attempted: false,
            fault: None,
            _allocation: budget
                .reserve(std::mem::size_of::<Self>())
                .map_err(|_| Error::Capacity)?,
        })
    }
    pub fn fault(&self) -> Option<SendFailure> {
        self.fault
    }
    pub fn send(&mut self, frame: Frame) -> Result<(), Frame> {
        if self.attempted {
            self.fault.get_or_insert(SendFailure::Closed);
            return Err(frame);
        }
        self.attempted = true;
        let result = if frame.tag != self.tag {
            Err(SendFailure::Binding)
        } else {
            self.queue.try_send(Lane::Ordinary, frame.as_bytes())
        };
        match result {
            Ok(()) => Ok(()),
            Err(f) => {
                self.fault = Some(f);
                Err(frame)
            }
        }
    }
}
