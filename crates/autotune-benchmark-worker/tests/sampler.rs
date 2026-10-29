use autotune_benchmark_worker::{
    monitor::{
        controller::Action, schedule::RecordError, Anchor, Binding, Controller, Metric, Record,
        Session, Status, StopReason, Ticket, Unit,
    },
    sampler::{map_field, sample_metric, MappingError},
    worker::Identity,
};
use autotune_probe::{
    nvml::SampleMetric,
    report::{Failure, Field, Reading, Reason, SymbolPresence},
};

fn binding() -> Binding {
    Binding {
        session: [1; 16],
        run: [2; 16],
        device: Identity {
            uuid: [3; 16],
            domain: 0,
            bus: 1,
            device: 2,
        },
        clock_token: [4; 16],
        qpc_frequency: 1_000_000_000,
    }
}

fn ticket(metric: Metric) -> Ticket {
    Ticket {
        binding: binding(),
        metric,
        slot: 7,
        scheduled_ns: 1_750_000_000,
        start_ns: 1_750_000_010,
    }
}

fn assert_raw_eq(actual: &Field, expected: &Field) {
    assert_eq!(actual.unit, expected.unit);
    assert_eq!(actual.symbol_presence, expected.symbol_presence);
    match (&actual.reading, &expected.reading) {
        (Reading::Available { value: a }, Reading::Available { value: b }) => assert_eq!(a, b),
        (Reading::Unavailable { failure: a }, Reading::Unavailable { failure: b }) => {
            assert_eq!(a.reason, b.reason);
            assert_eq!(a.vendor_status, b.vendor_status);
            assert_eq!(a.detail, b.detail);
        }
        _ => panic!("raw reading variant changed"),
    }
}

#[test]
fn maps_all_six_metrics_and_units() {
    let cases = [
        (
            Metric::Temperature,
            SampleMetric::Temperature,
            "degC",
            Unit::DegC,
            42,
        ),
        (
            Metric::Power,
            SampleMetric::Power,
            "mW",
            Unit::Milliwatt,
            123_000,
        ),
        (
            Metric::GraphicsClock,
            SampleMetric::GraphicsClock,
            "MHz",
            Unit::MHz,
            1500,
        ),
        (
            Metric::MemoryClock,
            SampleMetric::MemoryClock,
            "MHz",
            Unit::MHz,
            7000,
        ),
        (
            Metric::GpuUtilization,
            SampleMetric::GpuUtilization,
            "%",
            Unit::Percent,
            80,
        ),
        (
            Metric::MemoryUtilization,
            SampleMetric::MemoryUtilization,
            "%",
            Unit::Percent,
            10,
        ),
    ];
    for (metric, selected, unit, typed_unit, value) in cases {
        assert_eq!(sample_metric(metric), selected);
        let t = ticket(metric);
        let mapped = map_field(
            t,
            1_750_000_020,
            1_750_000_030,
            Field::available(unit, value),
        );
        assert_eq!(
            mapped.record,
            Ok(Record {
                ticket: t,
                end_ns: 1_750_000_020,
                receipt_ns: 1_750_000_030,
                unit: typed_unit,
                status: Status::Available,
                value: Some(value),
            })
        );
    }
}

#[test]
fn preserves_ticket_and_supplied_times() {
    let t = ticket(Metric::Temperature);
    let mapped = map_field(
        t,
        1_750_000_020,
        1_750_000_030,
        Field::available("degC", 42),
    );
    let record = mapped.record.unwrap();
    assert_eq!(record.ticket, t);
    assert_eq!(record.end_ns, 1_750_000_020);
    assert_eq!(record.receipt_ns, 1_750_000_030);
    // Conversion cannot certify or repair caller-supplied chronology.
    let unordered = map_field(t, u64::MAX, 0, Field::available("degC", 42));
    assert_eq!(unordered.record.unwrap().end_ns, u64::MAX);
    assert_eq!(unordered.record.unwrap().receipt_ns, 0);
}

