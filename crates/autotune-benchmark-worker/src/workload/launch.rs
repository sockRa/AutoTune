//! Fixed pilot process declaration/argument vocabulary. Not execution consent.
use super::{
    manifest::{BoundRun, PilotManifest},
    transport::{Queue, SendFailure},
    wire::WorkerBinding,
};
use crate::sampler::wire::Lane;
pub(super) const WORKER_ENVIRONMENT: &str = super::child_options::ENVIRONMENT;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
/// Rechecks the original shared cancellation flag on EVERY handoff, including
/// senders constructed before stop. Control still carries the stop protocol.
pub struct CancelQueue<Q> {
    inner: Q,
    cancelled: Arc<AtomicBool>,
}
impl<Q> CancelQueue<Q> {
    pub fn new(inner: Q, cancelled: Arc<AtomicBool>) -> Self {
        Self { inner, cancelled }
    }
}
impl<Q: Queue> Queue for CancelQueue<Q> {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        if lane == Lane::Ordinary && self.cancelled.load(Ordering::Acquire) {
            return Err(SendFailure::Closed);
        }
        self.inner.try_send(lane, bytes)
    }
}
use crate::{
    evidence::{Digest, Domain, Value},
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{hex, object, unhex},
    },
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Worker,
    Sampler,
}
impl Role {
    pub fn image_name(self) -> &'static str {
        match self {
            Self::Worker => "autotune-pilot-worker.exe",
            Self::Sampler => "autotune-pilot-sampler.exe",
        }
    }
    fn flag(self) -> &'static str {
        match self {
            Self::Worker => "--pilot-worker-v1",
            Self::Sampler => "--pilot-sampler-v1",
        }
    }
    pub fn limits(self) -> (u64, u64) {
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        match self {
            Self::Worker => (p.worker_commit_bytes, p.worker_cpu_ns_per_run),
            Self::Sampler => (p.sampler_commit_bytes, p.sampler_cpu_ns_per_session),
        }
    }
    /// Exact child-only block. This does not mutate the parent/system environment
    /// or prove which options the driver actually used. Runtime readback follows.
    pub fn environment(self) -> Vec<u16> {
        match self {
            Self::Worker => WORKER_ENVIRONMENT.encode_utf16().collect(),
            Self::Sampler => vec![0, 0],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Binding,
    Arguments,
    Capacity,
}
pub struct Declaration {
    binding: WorkerBinding,
    manifest: Digest,
    image: Digest,
    role: Role,
    tag: Digest,
    #[cfg(windows)]
    transport: Digest,
    _metadata: Reservation,
}
impl Declaration {
    pub fn new(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        role: Role,
        budget: &Budget,
    ) -> Result<Self, Error> {
        let _codec = budget
            .reserve(64 * 1024 * 16)
            .map_err(|_| Error::Capacity)?;
        let metadata = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        let binding = WorkerBinding::new(run, frequency, epoch).map_err(|_| Error::Binding)?;
        let tag = Digest::of(
            Domain::RawArtifact,
            object([
                ("version", Value::Text("1".into())),
                ("role", Value::Text(role.flag().into())),
                ("binding", binding.value()),
            ])
            .to_canonical()
            .as_bytes(),
        );
        Ok(Self {
            binding,
            manifest: run.manifest_digest(),
            image: match role {
                Role::Worker => run.worker_digest(),
                Role::Sampler => run.sampler_digest(),
            },
            role,
            tag,
            #[cfg(windows)]
            transport: binding.transport_digest(),
            _metadata: metadata,
        })
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn binding(&self) -> WorkerBinding {
        self.binding
    }
    pub fn image_digest(&self) -> Digest {
        self.image
    }
    #[cfg(windows)]
    pub(crate) fn origin_parts(&self) -> (super::launch_origin::Kind, Digest, Digest) {
        (
            match self.role {
                Role::Worker => super::launch_origin::Kind::Worker,
                Role::Sampler => super::launch_origin::Kind::Sampler,
            },
            self.transport,
            self.tag,
        )
    }
    pub(super) fn tag(&self) -> Digest {
        self.tag
    }
    pub(super) fn manifest_digest(&self) -> Digest {
        self.manifest
    }
    /// Fixed bounded numeric/hex tokens; no paths, secrets or arbitrary commands.
    /// Caller reserves command/UTF-16 storage before formatting these tokens.
    pub fn arguments(&self, handles: [usize; 5]) -> Result<Vec<String>, Error> {
        valid_handles(handles)?;
        let b = self.binding;
        Ok(vec![
            self.role.flag().into(),
            handles[0].to_string(),
            handles[1].to_string(),
            handles[2].to_string(),
            handles[3].to_string(),
            handles[4].to_string(),
            self.manifest.to_hex(),
            hex(&b.monitor.run),
            b.monitor.qpc_frequency.to_string(),
            hex(&b.epoch.clock_token),
            b.epoch.qpc_before.to_string(),
            b.epoch.qpc_after.to_string(),
            b.epoch.interrupt_100ns.to_string(),
            b.epoch.unbiased_100ns.to_string(),
            b.origin_ns.to_string(),
            self.tag.to_hex(),
        ])
    }
}
pub struct Arguments {
    handles: [usize; 5],
    role: Role,
    manifest: Digest,
    run: [u8; 16],
    frequency: u64,
    epoch: Anchor,
    origin_ns: u64,
    tag: Digest,
}
impl Arguments {
    /// Arguments exclude argv[0]. Parsing neither adopts nor inspects handles.
    pub fn parse(args: &[String]) -> Result<Self, Error> {
        if args.len() != 16 || args.iter().any(|s| s.len() > 128) {
            return Err(Error::Arguments);
        }
        let role = match args[0].as_str() {
            "--pilot-worker-v1" => Role::Worker,
            "--pilot-sampler-v1" => Role::Sampler,
            _ => return Err(Error::Arguments),
        };
        let handles = [1, 2, 3, 4, 5].map(|i| {
            decimal(&args[i]).and_then(|n| usize::try_from(n).map_err(|_| Error::Arguments))
        });
        let [a, b, c, d, e] = handles;
        let handles = [a?, b?, c?, d?, e?];
        valid_handles(handles)?;
        Ok(Self {
            handles,
            role,
            manifest: Digest::parse(&args[6]).map_err(|_| Error::Arguments)?,
            run: unhex(&Value::Text(args[7].clone())).map_err(|_| Error::Arguments)?,
            frequency: decimal(&args[8])?,
            epoch: Anchor {
                clock_token: unhex(&Value::Text(args[9].clone())).map_err(|_| Error::Arguments)?,
                qpc_before: decimal(&args[10])?,
                qpc_after: decimal(&args[11])?,
                interrupt_100ns: decimal(&args[12])?,
                unbiased_100ns: decimal(&args[13])?,
            },
            origin_ns: decimal(&args[14])?,
            tag: Digest::parse(&args[15]).map_err(|_| Error::Arguments)?,
        })
    }
    pub fn handles(&self) -> [usize; 5] {
        self.handles
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn frequency(&self) -> u64 {
        self.frequency
    }
    pub fn epoch(&self) -> Anchor {
        self.epoch
    }
    /// The child must receive a bounded canonical pilot manifest before this
    /// check, then validate image/handles/runtime/consent before native work.
    pub fn bind<'a>(
        &self,
        manifest: &'a PilotManifest,
        budget: &Budget,
    ) -> Result<BoundRun<'a>, Error> {
        if manifest.digest() != self.manifest {
            return Err(Error::Binding);
        }
        for i in 0..3 {
            let run = manifest
                .bind_run(i, self.origin_ns)
                .map_err(|_| Error::Binding)?;
            if run.run() == self.run {
                let d = Declaration::new(&run, self.frequency, self.epoch, self.role, budget)?;
                if d.tag != self.tag {
                    return Err(Error::Binding);
                }
                return Ok(run);
            }
        }
        Err(Error::Binding)
    }
}
pub(super) fn valid_handles(handles: [usize; 5]) -> Result<(), Error> {
    for (i, h) in handles.iter().enumerate() {
        if *h == 0 || *h == usize::MAX || handles[..i].contains(h) {
            return Err(Error::Arguments);
        }
    }
    Ok(())
}
pub(super) fn decimal(s: &str) -> Result<u64, Error> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::Arguments);
    }
    s.parse().map_err(|_| Error::Arguments)
}
