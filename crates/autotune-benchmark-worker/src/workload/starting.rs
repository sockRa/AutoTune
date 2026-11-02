//! Starting-setting readout before a pilot manifest exists. No invented run ID,
//! manifest placeholder, native authority, writes or execution consent.
use super::{
    manifest::{pilot_profile_canonical, Artifacts, BoundRun, PilotDraft, PilotManifest},
    settings::{self, Block, Current, Draft, Observation, Source},
};
use crate::{
    evidence::{decode, Digest, Domain, Value},
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        child::NativeAdmission,
        wire_model::{exact, get, hex, object, string, uint, unhex},
    },
    worker::DiscoveryBinding,
};
use autotune_probe::{
    nvml::{NvmlSampler, SettingMetric},
    report::{Field, Reading, Reason},
};
const CAP: usize = 64 * 1024;
#[derive(Clone, Debug)]
pub struct ScopeDraft {
    pub session: [u8; 16],
    pub capture_nonce: [u8; 16],
    pub clock_token: [u8; 16],
    pub frequency: u64,
    pub epoch: Anchor,
    pub raw_uuid: String,
    pub raw_pci: String,
    pub source_commit: String,
    pub artifacts: Artifacts,
    pub read_approval_ref: String,
}
pub struct Scope {
    draft: ScopeDraft,
    canonical: String,
    digest: Digest,
    _allocation: Reservation,
}
impl Scope {
    pub fn freeze(draft: ScopeDraft, budget: &Budget) -> Result<Self, Block> {
        if draft.session == [0; 16]
            || draft.capture_nonce == [0; 16]
            || draft.clock_token == [0; 16]
            || draft.epoch.clock_token != draft.clock_token
            || draft.raw_uuid.capacity() > 128
            || draft.raw_pci.capacity() > 64
            || draft.source_commit.capacity() > 40
            || draft.source_commit.len() != 40
            || !draft
                .source_commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || draft.read_approval_ref.capacity() > 1024
            || draft.read_approval_ref.trim().is_empty()
            || draft.read_approval_ref.contains('\0')
        {
            return Err(Block::Scope);
        }
        DiscoveryBinding::parse(&draft.raw_uuid, &draft.raw_pci).map_err(|_| Block::Identity)?;
        Clock::temporal(draft.frequency, draft.clock_token, draft.epoch)
            .map_err(|_| Block::Clock)?;
        if artifact_values(draft.artifacts)
            .iter()
            .any(|d| d.as_bytes() == &[0; 32])
        {
            return Err(Block::Scope);
        }
        let allocation = budget
            .reserve(CAP * 2 + std::mem::size_of::<Self>())
            .map_err(|_| Block::Capacity)?;
        let _codec = budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        let canonical = scope_value(&draft).to_canonical();
        if canonical.len() > 16 * 1024 {
            return Err(Block::Capacity);
        }
        let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
        Ok(Self {
            draft,
            canonical,
            digest,
            _allocation: allocation,
        })
    }
    pub fn draft(&self) -> &ScopeDraft {
        &self.draft
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    pub fn parse(bytes: &[u8], budget: &Budget) -> Result<Self, Block> {
        if bytes.len() > 16 * 1024 {
            return Err(Block::Capacity);
        }
        let _codec = budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        crate::sampler::wire::validate_complexity_limit(bytes, 256).map_err(|_| Block::Capacity)?;
        let v = decode(bytes, 16 * 1024).map_err(|_| Block::Scope)?;
        let o = exact(
            &v,
            &[
                "version",
                "scope",
                "session",
                "capture_nonce",
                "clock_token",
                "frequency",
                "epoch",
                "raw_uuid",
                "raw_pci",
                "source_commit",
                "artifacts",
                "profile",
                "read_approval_ref",
            ],
        )
        .map_err(|_| Block::Scope)?;
        if text(get(o, "version"))? != "1"
            || text(get(o, "scope"))? != "pilot_starting_settings_request"
        {
            return Err(Block::Scope);
        }
        let a = exact(
            get(o, "artifacts"),
            &[
                "source_tree",
                "lockfile",
                "controller",
                "worker",
                "sampler",
                "ptx",
                "protocol",
            ],
        )
        .map_err(|_| Block::Scope)?;
        let d = |key| Digest::parse(text(get(a, key))?).map_err(|_| Block::Scope);
        let draft = ScopeDraft {
            session: unhex(get(o, "session")).map_err(|_| Block::Scope)?,
            capture_nonce: unhex(get(o, "capture_nonce")).map_err(|_| Block::Scope)?,
            clock_token: unhex(get(o, "clock_token")).map_err(|_| Block::Scope)?,
            frequency: uint(get(o, "frequency")).map_err(|_| Block::Clock)?,
            epoch: settings::parse_anchor(get(o, "epoch"))?.ok_or(Block::Clock)?,
            raw_uuid: text(get(o, "raw_uuid"))?.into(),
            raw_pci: text(get(o, "raw_pci"))?.into(),
            source_commit: text(get(o, "source_commit"))?.into(),
            artifacts: Artifacts {
                source_tree: d("source_tree")?,
                lockfile: d("lockfile")?,
                controller: d("controller")?,
                worker: d("worker")?,
                sampler: d("sampler")?,
                ptx: d("ptx")?,
                protocol: d("protocol")?,
            },
            read_approval_ref: text(get(o, "read_approval_ref"))?.into(),
        };
        let result = Self::freeze(draft, budget)?;
        if result.canonical().as_bytes() != bytes {
            return Err(Block::Scope);
        }
        Ok(result)
    }
}
fn artifact_values(a: Artifacts) -> [Digest; 7] {
    [
        a.source_tree,
        a.lockfile,
        a.controller,
        a.worker,
        a.sampler,
        a.ptx,
        a.protocol,
    ]
}
fn scope_value(d: &ScopeDraft) -> Value {
    let a = d.artifacts;
    object([
        ("version", Value::Text("1".into())),
        (
            "scope",
            Value::Text("pilot_starting_settings_request".into()),
        ),
        ("session", Value::Text(hex(&d.session))),
        ("capture_nonce", Value::Text(hex(&d.capture_nonce))),
        ("clock_token", Value::Text(hex(&d.clock_token))),
        ("frequency", Value::U64(d.frequency)),
        ("epoch", settings::raw_anchor(d.epoch)),
        ("raw_uuid", Value::Text(d.raw_uuid.clone())),
        ("raw_pci", Value::Text(d.raw_pci.clone())),
        ("source_commit", Value::Text(d.source_commit.clone())),
        (
            "artifacts",
            object([
                ("source_tree", Value::Text(a.source_tree.to_hex())),
                ("lockfile", Value::Text(a.lockfile.to_hex())),
                ("controller", Value::Text(a.controller.to_hex())),
                ("worker", Value::Text(a.worker.to_hex())),
                ("sampler", Value::Text(a.sampler.to_hex())),
                ("ptx", Value::Text(a.ptx.to_hex())),
                ("protocol", Value::Text(a.protocol.to_hex())),
            ]),
        ),
        (
            "profile",
            Value::Text(Digest::of(Domain::Profile, pilot_profile_canonical().as_bytes()).to_hex()),
        ),
        (
            "read_approval_ref",
            Value::Text(d.read_approval_ref.clone()),
        ),
    ])
}
pub struct Snapshot {
    scope: Scope,
    source: Source,
    entry: Anchor,
    draft: Draft,
    canonical: String,
    digest: Digest,
    _allocation: Reservation,
}
struct Storage {
    scope: Scope,
    allocation: Reservation,
    _codec: Reservation,
}
impl Storage {
    fn reserve(scope: &Scope, budget: &Budget) -> Result<Self, Block> {
        let scope = Scope::parse(scope.canonical().as_bytes(), budget)?;
        let allocation = budget.reserve(CAP * 3).map_err(|_| Block::Capacity)?;
        let codec = budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        Ok(Self {
            scope,
            allocation,
            _codec: codec,
        })
    }
}
impl Snapshot {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn entry(&self) -> Anchor {
        self.entry
    }
    pub fn draft(&self) -> &Draft {
        &self.draft
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    /// Bounded injected observations retain simulated provenance.
    pub fn fixture(
        scope: &Scope,
        entry: Anchor,
        draft: &Draft,
        budget: &Budget,
    ) -> Result<Self, Block> {
        Self::finalize(
            entry,
            draft,
            Source::SimulatedFixture,
            Storage::reserve(scope, budget)?,
        )
    }
    /// Invoke on the already supervised serial native lane only. Independent
    /// preflight launch/control/read approval remains a caller prerequisite.
    pub fn collect_native(
        scope: &Scope,
        entry: Anchor,
        sampler: &NvmlSampler,
        admission: &NativeAdmission,
        clock: impl FnMut() -> Option<Anchor>,
        budget: &Budget,
    ) -> Result<Self, Block> {
        Self::collect(
            scope,
            entry,
            sampler.identity().unwrap_or(("", "")),
            admission,
            clock,
            |kind| sampler.setting(kind),
            budget,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn collect(
        scope: &Scope,
        entry: Anchor,
        identity: (&str, &str),
        admission: &NativeAdmission,
        clock: impl FnMut() -> Option<Anchor>,
        read: impl FnMut(SettingMetric) -> Field,
        budget: &Budget,
    ) -> Result<Self, Block> {
        let result = Self::collect_inner(scope, entry, identity, admission, clock, read, budget);
        // Exactly this starting readout ends the native read lane. Cleanup is a
        // separately supervised operation; no successor capture is admitted.
        admission.stop();
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn collect_inner(
        scope: &Scope,
        entry: Anchor,
        identity: (&str, &str),
        admission: &NativeAdmission,
        mut clock: impl FnMut() -> Option<Anchor>,
        mut read: impl FnMut(SettingMetric) -> Field,
        budget: &Budget,
    ) -> Result<Self, Block> {
        let staging = Storage::reserve(scope, budget)?;
        let mut validator = Clock::temporal(
            scope.draft.frequency,
            scope.draft.clock_token,
            scope.draft.epoch,
        )
        .map_err(|_| Block::Clock)?;
        let mut last = scope.draft.epoch;
        settings::checked_anchor(&mut validator, &mut last, Some(entry))?;
        let end = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .preflight_ns;
        let mut draft = Draft {
            raw_uuid: identity.0.into(),
            raw_pci: identity.1.into(),
            observations: SettingMetric::ALL.map(|kind| Observation {
                kind,
                before: None,
                after: None,
                raw: None,
            }),
            halt: None,
        };
        if identity != (scope.draft.raw_uuid.as_str(), scope.draft.raw_pci.as_str()) {
            draft.halt = Some(Block::Identity);
        } else if admission.begin().is_err() {
            draft.halt = Some(Block::Stopped);
        } else {
            for observation in &mut draft.observations {
                observation.before = clock();
                match settings::checked_anchor(&mut validator, &mut last, observation.before) {
                    Ok(n) if n < end => (),
                    Ok(_) => {
                        draft.halt = Some(Block::Phase);
                        break;
                    }
                    Err(e) => {
                        draft.halt = Some(e);
                        break;
                    }
                }
                if admission.stopped() {
                    draft.halt = Some(Block::Stopped);
                    break;
                }
                observation.raw = Some(read(observation.kind));
                if matches!(&observation.raw,Some(Field{reading:Reading::Unavailable{failure},..}) if failure.reason==Reason::GpuLost||failure.vendor_status==Some(15))
                {
                    draft.halt = Some(Block::GpuLost);
                }
                observation.after = clock();
                match settings::checked_anchor(&mut validator, &mut last, observation.after) {
                    Ok(n) if n < end => (),
                    Ok(_) => {
                        draft.halt.get_or_insert(Block::Phase);
                    }
                    Err(e) => {
                        draft.halt.get_or_insert(e);
                    }
                }
                if admission.stopped() {
                    draft.halt.get_or_insert(Block::Stopped);
                }
                if draft.halt.is_some() {
                    break;
                }
            }
            admission.complete();
        }
        if draft.halt.is_some() {
            admission.stop();
        }
        Self::finalize(entry, &draft, Source::NativeGetterPath, staging)
    }
    fn finalize(
        entry: Anchor,
        draft: &Draft,
        source: Source,
        storage: Storage,
    ) -> Result<Self, Block> {
        if draft.raw_uuid.capacity() > 128
            || draft.raw_pci.capacity() > 64
            || draft
                .observations
                .iter()
                .any(|o| o.raw.as_ref().is_some_and(|f| !settings::bounded(f)))
        {
            return Err(Block::Capacity);
        }
        let Storage {
            scope,
            allocation,
            _codec,
        } = storage;
        let canonical = object([
            ("version", Value::Text("1".into())),
            (
                "scope",
                Value::Text("pilot_starting_settings_evidence".into()),
            ),
            ("request_digest", Value::Text(scope.digest().to_hex())),
            ("request", Value::Text(scope.canonical().into())),
            (
                "source",
                Value::Text(
                    match source {
                        Source::NativeGetterPath => "native_getter_path",
                        Source::SimulatedFixture => "simulated_fixture",
                    }
                    .into(),
                ),
            ),
            ("entry", settings::raw_anchor(entry)),
            (
                "deadline_ns",
                Value::U64(
                    autotune_fma_reference::profile::Profile::approved_design()
                        .limits
                        .preflight_ns,
                ),
            ),
            ("raw_uuid", Value::Text(draft.raw_uuid.clone())),
            ("raw_pci", Value::Text(draft.raw_pci.clone())),
            (
                "observations",
                Value::Array(
                    draft
                        .observations
                        .iter()
                        .map(|o| {
                            object([
                                ("kind", Value::Text(settings::kind_name(o.kind).into())),
                                ("before", o.before.map_or(Value::Null, settings::raw_anchor)),
                                ("after", o.after.map_or(Value::Null, settings::raw_anchor)),
                                (
                                    "raw",
                                    o.raw.as_ref().map_or(Value::Null, settings::raw_value),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "halt",
                draft
                    .halt
                    .map_or(Value::Null, |b| Value::Text(settings::block_name(b).into())),
            ),
        ])
        .to_canonical();
        if canonical.len() > CAP {
            return Err(Block::Capacity);
        }
        let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
        Ok(Self {
            scope,
            entry,
            draft: draft.clone(),
            source,
            canonical,
            digest,
            _allocation: allocation,
        })
    }
    pub fn current(&self) -> Result<Current, Block> {
        let mut clock = Clock::temporal(
            self.scope.draft.frequency,
            self.scope.draft.clock_token,
            self.scope.draft.epoch,
        )
        .map_err(|_| Block::Clock)?;
        let mut last = self.scope.draft.epoch;
        let lower = settings::checked_anchor(&mut clock, &mut last, Some(self.entry))?;
        let end = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .preflight_ns;
        if lower >= end {
            return Err(Block::Phase);
        }
        settings::current_draft(
            &self.draft,
            (&self.scope.draft.raw_uuid, &self.scope.draft.raw_pci),
            clock,
            last,
            lower,
            end,
        )
    }
}
/// Strictly parsed peer claim; hashes/source tags authenticate no execution.
pub struct ReportedSnapshot(Snapshot);
impl ReportedSnapshot {
    pub fn snapshot(&self) -> &Snapshot {
        &self.0
    }
    pub fn parse(
        scope: &Scope,
        bytes: &[u8],
        expected: Digest,
        budget: &Budget,
    ) -> Result<Self, Block> {
        if bytes.len() > CAP {
            return Err(Block::Capacity);
        }
        if Digest::of(Domain::RawArtifact, bytes) != expected {
            return Err(Block::Scope);
        }
        let _codec = budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        let storage = Storage::reserve(scope, budget)?;
        crate::sampler::wire::validate_complexity_limit(bytes, 512).map_err(|_| Block::Capacity)?;
        let v = decode(bytes, CAP).map_err(|_| Block::Scope)?;
        let o = exact(
            &v,
            &[
                "version",
                "scope",
                "request_digest",
                "request",
                "source",
                "entry",
                "deadline_ns",
                "raw_uuid",
                "raw_pci",
                "observations",
                "halt",
            ],
        )
        .map_err(|_| Block::Scope)?;
        if text(get(o, "version"))? != "1"
            || text(get(o, "scope"))? != "pilot_starting_settings_evidence"
            || text(get(o, "request_digest"))? != scope.digest().to_hex()
            || text(get(o, "request"))? != scope.canonical()
        {
            return Err(Block::Scope);
        }
        let source = match text(get(o, "source"))? {
            "native_getter_path" => Source::NativeGetterPath,
            "simulated_fixture" => Source::SimulatedFixture,
            _ => return Err(Block::Scope),
        };
        let observations = get(o, "observations").as_array().ok_or(Block::Scope)?;
        let [a, b, c] = observations else {
            return Err(Block::Scope);
        };
        let draft = Draft {
            raw_uuid: text(get(o, "raw_uuid"))?.into(),
            raw_pci: text(get(o, "raw_pci"))?.into(),
            observations: [
                settings::parse_observation(a)?,
                settings::parse_observation(b)?,
                settings::parse_observation(c)?,
            ],
            halt: parse_halt(get(o, "halt"))?,
        };
        let entry = settings::parse_anchor(get(o, "entry"))?.ok_or(Block::Clock)?;
        let result = Snapshot::finalize(entry, &draft, source, storage)?;
        if result.canonical().as_bytes() != bytes {
            return Err(Block::Scope);
        }
        Ok(Self(result))
    }
    /// Checks the declaration's references/identity/artifacts only. A reported
    /// native source remains untrusted and cannot construct native admission.
    pub fn check_manifest(
        &self,
        draft: &PilotDraft,
        frequency: u64,
        epoch: Anchor,
    ) -> Result<Current, Block> {
        let s = &self.0.scope.draft;
        if self.0.source != Source::NativeGetterPath
            || draft.starting_evidence != self.0.digest
            || draft.session != s.session
            || draft.clock_token != s.clock_token
            || draft.runs.contains(&s.capture_nonce)
            || draft.raw_uuid != s.raw_uuid
            || draft.raw_pci != s.raw_pci
            || draft.source_commit != s.source_commit
            || draft.artifacts != s.artifacts
            || frequency != s.frequency
            || epoch != s.epoch
        {
            return Err(Block::Scope);
        }
        self.0.current()
    }
    pub fn freeze_manifest(
        &self,
        draft: PilotDraft,
        frequency: u64,
        epoch: Anchor,
        budget: &Budget,
    ) -> Result<PilotManifest, Block> {
        self.check_manifest(&draft, frequency, epoch)?;
        PilotManifest::freeze(draft, budget).map_err(|_| Block::Scope)
    }
    pub fn check_run(
        &self,
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
    ) -> Result<Current, Block> {
        self.check_manifest(run.manifest_draft(), frequency, epoch)?;
        let end = self
            .0
            .draft
            .observations
            .last()
            .and_then(|o| o.after)
            .ok_or(Block::Clock)?;
        let elapsed = Clock::temporal(frequency, epoch.clock_token, epoch)
            .map_err(|_| Block::Clock)?
            .observe(end)
            .map_err(|_| Block::Clock)?;
        if elapsed > run.origin_ns() {
            return Err(Block::Phase);
        }
        self.0.current()
    }
}
fn text(v: &Value) -> Result<&str, Block> {
    string(v).map_err(|_| Block::Scope)
}
fn parse_halt(v: &Value) -> Result<Option<Block>, Block> {
    if v == &Value::Null {
        return Ok(None);
    }
    [
        Block::Capacity,
        Block::Identity,
        Block::Clock,
        Block::Phase,
        Block::Stopped,
        Block::GpuLost,
        Block::Missing,
        Block::Unit,
        Block::Symbol,
        Block::Value,
        Block::Scope,
    ]
    .into_iter()
    .find(|b| Some(settings::block_name(*b)) == v.as_text())
    .map(Some)
    .ok_or(Block::Scope)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    fn a(ns: u64) -> Anchor {
        Anchor {
            clock_token: [3; 16],
            qpc_before: ns,
            qpc_after: ns,
            interrupt_100ns: ns / 100,
            unbiased_100ns: ns / 100,
        }
    }
    fn scope(b: &Budget) -> Scope {
        let d = Digest::parse(&"01".repeat(32)).unwrap();
        Scope::freeze(
            ScopeDraft {
                session: [1; 16],
                capture_nonce: [2; 16],
                clock_token: [3; 16],
                frequency: 1_000_000_000,
                epoch: a(0),
                raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
                raw_pci: "0000:01:02.0".into(),
                source_commit: "a".repeat(40),
                artifacts: Artifacts {
                    source_tree: d,
                    lockfile: d,
                    controller: d,
                    worker: d,
                    sampler: d,
                    ptx: d,
                    protocol: d,
                },
                read_approval_ref: "CPU-only declaration fixture".into(),
            },
            b,
        )
        .unwrap()
    }
    #[test]
    fn injected_serial_collector_reads_each_field_once_and_ends_the_lane() {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let s = scope(&b);
        let gate = NativeAdmission::default();
        let mut tick = 1000;
        let mut calls = Vec::new();
        let snapshot = Snapshot::collect(
            &s,
            a(100),
            (&s.draft.raw_uuid, &s.draft.raw_pci),
            &gate,
            || {
                tick += 100;
                Some(a(tick))
            },
            |kind| {
                calls.push(kind);
                Field::available(
                    kind.unit(),
                    if kind == SettingMetric::PowerLimit {
                        215000
                    } else {
                        0
                    },
                )
            },
            &b,
        )
        .unwrap();
        assert_eq!(calls, SettingMetric::ALL);
        assert!(snapshot.current().is_ok());
        assert!(gate.stopped());
        assert!(!gate.pending());
        assert!(gate.begin().is_err());
    }
    #[test]
    fn post_getter_clock_loss_retains_the_value_and_stops_successors() {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let s = scope(&b);
        let gate = NativeAdmission::default();
        let mut clocks = [Some(a(200)), None].into_iter();
        let mut calls = 0;
        let snapshot = Snapshot::collect(
            &s,
            a(100),
            (&s.draft.raw_uuid, &s.draft.raw_pci),
            &gate,
            || clocks.next().flatten(),
            |kind| {
                calls += 1;
                Field::available(kind.unit(), 215000)
            },
            &b,
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(snapshot.current(), Err(Block::Clock));
        assert!(snapshot.draft.observations[0].raw.is_some());
        assert_eq!(snapshot.draft.observations[0].after, None);
        assert!(gate.stopped());
        assert!(!gate.pending());
    }
    #[test]
    fn stop_during_return_is_retained_and_never_cleared_by_completion() {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let s = scope(&b);
        let gate = NativeAdmission::default();
        let mut clocks = 0;
        let mut calls = 0;
        let snapshot = Snapshot::collect(
            &s,
            a(100),
            (&s.draft.raw_uuid, &s.draft.raw_pci),
            &gate,
            || {
                clocks += 1;
                if clocks == 2 {
                    assert!(gate.pending());
                    gate.stop();
                }
                Some(a(100 + clocks * 100))
            },
            |kind| {
                calls += 1;
                Field::available(kind.unit(), 215000)
            },
            &b,
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(snapshot.current(), Err(Block::Stopped));
        assert!(snapshot.draft.observations[0].raw.is_some());
        assert!(gate.stopped());
        assert!(!gate.pending());
    }
    #[test]
    fn all_finalization_storage_is_reserved_before_getters_and_survives_pressure() {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let s = scope(&b);
        let gate = NativeAdmission::default();
        let pressure = RefCell::new(None);
        let mut tick = 100;
        let snapshot = Snapshot::collect(
            &s,
            a(100),
            (&s.draft.raw_uuid, &s.draft.raw_pci),
            &gate,
            || {
                tick += 100;
                Some(a(tick))
            },
            |kind| {
                if pressure.borrow().is_none() {
                    *pressure.borrow_mut() = Some(b.reserve(8 * 1024 * 1024 - b.used()).unwrap());
                }
                Field::available(
                    kind.unit(),
                    if kind == SettingMetric::PowerLimit {
                        215000
                    } else {
                        0
                    },
                )
            },
            &b,
        )
        .unwrap();
        assert!(snapshot.current().is_ok());
        assert!(gate.stopped());
        assert!(snapshot.canonical.len() < CAP);
    }
    #[test]
    fn reservation_or_identity_failure_never_enters_a_getter() {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let s = scope(&b);
        let gate = NativeAdmission::default();
        let snapshot = Snapshot::collect(
            &s,
            a(100),
            ("other", "other"),
            &gate,
            || panic!("no capture on identity mismatch"),
            |_| panic!("no getter"),
            &b,
        )
        .unwrap();
        assert_eq!(snapshot.current(), Err(Block::Identity));
        assert!(gate.stopped());
        let gate = NativeAdmission::default();
        let _pressure = b.reserve(8 * 1024 * 1024 - b.used()).unwrap();
        assert!(matches!(
            Snapshot::collect(
                &s,
                a(100),
                (&s.draft.raw_uuid, &s.draft.raw_pci),
                &gate,
                || panic!("no capture without storage"),
                |_| panic!("no getter"),
                &b
            ),
            Err(Block::Capacity)
        ));
        assert!(gate.stopped());
        assert!(!gate.pending());
    }
}
