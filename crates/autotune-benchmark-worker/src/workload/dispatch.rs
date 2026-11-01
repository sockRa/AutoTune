//! Worker-reported dispatch observations and bounded first-failure transport.
//! These bytes report assertions, not native execution or admission authority.
use super::wire::{parse_anchor, WorkerBinding};
use crate::{
    evidence::{Digest, Value},
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        wire::WireError,
        wire_model::{anchor_value, exact, get, object, string, uint},
    },
};
use sha2::{Digest as _, Sha256};

/// A wire allocation bound, not a new workload/dispatch operating limit.
pub const CHUNK_LANES: usize = 2048;
const MAX_CALLS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Input,
    Submit,
    UploadA,
    UploadB,
    UploadX,
    Sentinel,
    UploadCompletion,
    Launch,
    Query,
    Readback,
    Check,
    Hash,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Submit => "submit",
            Self::UploadA => "upload_a",
            Self::UploadB => "upload_b",
            Self::UploadX => "upload_x",
            Self::Sentinel => "sentinel",
            Self::UploadCompletion => "upload_completion",
            Self::Launch => "launch",
            Self::Query => "query",
            Self::Readback => "readback",
            Self::Check => "check",
            Self::Hash => "hash",
        }
    }
    fn parse(s: &str) -> Result<Self, WireError> {
        match s {
            "input" => Ok(Self::Input),
            "submit" => Ok(Self::Submit),
            "upload_a" => Ok(Self::UploadA),
            "upload_b" => Ok(Self::UploadB),
            "upload_x" => Ok(Self::UploadX),
            "sentinel" => Ok(Self::Sentinel),
            "upload_completion" => Ok(Self::UploadCompletion),
            "launch" => Ok(Self::Launch),
            "query" => Ok(Self::Query),
            "readback" => Ok(Self::Readback),
            "check" => Ok(Self::Check),
            "hash" => Ok(Self::Hash),
            _ => Err(WireError::Body),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub operation: Operation,
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    /// Raw native status where applicable; unknown/unreturned is null.
    pub status: Option<i32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Warmup,
    Measured,
}
impl Phase {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Measured => "measured",
        }
    }
    pub(super) fn parse(v: &Value) -> Result<Self, WireError> {
        match string(v)? {
            "warmup" => Ok(Self::Warmup),
            "measured" => Ok(Self::Measured),
            _ => Err(WireError::Body),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstFailure {
    pub lane: u32,
    pub expected: i32,
    pub actual_bits: u32,
    pub nonfinite: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub dispatch: u32,
    pub phase: Phase,
    pub seed: u64,
    pub lanes: u32,
    pub steps: u32,
    pub calls: Vec<Call>,
    pub pending: bool,
    pub readback_lanes: Option<u32>,
    pub checked_lanes: u32,
    /// All unequal lanes, including the nonfinite subset.
    pub mismatches: u32,
    pub nonfinite: u32,
    pub first_failure: Option<FirstFailure>,
    pub verified_fma: u64,
    /// Plain SHA-256 of the exact input/output bytes, not a consent digest.
    pub input_sha256: Option<Digest>,
    pub output_sha256: Option<Digest>,
}
impl Record {
    pub(super) fn allocation_bytes(&self) -> usize {
        self.calls.capacity() * std::mem::size_of::<Call>()
    }
    pub(super) fn validate(&self, b: WorkerBinding) -> Result<(), WireError> {
        let p = b.plan;
        let phase = if self.dispatch < p.warmup_dispatches {
            Phase::Warmup
        } else {
            Phase::Measured
        };
        if self.dispatch >= p.warmup_dispatches + p.measured_dispatches
            || self.phase != phase
            || self.seed != p.seed
            || self.lanes != p.shape.lanes()
            || self.steps != p.shape.steps().value()
            || self.calls.capacity() > MAX_CALLS
            || self.checked_lanes > self.lanes
            || self.mismatches > self.checked_lanes
            || self.nonfinite > self.mismatches
            || (self.mismatches == 0) != self.first_failure.is_none()
            || self
                .first_failure
                .is_some_and(|f| f.lane >= self.checked_lanes)
        {
            return Err(WireError::Body);
        }
        // Clock validity/order and call completeness are assessed by the parent;
        // preserve raw contradictory, missing and late anchors for that audit.
        if self
            .calls
            .iter()
            .flat_map(|c| [c.before, c.after])
            .flatten()
            .any(|a| a.clock_token != b.monitor.clock_token)
        {
            return Err(WireError::Binding);
        }
        let fully_checked = self.checked_lanes == self.lanes
            && self.readback_lanes == Some(self.lanes)
            && self.mismatches == 0
            && !self.pending
            && self.input_sha256.is_some()
            && self.output_sha256.is_some();
        let work = if fully_checked {
            u64::from(self.lanes) * u64::from(self.steps)
        } else {
            0
        };
        if self.verified_fma != work {
            return Err(WireError::Body);
        }
        Ok(())
    }
    pub(super) fn value(&self) -> Value {
        object([
            ("dispatch", Value::U64(self.dispatch.into())),
            ("phase", Value::Text(self.phase.name().into())),
            ("seed", Value::U64(self.seed)),
            ("lanes", Value::U64(self.lanes.into())),
            ("steps", Value::U64(self.steps.into())),
            (
                "calls",
                Value::Array(
                    self.calls
                        .iter()
                        .map(|c| {
                            object([
                                ("operation", Value::Text(c.operation.name().into())),
                                ("before", c.before.map_or(Value::Null, anchor_value)),
                                ("after", c.after.map_or(Value::Null, anchor_value)),
                                (
                                    "status",
                                    c.status.map_or(Value::Null, |n| Value::I64(n.into())),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("pending", Value::Bool(self.pending)),
            (
                "readback_lanes",
                self.readback_lanes
                    .map_or(Value::Null, |n| Value::U64(n.into())),
            ),
            ("checked_lanes", Value::U64(self.checked_lanes.into())),
            ("mismatches", Value::U64(self.mismatches.into())),
            ("nonfinite", Value::U64(self.nonfinite.into())),
            (
                "first_failure",
                self.first_failure.map_or(Value::Null, |f| {
                    object([
                        ("lane", Value::U64(f.lane.into())),
                        ("expected", Value::I64(f.expected.into())),
                        ("actual_bits", Value::U64(f.actual_bits.into())),
                        ("nonfinite", Value::Bool(f.nonfinite)),
                    ])
                }),
            ),
            ("verified_fma", Value::U64(self.verified_fma)),
            ("input_sha256", digest_value(self.input_sha256)),
            ("output_sha256", digest_value(self.output_sha256)),
        ])
    }
    pub(super) fn parse(v: &Value, b: WorkerBinding) -> Result<Self, WireError> {
        let o = exact(
            v,
            &[
                "dispatch",
                "phase",
                "seed",
                "lanes",
                "steps",
                "calls",
                "pending",
                "readback_lanes",
                "checked_lanes",
                "mismatches",
                "nonfinite",
                "first_failure",
                "verified_fma",
                "input_sha256",
                "output_sha256",
            ],
        )?;
        let Value::Array(raw_calls) = get(o, "calls") else {
            return Err(WireError::Body);
        };
        if raw_calls.len() > MAX_CALLS {
            return Err(WireError::Body);
        }
        let mut calls = Vec::with_capacity(raw_calls.len());
        for c in raw_calls {
            let c = exact(c, &["operation", "before", "after", "status"])?;
            let anchor = |v: &Value| match v {
                Value::Null => Ok(None),
                v => parse_anchor(v, b).map(Some),
            };
            calls.push(Call {
                operation: Operation::parse(string(get(c, "operation"))?)?,
                before: anchor(get(c, "before"))?,
                after: anchor(get(c, "after"))?,
                status: match get(c, "status") {
                    Value::Null => None,
                    v => Some(i32_value(v)?),
                },
            });
        }
        let first_failure = match get(o, "first_failure") {
            Value::Null => None,
            v => {
                let f = exact(v, &["lane", "expected", "actual_bits", "nonfinite"])?;
                Some(FirstFailure {
                    lane: u32_value(get(f, "lane"))?,
                    expected: i32_value(get(f, "expected"))?,
                    actual_bits: u32_value(get(f, "actual_bits"))?,
                    nonfinite: boolean(get(f, "nonfinite"))?,
                })
            }
        };
        Ok(Self {
            dispatch: u32_value(get(o, "dispatch"))?,
            phase: Phase::parse(get(o, "phase"))?,
            seed: uint(get(o, "seed"))?,
            lanes: u32_value(get(o, "lanes"))?,
            steps: u32_value(get(o, "steps"))?,
            calls,
            pending: boolean(get(o, "pending"))?,
            readback_lanes: match get(o, "readback_lanes") {
                Value::Null => None,
                v => Some(u32_value(v)?),
            },
            checked_lanes: u32_value(get(o, "checked_lanes"))?,
            mismatches: u32_value(get(o, "mismatches"))?,
            nonfinite: u32_value(get(o, "nonfinite"))?,
            first_failure,
            verified_fma: uint(get(o, "verified_fma"))?,
            input_sha256: parse_digest(get(o, "input_sha256"))?,
            output_sha256: parse_digest(get(o, "output_sha256"))?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureChunk {
    pub dispatch: u32,
    pub offset: u32,
    pub total_lanes: u32,
    pub bits: Vec<u32>,
}
impl FailureChunk {
    pub(super) fn allocation_bytes(&self) -> usize {
        self.bits.capacity() * 4
    }
    pub(super) fn validate(&self, b: WorkerBinding) -> Result<(), WireError> {
        if self.dispatch >= b.plan.warmup_dispatches + b.plan.measured_dispatches
            || self.total_lanes != b.plan.shape.lanes()
            || self.bits.is_empty()
            || self.bits.capacity() > CHUNK_LANES
            || u64::from(self.offset) + self.bits.len() as u64 > u64::from(self.total_lanes)
        {
            return Err(WireError::Body);
        }
        Ok(())
    }
    pub(super) fn value(&self) -> Value {
        object([
            ("dispatch", Value::U64(self.dispatch.into())),
            ("offset", Value::U64(self.offset.into())),
            ("total_lanes", Value::U64(self.total_lanes.into())),
            (
                "bits",
                Value::Array(self.bits.iter().map(|n| Value::U64((*n).into())).collect()),
            ),
        ])
    }
    pub(super) fn parse(v: &Value) -> Result<Self, WireError> {
        let o = exact(v, &["dispatch", "offset", "total_lanes", "bits"])?;
        let Value::Array(bits) = get(o, "bits") else {
            return Err(WireError::Body);
        };
        if bits.len() > CHUNK_LANES {
            return Err(WireError::Body);
        }
        Ok(Self {
            dispatch: u32_value(get(o, "dispatch"))?,
            offset: u32_value(get(o, "offset"))?,
            total_lanes: u32_value(get(o, "total_lanes"))?,
            bits: bits.iter().map(u32_value).collect::<Result<_, _>>()?,
        })
    }
}

/// Owns the first complete failing vector's reserved storage. Gaps, duplicates,
/// foreign chunks and hash contradictions poison it while preserving partial
/// data for diagnostics. This never replaces a failed observation with success.
pub struct FailureVector {
    binding: WorkerBinding,
    dispatch: u32,
    expected_sha256: Digest,
    bits: Vec<u32>,
    total: u32,
    failed: bool,
    complete: bool,
    _allocation: Reservation,
}
impl FailureVector {
    pub fn new(
        binding: WorkerBinding,
        record: &Record,
        failure_budget: &Budget,
    ) -> Result<Self, WireError> {
        record.validate(binding)?;
        if record.mismatches == 0
            || record.checked_lanes != record.lanes
            || record.readback_lanes != Some(record.lanes)
        {
            return Err(WireError::Body);
        }
        let expected_sha256 = record.output_sha256.ok_or(WireError::Body)?;
        let bytes = record.lanes as usize * 4;
        let allocation = failure_budget
            .reserve(bytes + std::mem::size_of::<Self>())
            .map_err(|_| WireError::Capacity)?;
        Ok(Self {
            binding,
            dispatch: record.dispatch,
            expected_sha256,
            bits: Vec::with_capacity(record.lanes as usize),
            total: record.lanes,
            failed: false,
            complete: false,
            _allocation: allocation,
        })
    }
    pub fn bits(&self) -> &[u32] {
        &self.bits
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn failed(&self) -> bool {
        self.failed
    }
    /// For transport/clock/parent-loss failures external to this accumulator.
    pub fn poison(&mut self) {
        self.failed = true;
        self.complete = false;
    }
    pub fn accept(&mut self, chunk: &FailureChunk) -> Result<(), WireError> {
        if self.failed {
            return Err(WireError::Poisoned);
        }
        if self.complete
            || chunk.validate(self.binding).is_err()
            || chunk.dispatch != self.dispatch
            || chunk.offset as usize != self.bits.len()
        {
            self.poison();
            return Err(WireError::Body);
        }
        self.bits.extend_from_slice(&chunk.bits);
        if self.bits.len() == self.total as usize {
            let mut hash = Sha256::new();
            for n in &self.bits {
                hash.update(n.to_le_bytes());
            }
            let actual = format!("{:x}", hash.finalize());
            if actual != self.expected_sha256.to_hex() {
                self.poison();
                return Err(WireError::Body);
            }
            self.complete = true;
        }
        Ok(())
    }
}
fn digest_value(d: Option<Digest>) -> Value {
    d.map_or(Value::Null, |d| Value::Text(d.to_hex()))
}
fn parse_digest(v: &Value) -> Result<Option<Digest>, WireError> {
    match v {
        Value::Null => Ok(None),
        v => Digest::parse(string(v)?)
            .map(Some)
            .map_err(|_| WireError::Body),
    }
}
fn u32_value(v: &Value) -> Result<u32, WireError> {
    uint(v)?.try_into().map_err(|_| WireError::Body)
}
fn i32_value(v: &Value) -> Result<i32, WireError> {
    v.as_i64()
        .map_err(|_| WireError::Body)?
        .try_into()
        .map_err(|_| WireError::Body)
}
fn boolean(v: &Value) -> Result<bool, WireError> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err(WireError::Body),
    }
}
