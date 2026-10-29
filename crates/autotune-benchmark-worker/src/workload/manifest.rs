//! Immutable three-stage pilot declaration; hashes bind assertions, not consent.
//! No GPU/process/clock calls and no change to the simulated worker's admission.
use crate::{
    evidence::{decode, Digest, Domain, Object, Value},
    monitor::Binding,
    sampler::budget::{Budget, Reservation},
    worker::{DiscoveryBinding, Identity},
};
use autotune_fma_reference::profile::{Phase, Profile, RunPlan};

const MANIFEST_BYTES: usize = 16 * 1024;
const KEYS: &[&str] = &[
    "version",
    "scope",
    "session",
    "runs",
    "clock_token",
    "raw_uuid",
    "raw_pci",
    "source_commit",
    "source_tree",
    "lockfile",
    "controller",
    "worker",
    "sampler",
    "ptx",
    "protocol",
    "profile",
    "starting_evidence",
    "environment_evidence",
    "approval_ref",
    "output_root",
];
const RUN_KEYS: &[&str] = &[
    "run",
    "seed",
    "lanes",
    "steps",
    "warmup_dispatches",
    "measured_dispatches",
    "envelope_ns",
    "phase_caps_ns",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Binding,
    Capacity,
    Canonical,
    Plan,
    Clock,
}
#[derive(Clone, Copy, Debug)]
pub struct Artifacts {
    pub source_tree: Digest,
    pub lockfile: Digest,
    pub controller: Digest,
    pub worker: Digest,
    pub sampler: Digest,
    pub ptx: Digest,
    pub protocol: Digest,
}
#[derive(Clone, Debug)]
pub struct PilotDraft {
    pub session: [u8; 16],
    pub runs: [[u8; 16]; 3],
    pub clock_token: [u8; 16],
    pub raw_uuid: String,
    pub raw_pci: String,
    pub source_commit: String,
    pub artifacts: Artifacts,
    pub starting_evidence: Digest,
    pub environment_evidence: Digest,
    pub approval_ref: String,
    pub output_root: String,
}
pub struct PilotManifest {
    draft: PilotDraft,
    identity: Identity,
    canonical: String,
    digest: Digest,
    profile_digest: Digest,
    profile: String,
    _allocation: Reservation,
}
pub struct BoundRun<'a> {
    manifest: &'a PilotManifest,
    index: usize,
    origin_ns: u64,
    plan: RunPlan,
}
impl PilotManifest {
    pub fn freeze(draft: PilotDraft, budget: &Budget) -> Result<Self, Error> {
        let strings = [
            &draft.raw_uuid,
            &draft.raw_pci,
            &draft.source_commit,
            &draft.approval_ref,
            &draft.output_root,
        ];
        if strings
            .iter()
            .try_fold(0usize, |n, s| n.checked_add(s.capacity()))
            .is_none_or(|n| n > MANIFEST_BYTES)
            || draft.raw_uuid.len() > 128
            || draft.raw_pci.len() > 128
            || draft.approval_ref.trim().is_empty()
            || draft.approval_ref.len() > 1024
            || draft.approval_ref.contains('\0')
            || !crate::sampler::manifest::valid_root(&draft.output_root)
            || draft.source_commit.len() != 40
            || !draft
                .source_commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || draft.session == [0; 16]
            || draft.clock_token == [0; 16]
            || draft.runs.contains(&[0; 16])
            || draft.runs[0] == draft.runs[1]
            || draft.runs[0] == draft.runs[2]
            || draft.runs[1] == draft.runs[2]
        {
            return Err(Error::Binding);
        }
        let a = draft.artifacts;
        if [
            a.source_tree,
            a.lockfile,
            a.controller,
            a.worker,
            a.sampler,
            a.ptx,
            a.protocol,
            draft.starting_evidence,
            draft.environment_evidence,
        ]
        .iter()
        .any(|d| d.as_bytes() == &[0; 32])
        {
            return Err(Error::Binding);
        }
        let identity = DiscoveryBinding::parse(&draft.raw_uuid, &draft.raw_pci)
            .map_err(|_| Error::Binding)?
            .identity;
        // Covers owned draft, canonical text, profile/manifest trees and their
        // temporary encodings under the explicit string/frame bounds. This is
        // an application ledger, not an allocator/OS memory guarantee.
        let allocation = budget
            .reserve(2 * frame_bytes())
            .map_err(|_| Error::Capacity)?;
        let profile = Profile::approved_design();
        let runs = || {
            profile
                .pilot
                .iter()
                .zip(draft.runs)
                .map(|(plan, run)| run_value(*plan, Some(run)))
                .collect()
        };
        let profile_text = object([
            ("id", text(&profile.id)),
            ("scope", text("benchmark_pilot")),
            ("limits", crate::sampler::manifest::limits_value()),
            (
                "plans",
                Value::Array(profile.pilot.iter().map(|p| run_value(*p, None)).collect()),
            ),
        ])
        .to_canonical();
        let profile_digest = Digest::of(Domain::Profile, profile_text.as_bytes());
        let canonical = object([
            ("version", text("1")),
            ("scope", text("benchmark_pilot")),
            ("session", text(&hex(&draft.session))),
            ("runs", Value::Array(runs())),
            ("clock_token", text(&hex(&draft.clock_token))),
            ("raw_uuid", text(&draft.raw_uuid)),
            ("raw_pci", text(&draft.raw_pci)),
            ("source_commit", text(&draft.source_commit)),
            ("source_tree", text(&a.source_tree.to_hex())),
            ("lockfile", text(&a.lockfile.to_hex())),
            ("controller", text(&a.controller.to_hex())),
            ("worker", text(&a.worker.to_hex())),
            ("sampler", text(&a.sampler.to_hex())),
            ("ptx", text(&a.ptx.to_hex())),
            ("protocol", text(&a.protocol.to_hex())),
            ("profile", text(&profile_digest.to_hex())),
            ("starting_evidence", text(&draft.starting_evidence.to_hex())),
            (
                "environment_evidence",
                text(&draft.environment_evidence.to_hex()),
            ),
            ("approval_ref", text(&draft.approval_ref)),
            ("output_root", text(&draft.output_root)),
        ])
        .to_canonical();
        if canonical.len() > MANIFEST_BYTES {
            return Err(Error::Capacity);
        }
        let digest = Digest::of(Domain::Manifest, canonical.as_bytes());
        Ok(Self {
            draft,
            identity,
            canonical,
            digest,
            profile_digest,
            profile: profile_text,
            _allocation: allocation,
        })
    }
    pub fn parse(bytes: &[u8], budget: &Budget) -> Result<Self, Error> {
        if bytes.len() > MANIFEST_BYTES {
            return Err(Error::Capacity);
        }
        let _codec = budget
            .reserve(frame_bytes() * 16)
            .map_err(|_| Error::Capacity)?;
        let value = decode(bytes, MANIFEST_BYTES).map_err(|_| Error::Canonical)?;
        let o = exact(&value, KEYS)?;
        if string(o, "version")? != "1" || string(o, "scope")? != "benchmark_pilot" {
            return Err(Error::Binding);
        }
        let runs = get(o, "runs")?.as_array().ok_or(Error::Plan)?;
        let [r0, r1, r2] = runs else {
            return Err(Error::Plan);
        };
        let run_id = |r: &Value| parse_id::<16>(string(exact(r, RUN_KEYS)?, "run")?);
        let d = |key| Digest::parse(string(o, key)?).map_err(|_| Error::Binding);
        let draft = PilotDraft {
            session: parse_id(string(o, "session")?)?,
            runs: [run_id(r0)?, run_id(r1)?, run_id(r2)?],
            clock_token: parse_id(string(o, "clock_token")?)?,
            raw_uuid: string(o, "raw_uuid")?.into(),
            raw_pci: string(o, "raw_pci")?.into(),
            source_commit: string(o, "source_commit")?.into(),
            artifacts: Artifacts {
                source_tree: d("source_tree")?,
                lockfile: d("lockfile")?,
                controller: d("controller")?,
                worker: d("worker")?,
                sampler: d("sampler")?,
                ptx: d("ptx")?,
                protocol: d("protocol")?,
            },
            starting_evidence: d("starting_evidence")?,
            environment_evidence: d("environment_evidence")?,
            approval_ref: string(o, "approval_ref")?.into(),
            output_root: string(o, "output_root")?.into(),
        };
        let frozen = Self::freeze(draft, budget)?;
        // Re-encoding enforces every fixed plan/profile value and exact bytes;
        // ignored alternate counts, seeds, versions or canonical forms cannot pass.
        if frozen.canonical.as_bytes() != bytes {
            return Err(Error::Canonical);
        }
        Ok(frozen)
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    pub fn profile_digest(&self) -> Digest {
        self.profile_digest
    }
    pub fn profile_canonical(&self) -> &str {
        &self.profile
    }
    pub fn bind_run(&self, index: usize, origin_ns: u64) -> Result<BoundRun<'_>, Error> {
        let profile = Profile::approved_design();
        let plan = *profile.pilot.get(index).ok_or(Error::Plan)?;
        let reserve = profile.pilot[index..].iter().try_fold(
            profile.limits.finalization_ns + profile.limits.shutdown_reserve_ns,
            |n, p| n.checked_add(p.envelope_ns).ok_or(Error::Clock),
        )?;
        if origin_ns
            .checked_add(reserve)
            .is_none_or(|n| n > profile.limits.pilot_session_ns)
        {
            return Err(Error::Clock);
        }
        Ok(BoundRun {
            manifest: self,
            index,
            origin_ns,
            plan,
        })
    }
}
impl BoundRun<'_> {
    pub fn run(&self) -> [u8; 16] {
        self.manifest.draft.runs[self.index]
    }
    pub fn plan(&self) -> RunPlan {
        self.plan
    }
    pub fn manifest_digest(&self) -> Digest {
        self.manifest.digest
    }
    pub fn worker_digest(&self) -> Digest {
        self.manifest.draft.artifacts.worker
    }
    pub fn sampler_digest(&self) -> Digest {
        self.manifest.draft.artifacts.sampler
    }
    pub fn binding(&self, qpc_frequency: u64) -> Result<Binding, Error> {
        if qpc_frequency == 0 {
            return Err(Error::Clock);
        }
        Ok(Binding {
            session: self.manifest.draft.session,
            run: self.run(),
            device: self.manifest.identity,
            clock_token: self.manifest.draft.clock_token,
            qpc_frequency,
        })
    }
    /// Pure bound calculation. The controller must retain the actual FIRST
    /// phase entry; this method observes/advances no lifecycle and grants no Start.
    pub fn phase_deadline(&self, phase: Phase, first_entry_ns: u64) -> Result<u64, Error> {
        self.plan
            .phase_deadline_ns(phase, self.origin_ns, first_entry_ns)
            .map_err(|_| Error::Clock)
    }
}
fn frame_bytes() -> usize {
    Profile::approved_design().limits.frame_bytes as usize
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut o = Object::new();
    for (k, v) in fields {
        o.insert(k, v).expect("fixed unique keys");
    }
    Value::Object(o)
}
fn run_value(p: RunPlan, run: Option<[u8; 16]>) -> Value {
    let mut o = Object::new();
    if let Some(run) = run {
        o.insert("run", text(&hex(&run))).expect("fixed key");
    }
    for (key, value) in [
        ("seed", Value::U64(p.seed)),
        ("lanes", Value::U64(p.shape.lanes().into())),
        ("steps", Value::U64(p.shape.steps().value().into())),
        ("warmup_dispatches", Value::U64(p.warmup_dispatches.into())),
        (
            "measured_dispatches",
            Value::U64(p.measured_dispatches.into()),
        ),
        ("envelope_ns", Value::U64(p.envelope_ns)),
        (
            "phase_caps_ns",
            Value::Array(p.phase_caps_ns.map(Value::U64).into()),
        ),
    ] {
        o.insert(key, value).expect("fixed unique keys");
    }
    Value::Object(o)
}
fn exact<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Object, Error> {
    let o = v.as_object().ok_or(Error::Canonical)?;
    o.require_exact(keys).map_err(|_| Error::Canonical)?;
    Ok(o)
}
fn get<'a>(o: &'a Object, key: &str) -> Result<&'a Value, Error> {
    o.get(key).ok_or(Error::Canonical)
}
fn string<'a>(o: &'a Object, key: &str) -> Result<&'a str, Error> {
    get(o, key)?.as_text().ok_or(Error::Canonical)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|n| format!("{n:02x}")).collect()
}
fn parse_id<const N: usize>(s: &str) -> Result<[u8; N], Error> {
    if s.len() != N * 2 {
        return Err(Error::Binding);
    }
    let mut result = [0; N];
    for (i, pair) in s.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let digit = |b| match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err(Error::Binding),
        };
        result[i] = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Ok(result)
}
