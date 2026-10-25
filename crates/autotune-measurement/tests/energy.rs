use autotune_measurement::{
    energy::*, Identity, Metric, Policy as RecordingPolicy, Reading, Recorder,
};
use serde_json::{json, Value};

fn identity() -> Identity {
    Identity::new("run", "synthetic", "v1", 42, "GPU-SIM", "pci").unwrap()
}

fn recorder() -> Recorder {
    Recorder::new(identity(), RecordingPolicy::new(100, 10).unwrap())
}

fn add(r: &mut Recorder, time: u64, value: i64) {
    r.append(
        &identity(),
        Metric::Power,
        Reading::Available { value },
        time,
        time,
    )
    .unwrap();
}

fn policy(start: u64, end: u64, gap: u64) -> Policy {
    Policy::new(start, end, gap).unwrap()
}

// Every fixture also verifies deterministic exports and byte-for-byte source preservation.
fn outcome(r: &Recorder, p: Policy) -> Outcome {
    let before = r.to_json().unwrap();
    let report = estimate(r, p);
    assert_eq!(report.to_json().unwrap(), estimate(r, p).to_json().unwrap());
    assert_eq!(before, r.to_json().unwrap());
    *report.outcome()
}

#[test]
fn exact_constant_ramp_unequal_zero_and_half_picojoule() {
    for (trace, expected) in [
        (vec![(0, 5), (3, 5), (10, 5)], 100),
        (vec![(0, 0), (3, 3), (10, 10)], 100),
        (vec![(0, 2), (3, 4), (10, 1)], 53),
        (vec![(0, 0), (3, 0), (10, 0)], 0),
    ] {
        let mut r = recorder();
        for (time, value) in trace {
            add(&mut r, time, value);
        }
        assert_eq!(
            outcome(&r, policy(0, 10, 7)),
            Outcome::Estimated {
                twice_picojoules: expected,
                interval_count: 2,
            }
        );
    }
}

#[test]
fn empty_no_power_single_missing_and_bracketed_endpoints() {
    for (times, reason) in [
        (vec![], UnavailableReason::MissingStart),
        (vec![0], UnavailableReason::MissingEnd),
        (vec![10], UnavailableReason::MissingStart),
        (vec![1, 10], UnavailableReason::MissingStart),
        (vec![0, 9], UnavailableReason::MissingEnd),
        (vec![0, 20], UnavailableReason::MissingStart),
    ] {
        let mut r = recorder();
        for time in times {
            add(&mut r, time, 1);
        }
        let p = if r.samples().last().is_some_and(|s| s.observation_ns == 20) {
            policy(1, 19, 20)
        } else {
            policy(0, 10, 10)
        };
        assert_eq!(outcome(&r, p), Outcome::Unavailable { reason });
    }
    let mut beyond_end = recorder();
    add(&mut beyond_end, 0, 1);
    add(&mut beyond_end, 20, 1);
    assert_eq!(
        outcome(&beyond_end, policy(0, 10, 20)),
        Outcome::Unavailable {
            reason: UnavailableReason::MissingEnd
        }
    );
    let mut r = recorder();
    r.append(
        &identity(),
        Metric::Temperature,
        Reading::Available { value: 3 },
        0,
        0,
    )
    .unwrap();
    assert_eq!(
        outcome(&r, policy(0, 10, 10)),
        Outcome::Unavailable {
            reason: UnavailableReason::MissingStart
        }
    );
}

#[test]
fn every_unavailable_reason_at_each_position_invalidates_window() {
    for reason in [
        autotune_measurement::UnavailableReason::Unsupported,
        autotune_measurement::UnavailableReason::PermissionDenied,
        autotune_measurement::UnavailableReason::QueryFailed,
        autotune_measurement::UnavailableReason::DeviceLost,
        autotune_measurement::UnavailableReason::NotSampled,
    ] {
        for bad in [0, 5, 10] {
            let mut r = recorder();
            for time in [0, 5, 10] {
                let reading = if time == bad {
                    Reading::Unavailable { reason }
                } else {
                    Reading::Available { value: 5 }
                };
                r.append(&identity(), Metric::Power, reading, time, time)
                    .unwrap();
            }
            assert_eq!(
                outcome(&r, policy(0, 10, 10)),
                Outcome::Unavailable {
                    reason: UnavailableReason::PowerUnavailable {
                        observation_ns: bad,
                        reason
                    },
                }
            );
        }
    }
}

