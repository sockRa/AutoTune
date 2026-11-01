use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{
        manifest::{Artifacts, PilotDraft, PilotManifest},
        settings::{Block, Capture, Comparison, Draft, Observation, Source},
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