#[test]
fn rejects_wrong_unit_and_unrepresentable_values() {
    let t = ticket(Metric::Temperature);
    for unit in ["DEGC", "degc", "mW", "", "degC "] {
        assert_eq!(
            map_field(t, 0, 0, Field::available(unit, 42)).record,
            Err(MappingError::Unit)
        );
    }
    let mut object = Field::available("degC", ());
    if let Reading::Available { value } = &mut object.reading {
        value["nested"] = "retained".into();
    }
    for raw in [
        Field::available("degC", 42.0),
        Field::available("degC", -0.0),
        Field::available("degC", "42"),
        Field::available("degC", true),
        Field::available("degC", ()),
        Field::available("degC", vec![42]),
        object,
        Field::available("degC", i64::MAX as u64 + 1),
        Field::available("degC", u64::MAX),
    ] {
        let expected = raw.clone();
        let mapped = map_field(t, 0, 0, raw);
        assert_eq!(mapped.record, Err(MappingError::Value));
        assert_raw_eq(&mapped.raw, &expected);
    }
    for value in [i64::MIN, -1, 0, i64::MAX] {
        assert_eq!(
            map_field(t, 0, 0, Field::available("degC", value))
                .record
                .unwrap()
                .value,
            Some(value)
        );
    }
}

#[test]
fn rejects_symbol_contradictions() {
    let t = ticket(Metric::Temperature);
    for symbol in [SymbolPresence::Absent, SymbolPresence::NotChecked] {
        let mut raw = Field::available("degC", 42);
        raw.symbol_presence = symbol;
        assert_eq!(map_field(t, 0, 0, raw).record, Err(MappingError::Symbol));
    }
    let cases = [
        (Reason::RuntimeMissing, SymbolPresence::NotChecked),
        (Reason::RuntimeLoadFailed, SymbolPresence::NotChecked),
        (Reason::SymbolMissing, SymbolPresence::Absent),
        (Reason::Unsupported, SymbolPresence::Present),
        (Reason::PermissionDenied, SymbolPresence::Present),
        (Reason::QueryFailed, SymbolPresence::Present),
        (Reason::InvalidValue, SymbolPresence::Present),
        (Reason::UnsupportedPlatform, SymbolPresence::NotChecked),
    ];
    for (reason, expected) in cases {
        for symbol in [
            SymbolPresence::Present,
            SymbolPresence::Absent,
            SymbolPresence::NotChecked,
        ] {
            let mut raw = Field::unavailable("degC", Failure::new(reason, "unchanged"));
            raw.symbol_presence = symbol;
            let mapped = map_field(t, 0, 0, raw);
            if symbol == expected {
                assert!(mapped.record.is_ok(), "{reason:?} {symbol:?}");
            } else {
                assert_eq!(mapped.record, Err(MappingError::Symbol));
            }
        }
    }
}

#[test]
fn maps_unavailable_without_losing_raw_evidence() {
    let cases = [
        (Reason::Unsupported, Some(3), Status::Unsupported),
        (Reason::SymbolMissing, None, Status::Unsupported),
        (Reason::UnsupportedPlatform, None, Status::Unsupported),
        (Reason::PermissionDenied, Some(4), Status::Failed(4)),
        (Reason::QueryFailed, Some(-999), Status::Failed(-999)),
        (Reason::InvalidValue, Some(999), Status::Failed(999)),
        (Reason::RuntimeMissing, None, Status::Unavailable),
        (Reason::RuntimeLoadFailed, None, Status::Unavailable),
        (Reason::PermissionDenied, None, Status::Unavailable),
        (Reason::QueryFailed, None, Status::Unavailable),
        (Reason::InvalidValue, None, Status::Unavailable),
    ];
    for (reason, vendor_status, expected_status) in cases {
        let raw = Field::unavailable(
            "degC",
            Failure {
                reason,
                vendor_status,
                detail: "raw \"status\" — åäö\nkept".into(),
            },
        );
        let expected = raw.clone();
        let mapped = map_field(ticket(Metric::Temperature), 10, 20, raw);
        let record = mapped.record.unwrap();
        assert_eq!(record.status, expected_status);
        assert_eq!(record.value, None);
        assert_raw_eq(&mapped.raw, &expected);
    }
    let bad_unit = Field::unavailable("MHz", Failure::new(Reason::QueryFailed, "kept"));
    assert_eq!(
        map_field(ticket(Metric::Temperature), 0, 0, bad_unit).record,
        Err(MappingError::Unit)
    );
}

#[test]
fn device_loss_is_not_optional_missing_data() {
    for metric in Metric::ALL {
        for (reason, vendor_status) in [
            (Reason::GpuLost, None),
            (Reason::GpuLost, Some(15)),
            (Reason::GpuLost, Some(-999)),
            (Reason::QueryFailed, Some(15)),
            (Reason::Unsupported, Some(15)),
        ] {
            let mut raw = Field::unavailable(
                "wrong",
                Failure {
                    reason,
                    vendor_status,
                    detail: "loss precedes other conversion faults".into(),
                },
            );
            raw.symbol_presence = SymbolPresence::Absent;
            let expected = raw.clone();
            let mapped = map_field(ticket(metric), 0, 0, raw);
            assert_eq!(mapped.record, Err(MappingError::DeviceLost));
            assert_raw_eq(&mapped.raw, &expected);
        }
    }
}

