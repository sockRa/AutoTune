//! Exact integration of a synthetic power trace, not measured physical energy.
use crate::{Freshness, Identity, Metric, Provenance, Reading, Recorder, TimeBasis};
use serde::{Serialize, Serializer};

/// Inclusive observation-time window and maximum adjacent power-sample gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Policy {
    start_ns: u64,
    end_ns: u64,
    maximum_gap_ns: u64,
}

impl Policy {
    pub fn new(start_ns: u64, end_ns: u64, maximum_gap_ns: u64) -> Result<Self, PolicyError> {
        if start_ns >= end_ns {
            return Err(PolicyError::NonPositiveWindow);
        }
        if maximum_gap_ns == 0 {
            return Err(PolicyError::ZeroMaximumGap);
        }
        Ok(Self {
            start_ns,
            end_ns,
            maximum_gap_ns,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    NonPositiveWindow,
    ZeroMaximumGap,
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PolicyError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnavailableReason {
    RecorderRejectedInput,
    MissingStart,
    MissingEnd,
    PowerUnavailable {
        observation_ns: u64,
        reason: crate::UnavailableReason,
    },
    StalePower {
        observation_ns: u64,
    },
    ExcessGap {
        left_ns: u64,
        right_ns: u64,
    },
    ArithmeticOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Estimated {
        /// Twice the energy in picojoules; odd values retain half-pJ precision.
        #[serde(serialize_with = "decimal_string")]
        twice_picojoules: u128,
        interval_count: usize,
    },
    Unavailable {
        reason: UnavailableReason,
    },
}

fn decimal_string<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

/// Owned, immutable report. Source evidence is never modified or re-aged.
#[derive(Debug, Serialize)]
pub struct Report {
    schema_version: u32,
    provenance: Provenance,
    time_basis: TimeBasis,
    identity: Identity,
    recorder_policy: crate::Policy,
    policy: Policy,
    algorithm: &'static str,
    outcome: Outcome,
}

impl Report {
    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

pub fn estimate(recorder: &Recorder, policy: Policy) -> Report {
    let outcome = match integrate(recorder, policy) {
        Ok((twice_picojoules, interval_count)) => Outcome::Estimated {
            twice_picojoules,
            interval_count,
        },
        Err(reason) => Outcome::Unavailable { reason },
    };
    Report {
        schema_version: 1,
        provenance: Provenance::Simulated,
        time_basis: TimeBasis::RunRelativeMonotonicNanoseconds,
        identity: recorder.identity().clone(),
        recorder_policy: *recorder.policy(),
        policy,
        algorithm: "piecewise_linear_power_observation_time_v1",
        outcome,
    }
}

fn integrate(recorder: &Recorder, policy: Policy) -> Result<(u128, usize), UnavailableReason> {
    if recorder.had_rejections() {
        return Err(UnavailableReason::RecorderRejectedInput);
    }
    let mut power = recorder.samples().iter().filter(|sample| {
        sample.metric == Metric::Power
            && (policy.start_ns..=policy.end_ns).contains(&sample.observation_ns)
    });
    let first = power.next().ok_or(UnavailableReason::MissingStart)?;
    if first.observation_ns != policy.start_ns {
        return Err(UnavailableReason::MissingStart);
    }
    let mut left_time = first.observation_ns;
    let mut left_value = valid_power(first)?;
    let mut total = 0u128;
    let mut intervals = 0;
    for right in power {
        let right_value = valid_power(right)?;
        // Recorder guarantees strictly increasing power observations.
        let delta = right.observation_ns - left_time;
        if delta > policy.maximum_gap_ns {
            return Err(UnavailableReason::ExcessGap {
                left_ns: left_time,
                right_ns: right.observation_ns,
            });
        }
        total = left_value
            .checked_add(right_value)
            .and_then(|sum| sum.checked_mul(u128::from(delta)))
            .and_then(|area| total.checked_add(area))
            .ok_or(UnavailableReason::ArithmeticOverflow)?;
        intervals += 1;
        left_time = right.observation_ns;
        left_value = right_value;
    }
    if left_time != policy.end_ns {
        return Err(UnavailableReason::MissingEnd);
    }
    Ok((total, intervals))
}

fn valid_power(sample: &crate::Sample) -> Result<u128, UnavailableReason> {
    match sample.reading {
        Reading::Unavailable { reason } => Err(UnavailableReason::PowerUnavailable {
            observation_ns: sample.observation_ns,
            reason,
        }),
        Reading::Available { .. } if sample.freshness == Freshness::Stale => {
            Err(UnavailableReason::StalePower {
                observation_ns: sample.observation_ns,
            })
        }
        // Recorder guarantees nonnegative power. Cast before any arithmetic.
        Reading::Available { value } => Ok(value as u128),
    }
}
