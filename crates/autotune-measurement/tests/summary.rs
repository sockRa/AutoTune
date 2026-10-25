use autotune_measurement::{
    summary::{self, Classification, InsufficientReason as Reason, Outcome, Policy, Role},
    Identity, Metric, Reading, Recorder, UnavailableReason,
};
use serde_json::{json, Value};

const METRICS: [Metric; 6] = [
    Metric::Temperature,
    Metric::GraphicsClock,
    Metric::MemoryClock,
    Metric::GpuUtilization,
    Metric::MemoryUtilization,
    Metric::Power,
];

fn recorder() -> Recorder {
    Recorder::new(
        Identity::new("run", "work", "v1", u64::MAX, "gpu", "pci").unwrap(),
        autotune_measurement::Policy::new(100, 5).unwrap(),
    )
}

fn append(recorder: &mut Recorder, metric: Metric, reading: Reading, time: u64, age: u64) {
    recorder
        .append(
            &recorder.identity().clone(),
            metric,
            reading,
            time,
            time + age,
        )
        .unwrap();
}

fn available(value: i64) -> Reading {
    Reading::Available { value }
}

fn zero() -> Policy {
    Policy::new(0, 0, 0, 0, 0, 0)
}

#[test]
fn interleaved_metrics_have_independent_leading_counts_and_global_indices() {
    let mut source = recorder();
    let policy = Policy::new(0, 1, 2, 3, 4, 5);
    for round in 0..6 {
        for metric in METRICS {
            append(&mut source, metric, available(round), round as u64, 0);
        }
    }
    let report = summary::describe(&source, policy).unwrap();
    for (index, metric) in METRICS.into_iter().enumerate() {
        let entry = &report.metrics()[index];
        assert_eq!(entry.metric(), metric);
        assert_eq!(entry.unit(), metric.unit());
        assert_eq!(entry.requested_warmup(), index);
        assert_eq!(entry.observed_count(), 6);
        assert_eq!(policy.warmup_for(metric), index);
        assert_eq!(
            entry.outcome(),
            &Outcome::Described {
                n: 6 - index,
                min: index as i64,
                max: 5
            }
        );
        for (round, sample) in entry.samples().iter().enumerate() {
            assert_eq!(sample.source_index, round * 6 + index);
            assert_eq!(
                sample.role,
                if round < index {
                    Role::Warmup
                } else {
                    Role::Included
                }
            );
        }
    }
}

#[test]
fn zero_warmup_single_and_signed_extreme_values_are_exact() {
    let mut source = recorder();
    append(&mut source, Metric::Temperature, available(i64::MIN), 0, 0);
    let single = summary::describe(&source, zero()).unwrap();
    assert_eq!(
        single.metrics()[0].outcome(),
        &Outcome::Described {
            n: 1,
            min: i64::MIN,
            max: i64::MIN
        }
    );
    append(&mut source, Metric::Temperature, available(-1), 1, 0);
    append(
        &mut source,
        Metric::Temperature,
        available(i64::MAX),
        u64::MAX,
        0,
    );
    let report = summary::describe(&source, zero()).unwrap();
    assert_eq!(
        report.metrics()[0].outcome(),
        &Outcome::Described {
            n: 3,
            min: i64::MIN,
            max: i64::MAX
        }
    );
    let exported = report.to_json().unwrap();
    let parsed: Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(
        parsed["metrics"][0]["outcome"]["min"].as_i64(),
        Some(i64::MIN)
    );
    assert_eq!(
        parsed["metrics"][0]["outcome"]["max"].as_i64(),
        Some(i64::MAX)
    );
    let raw: Value = serde_json::from_str(report.recorder_json()).unwrap();
    assert_eq!(raw["identity"]["workload_seed"].as_u64(), Some(u64::MAX));
    assert_eq!(raw["samples"][2]["observation_ns"].as_u64(), Some(u64::MAX));
    assert_eq!(raw["samples"][2]["receipt_ns"].as_u64(), Some(u64::MAX));
}

