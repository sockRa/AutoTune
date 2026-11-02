use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{
        manifest::{Artifacts, PilotDraft, PilotManifest},
        settings::{Block, Capture, Comparison, Draft, Observation, ReportedCapture, Source},
        settings_transport::{Child, Parent},
    },
};
fn digest(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn manifest(b: &Budget) -> PilotManifest {
    PilotManifest::freeze(
        PilotDraft {
            session: [1; 16],
            runs: [[2; 16], [3; 16], [4; 16]],
            clock_token: [5; 16],
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: digest(6),
                lockfile: digest(7),
                controller: digest(8),
                worker: digest(9),
                sampler: digest(10),
                ptx: digest(11),
                protocol: digest(12),
            },
            starting_evidence: digest(13),
            environment_evidence: digest(14),
            approval_ref: "not execution authority".into(),
            output_root: "D:\\capture\\pilot".into(),
        },
        b,
    )
    .unwrap()
}
fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [5; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
use autotune_probe::{
    nvml::SettingMetric,
    report::{Failure, Field, Reason, SymbolPresence},
};
fn draft() -> Draft {
    Draft {
        raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
        raw_pci: "0000:01:02.0".into(),
        observations: SettingMetric::ALL.map(|kind| Observation {
            kind,
            before: Some(a(1000)),
            after: Some(a(1000)),
            raw: Some(Field::available(
                kind.unit(),
                if kind == SettingMetric::PowerLimit {
                    215000
                } else {
                    0
                },
            )),
        }),
        halt: None,
    }
}
#[test]
fn current_settings_are_bound_raw_facts_not_write_or_native_permission() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let c = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
    assert_eq!(c.source(), Source::SimulatedFixture);
    assert_eq!(c.current().unwrap().power_limit_mw, 215000);
    assert_eq!(c.current().unwrap().graphics_offset_p0_mhz, 0);
    assert!(c
        .canonical()
        .contains("\"scope\":\"pilot_current_settings\""));
    assert!(c.canonical().contains("\"value\":\"215000\""));
    assert_eq!(
        c.digest(),
        Digest::of(
            autotune_benchmark_worker::evidence::Domain::RawArtifact,
            c.canonical().as_bytes()
        )
    );
}
#[test]
fn equality_checks_all_three_current_values_and_rejects_other_manifest_scope() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let first = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
    let other_run = m.bind_run(1, 0).unwrap();
    let mut changed = draft();
    let equal = Capture::fixture(&other_run, 1_000_000_000, a(0), &changed, &b).unwrap();
    assert_eq!(first.compare(&equal), Ok(Comparison::ObservedEqual));
    changed.observations[2].raw = Some(Field::available("MHz", -17));
    let other = Capture::fixture(&other_run, 1_000_000_000, a(0), &changed, &b).unwrap();
    assert_eq!(
        first.compare(&other),
        Ok(Comparison::Different {
            power_limit: false,
            graphics_offset_p0: false,
            memory_offset_p0: true
        })
    );
    let text = m.canonical().replace(&"a".repeat(40), &"b".repeat(40));
    let m2 = PilotManifest::parse(text.as_bytes(), &b).unwrap();
    let run2 = m2.bind_run(0, 0).unwrap();
    let foreign = Capture::fixture(&run2, 1_000_000_000, a(0), &draft(), &b).unwrap();
    assert_eq!(first.compare(&foreign), Err(Block::Scope));
}
#[test]
fn unavailable_fields_retain_raw_reason_status_and_detail_without_zero_fill() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    for reason in [
        Reason::Unsupported,
        Reason::PermissionDenied,
        Reason::GpuLost,
    ] {
        let mut d = draft();
        d.observations[1].raw = Some(Field::unavailable(
            "MHz",
            Failure {
                reason,
                vendor_status: Some(i32::MIN),
                detail: "exact getter failure".into(),
            },
        ));
        let c = Capture::fixture(&run, 1_000_000_000, a(0), &d, &b).unwrap();
        assert!(c.current().is_err());
        assert!(c.canonical().contains("-2147483648"));
        assert!(c.canonical().contains("exact getter failure"));
    }
}
#[test]
fn wrong_clock_tokens_and_missing_anchors_remain_distinct_in_canonical_evidence() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let good = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
    for kind in 0..3 {
        let mut d = draft();
        match kind {
            0 => d.observations[0].before.as_mut().unwrap().clock_token = [99; 16],
            1 => d.observations[0].before = None,
            _ => d.observations[0].after = Some(a(20_000_000_000)),
        };
        let c = Capture::fixture(&run, 1_000_000_000, a(0), &d, &b).unwrap();
        assert!(c.current().is_err());
        assert_ne!(c.digest(), good.digest());
        if kind == 0 {
            assert!(c.canonical().contains(&"63".repeat(16)));
        }
    }
}
#[test]
fn bad_identity_units_symbols_nonintegers_and_reordered_kinds_never_become_values() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    for kind in 0..6 {
        let mut d = draft();
        match kind {
            0 => d.raw_pci = "0000:02:02.0".into(),
            1 => d.observations[0].raw = Some(Field::available("W", 215)),
            2 => d.observations[1].raw.as_mut().unwrap().symbol_presence = SymbolPresence::Absent,
            3 => d.observations[0].raw = Some(Field::available("mW", 1.5)),
            4 => d.observations[0].raw = Some(Field::available("mW", 0)),
            _ => d.observations.swap(0, 1),
        };
        let c = Capture::fixture(&run, 1_000_000_000, a(0), &d, &b).unwrap();
        assert!(c.current().is_err());
        if kind == 3 {
            assert!(c.canonical().contains("float_repr"));
            assert!(c.canonical().contains("\"1.5\""));
        }
    }
}
#[test]
fn capacities_are_reserved_and_oversized_or_complex_input_stays_caller_owned() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let before = b.used();
    let d = draft();
    let c = Capture::fixture(&run, 1_000_000_000, a(0), &d, &b).unwrap();
    assert!(b.used() > before);
    drop(c);
    assert_eq!(b.used(), before);
    let mut bad = draft();
    bad.observations[0].raw = Some(Field::available(
        "mW",
        serde_json::json!({"nested":[1,2,3]}),
    ));
    assert!(matches!(
        Capture::fixture(&run, 1_000_000_000, a(0), &bad, &b),
        Err(Block::Capacity)
    ));
    assert!(bad.observations[0].raw.is_some());
    assert_eq!(b.used(), before);
    assert!(matches!(
        Capture::fixture(&run, 1_000_000_000, a(0), &d, &Budget::new(1024).unwrap()),
        Err(Block::Capacity)
    ));
}

