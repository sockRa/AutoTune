use autotune_benchmark_worker::evidence::{Digest, Domain};
use autotune_benchmark_worker::sampler::manifest::{profile_bytes, CaptureManifest, Draft};
fn draft() -> Draft {
    Draft {
        session: [1; 16],
        run: [2; 16],
        clock_token: [3; 16],
        raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
        raw_pci: "00000000:01:02.0".into(),
        source_commit: "a".repeat(40),
        source_tree: [4; 32],
        lockfile: [5; 32],
        controller: [6; 32],
        sampler: [7; 32],
        protocol: [8; 32],
        approval_ref: "exact separate approval reference".into(),
        output_root: "D:\\captures\\run-02".into(),
    }
}
#[test]
fn canonical_manifest_binds_identity_scope_artifacts_and_approval() {
    let m = CaptureManifest::freeze(draft()).unwrap();
    let bytes = m.canonical();
    let decoded = CaptureManifest::parse(bytes.as_bytes()).unwrap();
    assert_eq!(decoded.canonical(), bytes);
    assert_eq!(
        decoded.digest(),
        Digest::of(Domain::Manifest, bytes.as_bytes())
    );
    assert!(bytes.contains("\"scope\":\"readonly_sampler\""));
    assert_eq!(m.binding(1_000_000_000).unwrap().monitor.device.bus, 1);
    let mut changed = draft();
    changed.approval_ref = "a different approval".into();
    assert_ne!(
        CaptureManifest::freeze(changed).unwrap().digest(),
        m.digest()
    );
    assert_eq!(
        m.profile_digest(),
        Digest::of(Domain::Profile, profile_bytes().as_bytes())
    );
}
#[test]
fn invalid_identity_paths_empty_approval_and_unknown_fields_fail_closed() {
    for mutation in 0..5 {
        let mut d = draft();
        match mutation {
            0 => d.raw_pci = "00000000:01:02.9".into(),
            1 => d.approval_ref.clear(),
            2 => d.output_root = "D:\\captures\\..\\escape".into(),
            3 => d.source_commit = "xyz".into(),
            _ => d.session = [0; 16],
        };
        assert!(CaptureManifest::freeze(d).is_err());
    }
    let m = CaptureManifest::freeze(draft()).unwrap();
    let bad = m.canonical().replace(
        "\"scope\":\"readonly_sampler\"",
        "\"scope\":\"gpu_workload\"",
    );
    assert!(CaptureManifest::parse(bad.as_bytes()).is_err());
    let bad = m.canonical().replace(
        "\"scope\":\"readonly_sampler\"",
        "\"scope\":\"readonly_sampler\",\"extra\":null",
    );
    assert!(CaptureManifest::parse(bad.as_bytes()).is_err());
}
