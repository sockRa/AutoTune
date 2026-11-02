//! Bound raw current-setting facts, not consent/native admission or baseline truth.
mod receive;
use super::{manifest::BoundRun, wire::WorkerBinding};
use crate::{
    evidence::{Digest, Domain, Value},
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{hex, object},
    },
    worker::DiscoveryBinding,
};
use autotune_probe::{
    nvml::{NvmlSampler, SettingMetric},
    report::{Field, Reading, Reason, SymbolPresence},
};
pub(super) use receive::field as parse_field;
pub(super) use receive::parse_anchor;
pub use receive::ReportedCapture;
const CAP: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    NativeGetterPath,
    SimulatedFixture,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Capacity,
    Identity,
    Clock,
    Phase,
    Stopped,
    GpuLost,
    Missing,
    Unit,
    Symbol,
    Value,
    Scope,
}
#[derive(Clone, Debug)]
pub struct Observation {
    pub kind: SettingMetric,
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    pub raw: Option<Field>,
}
#[derive(Clone, Debug)]
pub struct Draft {
    pub raw_uuid: String,
    pub raw_pci: String,
    pub observations: [Observation; 3],
    pub halt: Option<Block>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Current {
    pub power_limit_mw: u32,
    pub graphics_offset_p0_mhz: i32,
    pub memory_offset_p0_mhz: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    ObservedEqual,
    Different {
        power_limit: bool,
        graphics_offset_p0: bool,
        memory_offset_p0: bool,
    },
}
pub struct Capture {
    pub(super) binding: WorkerBinding,
    manifest: Digest,
    expected_uuid: String,
    expected_pci: String,
    source: Source,
    draft: Draft,
    canonical: String,
    digest: Digest,
    _allocation: Reservation,
}
struct CaptureStorage {
    owned: Reservation,
    _codec: Reservation,
}
impl Capture {
    fn reserve(budget: &Budget) -> Result<CaptureStorage, Block> {
        let owned = budget
            .reserve(CAP * 2 + std::mem::size_of::<Self>())
            .map_err(|_| Block::Capacity)?;
        let codec = budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        Ok(CaptureStorage {
            owned,
            _codec: codec,
        })
    }
    /// Explicitly simulated input; oversize/complex input remains caller-owned
    /// on rejection. Native provenance cannot be supplied through this API.
    pub fn fixture(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        draft: &Draft,
        budget: &Budget,
    ) -> Result<Self, Block> {
        let binding = WorkerBinding::new(run, frequency, epoch).map_err(|_| Block::Clock)?;
        Self::freeze(run, binding, Source::SimulatedFixture, draft, budget)
    }
    /// Reads only current power limit and modern graphics/memory P0 offsets.
    /// Caller must have separate native read/containment/clock scope; no Start
    /// or consent authority follows. Parent control must not wait on this call.
    pub fn collect_native(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        sampler: &mut NvmlSampler,
        clock: impl FnMut() -> Option<Anchor>,
        stop: impl FnMut() -> bool,
        budget: &Budget,
    ) -> Result<Self, Block> {
        let binding = WorkerBinding::new(run, frequency, epoch).map_err(|_| Block::Clock)?;
        Self::collect_native_scoped(
            run,
            binding,
            sampler,
            (binding.epoch, binding.setup_boundary()),
            (clock, stop),
            budget,
        )
    }
    pub(super) fn collect_native_scoped(
        run: &BoundRun<'_>,
        binding: WorkerBinding,
        sampler: &mut NvmlSampler,
        window: (Anchor, u64),
        callbacks: (impl FnMut() -> Option<Anchor>, impl FnMut() -> bool),
        budget: &Budget,
    ) -> Result<Self, Block> {
        let identity = sampler.identity().ok_or(Block::Identity)?;
        Self::collect_window(
            run,
            binding,
            identity,
            window,
            (callbacks.0, callbacks.1, |kind| sampler.setting(kind)),
            budget,
        )
    }
    #[cfg(test)]
    fn collect(
        run: &BoundRun<'_>,
        binding: WorkerBinding,
        identity: (&str, &str),
        clock: impl FnMut() -> Option<Anchor>,
        stop: impl FnMut() -> bool,
        read: impl FnMut(SettingMetric) -> Field,
        budget: &Budget,
    ) -> Result<Self, Block> {
        Self::collect_window(
            run,
            binding,
            identity,
            (binding.epoch, binding.setup_boundary()),
            (clock, stop, read),
            budget,
        )
    }
    fn collect_window(
        run: &BoundRun<'_>,
        binding: WorkerBinding,
        identity: (&str, &str),
        window: (Anchor, u64),
        callbacks: (
            impl FnMut() -> Option<Anchor>,
            impl FnMut() -> bool,
            impl FnMut(SettingMetric) -> Field,
        ),
        budget: &Budget,
    ) -> Result<Self, Block> {
        let lower_ns = binding
            .elapsed(window.0)
            .map_err(|_| Block::Clock)?
            .max(binding.origin_ns);
        if lower_ns >= window.1 || window.1 > binding.setup_boundary() {
            return Err(Block::Phase);
        }
        let (mut clock, mut stop, mut read) = callbacks;
        // The closed native source returns scalar fields and bounded static
        // error details. Reserve raw staging before getters/model allocation.
        let _staging = budget.reserve(CAP).map_err(|_| Block::Capacity)?;
        let storage = Self::reserve(budget)?;
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
        if identity != (run.raw_uuid(), run.raw_pci()) {
            draft.halt = Some(Block::Identity);
            return Self::finalize(run, binding, Source::NativeGetterPath, &draft, storage);
        }
        let mut validator = Clock::new(binding.monitor, binding.epoch).map_err(|_| Block::Clock)?;
        let mut last = binding.epoch;
        checked_anchor(&mut validator, &mut last, Some(window.0))?;
        for observation in &mut draft.observations {
            observation.before = clock();
            let before = checked_anchor(&mut validator, &mut last, observation.before);
            let now = match before {
                Ok(n) => n,
                Err(f) => {
                    draft.halt = Some(f);
                    break;
                }
            };
            if now < lower_ns || now >= window.1 {
                draft.halt = Some(Block::Phase);
                break;
            }
            if stop() {
                draft.halt = Some(Block::Stopped);
                break;
            }
            observation.raw = Some(read(observation.kind));
            let lost = matches!(&observation.raw,Some(Field {reading:Reading::Unavailable {failure},..}) if failure.reason==Reason::GpuLost);
            if lost {
                draft.halt = Some(Block::GpuLost);
            }
            observation.after = clock();
            match checked_anchor(&mut validator, &mut last, observation.after) {
                Ok(n) if n < window.1 => (),
                Ok(_) => {
                    draft.halt.get_or_insert(Block::Phase);
                }
                Err(f) => {
                    draft.halt.get_or_insert(f);
                }
            }
            if draft.halt.is_some() {
                break;
            }
            if stop() {
                draft.halt = Some(Block::Stopped);
                break;
            }
        }
        Self::finalize(run, binding, Source::NativeGetterPath, &draft, storage)
    }
    fn freeze(
        run: &BoundRun<'_>,
        binding: WorkerBinding,
        source: Source,
        draft: &Draft,
        budget: &Budget,
    ) -> Result<Self, Block> {
        Self::finalize(run, binding, source, draft, Self::reserve(budget)?)
    }
    fn finalize(
        run: &BoundRun<'_>,
        binding: WorkerBinding,
        source: Source,
        draft: &Draft,
        storage: CaptureStorage,
    ) -> Result<Self, Block> {
        if draft.raw_uuid.capacity() > 128
            || draft.raw_pci.capacity() > 64
            || draft
                .observations
                .iter()
                .any(|o| o.raw.as_ref().is_some_and(|f| !bounded(f)))
        {
            return Err(Block::Capacity);
        }
        let CaptureStorage {
            owned: allocation,
            _codec,
        } = storage;
        let draft = draft.clone();
        let value = object([
            ("version", Value::Text("1".into())),
            ("scope", Value::Text("pilot_current_settings".into())),
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
            ("binding", binding.value()),
            ("expected_uuid", Value::Text(run.raw_uuid().into())),
            ("expected_pci", Value::Text(run.raw_pci().into())),
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
                                ("kind", Value::Text(kind_name(o.kind).into())),
                                ("before", o.before.map_or(Value::Null, raw_anchor)),
                                ("after", o.after.map_or(Value::Null, raw_anchor)),
                                ("raw", o.raw.as_ref().map_or(Value::Null, raw_value)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "halt",
                draft
                    .halt
                    .map_or(Value::Null, |f| Value::Text(block_name(f).into())),
            ),
        ]);
        let canonical = value.to_canonical();
        if canonical.len() > CAP {
            return Err(Block::Capacity);
        }
        let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
        Ok(Self {
            binding,
            manifest: run.manifest_digest(),
            expected_uuid: run.raw_uuid().into(),
            expected_pci: run.raw_pci().into(),
            source,
            draft,
            canonical,
            digest,
            _allocation: allocation,
        })
    }
    pub fn draft(&self) -> &Draft {
        &self.draft
    }
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    /// Readable values at the retained observations. This does not recapture
    /// them, assess age at Start, or grant safety/ownership/consent admission.
    pub fn current(&self) -> Result<Current, Block> {
        if let Some(halt) = self.draft.halt {
            return Err(halt);
        }
        if self.draft.raw_uuid != self.expected_uuid
            || self.draft.raw_pci != self.expected_pci
            || DiscoveryBinding::parse(&self.draft.raw_uuid, &self.draft.raw_pci).is_err()
        {
            return Err(Block::Identity);
        }
        let mut clock =
            Clock::new(self.binding.monitor, self.binding.epoch).map_err(|_| Block::Clock)?;
        let mut last = self.binding.epoch;
        let mut values = [0i64; 3];
        for (i, o) in self.draft.observations.iter().enumerate() {
            if o.kind != SettingMetric::ALL[i] {
                return Err(Block::Scope);
            }
            let before = checked_anchor(&mut clock, &mut last, o.before)?;
            let after = checked_anchor(&mut clock, &mut last, o.after)?;
            if before < self.binding.origin_ns || after >= self.binding.setup_boundary() {
                return Err(Block::Phase);
            }
            let field = o.raw.as_ref().ok_or(Block::Missing)?;
            if field.unit != o.kind.unit() {
                return Err(Block::Unit);
            }
            if field.symbol_presence != SymbolPresence::Present {
                return Err(Block::Symbol);
            }
            let Reading::Available { value } = &field.reading else {
                return Err(
                    if matches!(&field.reading,Reading::Unavailable {failure} if failure.reason==Reason::GpuLost)
                    {
                        Block::GpuLost
                    } else {
                        Block::Missing
                    },
                );
            };
            let value = value.as_i64().ok_or(Block::Value)?;
            if (i == 0 && !(1..=10_000_000).contains(&value))
                || (i > 0 && !(-100_000..=100_000).contains(&value))
            {
                return Err(Block::Value);
            }
            values[i] = value;
        }
        Ok(Current {
            power_limit_mw: values[0] as u32,
            graphics_offset_p0_mhz: values[1] as i32,
            memory_offset_p0_mhz: values[2] as i32,
        })
    }
    /// Equality of these three observed values only. Not unchanged-complete
    /// settings, write qualification, ownership or baseline comparability.
    pub fn compare(&self, other: &Self) -> Result<Comparison, Block> {
        let a = self.binding.monitor;
        let b = other.binding.monitor;
        if self.manifest != other.manifest
            || self.source != other.source
            || a.session != b.session
            || a.device != b.device
            || a.clock_token != b.clock_token
            || a.qpc_frequency != b.qpc_frequency
            || self.binding.epoch != other.binding.epoch
        {
            return Err(Block::Scope);
        }
        let a = self.current()?;
        let b = other.current()?;
        if a == b {
            Ok(Comparison::ObservedEqual)
        } else {
            Ok(Comparison::Different {
                power_limit: a.power_limit_mw != b.power_limit_mw,
                graphics_offset_p0: a.graphics_offset_p0_mhz != b.graphics_offset_p0_mhz,
                memory_offset_p0: a.memory_offset_p0_mhz != b.memory_offset_p0_mhz,
            })
        }
    }
}
pub(super) fn raw_anchor(a: Anchor) -> Value {
    object([
        ("clock_token", Value::Text(hex(&a.clock_token))),
        ("qpc_before", Value::U64(a.qpc_before)),
        ("qpc_after", Value::U64(a.qpc_after)),
        ("interrupt_100ns", Value::U64(a.interrupt_100ns)),
        ("unbiased_100ns", Value::U64(a.unbiased_100ns)),
    ])
}
fn checked_anchor(clock: &mut Clock, last: &mut Anchor, raw: Option<Anchor>) -> Result<u64, Block> {
    let raw = raw.ok_or(Block::Clock)?;
    if raw.qpc_before < last.qpc_after {
        return Err(Block::Clock);
    }
    let ns = clock.observe(raw).map_err(|_| Block::Clock)?;
    *last = raw;
    Ok(ns)
}
pub(super) fn bounded(f: &Field) -> bool {
    f.unit.capacity() <= 32
        && match &f.reading {
            Reading::Available { value } => match value {
                serde_json::Value::String(s) => s.capacity() <= 1024,
                serde_json::Value::Array(_) | serde_json::Value::Object(_) => false,
                _ => true,
            },
            Reading::Unavailable { failure } => failure.detail.capacity() <= 1024,
        }
}
pub(super) fn raw_value(f: &Field) -> Value {
    let symbol = match f.symbol_presence {
        SymbolPresence::Present => "present",
        SymbolPresence::Absent => "absent",
        SymbolPresence::NotChecked => "not_checked",
    };
    let reading = match &f.reading {
        Reading::Available { value } => {
            let (kind, value) = match value {
                serde_json::Value::Null => ("null", Value::Null),
                serde_json::Value::Bool(v) => ("bool", Value::Bool(*v)),
                serde_json::Value::String(s) => ("text", Value::Text(s.clone())),
                serde_json::Value::Number(n) => (
                    if n.as_i64().is_some() || n.as_u64().is_some() {
                        "integer"
                    } else {
                        "float_repr"
                    },
                    Value::Text(n.to_string()),
                ),
                _ => unreachable!("bounded scalar"),
            };
            object([
                ("status", Value::Text("available".into())),
                ("kind", Value::Text(kind.into())),
                ("value", value),
            ])
        }
        Reading::Unavailable { failure } => object([
            ("status", Value::Text("unavailable".into())),
            (
                "reason",
                Value::Text(
                    match failure.reason {
                        Reason::RuntimeMissing => "runtime_missing",
                        Reason::RuntimeLoadFailed => "runtime_load_failed",
                        Reason::SymbolMissing => "symbol_missing",
                        Reason::Unsupported => "unsupported",
                        Reason::PermissionDenied => "permission_denied",
                        Reason::QueryFailed => "query_failed",
                        Reason::GpuLost => "gpu_lost",
                        Reason::InvalidValue => "invalid_value",
                        Reason::UnsupportedPlatform => "unsupported_platform",
                    }
                    .into(),
                ),
            ),
            (
                "vendor_status",
                failure
                    .vendor_status
                    .map_or(Value::Null, |n| Value::I64(n.into())),
            ),
            ("detail", Value::Text(failure.detail.clone())),
        ]),
    };
    object([
        ("unit", Value::Text(f.unit.clone())),
        ("symbol", Value::Text(symbol.into())),
        ("reading", reading),
    ])
}
fn kind_name(k: SettingMetric) -> &'static str {
    match k {
        SettingMetric::PowerLimit => "power_limit",
        SettingMetric::GraphicsOffsetP0 => "graphics_offset_p0",
        SettingMetric::MemoryOffsetP0 => "memory_offset_p0",
    }
}
fn block_name(b: Block) -> &'static str {
    match b {
        Block::Capacity => "capacity",
        Block::Identity => "identity",
        Block::Clock => "clock",
        Block::Phase => "phase",
        Block::Stopped => "stopped",
        Block::GpuLost => "gpu_lost",
        Block::Missing => "missing",
        Block::Unit => "unit",
        Block::Symbol => "symbol",
        Block::Value => "value",
        Block::Scope => "scope",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::manifest::{Artifacts, PilotDraft, PilotManifest};
    fn a(ns: u64) -> Anchor {
        Anchor {
            clock_token: [5; 16],
            qpc_before: ns,
            qpc_after: ns,
            interrupt_100ns: ns / 100,
            unbiased_100ns: ns / 100,
        }
    }
    fn manifest(b: &Budget) -> PilotManifest {
        let d = Digest::parse(&"11".repeat(32)).unwrap();
        PilotManifest::freeze(
            PilotDraft {
                session: [1; 16],
                runs: [[2; 16], [3; 16], [4; 16]],
                clock_token: [5; 16],
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
                starting_evidence: d,
                environment_evidence: d,
                approval_ref: "CPU fixture, no authority".into(),
                output_root: "D:\\fixture\\settings".into(),
            },
            b,
        )
        .unwrap()
    }
    #[test]
    fn collector_engine_calls_each_requested_getter_once_with_raw_anchors() {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let run = m.bind_run(0, 0).unwrap();
        let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
        let mut ns = 0;
        let mut kinds = Vec::new();
        let c = Capture::collect(
            &run,
            binding,
            (run.raw_uuid(), run.raw_pci()),
            || {
                ns += 1000;
                Some(a(ns))
            },
            || false,
            |k| {
                kinds.push(k);
                Field::available(
                    k.unit(),
                    if k == SettingMetric::PowerLimit {
                        230000
                    } else {
                        -7
                    },
                )
            },
            &b,
        )
        .unwrap();
        assert_eq!(kinds, SettingMetric::ALL);
        assert_eq!(c.current().unwrap().power_limit_mw, 230000);
        assert_eq!(c.draft().observations[2].after, Some(a(6000)));
    }
    #[test]
    fn collector_engine_never_calls_successors_after_clock_stop_identity_or_gpu_loss() {
        for mode in 0..5 {
            let b = Budget::new(8 * 1024 * 1024).unwrap();
            let m = manifest(&b);
            let run = m.bind_run(0, 0).unwrap();
            let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
            let mut ticks = 0;
            let mut calls = 0;
            let identity = if mode == 3 {
                ("wrong", run.raw_pci())
            } else {
                (run.raw_uuid(), run.raw_pci())
            };
            let c = Capture::collect(
                &run,
                binding,
                identity,
                || {
                    ticks += 1;
                    if (mode == 0 || mode == 2) && ticks == 2 {
                        None
                    } else {
                        Some(a(if mode == 4 {
                            20_000_000_000
                        } else {
                            ticks * 1000
                        }))
                    }
                },
                || mode == 1,
                |k| {
                    calls += 1;
                    if mode == 2 {
                        Field::unavailable(
                            k.unit(),
                            autotune_probe::report::Failure::vendor(15, "exact setting getter"),
                        )
                    } else {
                        Field::available(k.unit(), 1)
                    }
                },
                &b,
            )
            .unwrap();
            assert!(c.current().is_err());
            assert_eq!(calls, usize::from(mode == 0 || mode == 2));
            if mode == 0 {
                assert!(c.draft().observations[0].raw.is_some());
                assert_eq!(c.draft().observations[0].after, None);
            }
            if mode == 2 {
                assert_eq!(c.current(), Err(Block::GpuLost));
                assert!(c.canonical().contains("\"vendor_status\":\"15\""));
            }
        }
    }
    #[test]
    fn all_finalization_memory_is_reserved_before_getters_and_survives_budget_pressure() {
        let model = Budget::new(8 * 1024 * 1024).unwrap();
        let m = manifest(&model);
        let run = m.bind_run(0, 0).unwrap();
        let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
        let small = Budget::new(CAP * 2).unwrap();
        let mut calls = 0;
        assert!(matches!(
            Capture::collect(
                &run,
                binding,
                (run.raw_uuid(), run.raw_pci()),
                || Some(a(1000)),
                || false,
                |k| {
                    calls += 1;
                    Field::available(k.unit(), 1)
                },
                &small
            ),
            Err(Block::Capacity)
        ));
        assert_eq!(calls, 0);
        assert_eq!(small.used(), 0);
        let b = Budget::new(2 * 1024 * 1024).unwrap();
        let mut pressure = None;
        let mut ns = 0;
        let c = Capture::collect(
            &run,
            binding,
            (run.raw_uuid(), run.raw_pci()),
            || {
                ns += 1000;
                Some(a(ns))
            },
            || false,
            |k| {
                if pressure.is_none() {
                    pressure = Some(b.reserve(2 * 1024 * 1024 - b.used()).unwrap());
                }
                Field::available(
                    k.unit(),
                    if k == SettingMetric::PowerLimit {
                        230000
                    } else {
                        0
                    },
                )
            },
            &b,
        )
        .unwrap();
        assert_eq!(c.current().unwrap().power_limit_mw, 230000);
        assert!(c.draft().observations.iter().all(|o| o.raw.is_some()));
        drop(c);
        drop(pressure);
        assert_eq!(b.used(), 0);
    }
    #[test]
    fn scoped_request_window_prevents_early_or_expired_getters_without_losing_returns() {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let m = manifest(&b);
        let run = m.bind_run(0, 0).unwrap();
        let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
        for (before, after, expected_calls) in [(499, 600, 0), (1000, 1100, 0), (500, 1000, 1)] {
            let mut clocks = [a(before), a(after)].into_iter();
            let mut calls = 0;
            let c = Capture::collect_window(
                &run,
                binding,
                (run.raw_uuid(), run.raw_pci()),
                (a(500), 1000),
                (
                    || clocks.next(),
                    || false,
                    |k| {
                        calls += 1;
                        Field::available(k.unit(), 1)
                    },
                ),
                &b,
            )
            .unwrap();
            assert_eq!(calls, expected_calls);
            assert_eq!(
                c.current(),
                Err(if before < 500 {
                    Block::Clock
                } else {
                    Block::Phase
                })
            );
            assert_eq!(c.draft().observations[0].raw.is_some(), calls == 1);
        }
    }
}