#[test]
fn reported_artifacts_preserve_scalar_types_failures_and_raw_contradictions() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    for case in 0..10 {
        let mut d = draft();
        match case {
            0 => (),
            1 => d.observations[0].before.as_mut().unwrap().clock_token = [99; 16],
            2 => d.observations[1].after = None,
            3 => d.observations[2].raw = None,
            4 => d.observations.swap(0, 1),
            5 => d.observations[0].raw = Some(Field::available("mW", 1.5)),
            6 => d.observations[0].raw = Some(Field::available("mW", true)),
            7 => d.observations[0].raw = Some(Field::available("mW", serde_json::Value::Null)),
            8 => d.observations[0].raw = Some(Field::available("mW", "215000")),
            _ => {
                d.halt = Some(Block::GpuLost);
                d.observations[0].raw = Some(Field::unavailable(
                    "mW",
                    Failure {
                        reason: Reason::GpuLost,
                        vendor_status: Some(i32::MIN),
                        detail: "raw\nloss".into(),
                    },
                ));
            }
        }
        let c = Capture::fixture(&run, 1_000_000_000, a(0), &d, &b).unwrap();
        let r = ReportedCapture::parse(
            &run,
            1_000_000_000,
            a(0),
            c.canonical().as_bytes(),
            c.digest(),
            &b,
        )
        .unwrap();
        assert_eq!(r.canonical(), c.canonical());
        assert_eq!(r.digest(), c.digest());
        assert_eq!(r.current(), c.current());
        assert_eq!(r.reported_source(), Source::SimulatedFixture);
    }
}