#[test]
fn stale_at_each_position_is_not_bridged() {
    for bad in [0, 5, 10] {
        let mut r = recorder();
        for time in [0, 5, 10] {
            let receipt = if time >= bad { time + 11 } else { time };
            // Once stale receipt occurs, keep later receipts monotonic but later samples fresh.
            let receipt = if time > bad {
                (bad + 11).max(time)
            } else {
                receipt
            };
            r.append(
                &identity(),
                Metric::Power,
                Reading::Available { value: 5 },
                time,
                receipt,
            )
            .unwrap();
        }
        assert_eq!(
            outcome(&r, policy(0, 10, 10)),
            Outcome::Unavailable {
                reason: UnavailableReason::StalePower {
                    observation_ns: bad
                },
            }
        );
    }
}

#[test]
fn gap_boundary_is_inclusive() {
    let mut r = recorder();
    add(&mut r, 0, 2);
    add(&mut r, 10, 2);
    assert_eq!(
        outcome(&r, policy(0, 10, 10)),
        Outcome::Estimated {
            twice_picojoules: 40,
            interval_count: 1
        }
    );
    assert_eq!(
        outcome(&r, policy(0, 10, 9)),
        Outcome::Unavailable {
            reason: UnavailableReason::ExcessGap {
                left_ns: 0,
                right_ns: 10
            }
        }
    );
}

#[test]
fn irrelevant_failures_and_outside_window_staleness_are_ignored() {
    let mut r = recorder();
    r.append(
        &identity(),
        Metric::Power,
        Reading::Unavailable {
            reason: autotune_measurement::UnavailableReason::DeviceLost,
        },
        0,
        11,
    )
    .unwrap();
    add(&mut r, 20, 1);
    r.append(
        &identity(),
        Metric::Temperature,
        Reading::Unavailable {
            reason: autotune_measurement::UnavailableReason::QueryFailed,
        },
        0,
        20,
    )
    .unwrap();
    add(&mut r, 30, 3);
    r.append(
        &identity(),
        Metric::Power,
        Reading::Available { value: 999 },
        31,
        50,
    )
    .unwrap();
    r.append(
        &identity(),
        Metric::Power,
        Reading::Unavailable {
            reason: autotune_measurement::UnavailableReason::NotSampled,
        },
        51,
        51,
    )
    .unwrap();
    assert_eq!(
        outcome(&r, policy(20, 30, 10)),
        Outcome::Estimated {
            twice_picojoules: 40,
            interval_count: 1
        }
    );
}

#[test]
fn any_rejected_metric_or_time_blocks_later_good_evidence() {
    for metric in [Metric::Temperature, Metric::Power] {
        let mut r = recorder();
        assert!(r
            .append(
                &identity(),
                metric,
                Reading::Available { value: 1 },
                100,
                99
            )
            .is_err());
        add(&mut r, 0, 1);
        add(&mut r, 10, 1);
        assert_eq!(
            outcome(&r, policy(0, 10, 10)),
            Outcome::Unavailable {
                reason: UnavailableReason::RecorderRejectedInput
            }
        );
    }
}

#[test]
fn invalid_policy_is_not_unavailable_evidence() {
    assert_eq!(Policy::new(5, 5, 1), Err(PolicyError::NonPositiveWindow));
    assert_eq!(Policy::new(6, 5, 1), Err(PolicyError::NonPositiveWindow));
    assert_eq!(Policy::new(0, 1, 0), Err(PolicyError::ZeroMaximumGap));
    assert!(Policy::new(0, u64::MAX, u64::MAX).is_ok());
}

#[test]
fn observation_times_not_receipts_and_extreme_integer_precision() {
    let mut r = Recorder::new(identity(), RecordingPolicy::new(3, u64::MAX).unwrap());
    for time in [0, u64::MAX - 1, u64::MAX] {
        r.append(
            &identity(),
            Metric::Power,
            Reading::Available { value: i64::MAX },
            time,
            u64::MAX,
        )
        .unwrap();
    }
    let expected = 2 * (i64::MAX as u128) * u128::from(u64::MAX);
    assert_eq!(
        outcome(&r, policy(0, u64::MAX, u64::MAX)),
        Outcome::Estimated {
            twice_picojoules: expected,
            interval_count: 2
        }
    );
    assert_eq!(
        outcome(&r, policy(u64::MAX - 1, u64::MAX, 1)),
        Outcome::Estimated {
            twice_picojoules: 2 * (i64::MAX as u128),
            interval_count: 1
        }
    );
    let value: Value = serde_json::from_str(
        &estimate(&r, policy(0, u64::MAX, u64::MAX))
            .to_json()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["outcome"]["twice_picojoules"], expected.to_string());
}

