//! Per-metric synthetic descriptions, not baseline or statistical qualification.
use crate::{Freshness, Identity, Metric, Provenance, Reading, Recorder, TimeBasis, Unit};
use serde::Serialize;

/// Explicit leading observation counts, including missing/stale observations.
/// Counts are not allocation requests; zero is a valid caller selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Policy {
    temperature: usize,
    graphics_clock: usize,
    memory_clock: usize,
    gpu_utilization: usize,
    memory_utilization: usize,
    power: usize,
}

impl Policy {
    pub fn new(
        temperature: usize,
        graphics_clock: usize,
        memory_clock: usize,
        gpu_utilization: usize,
        memory_utilization: usize,
        power: usize,
    ) -> Self {
        Self {
            temperature,
            graphics_clock,
            memory_clock,
            gpu_utilization,
            memory_utilization,
            power,
        }
    }

    pub fn warmup_for(self, metric: Metric) -> usize {
        match metric {
            Metric::Temperature => self.temperature,
            Metric::GraphicsClock => self.graphics_clock,
            Metric::MemoryClock => self.memory_clock,
            Metric::GpuUtilization => self.gpu_utilization,
            Metric::MemoryUtilization => self.memory_utilization,
            Metric::Power => self.power,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Warmup,
    /// Selected for calculation, not necessarily usable evidence.
    Included,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Classification {
    pub source_index: usize,
    pub role: Role,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InsufficientReason {
    RecorderRejectedInput,
    NoObservations,
    WarmupNotFulfilled,
    NoIncludedObservations,
    Unavailable {
        source_index: usize,
        reason: crate::UnavailableReason,
    },
    Stale {
        source_index: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Described { n: usize, min: i64, max: i64 },
    Insufficient { reason: InsufficientReason },
}

#[derive(Debug, Serialize)]
pub struct MetricSummary {
    metric: Metric,
    unit: Unit,
    requested_warmup: usize,
    observed_count: usize,
    samples: Vec<Classification>,
    outcome: Outcome,
}

impl MetricSummary {
    pub fn metric(&self) -> Metric {
        self.metric
    }

    pub fn unit(&self) -> Unit {
        self.unit
    }

    pub fn requested_warmup(&self) -> usize {
        self.requested_warmup
    }

    pub fn observed_count(&self) -> usize {
        self.observed_count
    }

    pub fn samples(&self) -> &[Classification] {
        &self.samples
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Assessment {
    NotAssessed,
}

/// Immutable owned snapshot retaining the exact original recorder JSON bytes.
#[derive(Debug, Serialize)]
pub struct Report {
    schema_version: u32,
    provenance: Provenance,
    time_basis: TimeBasis,
    identity: Identity,
    recorder_policy: crate::Policy,
    policy: Policy,
    had_rejections: bool,
    variation: Assessment,
    drift: Assessment,
    recorder_json: String,
    metrics: Vec<MetricSummary>,
}

impl Report {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn recorder_policy(&self) -> &crate::Policy {
        &self.recorder_policy
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn had_rejections(&self) -> bool {
        self.had_rejections
    }

    pub fn recorder_json(&self) -> &str {
        &self.recorder_json
    }

    pub fn metrics(&self) -> &[MetricSummary] {
        &self.metrics
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

pub fn describe(recorder: &Recorder, policy: Policy) -> Result<Report, serde_json::Error> {
    let recorder_json = recorder.to_json()?;
    let metrics = [
        Metric::Temperature,
        Metric::GraphicsClock,
        Metric::MemoryClock,
        Metric::GpuUtilization,
        Metric::MemoryUtilization,
        Metric::Power,
    ]
    .into_iter()
    .map(|metric| describe_metric(recorder, metric, policy.warmup_for(metric)))
    .collect();
    Ok(Report {
        schema_version: 1,
        provenance: Provenance::Simulated,
        time_basis: TimeBasis::RunRelativeMonotonicNanoseconds,
        identity: recorder.identity().clone(),
        recorder_policy: *recorder.policy(),
        policy,
        had_rejections: recorder.had_rejections(),
        variation: Assessment::NotAssessed,
        drift: Assessment::NotAssessed,
        recorder_json,
        metrics,
    })
}

fn describe_metric(recorder: &Recorder, metric: Metric, requested_warmup: usize) -> MetricSummary {
    let mut samples = Vec::new();
    let mut extrema: Option<(usize, i64, i64)> = None;
    let mut failure = None;
    for (source_index, sample) in recorder.samples().iter().enumerate() {
        if sample.metric != metric {
            continue;
        }
        let role = if samples.len() < requested_warmup {
            Role::Warmup
        } else {
            Role::Included
        };
        samples.push(Classification { source_index, role });
        if role == Role::Warmup {
            continue;
        }
        match sample.reading {
            Reading::Unavailable { reason } => {
                failure.get_or_insert(InsufficientReason::Unavailable {
                    source_index,
                    reason,
                });
            }
            Reading::Available { .. } if sample.freshness == Freshness::Stale => {
                failure.get_or_insert(InsufficientReason::Stale { source_index });
            }
            Reading::Available { value } => {
                extrema = Some(match extrema {
                    None => (1, value, value),
                    Some((n, min, max)) => (n + 1, min.min(value), max.max(value)),
                });
            }
        }
    }
    let observed_count = samples.len();
    let reason = if recorder.had_rejections() {
        Some(InsufficientReason::RecorderRejectedInput)
    } else if observed_count == 0 {
        Some(InsufficientReason::NoObservations)
    } else if observed_count < requested_warmup {
        Some(InsufficientReason::WarmupNotFulfilled)
    } else if observed_count == requested_warmup {
        Some(InsufficientReason::NoIncludedObservations)
    } else {
        failure
    };
    let outcome = match (reason, extrema) {
        (Some(reason), _) => Outcome::Insufficient { reason },
        (None, Some((n, min, max))) => Outcome::Described { n, min, max },
        (None, None) => unreachable!("nonempty complete included evidence has extrema"),
    };
    MetricSummary {
        metric,
        unit: metric.unit(),
        requested_warmup,
        observed_count,
        samples,
        outcome,
    }
}
