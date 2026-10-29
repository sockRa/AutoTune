use autotune_benchmark_worker::{
    evidence::{decode, Digest, Domain},
    sampler::budget::Budget,
    workload::manifest::{Artifacts, PilotDraft, PilotManifest},
};
fn digest(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn draft() -> PilotDraft {
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
        approval_ref: "separate exact workload permission required".into(),
        output_root: "D:\\capture\\pilot".into(),
    }
}
#[test]
fn pilot_preserves_all_three_frozen_plans_and_is_not_readonly_or_acceptance_scope() {
    let budget = Budget::new(2 * 1024 * 1024).unwrap();
    let m = PilotManifest::freeze(draft(), &budget).unwrap();
    let raw = decode(m.canonical().as_bytes(), 16384).unwrap();
    let object = raw.as_object().unwrap();
    assert_eq!(
        object.get("scope").unwrap().as_text(),
        Some("benchmark_pilot")
    );
    let runs = object.get("runs").unwrap().as_array().unwrap();
    assert_eq!(runs.len(), 3);
    for (i, (lanes, steps)) in [(65536, 4096), (262144, 16384), (262144, 65536)]
        .into_iter()
        .enumerate()
    {
        let r = runs[i].as_object().unwrap();
        assert_eq!(r.get("seed").unwrap().as_u64().unwrap(), i as u64);
        assert_eq!(r.get("lanes").unwrap().as_u64().unwrap(), lanes);
        assert_eq!(r.get("steps").unwrap().as_u64().unwrap(), steps);
        assert_eq!(r.get("warmup_dispatches").unwrap().as_u64().unwrap(), 128);
        assert_eq!(r.get("measured_dispatches").unwrap().as_u64().unwrap(), 256);
    }
    assert_eq!(
        m.digest(),
        Digest::of(Domain::Manifest, m.canonical().as_bytes())
    );
    assert_eq!(
        m.profile_digest(),
        Digest::of(Domain::Profile, m.profile_canonical().as_bytes())
    );
    let profile = decode(m.profile_canonical().as_bytes(), 16384).unwrap();
    let plans = profile
        .as_object()
        .unwrap()
        .get("plans")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(plans
        .iter()
        .all(|p| p.as_object().unwrap().get("run").is_none()));
    assert!(budget.used() > m.canonical().len());
    drop(m);
    assert_eq!(budget.used(), 0);
}