#[test]
fn nonmonotonic_included_values_retain_repeated_interior_extrema() {
    let mut source = recorder();
    for (time, value) in [99, 3, -8, 12, -8, 12, 4].into_iter().enumerate() {
        append(
            &mut source,
            Metric::Temperature,
            available(value),
            time as u64,
            0,
        );
    }
    let report = summary::describe(&source, Policy::new(1, 0, 0, 0, 0, 0)).unwrap();
    assert_eq!(
        report.metrics()[0].outcome(),
        &Outcome::Described {
            n: 6,
            min: -8,
            max: 12
        }
    );
}

#[test]
fn classifications_follow_append_order_not_cross_metric_observation_time() {
    let mut source = recorder();
    append(&mut source, Metric::Temperature, available(-4), 100, 0);
    // Observation time may precede another metric's while receipt remains ordered.
    append(&mut source, Metric::Power, available(7), 96, 4);
    append(&mut source, Metric::Temperature, available(-8), 101, 0);
    append(&mut source, Metric::Power, available(9), 100, 1);
    let report = summary::describe(&source, Policy::new(1, 0, 0, 0, 0, 1)).unwrap();
    for (metric_index, source_indices, value) in [(0, [0, 2], -8), (5, [1, 3], 9)] {
        let entry = &report.metrics()[metric_index];
        assert_eq!(
            entry.samples(),
            &[
                Classification {
                    source_index: source_indices[0],
                    role: Role::Warmup
                },
                Classification {
                    source_index: source_indices[1],
                    role: Role::Included
                },
            ]
        );
        assert_eq!(
            entry.outcome(),
            &Outcome::Described {
                n: 1,
                min: value,
                max: value
            }
        );
    }
}

#[test]
fn absent_short_exhausted_and_huge_warmup_remain_distinct() {
    let mut source = recorder();
    append(&mut source, Metric::Temperature, available(8), 0, 0);
    for (count, reason) in [
        (1, Reason::NoIncludedObservations),
        (2, Reason::WarmupNotFulfilled),
        (usize::MAX, Reason::WarmupNotFulfilled),
    ] {
        let report = summary::describe(&source, Policy::new(count, 0, 0, 0, 0, 0)).unwrap();
        assert_eq!(
            report.metrics()[0].outcome(),
            &Outcome::Insufficient { reason }
        );
        assert_eq!(
            report.metrics()[0].samples(),
            &[Classification {
                source_index: 0,
                role: Role::Warmup
            }]
        );
        let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(json["policy"]["temperature"].as_u64(), Some(count as u64));
        assert!(json["metrics"][0]["outcome"].get("min").is_none());
        assert!(json["metrics"][0]["outcome"].get("max").is_none());
        assert!(json["metrics"][0]["outcome"].get("n").is_none());
        assert_eq!(
            json["metrics"][0]["outcome"],
            json!({"status": "insufficient", "reason": {
                "kind": if count == 1 { "no_included_observations" } else { "warmup_not_fulfilled" }
            }})
        );
    }
    for count in [0, usize::MAX] {
        let report = summary::describe(
            &recorder(),
            Policy::new(count, count, count, count, count, count),
        )
        .unwrap();
        assert!(report.metrics().iter().all(|entry| entry.outcome()
            == &Outcome::Insufficient {
                reason: Reason::NoObservations
            }));
    }
}

#[test]
fn missing_and_stale_warmup_count_but_do_not_block_complete_included_evidence() {
    let mut source = recorder();
    append(
        &mut source,
        Metric::Temperature,
        Reading::Unavailable {
            reason: UnavailableReason::DeviceLost,
        },
        0,
        6,
    );
    append(&mut source, Metric::Temperature, available(99), 7, 6);
    append(&mut source, Metric::Temperature, available(-7), 14, 0);
    let report = summary::describe(&source, Policy::new(2, 0, 0, 0, 0, 0)).unwrap();
    assert_eq!(
        report.metrics()[0].outcome(),
        &Outcome::Described {
            n: 1,
            min: -7,
            max: -7
        }
    );
    assert_eq!(report.metrics()[0].observed_count(), 3);
    assert_eq!(report.recorder_json(), source.to_json().unwrap());
}

