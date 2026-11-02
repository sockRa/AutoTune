//! Strict parent reader. A reported native path is still an untrusted claim.
use super::*;
use crate::{
    evidence::decode,
    sampler::wire_model::{exact, get, string, uint, unhex},
};
use autotune_probe::report::Failure;

pub struct ReportedCapture(Capture);
impl ReportedCapture {
    /// Bind canonical reported bytes to the expected run and artifact digest.
    /// This does not authenticate a peer, prove native execution, or grant Start.
    pub fn parse(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        bytes: &[u8],
        expected_digest: Digest,
        budget: &Budget,
    ) -> Result<Self, Block> {
        if bytes.len() > CAP {
            return Err(Block::Capacity);
        }
        crate::sampler::wire::validate_complexity_limit(bytes, 256).map_err(|_| Block::Capacity)?;
        if Digest::of(Domain::RawArtifact, bytes) != expected_digest {
            return Err(Block::Scope);
        }
        let binding = WorkerBinding::new(run, frequency, epoch).map_err(|_| Block::Clock)?;
        let _decode = budget.reserve(CAP * 16).map_err(|_| Block::Capacity)?;
        let storage = Capture::reserve(budget)?;
        let value = decode(bytes, CAP).map_err(|_| Block::Scope)?;
        let o = exact(
            &value,
            &[
                "version",
                "scope",
                "source",
                "binding",
                "expected_uuid",
                "expected_pci",
                "raw_uuid",
                "raw_pci",
                "observations",
                "halt",
            ],
        )
        .map_err(|_| Block::Scope)?;
        if text(get(o, "version"))? != "1"
            || text(get(o, "scope"))? != "pilot_current_settings"
            || get(o, "binding").to_canonical() != binding.value().to_canonical()
            || text(get(o, "expected_uuid"))? != run.raw_uuid()
            || text(get(o, "expected_pci"))? != run.raw_pci()
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
            raw_uuid: bounded_text(get(o, "raw_uuid"), 128)?,
            raw_pci: bounded_text(get(o, "raw_pci"), 64)?,
            observations: [observation(a)?, observation(b)?, observation(c)?],
            halt: match get(o, "halt") {
                Value::Null => None,
                v => Some(
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
                    .find(|b| Some(block_name(*b)) == v.as_text())
                    .ok_or(Block::Scope)?,
                ),
            },
        };
        // Keep contradictory raw observations; only schema/canonical/binding
        // validity is required here. current() separately assesses readability.
        drop(value);
        let capture = Capture::finalize(run, binding, source, &draft, storage)?;
        if capture.canonical().as_bytes() != bytes {
            return Err(Block::Scope);
        }
        Ok(Self(capture))
    }
    pub fn draft(&self) -> &Draft {
        self.0.draft()
    }
    pub fn reported_source(&self) -> Source {
        self.0.source()
    }
    pub fn canonical(&self) -> &str {
        self.0.canonical()
    }
    pub fn digest(&self) -> Digest {
        self.0.digest()
    }
    pub fn current(&self) -> Result<Current, Block> {
        self.0.current()
    }
}
fn text(v: &Value) -> Result<&str, Block> {
    string(v).map_err(|_| Block::Scope)
}
fn bounded_text(v: &Value, limit: usize) -> Result<String, Block> {
    let s = text(v)?;
    if s.len() > limit {
        return Err(Block::Capacity);
    }
    Ok(s.into())
}
pub(in crate::workload) fn observation(v: &Value) -> Result<Observation, Block> {
    let o = exact(v, &["kind", "before", "after", "raw"]).map_err(|_| Block::Scope)?;
    let kind = SettingMetric::ALL
        .into_iter()
        .find(|k| Some(kind_name(*k)) == get(o, "kind").as_text())
        .ok_or(Block::Scope)?;
    Ok(Observation {
        kind,
        before: parse_anchor(get(o, "before"))?,
        after: parse_anchor(get(o, "after"))?,
        raw: match get(o, "raw") {
            Value::Null => None,
            v => Some(field(v)?),
        },
    })
}
pub(in crate::workload) fn parse_anchor(v: &Value) -> Result<Option<Anchor>, Block> {
    if v == &Value::Null {
        return Ok(None);
    }
    let o = exact(
        v,
        &[
            "clock_token",
            "qpc_before",
            "qpc_after",
            "interrupt_100ns",
            "unbiased_100ns",
        ],
    )
    .map_err(|_| Block::Scope)?;
    Ok(Some(Anchor {
        clock_token: unhex(get(o, "clock_token")).map_err(|_| Block::Scope)?,
        qpc_before: uint(get(o, "qpc_before")).map_err(|_| Block::Scope)?,
        qpc_after: uint(get(o, "qpc_after")).map_err(|_| Block::Scope)?,
        interrupt_100ns: uint(get(o, "interrupt_100ns")).map_err(|_| Block::Scope)?,
        unbiased_100ns: uint(get(o, "unbiased_100ns")).map_err(|_| Block::Scope)?,
    }))
}
pub(in crate::workload) fn field(v: &Value) -> Result<Field, Block> {
    let o = exact(v, &["unit", "symbol", "reading"]).map_err(|_| Block::Scope)?;
    let symbol_presence = match text(get(o, "symbol"))? {
        "present" => SymbolPresence::Present,
        "absent" => SymbolPresence::Absent,
        "not_checked" => SymbolPresence::NotChecked,
        _ => return Err(Block::Scope),
    };
    let v = get(o, "reading");
    let status = v
        .as_object()
        .and_then(|o| o.get("status"))
        .ok_or(Block::Scope)?;
    let reading = match text(status)? {
        "available" => {
            let o = exact(v, &["status", "kind", "value"]).map_err(|_| Block::Scope)?;
            let v = get(o, "value");
            let value = match (text(get(o, "kind"))?, v) {
                ("null", Value::Null) => serde_json::Value::Null,
                ("bool", Value::Bool(b)) => serde_json::Value::Bool(*b),
                ("text", _) => serde_json::Value::String(bounded_text(v, 1024)?),
                ("integer" | "float_repr", _) => {
                    let n = text(v)?;
                    if n.len() > 32 {
                        return Err(Block::Capacity);
                    }
                    serde_json::Value::Number(n.parse().map_err(|_| Block::Scope)?)
                }
                _ => return Err(Block::Scope),
            };
            Reading::Available { value }
        }
        "unavailable" => {
            let o = exact(v, &["status", "reason", "vendor_status", "detail"])
                .map_err(|_| Block::Scope)?;
            let reason = match text(get(o, "reason"))? {
                "runtime_missing" => Reason::RuntimeMissing,
                "runtime_load_failed" => Reason::RuntimeLoadFailed,
                "symbol_missing" => Reason::SymbolMissing,
                "unsupported" => Reason::Unsupported,
                "permission_denied" => Reason::PermissionDenied,
                "query_failed" => Reason::QueryFailed,
                "gpu_lost" => Reason::GpuLost,
                "invalid_value" => Reason::InvalidValue,
                "unsupported_platform" => Reason::UnsupportedPlatform,
                _ => return Err(Block::Scope),
            };
            Reading::Unavailable {
                failure: Failure {
                    reason,
                    vendor_status: match get(o, "vendor_status") {
                        Value::Null => None,
                        n => Some(
                            n.as_i64()
                                .map_err(|_| Block::Scope)?
                                .try_into()
                                .map_err(|_| Block::Scope)?,
                        ),
                    },
                    detail: bounded_text(get(o, "detail"), 1024)?,
                },
            }
        }
        _ => return Err(Block::Scope),
    };
    Ok(Field {
        unit: bounded_text(get(o, "unit"), 32)?,
        symbol_presence,
        reading,
    })
}
