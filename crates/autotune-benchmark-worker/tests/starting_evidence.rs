use autotune_benchmark_worker::{
    evidence::{Digest, Domain},
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{
        manifest::{Artifacts, PilotDraft, PilotManifest},
        settings::{Block, Draft, Observation, Source},
        starting::{ReportedSnapshot, Scope, ScopeDraft, Snapshot},
    },
};
use autotune_probe::{
    nvml::SettingMetric,
    report::{Failure, Field},
};
fn d(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [3; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn scope(b: &Budget) -> Scope {
    Scope::freeze(
        ScopeDraft {
            session: [1; 16],
            capture_nonce: [2; 16],
            clock_token: [3; 16],
            frequency: 1_000_000_000,
            epoch: a(0),
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: d(4),
                lockfile: d(5),
                controller: d(6),
                worker: d(7),
                sampler: d(8),
                ptx: d(9),
                protocol: d(10),
            },
            read_approval_ref: "read declaration; not compute consent".into(),
        },
        b,
    )
    .unwrap()
}
fn draft() -> Draft {
    Draft {
        raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
        raw_pci: "0000:01:02.0".into(),
        observations: SettingMetric::ALL.map(|kind| Observation {
            kind,
            before: Some(a(2000 + kind as u64 * 2000)),
            after: Some(a(3000 + kind as u64 * 2000)),
            raw: Some(Field::available(
                kind.unit(),
                match kind {
                    SettingMetric::PowerLimit => 215000,
                    SettingMetric::GraphicsOffsetP0 => -100,
                    SettingMetric::MemoryOffsetP0 => 200,
                },
            )),
        }),
        halt: None,
    }
}
fn pilot(s: &Scope, digest: Digest) -> PilotDraft {
    let s = s.draft();
    PilotDraft {
        session: s.session,
        runs: [[11; 16], [12; 16], [13; 16]],
        clock_token: s.clock_token,
        raw_uuid: s.raw_uuid.clone(),
        raw_pci: s.raw_pci.clone(),
        source_commit: s.source_commit.clone(),
        artifacts: s.artifacts,
        starting_evidence: digest,
        environment_evidence: d(14),
        approval_ref: "separate compute declaration; no approval inferred".into(),
        output_root: "D:\\capture\\pilot".into(),
    }
}
/// Fabricated peer source claim exercises parsing; it proves no native read.
fn reported(s: &Scope, b: &Budget) -> ReportedSnapshot {
    let snapshot = Snapshot::fixture(s, a(1000), &draft(), b).unwrap();
    let bytes = snapshot
        .canonical()
        .replace("simulated_fixture", "native_getter_path");
    ReportedSnapshot::parse(
        s,
        bytes.as_bytes(),
        Digest::of(Domain::RawArtifact, bytes.as_bytes()),
        b,
    )
    .unwrap()
}
#[test]
fn starting_scope_has_no_run_or_manifest_placeholder_and_roundtrips_exactly() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    assert!(!s.canonical().contains("\"run\""));
    assert!(!s.canonical().contains("starting_evidence"));
    assert_eq!(
        Scope::parse(s.canonical().as_bytes(), &b).unwrap().digest(),
        s.digest()
    );
    assert_eq!(s.draft().epoch, a(0));
}
#[test]
fn fixture_readability_does_not_become_native_or_compute_authority() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    let snapshot = Snapshot::fixture(&s, a(1000), &draft(), &b).unwrap();
    assert_eq!(snapshot.source(), Source::SimulatedFixture);
    assert_eq!(snapshot.current().unwrap().graphics_offset_p0_mhz, -100);
    let r = ReportedSnapshot::parse(&s, snapshot.canonical().as_bytes(), snapshot.digest(), &b)
        .unwrap();
    assert!(matches!(
        r.freeze_manifest(pilot(&s, snapshot.digest()), 1_000_000_000, a(0), &b),
        Err(Block::Scope)
    ));
}
#[test]
fn reported_starting_artifact_can_bind_a_declaration_without_faking_an_earlier_manifest() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    let r = reported(&s, &b);
    let m = r
        .freeze_manifest(pilot(&s, r.snapshot().digest()), 1_000_000_000, a(0), &b)
        .unwrap();
    let run = m.bind_run(0, 8000).unwrap();
    assert_eq!(
        r.check_run(&run, 1_000_000_000, a(0))
            .unwrap()
            .memory_offset_p0_mhz,
        200
    );
    let early = m.bind_run(0, 6000).unwrap();
    assert_eq!(r.check_run(&early, 1_000_000_000, a(0)), Err(Block::Phase));
    assert!(m.canonical().contains(&r.snapshot().digest().to_hex()));
}
#[test]
fn all_scope_references_and_original_clock_basis_must_match_the_declaration() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    let r = reported(&s, &b);
    for which in 0..8 {
        let mut p = pilot(&s, r.snapshot().digest());
        let mut epoch = a(0);
        let mut frequency = 1_000_000_000;
        match which {
            0 => p.starting_evidence = d(77),
            1 => p.raw_pci = "0000:01:02.1".into(),
            2 => p.source_commit = "b".repeat(40),
            3 => p.artifacts.worker = d(77),
            4 => p.session = [77; 16],
            5 => p.runs[0] = s.draft().capture_nonce,
            6 => epoch = a(100),
            _ => frequency = 999_000_000,
        };
        assert!(r.check_manifest(&p, frequency, epoch).is_err());
    }
}
#[test]
fn digest_scope_canonical_and_duplicate_key_rejections_keep_borrowed_bytes() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    let snapshot = Snapshot::fixture(&s, a(1000), &draft(), &b).unwrap();
    let bytes = snapshot.canonical().as_bytes();
    assert!(ReportedSnapshot::parse(&s, bytes, d(77), &b).is_err());
    let tampered = snapshot
        .canonical()
        .replace("\"version\":\"1\"", "\"version\":\"1\",\"version\":\"1\"");
    assert!(ReportedSnapshot::parse(
        &s,
        tampered.as_bytes(),
        Digest::of(Domain::RawArtifact, tampered.as_bytes()),
        &b
    )
    .is_err());
    let mut other = s.draft().clone();
    other.capture_nonce = [77; 16];
    let other = Scope::freeze(other, &b).unwrap();
    assert!(ReportedSnapshot::parse(&other, bytes, snapshot.digest(), &b).is_err());
    assert_eq!(bytes, snapshot.canonical().as_bytes());
}
#[test]
fn raw_failures_fractional_values_and_foreign_clocks_roundtrip_without_repair() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    for which in 0..3 {
        let mut raw = draft();
        match which {
            0 => {
                raw.observations[1].raw = Some(Field::unavailable(
                    "MHz",
                    Failure::vendor(15, "getter returned"),
                ))
            }
            1 => raw.observations[1].raw = Some(Field::available("MHz", 1.5)),
            _ => {
                raw.observations[1].after = Some(Anchor {
                    clock_token: [77; 16],
                    ..a(5000)
                })
            }
        };
        let snapshot = Snapshot::fixture(&s, a(1000), &raw, &b).unwrap();
        let r = ReportedSnapshot::parse(&s, snapshot.canonical().as_bytes(), snapshot.digest(), &b)
            .unwrap();
        assert!(r.snapshot().current().is_err());
        assert_eq!(
            r.snapshot().draft().observations[1].raw,
            raw.observations[1].raw
        );
        assert_eq!(
            r.snapshot().draft().observations[1].after,
            raw.observations[1].after
        );
    }
}
#[test]
fn preflight_equality_and_entry_chronology_are_checked_without_rebasing() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    let mut raw = draft();
    raw.observations[2].after = Some(a(10_000_000_000));
    let snapshot = Snapshot::fixture(&s, a(1000), &raw, &b).unwrap();
    assert_eq!(snapshot.current(), Err(Block::Phase));
    let snapshot = Snapshot::fixture(&s, a(5000), &draft(), &b).unwrap();
    assert!(snapshot.current().is_err());
}
#[test]
fn unchanged_pilot_profile_bytes_are_shared_with_starting_scope() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let s = scope(&b);
    let m = PilotManifest::freeze(pilot(&s, d(15)), &b).unwrap();
    assert!(s.canonical().contains(&m.profile_digest().to_hex()));
    let held = b.used();
    let _pressure = b.reserve(8 * 1024 * 1024 - held).unwrap();
    assert!(matches!(
        Snapshot::fixture(&s, a(1000), &draft(), &b),
        Err(Block::Capacity)
    ));
}
