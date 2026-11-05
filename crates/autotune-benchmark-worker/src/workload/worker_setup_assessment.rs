//! Consistency of reported setup facts only. Neither a source label nor a
//! successful screen authenticates execution, peer identity, consent or admission.
use super::{
    child_options::{Read, Variable},
    runtime_evidence,
    setup_model::{Entry, Fault as TraceFault, Operation, Output},
    wire::{Message, WorkerBinding},
    worker_setup::{Preparation, Receipt},
    worker_setup_delivery::{Arrival, CHUNK_BYTES},
};
use crate::{
    evidence::Digest,
    monitor::{
        clock::{Clock, ClockError},
        Anchor,
    },
};
use autotune_fma_reference::profile::Profile;
use autotune_probe::runtime_file::{Kind, CHUNK_BYTES as FILE_CHUNK_BYTES};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Report,
    Call(usize),
    File,
    Ready,
    Arrival(usize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Binding,
    Simulated,
    Preparation,
    Pending,
    Trace(TraceFault),
    Overflow,
    RejectedReturn,
    Failure,
    Loader,
    File(runtime_evidence::Fault),
    FileReads,
    FilePath,
    Missing,
    Order,
    Attempt,
    Status(i32),
    Output,
    Environment(Variable),
    Version,
    LoadingMode,
    DeviceCount,
    Mapping,
    Capability,
    Context,
    Memory,
    Trailing,
    Ready,
    Clock(Option<ClockError>),
    Deadline,
    Transport,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejection {
    pub site: Site,
    pub cause: Cause,
}
/// Consistent reported facts, never an admission or physical qualification token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub receipt: Digest,
    pub driver_version: i32,
    pub selected_ordinal: u32,
    pub selected_device: i32,
    pub dll_sha256: [u8; 32],
    pub dll_bytes: u64,
    pub calls: usize,
    pub file_checks: usize,
    pub setup_complete: Anchor,
    pub ready: Anchor,
}
fn reject(site: Site, cause: Cause) -> Rejection {
    Rejection { site, cause }
}
struct Timeline {
    clock: Clock,
    last: Anchor,
    deadline: u64,
}
impl Timeline {
    fn new(b: WorkerBinding, deadline: u64) -> Result<Self, Rejection> {
        Ok(Self {
            clock: Clock::new(b.monitor, b.epoch)
                .map_err(|e| reject(Site::Report, Cause::Clock(Some(e))))?,
            last: b.epoch,
            deadline,
        })
    }
    fn point(&mut self, a: Option<Anchor>, site: Site) -> Result<u64, Rejection> {
        let a = a.ok_or_else(|| reject(site, Cause::Clock(None)))?;
        if a.qpc_before < self.last.qpc_after {
            return Err(reject(site, Cause::Clock(Some(ClockError::Order))));
        }
        let elapsed = self
            .clock
            .observe(a)
            .map_err(|e| reject(site, Cause::Clock(Some(e))))?;
        self.last = a;
        if a.interrupt_100ns >= self.deadline {
            return Err(reject(site, Cause::Deadline));
        }
        Ok(elapsed)
    }
}
struct Calls<'a> {
    entries: &'a [Entry],
    next: usize,
    time: Timeline,
}
impl Calls<'_> {
    fn take(&mut self, op: Operation) -> Result<Output, Rejection> {
        let site = Site::Call(self.next);
        let e = self
            .entries
            .get(self.next)
            .ok_or_else(|| reject(site, Cause::Missing))?;
        if e.operation != op {
            return Err(reject(site, Cause::Order));
        }
        self.time.point(e.before, site)?;
        if !e.attempted {
            return Err(reject(site, Cause::Attempt));
        }
        let raw = e.raw.ok_or_else(|| reject(site, Cause::Missing))?;
        // Preserve native status precedence over a later post-clock failure.
        if raw.status != 0 {
            return Err(reject(site, Cause::Status(raw.status)));
        }
        self.time.point(e.after, site)?;
        self.next += 1;
        Ok(raw.output)
    }
    fn require(&mut self, op: Operation, expected: Output, cause: Cause) -> Result<(), Rejection> {
        let i = self.next;
        if self.take(op)? != expected {
            return Err(reject(Site::Call(i), cause));
        }
        Ok(())
    }
    fn integer(&mut self, op: Operation) -> Result<i32, Rejection> {
        let i = self.next;
        match self.take(op)? {
            Output::Integer(n) => Ok(n),
            _ => Err(reject(Site::Call(i), Cause::Output)),
        }
    }
    fn env(&mut self, v: Variable, reported: Option<Read>) -> Result<(), Rejection> {
        let i = self.next;
        match self.take(Operation::Environment(v))? {
            Output::Environment(r) if r.matches(v) && Some(r) == reported => Ok(()),
            _ => Err(reject(Site::Call(i), Cause::Environment(v))),
        }
    }
}
/// Readiness is assessed against the exact already-read receipt and transport
/// observations. These are still untrusted assertions, not native authority.
pub fn screen(
    receipt: &Receipt,
    b: WorkerBinding,
    ready: &Message,
    arrivals: &[Arrival],
) -> Result<Summary, Rejection> {
    let r = receipt.reported();
    let root = Site::Report;
    if receipt.binding() != b {
        return Err(reject(root, Cause::Binding));
    }
    if r.simulated {
        return Err(reject(root, Cause::Simulated));
    }
    if r.preparation != Preparation::Ready {
        return Err(reject(root, Cause::Preparation));
    }
    if r.pending {
        return Err(reject(root, Cause::Pending));
    }
    if let Some(f) = r.fault {
        return Err(reject(root, Cause::Trace(f)));
    }
    if r.overflow.is_some() || r.file_overflow.is_some() {
        return Err(reject(root, Cause::Overflow));
    }
    if r.rejected.is_some() {
        return Err(reject(root, Cause::RejectedReturn));
    }
    if r.cpu_fault.is_some() || r.native_fault.is_some() {
        return Err(reject(root, Cause::Failure));
    }
    if r.load_error.is_some() {
        return Err(reject(root, Cause::Loader));
    }
    let base = b.epoch.interrupt_100ns;
    if r.deadline_raw <= base.saturating_add(b.origin_ns / 100)
        || r.deadline_raw > base.saturating_add(b.setup_boundary() / 100)
        || r.deadline_raw == u64::MAX
    {
        return Err(reject(root, Cause::Deadline));
    }
    let mut time = Timeline::new(b, r.deadline_raw)?;
    if time.point(Some(r.first), root)? < b.origin_ns {
        return Err(reject(root, Cause::Binding));
    }
    let mut calls = Calls {
        entries: &r.entries,
        next: 0,
        time,
    };
    calls.env(Variable::CacheDisable, r.environment.cache_disable)?;
    calls.env(Variable::ModuleLoading, r.environment.module_loading)?;
    let file = r
        .file
        .as_ref()
        .ok_or_else(|| reject(Site::File, Cause::Missing))?;
    runtime_evidence::check_file_record(file, Kind::Cuda, &r.file_checks)
        .map_err(|e| reject(Site::File, Cause::File(e)))?;
    // A successful nonempty stream has positive reads followed by one zero EOF.
    if file.report.reads < 2
        || u64::from(file.report.reads - 1) > file.report.bytes
        || file.report.bytes > u64::from(file.report.reads - 1) * FILE_CHUNK_BYTES as u64
    {
        return Err(reject(Site::File, Cause::FileReads));
    }
    let name = [110u16, 118, 99, 117, 100, 97, 46, 100, 108, 108];
    let requested = file.requested.as_ref().expect("checked file path");
    let loaded = file.loaded.as_ref().expect("checked loaded path");
    let directory = file.directory.expect("checked directory").length as usize;
    let suffix = |path: &[u16]| {
        path.len() > name.len()
            && matches!(path[path.len() - name.len() - 1], 47 | 92)
            && path[path.len() - name.len()..]
                .iter()
                .zip(&name)
                .all(|(a, b)| {
                    let a = if (65..=90).contains(a) { a + 32 } else { *a };
                    a == *b
                })
    };
    let joined = (requested.len() == directory + 1 + name.len()
        && matches!(requested[directory], 47 | 92))
        || (requested.len() == directory + name.len()
            && matches!(requested[directory - 1], 47 | 92));
    if !joined || !suffix(requested) || !suffix(loaded) {
        return Err(reject(Site::File, Cause::FilePath));
    }
    for c in &r.file_checks {
        calls.time.point(c.anchor, Site::File)?;
    }
    calls.require(Operation::Initialize, Output::None, Cause::Output)?;
    let version_site = Site::Call(calls.next);
    let version = calls.integer(Operation::DriverVersion)?;
    if version < 10010 || r.driver_version != Some(version) {
        return Err(reject(version_site, Cause::Version));
    }
    let loading_site = Site::Call(calls.next);
    let mode = calls.integer(Operation::ModuleLoadingMode)?;
    if mode != 1 || r.module_loading_mode != Some(mode) {
        return Err(reject(loading_site, Cause::LoadingMode));
    }
    let count_site = Site::Call(calls.next);
    let count = calls.integer(Operation::DeviceCount)?;
    if !(1..=64).contains(&count) {
        return Err(reject(count_site, Cause::DeviceCount));
    }
    let mut devices = [None; 64];
    let mut uuids = [None; 64];
    let mut selected = None;
    for ordinal in 0..count as u32 {
        let site = Site::Call(calls.next);
        let device = calls.integer(Operation::Device(ordinal))?;
        if device < 0 || devices[..ordinal as usize].contains(&Some(device)) {
            return Err(reject(site, Cause::Mapping));
        }
        devices[ordinal as usize] = Some(device);
        let site = Site::Call(calls.next);
        let uuid = match calls.take(Operation::Uuid(device))? {
            Output::Uuid(v) => v,
            _ => return Err(reject(site, Cause::Output)),
        };
        if uuid == [0; 16] || uuids[..ordinal as usize].contains(&Some(uuid)) {
            return Err(reject(site, Cause::Mapping));
        }
        uuids[ordinal as usize] = Some(uuid);
        if uuid == b.monitor.device.uuid {
            if selected.is_some() {
                return Err(reject(site, Cause::Mapping));
            }
            for (attribute, value) in [
                (50, i32::from(b.monitor.device.domain)),
                (33, i32::from(b.monitor.device.bus)),
                (34, i32::from(b.monitor.device.device)),
                (75, 7),
                (76, 5),
            ] {
                calls.require(
                    Operation::Attribute { device, attribute },
                    Output::Integer(value),
                    if attribute >= 75 {
                        Cause::Capability
                    } else {
                        Cause::Mapping
                    },
                )?;
            }
            selected = Some((ordinal, device));
        }
    }
    let (ordinal, device) =
        selected.ok_or_else(|| reject(Site::Call(calls.next), Cause::Mapping))?;
    calls.require(
        Operation::ContextCreate,
        Output::HandleIsNull(false),
        Cause::Context,
    )?;
    calls.require(
        Operation::ContextDevice,
        Output::Integer(device),
        Cause::Context,
    )?;
    calls.require(
        Operation::ModuleLoad,
        Output::HandleIsNull(false),
        Cause::Output,
    )?;
    calls.require(
        Operation::Function,
        Output::HandleIsNull(false),
        Cause::Output,
    )?;
    let site = Site::Call(calls.next);
    let (free, total) = match calls.take(Operation::MemoryInfo)? {
        Output::Memory { free, total } => (free as u64, total as u64),
        _ => return Err(reject(site, Cause::Output)),
    };
    let limits = Profile::approved_design().limits;
    let planned = b.plan.shape.application_buffer_bytes();
    if free > total
        || planned > limits.application_gpu_bytes
        || planned
            .checked_add(limits.free_gpu_headroom_bytes)
            .is_none_or(|required| free < required)
    {
        return Err(reject(site, Cause::Memory));
    }
    for n in 0..4 {
        calls.require(
            Operation::Allocate(n),
            Output::AllocationIsZero(false),
            Cause::Memory,
        )?;
    }
    calls.require(
        Operation::StreamCreate,
        Output::HandleIsNull(false),
        Cause::Output,
    )?;
    if calls.next != r.entries.len() {
        return Err(reject(Site::Call(calls.next), Cause::Trailing));
    }
    let complete = calls.time.last;
    ready
        .validate(b)
        .map_err(|_| reject(Site::Ready, Cause::Ready))?;
    let (ready_anchor, ready_version) = match ready {
        Message::Ready {
            anchor,
            driver_version,
            ..
        } => (*anchor, *driver_version),
        _ => return Err(reject(Site::Ready, Cause::Ready)),
    };
    if ready_version != version {
        return Err(reject(Site::Ready, Cause::Version));
    }
    calls.time.point(Some(ready_anchor), Site::Ready)?;
    let count = receipt.canonical().len().div_ceil(CHUNK_BYTES);
    if arrivals.len() != count + 1 {
        return Err(reject(root, Cause::Transport));
    }
    // Reception and worker Ready captures can overlap; validate each branch
    // from the common completed setup, then require Ready's causal final receipt.
    let mut reception = Timeline::new(b, r.deadline_raw)?;
    reception.point(Some(complete), Site::Report)?;
    for (i, a) in arrivals.iter().enumerate() {
        if a.index != if i < count { Some(i as u32) } else { None } {
            return Err(reject(Site::Arrival(i), Cause::Transport));
        }
        reception.point(a.receipt, Site::Arrival(i))?;
    }
    if reception.last.qpc_before < ready_anchor.qpc_after {
        return Err(reject(Site::Ready, Cause::Clock(Some(ClockError::Order))));
    }
    Ok(Summary {
        receipt: receipt.digest(),
        driver_version: version,
        selected_ordinal: ordinal,
        selected_device: device,
        dll_sha256: file.report.sha256.expect("complete file"),
        dll_bytes: file.report.bytes,
        calls: r.entries.len(),
        file_checks: r.file_checks.len(),
        setup_complete: complete,
        ready: ready_anchor,
    })
}