fn raw_digest(bytes: &[u8]) -> Digest {
    Digest::of(
        autotune_benchmark_worker::evidence::Domain::RawArtifact,
        bytes,
    )
}
#[test]
fn reported_reader_rejects_schema_binding_digest_and_canonical_tampering() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let c = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
    for text in [
        c.canonical()
            .replace("\"version\":\"1\"", "\"version\":\"2\""),
        c.canonical()
            .replace("pilot_current_settings", "readonly-sampler"),
        c.canonical()
            .replace("\"value\":\"215000\"", "\"value\":\"0215000\""),
        c.canonical()
            .replace("\"kind\":\"integer\"", "\"kind\":\"float_repr\""),
        c.canonical()
            .replacen("\"version\":\"1\"", "\"version\":\"1\",\"extra\":null", 1),
        format!("{} ", c.canonical()),
    ] {
        assert!(ReportedCapture::parse(
            &run,
            1_000_000_000,
            a(0),
            text.as_bytes(),
            raw_digest(text.as_bytes()),
            &b
        )
        .is_err());
    }
    assert!(ReportedCapture::parse(
        &run,
        1_000_000_000,
        a(0),
        c.canonical().as_bytes(),
        digest(99),
        &b
    )
    .is_err());
    let other = m.bind_run(1, 0).unwrap();
    assert!(ReportedCapture::parse(
        &other,
        1_000_000_000,
        a(0),
        c.canonical().as_bytes(),
        c.digest(),
        &b
    )
    .is_err());
    // A native path text can be claimed by a sender. The result remains the
    // distinct ReportedCapture type, with no conversion into a native Capture.
    let claim = c
        .canonical()
        .replace("simulated_fixture", "native_getter_path");
    let r = ReportedCapture::parse(
        &run,
        1_000_000_000,
        a(0),
        claim.as_bytes(),
        raw_digest(claim.as_bytes()),
        &b,
    )
    .unwrap();
    assert_eq!(r.reported_source(), Source::NativeGetterPath);
}

#[test]
fn settings_exchange_roundtrips_to_parent_with_exact_request_and_raw_export() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let baseline = b.used();
    {
        let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        let request_frame = parent.issue(a(500)).unwrap();
        let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
        let mut request = child.accept(request_frame.as_bytes()).unwrap();
        assert_eq!(request.issued(), a(500));
        assert!(child.accept(request_frame.as_bytes()).is_err());
        let c = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
        let response = request.respond(&c).unwrap();
        assert!(request.respond(&c).is_err());
        let receipt = parent.receive(&run, response.as_bytes(), a(2000)).unwrap();
        assert_eq!(receipt.fault(), None);
        assert_eq!(receipt.capture().digest(), c.digest());
        assert_eq!(receipt.capture().current(), c.current());
        assert_eq!(receipt.frame(), response.as_bytes());
        assert!(parent.receive(&run, response.as_bytes(), a(3000)).is_err());
        assert!(parent.issue(a(4000)).is_err());
    }
    assert_eq!(b.used(), baseline);
}

#[test]
fn late_stopped_and_contradictory_responses_are_retained_but_never_clear_faults() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    for case in 0..6 {
        let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
        let frame = parent.issue(a(500)).unwrap();
        let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
        let mut request = child.accept(frame.as_bytes()).unwrap();
        let mut d = draft();
        let mut received = a(2000);
        match case {
            0 => {
                assert_eq!(parent.tick(a(10_000_000_000)), Err(Block::Phase));
                received = a(10_000_000_100);
            }
            1 => parent.stop(),
            2 => d.observations[0].before = Some(a(400)),
            3 => d.observations[2].after = Some(a(3000)),
            4 => d.observations[0].before.as_mut().unwrap().clock_token = [88; 16],
            _ => {
                d.observations[0].raw =
                    Some(Field::unavailable("mW", Failure::vendor(15, "raw lost")))
            }
        }
        let c = Capture::fixture(&run, 1_000_000_000, a(0), &d, &b).unwrap();
        let response = request.respond(&c).unwrap();
        let receipt = parent.receive(&run, response.as_bytes(), received).unwrap();
        assert!(receipt.fault().is_some());
        assert_eq!(receipt.capture().canonical(), c.canonical());
        assert_eq!(receipt.frame(), response.as_bytes());
        assert_eq!(parent.fault(), receipt.fault());
    }
}

