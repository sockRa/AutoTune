//! Separate first-frame pilot bootstrap. No readonly protocol/native authority.
use super::{
    launch::{Arguments, Declaration, Role},
    manifest::PilotManifest,
    settings::{parse_anchor, raw_anchor},
    wire::WorkerBinding,
};
use crate::{
    evidence::{decode, Value},
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{exact, get, object, string},
    },
};
const CAP: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Capacity,
    Frame,
    Binding,
    Clock,
    Deadline,
}
/// Continuous temporal checks before the manifest/device binding is known.
/// Raw observations remain bounded and owned; this never asserts identity.
pub struct EntryClock {
    clock: Clock,
    last: Anchor,
    raw: Vec<Anchor>,
    overflow: Option<Anchor>,
    missing: bool,
    fault: Option<Error>,
    clock_error: Option<crate::monitor::clock::ClockError>,
    _allocation: Reservation,
}
impl EntryClock {
    pub fn new(
        frequency: u64,
        epoch: Anchor,
        first: Anchor,
        budget: &Budget,
    ) -> Result<Self, Error> {
        let allocation = budget
            .reserve(CAP + std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        let clock =
            Clock::temporal(frequency, epoch.clock_token, epoch).map_err(|_| Error::Clock)?;
        let mut raw = Vec::new();
        raw.try_reserve_exact(CAP / std::mem::size_of::<Anchor>())
            .map_err(|_| Error::Capacity)?;
        let mut result = Self {
            clock,
            last: epoch,
            raw,
            overflow: None,
            missing: false,
            fault: None,
            clock_error: None,
            _allocation: allocation,
        };
        result.observe(Some(first))?;
        Ok(result)
    }
    pub fn observe(&mut self, anchor: Option<Anchor>) -> Result<u64, Error> {
        if let Some(f) = self.fault {
            return Err(f);
        }
        let result = (|| {
            let Some(a) = anchor else {
                self.missing = true;
                return Err(Error::Clock);
            };
            if self.raw.len() == CAP / std::mem::size_of::<Anchor>() {
                self.overflow = Some(a);
                return Err(Error::Capacity);
            }
            self.raw.push(a);
            if a.qpc_before < self.last.qpc_after {
                self.clock_error = Some(crate::monitor::clock::ClockError::Order);
                return Err(Error::Clock);
            }
            let n = self.clock.observe(a).map_err(|e| {
                self.clock_error = Some(e);
                Error::Clock
            })?;
            self.last = a;
            Ok(n)
        })();
        if let Err(f) = result {
            self.fault = Some(f);
        }
        result
    }
    pub fn raw(&self) -> &[Anchor] {
        &self.raw
    }
    pub fn overflow(&self) -> Option<Anchor> {
        self.overflow
    }
    pub fn missing(&self) -> bool {
        self.missing
    }
    pub fn clock_error(&self) -> Option<crate::monitor::clock::ClockError> {
        self.clock_error
    }
}
pub struct Frame {
    bytes: Vec<u8>,
    pub(super) binding: crate::evidence::Digest,
    _allocation: Reservation,
}
impl Frame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn encode(
        declaration: &Declaration,
        manifest: &PilotManifest,
        setup_entry: Anchor,
        budget: &Budget,
    ) -> Result<Self, Error> {
        let _codec = budget.reserve(CAP * 16).map_err(|_| Error::Capacity)?;
        let allocation = budget
            .reserve(CAP + std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        let b = declaration.binding();
        let entry = b.elapsed(setup_entry).map_err(|_| Error::Clock)?;
        if entry < b.origin_ns || entry >= b.setup_boundary() {
            return Err(Error::Deadline);
        }
        // Validate the supplied manifest rather than silently sending a foreign
        // declaration through an otherwise matching transport binding.
        if manifest.digest() != declaration.manifest_digest() {
            return Err(Error::Binding);
        }
        let text = object([
            ("version", Value::Text("1".into())),
            ("role", Value::Text("pilot_bootstrap".into())),
            (
                "process_role",
                Value::Text(role_name(declaration.role()).into()),
            ),
            ("declaration", Value::Text(declaration.tag().to_hex())),
            ("setup_entry", raw_anchor(setup_entry)),
            ("manifest", Value::Text(manifest.canonical().into())),
        ])
        .to_canonical();
        let len = text
            .len()
            .checked_add(4)
            .filter(|n| *n <= CAP)
            .ok_or(Error::Capacity)?;
        let mut bytes = Vec::with_capacity(len);
        bytes.extend_from_slice(&(len as u32).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
        Ok(Self {
            bytes,
            binding: b.transport_digest(),
            _allocation: allocation,
        })
    }
}
pub struct Bound {
    manifest: PilotManifest,
    binding: WorkerBinding,
    role: Role,
    setup_entry: Anchor,
    receipt: Anchor,
    deadline_ns: u64,
    index: usize,
    _metadata: Reservation,
}
impl Bound {
    /// Caller retains the original framed packet on every branch. first_entry
    /// and receipt are actual child observations, never supplied by the parent.
    pub fn parse(
        args: &Arguments,
        expected_role: Role,
        bytes: &[u8],
        first_entry: Anchor,
        receipt: Anchor,
        budget: &Budget,
    ) -> Result<Self, Error> {
        if args.role() != expected_role {
            return Err(Error::Binding);
        }
        let prefix = bytes.get(..4).ok_or(Error::Frame)?;
        let len = u32::from_le_bytes(prefix.try_into().map_err(|_| Error::Frame)?) as usize;
        if len != bytes.len() || !(4..=CAP).contains(&len) {
            return Err(Error::Frame);
        }
        crate::sampler::wire::validate_complexity_limit(&bytes[4..], 256)
            .map_err(|_| Error::Capacity)?;
        let _codec = budget.reserve(CAP * 16).map_err(|_| Error::Capacity)?;
        let metadata = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        let v = decode(&bytes[4..], CAP - 4).map_err(|_| Error::Frame)?;
        let o = exact(
            &v,
            &[
                "version",
                "role",
                "process_role",
                "declaration",
                "setup_entry",
                "manifest",
            ],
        )
        .map_err(|_| Error::Frame)?;
        if string(get(o, "version")).ok() != Some("1")
            || string(get(o, "role")).ok() != Some("pilot_bootstrap")
            || string(get(o, "process_role")).ok() != Some(role_name(expected_role))
        {
            return Err(Error::Binding);
        }
        let manifest = PilotManifest::parse(
            string(get(o, "manifest"))
                .map_err(|_| Error::Frame)?
                .as_bytes(),
            budget,
        )
        .map_err(|_| Error::Binding)?;
        let run = args.bind(&manifest, budget).map_err(|_| Error::Binding)?;
        let index = run.index();
        let declaration =
            Declaration::new(&run, args.frequency(), args.epoch(), expected_role, budget)
                .map_err(|_| Error::Binding)?;
        if string(get(o, "declaration")).ok() != Some(declaration.tag().to_hex().as_str()) {
            return Err(Error::Binding);
        }
        let binding = declaration.binding();
        let setup_entry = parse_anchor(get(o, "setup_entry"))
            .map_err(|_| Error::Frame)?
            .ok_or(Error::Clock)?;
        let mut clock = Clock::new(binding.monitor, args.epoch()).map_err(|_| Error::Clock)?;
        let mut last = args.epoch();
        let mut observe = |a: Anchor| {
            if a.qpc_before < last.qpc_after {
                return Err(Error::Clock);
            }
            let n = clock.observe(a).map_err(|_| Error::Clock)?;
            last = a;
            Ok(n)
        };
        let entry = observe(setup_entry)?;
        observe(first_entry)?;
        let now = observe(receipt)?;
        let deadline_ns = entry
            .checked_add(binding.plan.phase_caps_ns[1])
            .ok_or(Error::Deadline)?
            .min(binding.setup_boundary());
        if entry < binding.origin_ns || now >= deadline_ns {
            return Err(Error::Deadline);
        }
        drop(declaration);
        Ok(Self {
            manifest,
            binding,
            role: expected_role,
            setup_entry,
            receipt,
            deadline_ns,
            index,
            _metadata: metadata,
        })
    }
    pub fn binding(&self) -> WorkerBinding {
        self.binding
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn manifest(&self) -> &PilotManifest {
        &self.manifest
    }
    pub fn setup_entry(&self) -> Anchor {
        self.setup_entry
    }
    pub fn receipt(&self) -> Anchor {
        self.receipt
    }
    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ns
    }
    pub fn run(&self) -> super::manifest::BoundRun<'_> {
        self.manifest
            .bind_run(self.index, self.binding.origin_ns)
            .expect("validated bootstrap run")
    }
}
fn role_name(r: Role) -> &'static str {
    match r {
        Role::Worker => "worker",
        Role::Sampler => "sampler",
    }
}