#[test]
fn complete_estimated_and_unavailable_export_contracts() {
    let mut r = recorder();
    add(&mut r, 0, 0);
    add(&mut r, 1, 1);
    let p = policy(0, 1, 1);
    let mut expected = json!({
        "schema_version": 1,
        "provenance": "simulated",
        "time_basis": "run_relative_monotonic_nanoseconds",
        "identity": { "run_id": "run", "workload_id": "synthetic", "workload_version": "v1", "workload_seed": 42, "gpu_uuid": "GPU-SIM", "pci_bus_id": "pci" },
        "recorder_policy": { "sample_capacity": 100, "maximum_age_ns": 10 },
        "policy": { "start_ns": 0, "end_ns": 1, "maximum_gap_ns": 1 },
        "algorithm": "piecewise_linear_power_observation_time_v1",
        "outcome": { "status": "estimated", "twice_picojoules": "1", "interval_count": 1 }
    });
    assert_eq!(serde_json::to_value(estimate(&r, p)).unwrap(), expected);
    assert_eq!(
        outcome(&r, p),
        Outcome::Estimated {
            twice_picojoules: 1,
            interval_count: 1
        }
    );
    expected["outcome"] = json!({ "status": "unavailable", "reason": { "kind": "missing_start" } });
    let empty = recorder();
    assert_eq!(serde_json::to_value(estimate(&empty, p)).unwrap(), expected);
    assert_eq!(
        outcome(&empty, p),
        Outcome::Unavailable {
            reason: UnavailableReason::MissingStart
        }
    );
}

#[test]
fn nested_unavailable_export_contracts_include_metadata_without_quantity() {
    for (reading, receipt, gap, reason) in [
        (
            Reading::Unavailable {
                reason: autotune_measurement::UnavailableReason::DeviceLost,
            },
            10,
            10,
            json!({ "kind": "power_unavailable", "observation_ns": 10, "reason": "device_lost" }),
        ),
        (
            Reading::Available { value: 2 },
            21,
            10,
            json!({ "kind": "stale_power", "observation_ns": 10 }),
        ),
        (
            Reading::Available { value: 2 },
            10,
            9,
            json!({ "kind": "excess_gap", "left_ns": 0, "right_ns": 10 }),
        ),
    ] {
        let mut r = recorder();
        add(&mut r, 0, 1);
        r.append(&identity(), Metric::Power, reading, 10, receipt)
            .unwrap();
        let export: Value =
            serde_json::from_str(&estimate(&r, policy(0, 10, gap)).to_json().unwrap()).unwrap();
        assert_eq!(
            export,
            json!({
                "schema_version": 1,
                "provenance": "simulated",
                "time_basis": "run_relative_monotonic_nanoseconds",
                "identity": { "run_id": "run", "workload_id": "synthetic", "workload_version": "v1", "workload_seed": 42, "gpu_uuid": "GPU-SIM", "pci_bus_id": "pci" },
                "recorder_policy": { "sample_capacity": 100, "maximum_age_ns": 10 },
                "policy": { "start_ns": 0, "end_ns": 10, "maximum_gap_ns": gap },
                "algorithm": "piecewise_linear_power_observation_time_v1",
                "outcome": { "status": "unavailable", "reason": reason }
            })
        );
        assert!(export["outcome"].get("twice_picojoules").is_none());
        assert!(export["outcome"].get("interval_count").is_none());
    }
}

#[test]
fn owned_report_is_independent_of_later_samples_and_rejections() {
    let mut r = recorder();
    add(&mut r, 0, 1);
    add(&mut r, 10, 2);
    let p = policy(0, 10, 10);
    let report = estimate(&r, p);
    let original = report.to_json().unwrap();
    assert!(matches!(report.outcome(), Outcome::Estimated { .. }));
    add(&mut r, 20, 3);
    assert_eq!(report.to_json().unwrap(), original);
    assert!(r
        .append(
            &identity(),
            Metric::Power,
            Reading::Available { value: 4 },
            20,
            20
        )
        .is_err());
    assert_eq!(report.to_json().unwrap(), original);
    assert_eq!(
        outcome(&r, p),
        Outcome::Unavailable {
            reason: UnavailableReason::RecorderRejectedInput
        }
    );
}