#[test]
fn raw_field_survives_success_and_every_error() {
    let mut bad_symbol = Field::available("degC", 42);
    bad_symbol.symbol_presence = SymbolPresence::Absent;
    for (raw, error) in [
        (Field::available("degC", 42), None),
        (Field::available("degC", "42"), Some(MappingError::Value)),
        (Field::available("DEGC", 42), Some(MappingError::Unit)),
        (bad_symbol, Some(MappingError::Symbol)),
        (
            Field::unavailable(
                "degC",
                Failure::new(Reason::GpuLost, "exact raw loss detail\n\0"),
            ),
            Some(MappingError::DeviceLost),
        ),
    ] {
        let expected = raw.clone();
        let mapped = map_field(ticket(Metric::Temperature), 10, 20, raw);
        assert_eq!(mapped.record.err(), error);
        assert_raw_eq(&mapped.raw, &expected);
    }
}

fn anchor(ns: u64) -> Anchor {
    Anchor {
        clock_token: binding().clock_token,
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}

fn normal_field(metric: Metric) -> Field {
    match metric {
        Metric::Temperature => Field::available("degC", 50),
        Metric::Power => Field::available("mW", 150_000),
        Metric::GraphicsClock => Field::available("MHz", 1500),
        Metric::MemoryClock => Field::available("MHz", 7000),
        Metric::GpuUtilization => Field::available("%", 80),
        Metric::MemoryUtilization => Field::available("%", 10),
    }
}

fn controller_at_metric(metric: Metric) -> (Controller, Ticket) {
    let mut controller = Controller::new(binding(), anchor(0), Session::Pilot).unwrap();
    loop {
        let t = controller.begin(anchor(0)).unwrap();
        if t.metric == metric {
            return (controller, t);
        }
        let mapped = map_field(t, 0, 0, normal_field(t.metric));
        controller.complete(anchor(0), mapped.record.unwrap());
    }
}

#[test]
fn converted_records_obey_existing_controller_validation() {
    for metric in Metric::ALL {
        let (mut c, t) = controller_at_metric(metric);
        let raw = normal_field(metric);
        let expected = raw.clone();
        let mapped = map_field(t, 0, 0, raw);
        c.complete(anchor(0), mapped.record.unwrap());
        assert_eq!(c.report().stop(), None);
        assert_eq!(c.report().pending(), None);
        assert_eq!(c.report().events().last().unwrap().record_error, None);
        assert_raw_eq(&mapped.raw, &expected);
    }

    for (metric, raw, end_ns, receipt_ns, error) in [
        (
            Metric::Temperature,
            Field::available("degC", -1),
            0,
            0,
            Some(RecordError::Value),
        ),
        (
            Metric::Power,
            Field::available("mW", -1),
            0,
            0,
            Some(RecordError::Value),
        ),
        (
            Metric::GpuUtilization,
            Field::available("%", 101),
            0,
            0,
            Some(RecordError::Value),
        ),
        (
            Metric::MemoryUtilization,
            Field::available("%", 101),
            0,
            0,
            Some(RecordError::Value),
        ),
        (
            Metric::Temperature,
            Field::available("degC", 50),
            50_000_000,
            50_000_000,
            None,
        ),
        (
            Metric::Temperature,
            Field::available("degC", 50),
            50_000_001,
            50_000_001,
            Some(RecordError::Bracket),
        ),
        (
            Metric::Temperature,
            Field::available("degC", 50),
            0,
            100_000_000,
            None,
        ),
        (
            Metric::Temperature,
            Field::available("degC", 50),
            0,
            100_000_001,
            Some(RecordError::Transport),
        ),
    ] {
        let (mut c, t) = controller_at_metric(metric);
        let expected = raw.clone();
        let mapped = map_field(t, end_ns, receipt_ns, raw);
        // Controller processing occurs at the next injected 100ns clock reading,
        // not by changing the asserted raw receipt or query-end times.
        c.complete(
            anchor(receipt_ns.div_ceil(100) * 100),
            mapped.record.unwrap(),
        );
        assert_eq!(c.report().events().last().unwrap().record_error, error);
        assert_eq!(
            c.report().stop().map(|s| s.reason),
            error.map(StopReason::Record)
        );
        assert_raw_eq(&mapped.raw, &expected);
    }
}

#[test]
fn conversion_error_closes_admission_and_retains_raw() {
    for (metric, raw, expected_error) in [
        (
            Metric::Temperature,
            Field::available("degC", "malformed"),
            MappingError::Value,
        ),
        (
            Metric::MemoryUtilization,
            Field::unavailable("%", Failure::vendor(15, "lost optional getter")),
            MappingError::DeviceLost,
        ),
    ] {
        let (mut c, t) = controller_at_metric(metric);
        if metric == Metric::MemoryUtilization {
            assert!(c.report().fixture_compute_eligible());
        }
        let expected_raw = raw.clone();
        let mapped = map_field(t, 1_000_000, 2_000_000, raw);
        // Caller composition: conversion itself has no controller side effect.
        match mapped.record {
            Ok(record) => c.complete(anchor(2_000_000), record),
            Err(_) => c.request_stop(anchor(2_000_000)),
        }
        assert_eq!(mapped.record, Err(expected_error));
        assert_raw_eq(&mapped.raw, &expected_raw);
        let stop = c.report().stop().unwrap();
        assert_eq!(stop.reason, StopReason::Requested);
        assert_eq!(c.report().pending(), Some(t));
        let safety = *c.report().safety();
        let late = map_field(t, 3_000_000, 4_000_000, normal_field(metric));
        c.complete(anchor(4_000_000), late.record.unwrap());
        assert_eq!(c.report().stop(), Some(stop));
        assert_eq!(*c.report().safety(), safety);
        assert_eq!(c.report().pending(), None);
        assert!(!c.report().fixture_compute_eligible());
        assert!(c.begin(anchor(5_000_000)).is_none());
        assert_raw_eq(&mapped.raw, &expected_raw);
    }
}

#[test]
fn late_return_cannot_reopen_stopped_controller() {
    let (mut c, last) = controller_at_metric(Metric::MemoryUtilization);
    let mapped = map_field(last, 0, 0, normal_field(last.metric));
    c.complete(anchor(0), mapped.record.unwrap());
    let t = c.begin(anchor(250_000_000)).unwrap();
    assert_eq!(t.metric, Metric::Temperature);
    assert!(c.report().fixture_compute_eligible());
    c.request_stop(anchor(251_000_000));
    let stop = c.report().stop();
    let safety = *c.report().safety();
    let raw = Field::available("degC", 51);
    let expected = raw.clone();
    let late = map_field(t, 252_000_000, 253_000_000, raw);
    c.complete(anchor(253_000_000), late.record.unwrap());
    assert_eq!(c.report().stop(), stop);
    assert_eq!(*c.report().safety(), safety);
    assert_eq!(c.report().pending(), None);
    assert!(!c.report().fixture_compute_eligible());
    assert_eq!(
        c.report().events().last().unwrap().action,
        Action::Complete(late.record.unwrap())
    );
    assert_raw_eq(&late.raw, &expected);
    assert!(c.begin(anchor(254_000_000)).is_none());
}

#[test]
fn missing_optional_metric_is_not_zero_or_fresh_safety() {
    let (mut c, t) = controller_at_metric(Metric::MemoryUtilization);
    let safety = *c.report().safety();
    assert!(c.report().fixture_compute_eligible());
    let raw = Field::unavailable("%", Failure::vendor(3, "optional utilization unsupported"));
    let expected = raw.clone();
    let mapped = map_field(t, 1_000_000, 2_000_000, raw);
    let record = mapped.record.unwrap();
    assert_eq!(record.status, Status::Unsupported);
    assert_eq!(record.value, None);
    c.complete(anchor(2_000_000), record);
    assert_eq!(c.report().stop(), None);
    assert!(c.report().baseline_insufficient()[Metric::MemoryUtilization as usize]);
    assert_eq!(*c.report().safety(), safety);
    assert_eq!(
        c.report().events().last().unwrap().action,
        Action::Complete(record)
    );
    assert_raw_eq(&mapped.raw, &expected);
    c.poll(anchor(750_000_000));
    assert_eq!(
        c.report().stop().unwrap().reason,
        StopReason::SafetyGap(Metric::Temperature)
    );
    assert!(!c.report().fixture_compute_eligible());
}
