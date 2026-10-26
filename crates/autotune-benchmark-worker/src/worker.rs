use autotune_fma_reference::{
    input::{DispatchIndex, Recipe, DISPATCH_CAPACITY},
    profile::{Limits, Profile},
    reference::{CheckReport, PreparedReference},
};
use sha2::{Digest, Sha256};

/// The approved profile is the only source of frozen enforcement values; local
/// literals could silently drift from an approved revision.
fn limits() -> Limits {
    static LIMITS: std::sync::OnceLock<Limits> = std::sync::OnceLock::new();
    *LIMITS.get_or_init(|| Profile::approved_design().limits)
}

/// PCI function belongs to the discovery side only; CUDA does not query it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub uuid: [u8; 16],
    pub domain: u16,
    pub bus: u8,
    pub device: u8,
}
impl Identity {
    pub fn valid(self) -> bool {
        self.uuid != [0; 16] && self.device <= 31
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryBinding {
    pub raw_uuid: String,
    pub raw_pci: String,
    pub identity: Identity,
    pub pci_function: u8,
}
impl DiscoveryBinding {
    /// Full-device UUID and PCI only; partition/MIG/unknown/ambiguous forms
    /// reject. Raw NVML function is retained, never attributed to CUDA.
    pub fn parse(uuid: &str, pci: &str) -> Result<Self, &'static str> {
        if uuid.len() != 40 || !uuid.starts_with("GPU-") || pci.len() > 24 {
            return Err("identity form/length");
        }
        let body = &uuid.as_bytes()[4..];
        let mut bytes = [0u8; 16];
        let mut nibble = 0;
        for (i, ch) in body.iter().copied().enumerate() {
            if [8, 13, 18, 23].contains(&i) {
                if ch != b'-' {
                    return Err("UUID separator");
                }
                continue;
            }
            let value = (ch as char).to_digit(16).ok_or("UUID hex")? as u8;
            if nibble % 2 == 0 {
                bytes[nibble / 2] = value << 4;
            } else {
                bytes[nibble / 2] |= value;
            }
            nibble += 1;
        }
        let (domain, rest) = pci.split_once(':').ok_or("PCI domain separator")?;
        let (bus, rest) = rest.split_once(':').ok_or("PCI bus separator")?;
        let (device, function) = rest.split_once('.').ok_or("PCI function separator")?;
        let parts = [domain, bus, device, function];
        let mut values = [0u32; 4];
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty()
                || part.len() > [8, 2, 2, 1][i]
                || !part.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("PCI form");
            }
            values[i] = u32::from_str_radix(part, 16).map_err(|_| "PCI hex")?;
        }
        if values[0] > u16::MAX as u32 || values[1] > 255 || values[2] > 31 || values[3] > 7 {
            return Err("PCI range");
        }
        let identity = Identity {
            uuid: bytes,
            domain: values[0] as u16,
            bus: values[1] as u8,
            device: values[2] as u8,
        };
        if !identity.valid() {
            return Err("invalid UUID");
        }
        Ok(Self {
            raw_uuid: uuid.into(),
            raw_pci: pci.into(),
            identity,
            pci_function: values[3] as u8,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    SimulatedFixture,
    Native,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub operation: &'static str,
    pub raw_status: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    Pending,
    Complete,
}

/// Serialized calls only. Submit must initialize the entire device output to
/// the supplied nonfinite sentinel before launch. Readback returns exact length.
/// An error may mean an earlier native operation is still outstanding.
pub trait Backend {
    fn provenance(&self) -> Provenance;
    fn identity(&mut self) -> Result<Identity, Fault>;
    fn submit(
        &mut self,
        a: &[f32],
        b: &[f32],
        x: &[f32],
        steps: u32,
        sentinel: &[f32],
    ) -> Result<(), Fault>;
    fn query(&mut self) -> Result<Query, Fault>;
    fn readback(&mut self, output: &mut [f32]) -> Result<usize, Fault>;
}
#[derive(Clone, Copy, Debug)]
pub struct Request {
    pub run: [u8; 16],
    pub identity: Identity,
    pub recipe: Recipe,
    pub dispatches: u32,
    pub start_ns: u64,
    pub deadline_ns: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    CheckedFixture,
    Stopped,
    DeadlineUnknown,
    ClockFault,
    BackendFault(Fault),
    IdentityFault,
    Incorrect,
    LengthFault,
    LiveAdmissionUnavailable,
}
#[derive(Clone, Debug)]
pub struct DispatchRecord {
    pub dispatch: DispatchIndex,
    pub check: CheckReport,
    pub input_digest: [u8; 32],
    pub output_digest: [u8; 32],
    pub submitted_ns: u64,
    pub observed_ns: u64,
}
#[derive(Clone, Debug)]
pub struct Report {
    run: [u8; 16],
    identity: Identity,
    recipe: Recipe,
    records: Vec<DispatchRecord>,
    outcome: Option<Outcome>,
    stop_ns: Option<u64>,
    failure_output: Option<Vec<u32>>,
    raw_fault: Option<Fault>,
    readback_length: Option<usize>,
    last_check: Option<CheckReport>,
}
impl Report {
    pub fn run(&self) -> [u8; 16] {
        self.run
    }
    pub fn identity(&self) -> Identity {
        self.identity
    }
    pub fn recipe(&self) -> Recipe {
        self.recipe
    }
    pub fn records(&self) -> &[DispatchRecord] {
        &self.records
    }
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }
    pub fn stop_ns(&self) -> Option<u64> {
        self.stop_ns
    }
    pub fn failure_output(&self) -> Option<&[u32]> {
        self.failure_output.as_deref()
    }
    pub fn provenance(&self) -> Provenance {
        Provenance::SimulatedFixture
    }
    pub fn raw_fault(&self) -> Option<Fault> {
        self.raw_fault
    }
    pub fn readback_length(&self) -> Option<usize> {
        self.readback_length
    }
    pub fn last_check(&self) -> Option<CheckReport> {
        self.last_check
    }
    /// Neither correctness nor fixture completion proves process/native exit.
    pub fn native_quiescence_known(&self) -> bool {
        false
    }
}
pub struct Worker<B> {
    backend: B,
    request: Request,
    prepared: PreparedReference,
    x: Vec<f32>,
    output: Vec<f32>,
    report: Report,
    last_ns: u64,
    in_flight: Option<u64>,
    stream_complete: bool,
}
impl<B: Backend> Worker<B> {
    pub fn new(mut backend: B, request: Request) -> Result<Self, Outcome> {
        if backend.provenance() != Provenance::SimulatedFixture {
            return Err(Outcome::LiveAdmissionUnavailable);
        }
        if request.run == [0; 16]
            || !request.identity.valid()
            || request.dispatches == 0
            || request.dispatches > DISPATCH_CAPACITY
            || request.deadline_ns <= request.start_ns
        {
            return Err(Outcome::IdentityFault);
        }
        if backend.identity().map_err(Outcome::BackendFault)? != request.identity {
            return Err(Outcome::IdentityFault);
        }
        let len = request.recipe.shape().lanes() as usize;
        Ok(Self {
            backend,
            prepared: PreparedReference::new(request.recipe),
            x: vec![0.0; len],
            output: vec![f32::NAN; len],
            report: Report {
                run: request.run,
                identity: request.identity,
                recipe: request.recipe,
                records: Vec::with_capacity(request.dispatches as usize),
                outcome: None,
                stop_ns: None,
                failure_output: None,
                raw_fault: None,
                readback_length: None,
                last_check: None,
            },
            last_ns: request.start_ns,
            in_flight: None,
            stream_complete: false,
            request,
        })
    }
    pub fn report(&self) -> &Report {
        &self.report
    }
    fn finish(&mut self, outcome: Outcome) {
        self.report.outcome.get_or_insert(outcome);
    }
    fn observe(&mut self, clock: &mut impl FnMut() -> u64) -> Option<u64> {
        let now_ns = clock();
        if now_ns < self.last_ns {
            self.finish(Outcome::ClockFault);
            return None;
        }
        self.last_ns = now_ns;
        let limits = limits();
        let cap = if self.stream_complete {
            limits.cycle_ns
        } else {
            limits.dispatch_completion_ns
        };
        if now_ns >= self.request.deadline_ns
            || self.in_flight.is_some_and(|start| now_ns - start >= cap)
        {
            self.finish(Outcome::DeadlineUnknown);
            return None;
        }
        Some(now_ns)
    }
    fn returned<T>(
        &mut self,
        clock: &mut impl FnMut() -> u64,
        result: Result<T, Fault>,
    ) -> Option<T> {
        if let Err(fault) = &result {
            self.report.raw_fault.get_or_insert(*fault);
        }
        self.observe(clock)?;
        match result {
            Ok(value) => Some(value),
            Err(fault) => {
                self.finish(Outcome::BackendFault(fault));
                None
            }
        }
    }
    /// Injected monotonic clock is read before admission and after every backend
    /// return/check. A blocked call still needs an independent process supervisor;
    /// these observations never promise hard real-time enforcement.
    pub fn advance(&mut self, clock: &mut impl FnMut() -> u64, stop: bool) {
        if self.report.outcome.is_some() {
            return;
        }
        let Some(now_ns) = self.observe(clock) else {
            return;
        };
        if stop {
            self.report.stop_ns.get_or_insert(now_ns);
        }
        if self.report.stop_ns.is_some() && self.in_flight.is_none() {
            self.finish(Outcome::Stopped);
            return;
        }
        let identity = self.backend.identity();
        match self.returned(clock, identity) {
            Some(id) if id == self.request.identity => (),
            Some(_) => {
                self.finish(Outcome::IdentityFault);
                return;
            }
            None => return,
        }
        if let Some(submitted_ns) = self.in_flight {
            let query = self.backend.query();
            match self.returned(clock, query) {
                Some(Query::Pending) | None => return,
                Some(Query::Complete) => self.stream_complete = true,
            }
            let readback = self.backend.readback(&mut self.output);
            if let Ok(length) = &readback {
                self.report.readback_length = Some(*length);
            }
            match self.returned(clock, readback) {
                None => return,
                Some(len) if len != self.output.len() => {
                    self.report.failure_output =
                        Some(self.output.iter().map(|v| v.to_bits()).collect());
                    self.finish(Outcome::LengthFault);
                    return;
                }
                Some(_) => (),
            }
            let dispatch =
                DispatchIndex::new(self.report.records.len() as u32).expect("bounded request");
            let check = self
                .prepared
                .check(dispatch, &self.output)
                .expect("fixed output allocation");
            self.report.last_check = Some(check);
            if !check.matches_reference() {
                self.report.failure_output =
                    Some(self.output.iter().map(|v| v.to_bits()).collect());
            }
            let input_digest = digest(&[self.prepared.a(), self.prepared.b(), &self.x]);
            let output_digest = digest(&[&self.output]);
            let Some(observed_ns) = self.observe(clock) else {
                return;
            };
            self.report.records.push(DispatchRecord {
                dispatch,
                check,
                input_digest,
                output_digest,
                submitted_ns,
                observed_ns,
            });
            self.in_flight = None;
            if !check.matches_reference() {
                self.finish(Outcome::Incorrect);
                return;
            }
            // Return after each full check: no same-step successor hidden after stop.
            if self.report.stop_ns.is_some() {
                self.finish(Outcome::Stopped);
            } else if self.report.records.len() == self.request.dispatches as usize {
                self.finish(Outcome::CheckedFixture);
            }
            return;
        }
        if self.report.stop_ns.is_some() {
            self.finish(Outcome::Stopped);
            return;
        }
        let dispatch =
            DispatchIndex::new(self.report.records.len() as u32).expect("bounded request");
        self.prepared
            .fill_x(dispatch, &mut self.x)
            .expect("fixed input allocation");
        self.output.fill(f32::NAN);
        // Mark uncertainty before entry, including when submit returns an error.
        self.in_flight = Some(now_ns);
        self.stream_complete = false;
        if self.observe(clock).is_none() {
            return;
        }
        let submitted = self.backend.submit(
            self.prepared.a(),
            self.prepared.b(),
            &self.x,
            self.request.recipe.shape().steps().value(),
            &self.output,
        );
        self.returned(clock, submitted);
    }
}
fn digest(slices: &[&[f32]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for slice in slices {
        for value in *slice {
            hash.update(value.to_bits().to_le_bytes());
        }
    }
    hash.finalize().into()
}
