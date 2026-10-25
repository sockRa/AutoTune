use autotune_measurement::*;
use serde_json::{json, Value};

fn identity() -> Identity {
    Identity::new("run-1", "synthetic", "v1", 42, "GPU-SIM", "0000:01:00.0").unwrap()
}

fn recorder(capacity: usize, age: u64) -> Recorder {
    Recorder::new(identity(), Policy::new(capacity, age).unwrap())
}

fn available(value: i64) -> Reading {
    Reading::Available { value }
}

#[test]
fn attribution_and_immutable_metadata() {
    let mut r = recorder(4, 10);
    let original = r.identity().clone();
    let policy = *r.policy();
    r.append(&identity(), Metric::Power, available(1200), 1, 2)
        .unwrap();
    assert_eq!(r.identity(), &original);
    assert_eq!(r.policy(), &policy);
    // Mutating an owned copy cannot alter retained evidence.
    let mut copy = r.samples().to_vec();
    copy[0].receipt_ns = 999;
    assert_eq!(r.samples()[0].receipt_ns, 2);
    let export: Value = serde_json::from_str(&r.to_json().unwrap()).unwrap();
    assert_eq!(
        export["identity"],
        json!({
            "run_id": "run-1", "workload_id": "synthetic", "workload_version": "v1",
            "workload_seed": 42, "gpu_uuid": "GPU-SIM", "pci_bus_id": "0000:01:00.0"
        })
    );
    assert_eq!(
        export["policy"],
        json!({"sample_capacity": 4, "maximum_age_ns": 10})
    );
}

#[test]
fn all_metrics_derive_units_and_preserve_integer_values() {
    let mut r = recorder(6, 1);
    for (metric, value, unit) in [
        (Metric::Temperature, i64::MIN, "degC"),
        (Metric::GraphicsClock, i64::MAX, "MHz"),
        (Metric::MemoryClock, 0, "MHz"),
        (Metric::GpuUtilization, 0, "%"),
        (Metric::MemoryUtilization, 100, "%"),
        (Metric::Power, i64::MAX, "mW"),
    ] {
        r.append(&identity(), metric, available(value), 0, 0)
            .unwrap();
        let sample = r.samples().last().unwrap();
        assert_eq!(sample.reading, available(value));
        assert_eq!(serde_json::to_value(sample.unit).unwrap(), unit);
    }
    assert_eq!(r.samples().len(), 6);
    assert!(!r.had_rejections());
}

#[test]
fn invalid_numbers_reject_without_ordering_mutation() {
    for (metric, value) in [
        (Metric::GraphicsClock, -1),
        (Metric::MemoryClock, i64::MIN),
        (Metric::Power, -1),
        (Metric::GpuUtilization, -1),
        (Metric::GpuUtilization, 101),
        (Metric::MemoryUtilization, -1),
        (Metric::MemoryUtilization, i64::MAX),
    ] {
        let mut r = recorder(1, 10);
        assert_eq!(
            r.append(&identity(), metric, available(value), 100, 100),
            Err(Error::InvalidValue { metric })
        );
        r.append(&identity(), metric, available(0), 0, 0).unwrap();
        assert_eq!(r.samples().len(), 1);
        assert!(r.had_rejections());
    }
}

#[test]
fn missing_reasons_are_independent_and_never_zero() {
    let mut r = recorder(6, 1);
    for (index, reason) in [
        UnavailableReason::Unsupported,
        UnavailableReason::PermissionDenied,
        UnavailableReason::QueryFailed,
        UnavailableReason::DeviceLost,
        UnavailableReason::NotSampled,
    ]
    .into_iter()
    .enumerate()
    {
        r.append(
            &identity(),
            Metric::Power,
            Reading::Unavailable { reason },
            index as u64,
            index as u64,
        )
        .unwrap();
        let sample = serde_json::to_value(r.samples().last().unwrap()).unwrap();
        assert_eq!(sample["reading"]["status"], "unavailable");
        assert_eq!(
            sample["reading"]["reason"],
            serde_json::to_value(reason).unwrap()
        );
        assert!(sample["reading"].get("value").is_none());
    }
    r.append(&identity(), Metric::Temperature, available(-5), 4, 4)
        .unwrap();
    assert_eq!(r.samples()[5].reading, available(-5));
}

