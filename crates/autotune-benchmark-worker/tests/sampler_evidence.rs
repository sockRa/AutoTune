use autotune_benchmark_worker::{
    evidence::{
        decode_envelope, encode_envelope, AcceptPolicy, Admission, Correctness, Coverage,
        NativeCompletion, Provenance, Verdict,
    },
    monitor::Anchor,
    sampler::{
        manifest::{CaptureManifest, Draft},
        producer::{build_envelope, raw_events},
        supervisor::{Input, Supervisor},
        wire::Message,
    },
};
fn manifest() -> CaptureManifest {
    CaptureManifest::freeze(Draft {
        session: [1; 16],
        run: [2; 16],
        clock_token: [3; 16],
        raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
        raw_pci: "0000:01:02.0".into(),
        source_commit: "a".repeat(40),
        source_tree: [4; 32],
        lockfile: [5; 32],
        controller: [6; 32],
        sampler: [7; 32],
        protocol: [8; 32],
        approval_ref: "separate exact approval".into(),
        output_root: "D:\\capture\\run".into(),
    })
    .unwrap()
}
fn anchor(ns: u64) -> Anchor {
    Anchor {
        clock_token: [3; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
#[test]
fn sampler_only_cancelled_export_is_bound_and_never_a_qualified_baseline() {
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    s.advance(Some(anchor(100)), Input::Stop);
    let envelope = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    assert_eq!(envelope.result.admission, Admission::Blocked);
    assert_eq!(envelope.result.correctness, Correctness::NotChecked);
    assert_eq!(envelope.result.native_completion, NativeCompletion::Unknown);
    assert_eq!(envelope.result.coverage, Coverage::Incomplete);
    assert_eq!(envelope.result.verdict, Verdict::Blocked);
    assert!(envelope.dispatches.is_empty());
    assert!(!envelope.result.worker_exit.is_known());
    let bytes = encode_envelope(&envelope).unwrap();
    assert!(decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive).is_err());
    assert!(decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::AllowSimulated).is_ok());
    let raw = raw_events(&s).unwrap();
    assert!(raw.contains("\"kind\":\"stop\""));
}
#[test]
fn actual_getter_anchor_and_duplicate_rejected_record_survive_raw_export() {
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    s.advance(Some(anchor(0)), Input::Launched { creation: 123 });
    let effects = s.advance(
        Some(anchor(2_000)),
        Input::Message(Message::Ready {
            raw_uuid: m.draft().raw_uuid.clone(),
            raw_pci: m.draft().raw_pci.clone(),
            sampler_digest: m.draft().sampler,
            frequency: 1_000_000_000,
            anchor: anchor(1_000),
            runtime: "actual runtime fixture".into(),
        }),
    );
    let ticket = effects
        .into_iter()
        .find_map(|e| match e {
            autotune_benchmark_worker::sampler::supervisor::Effect::Send(Message::Query(t)) => {
                Some(t)
            }
            _ => None,
        })
        .unwrap();
    for receipt in [30_000, 40_000] {
        s.advance(
            Some(anchor(receipt)),
            Input::Message(Message::MetricRecord {
                ticket,
                start: anchor(10_000),
                end: anchor(20_000),
                raw: autotune_probe::report::Field::available("degC", 42),
            }),
        );
    }
    let envelope = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    assert_eq!(envelope.telemetry.len(), 1);
    assert_eq!(envelope.integrity.observed_slots, 1);
    assert_eq!(envelope.integrity.raw_records, 2);
    assert_eq!(
        envelope.telemetry[0].query_start_ns,
        autotune_benchmark_worker::evidence::Obs::Known(10_000)
    );
    assert_eq!(envelope.result.verdict, Verdict::Invalid);
    assert!(encode_envelope(&envelope).is_ok());
    assert_eq!(
        raw_events(&s)
            .unwrap()
            .matches("\"kind\":\"metric_record\"")
            .count(),
        2
    );
}

#[test]
fn late_lifecycle_at_same_timestamp_cannot_populate_primary_fields() {
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    s.advance(Some(anchor(0)), Input::Stop);
    s.advance(Some(anchor(0)), Input::Launched { creation: 123 });
    s.advance(
        Some(anchor(2_000)),
        Input::Message(Message::Ready {
            raw_uuid: m.draft().raw_uuid.clone(),
            raw_pci: m.draft().raw_pci.clone(),
            sampler_digest: m.draft().sampler,
            frequency: 1_000_000_000,
            anchor: anchor(1_000),
            runtime: "late".into(),
        }),
    );
    let e = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    assert!(!e.lifecycle.launch_ns.is_known());
    assert!(!e.lifecycle.ready_ns.is_known());
}

#[test]
fn full_sampler_only_capture_fits_retention_and_exports_without_padding_slots() {
    use autotune_benchmark_worker::sampler::{
        runtime::{Operation, ResourceObservation},
        supervisor::{Effect, ProcessObservation},
    };
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    s.advance(Some(anchor(0)), Input::Launched { creation: 123 });
    let first = s.advance(
        Some(anchor(1_000)),
        Input::Message(Message::Ready {
            raw_uuid: m.draft().raw_uuid.clone(),
            raw_pci: m.draft().raw_pci.clone(),
            sampler_digest: m.draft().sampler,
            frequency: 1_000_000_000,
            anchor: anchor(0),
            runtime: "runtime fixture".into(),
        }),
    );
    let mut pending = first.into_iter().find_map(|e| match e {
        Effect::Send(Message::Query(t)) => Some(t),
        _ => None,
    });
    let resources = ResourceObservation {
        commit_bytes: Some(64 * 1024 * 1024),
        user_100ns: Some(10_000),
        kernel_100ns: Some(10_000),
        memory_error: None,
        time_error: None,
    };
    for tick in 1..=1001u64 {
        let now = tick * 10_000_000;
        let stopped = s.terminal().is_some();
        let process = ProcessObservation {
            creation: 123,
            wait_status: if stopped { 0 } else { 258 },
            wait_error: None,
            exit_code: stopped.then_some(0),
            exit_error: None,
            termination: None,
            active_processes: Some(if stopped { 0 } else { 1 }),
            accounting_error: None,
        };
        let mut effects = s.advance(
            Some(anchor(now)),
            Input::Host {
                watchdog_100ns: now / 100,
                resources,
                sampler_resources: Some(resources),
                process: Some(process),
            },
        );
        if let Some(ticket) = pending.take() {
            let (unit, value) = match ticket.metric {
                autotune_benchmark_worker::monitor::Metric::Temperature => ("degC", 42),
                autotune_benchmark_worker::monitor::Metric::Power => ("mW", 100_000),
                autotune_benchmark_worker::monitor::Metric::GraphicsClock
                | autotune_benchmark_worker::monitor::Metric::MemoryClock => ("MHz", 1500),
                _ => ("%", 10),
            };
            effects.extend(s.advance(
                Some(anchor(now)),
                Input::Message(Message::MetricRecord {
                    ticket,
                    start: anchor(ticket.start_ns + 100),
                    end: anchor(ticket.start_ns + 200),
                    raw: autotune_probe::report::Field::available(unit, value),
                }),
            ));
        }
        for effect in effects {
            if let Effect::Send(Message::Query(t)) = effect {
                assert!(pending.replace(t).is_none());
                s.advance(
                    Some(anchor(now)),
                    Input::Operation {
                        operation: Operation::QuerySend,
                        result: Ok(()),
                    },
                );
            }
        }
    }
    assert_eq!(
        s.terminal().unwrap().reason,
        autotune_benchmark_worker::sampler::supervisor::Fault::Preflight
    );
    assert!(s.exit_confirmed());
    let raw = raw_events(&s).unwrap();
    assert!(!raw.is_empty());
    let mut envelope = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    assert!(envelope.integrity.raw_records >= 240);
    assert_eq!(envelope.integrity.expected_slots, 246);
    assert_eq!(envelope.result.verdict, Verdict::Blocked);
    envelope
        .account_artifacts(
            m.canonical(),
            &autotune_benchmark_worker::sampler::manifest::profile_bytes(),
            &raw,
        )
        .unwrap();
    let encoded = envelope.encode().unwrap();
    assert!(encoded.session_digest().is_some());
}

#[test]
fn exported_session_requires_exact_artifact_byte_accounting_before_encoding() {
    use autotune_benchmark_worker::{
        evidence::Obs,
        sampler::{manifest::profile_bytes, wire::WireError},
    };
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    s.advance(Some(anchor(100)), Input::Stop);
    let mut envelope = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    assert!(matches!(envelope.encode(), Err(WireError::Body)));
    let raw = raw_events(&s).unwrap();
    let profile = profile_bytes();
    envelope
        .account_artifacts(m.canonical(), &profile, &raw)
        .unwrap();
    let expected = (m.canonical().len() + profile.len() + raw.len()) as u64;
    let encoded = envelope.encode().unwrap();
    let decoded = decode_envelope(
        encoded.as_bytes(),
        encoded.len(),
        AcceptPolicy::AllowSimulated,
    )
    .unwrap();
    assert_eq!(decoded.integrity.observed_bytes, expected);
    assert_eq!(decoded.lifecycle.stop_requested_ns, Obs::Known(100));
    // The session and index are deliberately outside this byte scope; the
    // capture index accounts those leaves independently without a hash cycle.
    assert_eq!(envelope.encode().unwrap().as_bytes(), encoded.as_bytes());
}

#[test]
fn writer_failure_retains_operation_signed_os_code_and_unknown_cancellation_completion() {
    use autotune_benchmark_worker::sampler::writer::Failure;
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    s.advance(
        Some(anchor(100)),
        Input::WriterFailure(Failure {
            operation: "raw-events.ndjson",
            os_code: Some(-7),
            kind: std::io::ErrorKind::Other,
        }),
    );
    s.advance(
        Some(anchor(200)),
        Input::WriterShutdown {
            cancellation: Err(995),
            finished: false,
        },
    );
    let raw = raw_events(&s).unwrap();
    assert!(raw.contains("\"operation\":\"raw-events.ndjson\",\"os_code\":\"-7\""));
    assert!(raw.contains("\"kind\":\"writer_shutdown\""));
    assert!(raw.contains("\"cancellation\":\"995\",\"finished\":false"));
    let e = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    assert_eq!(e.result.verdict, Verdict::Invalid);
    assert!(!s.exit_confirmed());
}
#[test]
fn raw_rejection_export_preserves_binary_prefix_and_typed_os_error() {
    use autotune_benchmark_worker::sampler::{
        framing::{Phase, ReadFault, Rejection},
        wire::Lane,
    };
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    s.advance(
        Some(anchor(100)),
        Input::Rejected {
            lane: Lane::Control,
            bytes: vec![255, 0, 1],
            rejection: Rejection {
                fault: ReadFault::Os(995),
                phase: Phase::Prefix,
                reported: 3,
                queue_overflow: false,
            },
        },
    );
    let raw = raw_events(&s).unwrap();
    assert!(raw.contains("\"bytes_hex\":\"ff0001\""));
    assert!(raw.contains("\"os_code\":\"995\""));
}

#[test]
fn invalid_query_order_still_exports_raw_anchors_and_explicit_unknown_projection() {
    let m = manifest();
    let mut s =
        Supervisor::new(m.binding(1_000_000_000).unwrap(), anchor(0), m.selection()).unwrap();
    let ticket = autotune_benchmark_worker::monitor::Ticket {
        binding: s.binding().monitor,
        metric: autotune_benchmark_worker::monitor::Metric::Temperature,
        slot: 0,
        scheduled_ns: 0,
        start_ns: 0,
    };
    s.advance(
        Some(anchor(100)),
        Input::Message(Message::MetricRecord {
            ticket,
            start: anchor(300),
            end: anchor(200),
            raw: autotune_probe::report::Field::available("degC", 42),
        }),
    );
    let mut e = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    assert!(!e.telemetry[0].query_start_ns.is_known());
    assert!(!e.telemetry[0].value.is_known());
    let raw = raw_events(&s).unwrap();
    assert!(raw.contains("\"qpc_before\":\"300\""));
    e.account_artifacts(m.canonical(), "profile", &raw).unwrap();
    assert_eq!(
        e.integrity.observed_bytes as usize,
        m.canonical().len() + 7 + raw.len()
    );
    assert!(e.encode().is_ok());
}