#[test]
fn every_execution_identity_and_artifact_change_requires_a_different_manifest_digest() {
    let budget = Budget::new(2 * 1024 * 1024).unwrap();
    let original = PilotManifest::freeze(draft(), &budget).unwrap();
    let mut variants = Vec::new();
    let mut d = draft();
    d.session = [20; 16];
    variants.push(d);
    let mut d = draft();
    d.runs[1] = [21; 16];
    variants.push(d);
    let mut d = draft();
    d.clock_token = [22; 16];
    variants.push(d);
    let mut d = draft();
    d.raw_pci = "0000:01:02.1".into();
    variants.push(d);
    let mut d = draft();
    d.source_commit = "b".repeat(40);
    variants.push(d);
    let mut d = draft();
    d.artifacts.worker = digest(23);
    variants.push(d);
    let mut d = draft();
    d.artifacts.ptx = digest(24);
    variants.push(d);
    let mut d = draft();
    d.starting_evidence = digest(25);
    variants.push(d);
    let mut d = draft();
    d.environment_evidence = digest(26);
    variants.push(d);
    let mut d = draft();
    d.approval_ref = "different exact scope".into();
    variants.push(d);
    let mut d = draft();
    d.output_root = "D:\\capture\\different".into();
    variants.push(d);
    for d in variants {
        let changed = PilotManifest::freeze(d, &budget).unwrap();
        assert_ne!(changed.digest(), original.digest());
        assert_eq!(changed.profile_digest(), original.profile_digest());
    }
}
#[test]
fn malformed_canonical_inputs_fail_without_leaking_codec_or_model_reservations() {
    let budget = Budget::new(2 * 1024 * 1024).unwrap();
    let m = PilotManifest::freeze(draft(), &budget).unwrap();
    let charged = budget.used();
    for bytes in [
        m.canonical().replacen(
            "\"version\":\"1\"",
            "\"version\":\"1\",\"version\":\"1\"",
            1,
        ),
        format!("{} ", m.canonical()),
        m.canonical().replacen("\"seed\":\"0\"", "\"seed\":0", 1),
        m.canonical()
            .replacen("\"seed\":\"0\"", "\"seed\":\"00\"", 1),
        m.canonical()
            .replacen("\"profile\":\"", "\"profile\":\"f", 1),
        m.canonical()[..m.canonical().len() - 1].into(),
        "[".repeat(16385),
    ] {
        assert!(PilotManifest::parse(bytes.as_bytes(), &budget).is_err());
        assert_eq!(budget.used(), charged);
    }
    assert!(PilotManifest::parse(m.canonical().as_bytes(), &Budget::new(1024).unwrap()).is_err());
}
#[test]
fn bound_run_retains_original_device_clock_artifacts_and_reserves_every_remaining_stage() {
    let budget = Budget::new(2 * 1024 * 1024).unwrap();
    let m = PilotManifest::freeze(draft(), &budget).unwrap();
    let r = m.bind_run(1, 45_000_000_000).unwrap();
    let b = r.binding(10_000_000).unwrap();
    assert_eq!(b.session, [1; 16]);
    assert_eq!(b.run, [3; 16]);
    assert_eq!(b.clock_token, [5; 16]);
    assert_eq!(b.qpc_frequency, 10_000_000);
    assert_eq!(r.manifest_digest(), m.digest());
    assert_eq!(r.worker_digest(), digest(9));
    assert_eq!(r.sampler_digest(), digest(10));
    assert!(r.binding(0).is_err());
    // All three 45 s envelopes plus 10 s finalization and 8 s shutdown fit
    // exactly at origin 27 s. A single ns more cannot consume later stages.
    assert!(m.bind_run(0, 27_000_000_000).is_ok());
    assert!(m.bind_run(0, 27_000_000_001).is_err());
    assert!(m.bind_run(2, 117_000_000_000).is_ok());
    assert!(m.bind_run(2, 117_000_000_001).is_err());
}
#[test]
fn rejects_reused_ids_unbound_artifacts_and_path_or_allocation_abuse() {
    let budget = Budget::new(2 * 1024 * 1024).unwrap();
    let mut reused = draft();
    reused.runs[2] = reused.runs[0];
    assert!(PilotManifest::freeze(reused, &budget).is_err());
    let mut zero = draft();
    zero.artifacts.worker = digest(0);
    assert!(PilotManifest::freeze(zero, &budget).is_err());
    let mut path = draft();
    path.output_root = "D:\\capture\\..\\other".into();
    assert!(PilotManifest::freeze(path, &budget).is_err());
    let mut oversized = draft();
    oversized.approval_ref = "x".repeat(16385);
    assert!(PilotManifest::freeze(oversized, &budget).is_err());
    assert!(PilotManifest::freeze(draft(), &Budget::new(100).unwrap()).is_err());
    assert_eq!(budget.used(), 0);
}
#[test]
fn strict_roundtrip_rejects_mutated_plan_scope_and_original_phase_deadlines_do_not_rebase() {
    use autotune_fma_reference::profile::Phase;
    let budget = Budget::new(2 * 1024 * 1024).unwrap();
    let m = PilotManifest::freeze(draft(), &budget).unwrap();
    let parsed = PilotManifest::parse(m.canonical().as_bytes(), &budget).unwrap();
    assert_eq!(parsed.digest(), m.digest());
    for (from, to) in [
        ("benchmark_pilot", "readonly_sampler"),
        ("\"128\"", "\"127\""),
    ] {
        let bad = m.canonical().replacen(from, to, 1);
        assert!(PilotManifest::parse(bad.as_bytes(), &budget).is_err());
    }
    let r = m.bind_run(0, 1_000_000_000).unwrap();
    assert_eq!(r.run(), [2; 16]);
    assert_eq!(
        r.phase_deadline(Phase::Setup, 11_000_000_000).unwrap(),
        21_000_000_000
    );
    assert_eq!(
        r.phase_deadline(Phase::Setup, 20_000_000_000).unwrap(),
        21_000_000_000
    );
    assert!(r.phase_deadline(Phase::Setup, 999_999_999).is_err());
    assert!(m.bind_run(3, 0).is_err());
    assert!(m.bind_run(0, u64::MAX).is_err());
}