#[test]
fn unavailable_samples_advance_ordering_watermarks() {
    let mut r = recorder(3, 10);
    r.append(
        &identity(),
        Metric::Power,
        Reading::Unavailable {
            reason: UnavailableReason::QueryFailed,
        },
        10,
        20,
    )
    .unwrap();
    let before = r.samples().to_vec();
    assert!(!r.had_rejections());
    for (metric, observation, receipt, error) in [
        (
            Metric::Power,
            10,
            30,
            Error::ObservationNotIncreasing {
                metric: Metric::Power,
            },
        ),
        (
            Metric::Power,
            9,
            30,
            Error::ObservationNotIncreasing {
                metric: Metric::Power,
            },
        ),
        (Metric::Temperature, 11, 19, Error::ReceiptRegression),
    ] {
        assert_eq!(
            r.append(&identity(), metric, available(1), observation, receipt),
            Err(error)
        );
        assert_eq!(r.samples(), before);
        assert!(r.had_rejections());
    }
    r.append(&identity(), Metric::Power, available(2), 11, 20)
        .unwrap();
    r.append(&identity(), Metric::Temperature, available(3), 10, 20)
        .unwrap();
    assert_eq!(&r.samples()[..1], before);
    assert_eq!(r.samples().len(), 3);
    assert!(r.had_rejections());
    assert_eq!(
        serde_json::from_str::<Value>(&r.to_json().unwrap()).unwrap()["had_rejections"],
        true
    );
}

#[test]
fn freshness_at_receipt_preserves_stale_and_missing_samples() {
    let mut r = recorder(4, 10);
    for (time, receipt, expected) in [
        (1, 10, Freshness::Fresh),
        (2, 12, Freshness::Fresh),
        (3, 14, Freshness::Stale),
    ] {
        r.append(&identity(), Metric::Power, available(9), time, receipt)
            .unwrap();
        let sample = r.samples().last().unwrap();
        assert_eq!(sample.freshness, expected);
        assert_eq!((sample.observation_ns, sample.receipt_ns), (time, receipt));
        assert_eq!(sample.reading, available(9));
    }
    r.append(
        &identity(),
        Metric::Temperature,
        Reading::Unavailable {
            reason: UnavailableReason::NotSampled,
        },
        0,
        14,
    )
    .unwrap();
    assert_eq!(r.samples()[3].freshness, Freshness::Stale);
    // Export does not reevaluate age or replace stale values.
    assert_eq!(r.to_json().unwrap(), r.to_json().unwrap());
}

#[test]
fn all_timing_errors_are_atomic_and_marker_is_sticky() {
    let mut r = recorder(3, 10);
    r.append(&identity(), Metric::Power, available(1), 10, 20)
        .unwrap();
    let before = r.samples().to_vec();
    for (metric, observation, receipt, error) in [
        (Metric::Temperature, 30, 29, Error::ObservationAfterReceipt),
        (Metric::Temperature, 18, 19, Error::ReceiptRegression),
        (
            Metric::Power,
            10,
            90,
            Error::ObservationNotIncreasing {
                metric: Metric::Power,
            },
        ),
        (
            Metric::Power,
            9,
            90,
            Error::ObservationNotIncreasing {
                metric: Metric::Power,
            },
        ),
    ] {
        assert_eq!(
            r.append(&identity(), metric, available(1), observation, receipt),
            Err(error)
        );
        assert_eq!(r.samples(), before);
    }
    r.append(&identity(), Metric::Temperature, available(2), 0, 20)
        .unwrap();
    r.append(&identity(), Metric::Power, available(2), 11, 20)
        .unwrap();
    assert!(r.had_rejections());
    assert_eq!(
        serde_json::from_str::<Value>(&r.to_json().unwrap()).unwrap()["had_rejections"],
        true
    );
}

#[test]
fn equal_receipts_and_cross_metric_observation_times_are_allowed() {
    let mut r = recorder(3, 1);
    for metric in [Metric::Power, Metric::Temperature, Metric::GpuUtilization] {
        r.append(&identity(), metric, available(0), 10, 10).unwrap();
    }
    assert_eq!(
        r.samples().iter().map(|s| s.metric).collect::<Vec<_>>(),
        [Metric::Power, Metric::Temperature, Metric::GpuUtilization]
    );
}

#[test]
fn all_identity_components_are_bound() {
    for binding in [
        Identity::new("other", "synthetic", "v1", 42, "GPU-SIM", "0000:01:00.0"),
        Identity::new("run-1", "other", "v1", 42, "GPU-SIM", "0000:01:00.0"),
        Identity::new("run-1", "synthetic", "other", 42, "GPU-SIM", "0000:01:00.0"),
        Identity::new("run-1", "synthetic", "v1", 43, "GPU-SIM", "0000:01:00.0"),
        Identity::new("run-1", "synthetic", "v1", 42, "other", "0000:01:00.0"),
        Identity::new("run-1", "synthetic", "v1", 42, "GPU-SIM", "other"),
    ] {
        let mut r = recorder(2, 1);
        r.append(&identity(), Metric::Power, available(1), 0, 0)
            .unwrap();
        let before = r.samples().to_vec();
        assert_eq!(
            r.append(&binding.unwrap(), Metric::Power, available(2), 10, 10),
            Err(Error::BindingMismatch)
        );
        assert_eq!(r.samples(), before);
        r.append(&identity(), Metric::Power, available(3), 1, 1)
            .unwrap();
        assert!(r.had_rejections());
    }
}