#[test]
fn every_included_unavailable_reason_blocks_all_extrema_without_filtering() {
    for reason in [
        UnavailableReason::Unsupported,
        UnavailableReason::PermissionDenied,
        UnavailableReason::QueryFailed,
        UnavailableReason::DeviceLost,
        UnavailableReason::NotSampled,
    ] {
        let mut source = recorder();
        append(&mut source, Metric::Power, available(10), 0, 0);
        append(
            &mut source,
            Metric::Power,
            Reading::Unavailable { reason },
            1,
            0,
        );
        append(&mut source, Metric::Power, available(20), 2, 0);
        append(&mut source, Metric::Temperature, available(-7), 2, 0);
        let report = summary::describe(&source, zero()).unwrap();
        assert_eq!(
            report.metrics()[0].outcome(),
            &Outcome::Described {
                n: 1,
                min: -7,
                max: -7
            }
        );
        assert_eq!(
            report.metrics()[5].outcome(),
            &Outcome::Insufficient {
                reason: Reason::Unavailable {
                    source_index: 1,
                    reason
                }
            }
        );
        assert_eq!(report.metrics()[5].samples().len(), 3);
        assert!(report.metrics()[5]
            .samples()
            .iter()
            .all(|sample| sample.role == Role::Included));
        let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(
            json["metrics"][5]["outcome"],
            json!({"status": "insufficient", "reason": {"kind": "unavailable", "source_index": 1, "reason": serde_json::to_value(reason).unwrap()}})
        );
    }
}

#[test]
fn included_stale_blocks_metric_without_affecting_other_metrics() {
    let mut source = recorder();
    append(&mut source, Metric::Temperature, available(10), 0, 6);
    append(&mut source, Metric::Temperature, available(20), 7, 0);
    append(&mut source, Metric::Power, available(30), 7, 0);
    let report = summary::describe(&source, zero()).unwrap();
    assert_eq!(
        report.metrics()[0].outcome(),
        &Outcome::Insufficient {
            reason: Reason::Stale { source_index: 0 }
        }
    );
    assert_eq!(
        report.metrics()[5].outcome(),
        &Outcome::Described {
            n: 1,
            min: 30,
            max: 30
        }
    );
    let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    assert_eq!(
        json["metrics"][0]["outcome"],
        json!({"status": "insufficient", "reason": {"kind": "stale", "source_index": 0}})
    );
}

#[test]
fn freshness_uses_receipt_age_boundary_without_reaging() {
    let mut source = recorder(); // maximum_age_ns = 5
    append(&mut source, Metric::Temperature, available(-7), 0, 5);
    append(&mut source, Metric::Power, available(30), 0, 6);
    assert_eq!(
        source.samples()[0].freshness,
        autotune_measurement::Freshness::Fresh
    );
    assert_eq!(
        source.samples()[1].freshness,
        autotune_measurement::Freshness::Stale
    );
    let before = source.to_json().unwrap();
    let report = summary::describe(&source, zero()).unwrap();
    assert_eq!(
        report.metrics()[0].outcome(),
        &Outcome::Described {
            n: 1,
            min: -7,
            max: -7
        }
    );
    assert_eq!(
        report.metrics()[5].outcome(),
        &Outcome::Insufficient {
            reason: Reason::Stale { source_index: 1 }
        }
    );
    assert_eq!(source.to_json().unwrap(), before);
    let exported = report.to_json().unwrap();
    append(
        &mut source,
        Metric::MemoryClock,
        available(100),
        u64::MAX,
        0,
    );
    let later = summary::describe(&source, zero()).unwrap();
    assert_eq!(later.metrics()[0].outcome(), report.metrics()[0].outcome());
    assert_eq!(later.metrics()[5].outcome(), report.metrics()[5].outcome());
    assert_eq!(report.to_json().unwrap(), exported);
    assert_eq!(report.recorder_json(), before);
}

#[test]
fn capacity_rejection_retains_samples_but_makes_every_metric_insufficient() {
    let mut source = Recorder::new(
        recorder().identity().clone(),
        autotune_measurement::Policy::new(6, 5).unwrap(),
    );
    for metric in METRICS {
        append(&mut source, metric, available(5), 0, 0);
    }
    let retained = source.samples().to_vec();
    assert_eq!(
        source.append(
            &source.identity().clone(),
            Metric::Temperature,
            available(9),
            1,
            1
        ),
        Err(autotune_measurement::Error::CapacityReached)
    );
    assert_eq!(source.samples(), retained);
    let report = summary::describe(&source, zero()).unwrap();
    assert_eq!(report.recorder_json(), source.to_json().unwrap());
    assert!(report.had_rejections());
    let exported: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    for (index, entry) in report.metrics().iter().enumerate() {
        assert_eq!(entry.observed_count(), 1);
        assert_eq!(
            entry.samples(),
            &[Classification {
                source_index: index,
                role: Role::Included
            }]
        );
        assert_eq!(
            entry.outcome(),
            &Outcome::Insufficient {
                reason: Reason::RecorderRejectedInput
            }
        );
        assert_eq!(
            exported["metrics"][index]["outcome"],
            json!({"status": "insufficient", "reason": {"kind": "recorder_rejected_input"}})
        );
    }
}