#[test]
fn settings_exchange_rejects_foreign_response_and_poisoned_attempts_without_retry() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    let frame = parent.issue(a(500)).unwrap();
    let mut other = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    let other_frame = other.issue(a(600)).unwrap();
    let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
    let mut request = child.accept(other_frame.as_bytes()).unwrap();
    let c = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
    let response = request.respond(&c).unwrap();
    assert!(parent.receive(&run, response.as_bytes(), a(2000)).is_err());
    assert!(parent.receive(&run, response.as_bytes(), a(2000)).is_err());
    let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
    assert!(child.accept(&[4, 0, 0, 0]).is_err());
    assert!(child.accept(frame.as_bytes()).is_err());
    let other_run = m.bind_run(1, 0).unwrap();
    let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
    let mut request = child.accept(frame.as_bytes()).unwrap();
    let foreign = Capture::fixture(&other_run, 1_000_000_000, a(0), &draft(), &b).unwrap();
    assert!(request.respond(&foreign).is_err());
    assert!(request.respond(&c).is_err());
}

#[test]
fn receive_capacity_failure_returns_raw_bytes_and_preserves_accounting() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let c = Capture::fixture(&run, 1_000_000_000, a(0), &draft(), &b).unwrap();
    let small = Budget::new(1024).unwrap();
    let baseline = small.used();
    assert!(matches!(
        ReportedCapture::parse(
            &run,
            1_000_000_000,
            a(0),
            c.canonical().as_bytes(),
            c.digest(),
            &small
        ),
        Err(Block::Capacity)
    ));
    assert_eq!(small.used(), baseline);
    assert!(c.canonical().contains("215000"));
    let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(0), b.clone()).unwrap();
    let request = parent.issue(a(500)).unwrap();
    let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
    let response = child
        .accept(request.as_bytes())
        .unwrap()
        .respond(&c)
        .unwrap();
    let pressure = b.reserve(8 * 1024 * 1024 - b.used() - 500).unwrap();
    let held = b.used();
    assert!(matches!(
        parent.receive(&run, response.as_bytes(), a(2000)),
        Err(Block::Capacity)
    ));
    assert_eq!(b.used(), held);
    drop(pressure);
    assert!(parent.receive(&run, response.as_bytes(), a(2000)).is_err());
    assert!(!response.as_bytes().is_empty());
}

#[test]
fn child_and_parent_preserve_the_original_relative_and_cumulative_setup_deadline() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    for (entry, expected_deadline) in [(1000, 10_000_001_000), (15_000_000_000, 20_000_000_000)] {
        let mut parent = Parent::new(&run, 1_000_000_000, a(0), a(entry), b.clone()).unwrap();
        let frame = parent.issue(a(entry)).unwrap();
        let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
        assert!(child.accept(frame.as_bytes()).is_ok());
        assert_eq!(parent.tick(a(expected_deadline)), Err(Block::Phase));
        let text = std::str::from_utf8(&frame.as_bytes()[4..]).unwrap();
        let changed = text.replace(
            &format!("\"deadline_ns\":\"{expected_deadline}\""),
            &format!("\"deadline_ns\":\"{}\"", expected_deadline + 1),
        );
        assert_ne!(changed, text);
        let mut bytes = ((changed.len() + 4) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(changed.as_bytes());
        let mut child = Child::new(&run, 1_000_000_000, a(0), b.clone()).unwrap();
        assert!(child.accept(&bytes).is_err());
        assert!(child.accept(frame.as_bytes()).is_err());
    }
}