#[test]
fn identity_validation_covers_every_field_and_byte_boundary() {
    for bad in [
        "".to_owned(),
        " \t ".to_owned(),
        "x\0y".to_owned(),
        "x\ny".to_owned(),
        "x\u{7f}".to_owned(),
        "a".repeat(257),
        "\u{e9}".repeat(129),
    ] {
        for (index, field) in [
            "run_id",
            "workload_id",
            "workload_version",
            "gpu_uuid",
            "pci_bus_id",
        ]
        .into_iter()
        .enumerate()
        {
            let mut fields = ["run", "work", "ver", "gpu", "pci"];
            fields[index] = &bad;
            assert_eq!(
                Identity::new(fields[0], fields[1], fields[2], 0, fields[3], fields[4]),
                Err(Error::InvalidIdentity { field })
            );
        }
    }
    for valid in ["a".repeat(256), "\u{e9}".repeat(128)] {
        assert!(Identity::new(&valid, &valid, &valid, u64::MAX, &valid, &valid).is_ok());
    }
}

#[test]
fn capacity_rejects_without_eviction_or_evidence_change() {
    let mut r = recorder(1, 1);
    r.append(&identity(), Metric::Power, available(9), 0, 0)
        .unwrap();
    let before = r.samples().to_vec();
    for time in [1, u64::MAX] {
        assert_eq!(
            r.append(&identity(), Metric::Power, available(10), time, time),
            Err(Error::CapacityReached)
        );
        assert_eq!(r.samples(), before);
    }
    assert!(r.had_rejections());
}

#[test]
fn policy_and_timestamp_integer_boundaries() {
    assert_eq!(Policy::new(0, 1), Err(Error::ZeroCapacity));
    assert_eq!(Policy::new(1, 0), Err(Error::ZeroMaximumAge));
    // Construction must not preallocate this caller-sized bound.
    let mut r = recorder(usize::MAX, u64::MAX);
    r.append(&identity(), Metric::Power, available(0), 0, u64::MAX)
        .unwrap();
    assert_eq!(r.samples()[0].freshness, Freshness::Fresh);
    r.append(&identity(), Metric::Power, available(0), u64::MAX, u64::MAX)
        .unwrap();
    assert_eq!(r.samples()[1].freshness, Freshness::Fresh);
    assert_eq!(
        r.append(&identity(), Metric::Temperature, available(1), u64::MAX, 0),
        Err(Error::ObservationAfterReceipt)
    );
    let mut stale = recorder(1, u64::MAX - 1);
    stale
        .append(&identity(), Metric::Power, available(1), 0, u64::MAX)
        .unwrap();
    assert_eq!(stale.samples()[0].freshness, Freshness::Stale);
}

#[test]
fn export_is_versioned_simulated_deterministic_and_not_a_completeness_claim() {
    fn run() -> String {
        let mut r = recorder(3, 1);
        r.append(&identity(), Metric::Power, available(99), 0, 2)
            .unwrap();
        r.append(
            &identity(),
            Metric::Temperature,
            Reading::Unavailable {
                reason: UnavailableReason::DeviceLost,
            },
            2,
            2,
        )
        .unwrap();
        assert_eq!(
            r.append(&identity(), Metric::Power, available(100), 0, 2),
            Err(Error::ObservationNotIncreasing {
                metric: Metric::Power
            })
        );
        r.to_json().unwrap()
    }
    assert_eq!(run(), run());
    let export: Value = serde_json::from_str(&run()).unwrap();
    assert_eq!(export["schema_version"], 1);
    assert_eq!(export["provenance"], "simulated");
    assert_eq!(export["time_basis"], "run_relative_monotonic_nanoseconds");
    assert_eq!(export["had_rejections"], true);
    assert_eq!(
        export["samples"][0],
        json!({
            "metric": "power", "unit": "mW", "reading": {"status": "available", "value": 99},
            "observation_ns": 0, "receipt_ns": 2, "freshness": "stale"
        })
    );
    assert_eq!(
        export["samples"][1],
        json!({
            "metric": "temperature", "unit": "degC",
            "reading": {"status": "unavailable", "reason": "device_lost"},
            "observation_ns": 2, "receipt_ns": 2, "freshness": "fresh"
        })
    );
    assert_eq!(export.as_object().unwrap().len(), 7);
    assert!(export.get("complete").is_none());
    assert!(export.get("last_receipt").is_none());
    assert_eq!(
        serde_json::from_str::<Value>(&recorder(1, 1).to_json().unwrap()).unwrap()
            ["had_rejections"],
        false
    );
}