#[test]
fn any_recorder_rejection_overrides_every_metric_even_after_valid_appends() {
    for reject_before in [true, false] {
        let mut source = recorder();
        let reject = |source: &mut Recorder| {
            assert!(source
                .append(
                    &source.identity().clone(),
                    Metric::Power,
                    available(-1),
                    0,
                    0
                )
                .is_err());
        };
        if reject_before {
            reject(&mut source);
        }
        for metric in METRICS {
            append(&mut source, metric, available(5), 0, 0);
        }
        if !reject_before {
            reject(&mut source);
        }
        let report = summary::describe(&source, zero()).unwrap();
        assert!(report.had_rejections());
        assert!(report.metrics().iter().all(|entry| entry.outcome()
            == &Outcome::Insufficient {
                reason: Reason::RecorderRejectedInput
            }));
        let exported: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        for entry in exported["metrics"].as_array().unwrap() {
            assert_eq!(
                entry["outcome"],
                json!({"status": "insufficient", "reason": {"kind": "recorder_rejected_input"}})
            );
        }
    }
}

#[test]
fn report_and_source_are_independent_and_export_shape_is_exact_and_deterministic() {
    let mut source = Recorder::new(
        Identity::new(
            "run\"\\雪",
            "work\\\"å",
            "v1-é",
            u64::MAX,
            "gpu-雪",
            "pci\\\"",
        )
        .unwrap(),
        autotune_measurement::Policy::new(100, 5).unwrap(),
    );
    append(&mut source, Metric::Temperature, available(-12), 0, 0);
    let before = source.to_json().unwrap();
    let report = summary::describe(&source, zero()).unwrap();
    let exported = report.to_json().unwrap();
    let decoded: Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(decoded["recorder_json"].as_str(), Some(before.as_str()));
    assert_eq!(before, source.to_json().unwrap());
    assert_eq!(report.identity(), source.identity());
    assert_eq!(report.recorder_policy(), source.policy());
    assert_eq!(report.policy(), &zero());
    assert_eq!(
        report.to_json().unwrap(),
        summary::describe(&source, zero())
            .unwrap()
            .to_json()
            .unwrap()
    );
    let mut metrics = Vec::new();
    for (index, metric) in METRICS.into_iter().enumerate() {
        metrics.push(json!({
            "metric": metric, "unit": metric.unit(), "requested_warmup": 0,
            "observed_count": if index == 0 { 1 } else { 0 },
            "samples": if index == 0 { json!([{"source_index": 0, "role": "included"}]) } else { json!([]) },
            "outcome": if index == 0 { json!({"status": "described", "n": 1, "min": -12, "max": -12}) } else { json!({"status": "insufficient", "reason": {"kind": "no_observations"}}) }
        }));
    }
    assert_eq!(
        serde_json::from_str::<Value>(&exported).unwrap(),
        json!({
            "schema_version": 1, "provenance": "simulated",
            "time_basis": "run_relative_monotonic_nanoseconds", "identity": source.identity(),
            "recorder_policy": source.policy(), "policy": zero(), "had_rejections": false,
            "variation": "not_assessed", "drift": "not_assessed", "recorder_json": before,
            "metrics": metrics
        })
    );
    append(&mut source, Metric::Temperature, available(9), 1, 0);
    assert!(source
        .append(
            &source.identity().clone(),
            Metric::Power,
            available(-1),
            1,
            1
        )
        .is_err());
    assert_ne!(before, source.to_json().unwrap());
    drop(source);
    assert_eq!(report.recorder_json(), before);
    assert_eq!(report.to_json().unwrap(), exported);
}
