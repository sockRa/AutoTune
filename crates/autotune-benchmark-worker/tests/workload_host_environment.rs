use autotune_benchmark_worker::{
    evidence::{Digest, Domain},
    monitor::Anchor,
    sampler::budget::{Budget, Reservation},
    workload::{
        host_environment::{
            BoolReturn, Conditions, Fault, Operation, Raw, Reader, Reported, Snapshot,
        },
        manifest::{Artifacts, PilotDraft, PilotManifest},
        parent_sampler::{Fault as ParentFault, ParentSampler},
        settings::{Draft, Observation},
        starting::{ReportedSnapshot, Scope, ScopeDraft, Snapshot as Starting},
    },
};
use autotune_probe::{nvml::SettingMetric, report::Field};
const LIMIT: usize = 24 * 1024 * 1024;
include!("support/parent_context.rs");
fn budget() -> Budget {
    Budget::new(LIMIT).unwrap()
}
fn a(n: u64) -> Anchor {
    Anchor {
        clock_token: [3; 16],
        qpc_before: n,
        qpc_after: n,
        interrupt_100ns: n / 100,
        unbiased_100ns: n / 100,
    }
}
fn d(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
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
            read_approval_ref: "CPU fixture only".into(),
        },
        b,
    )
    .unwrap()
}
fn conditions() -> Conditions {
    Conditions {
        asserted_at: a(100),
        background_load: Some(false),
        competing_tuner: Some(false),
        reference: "retained owner assertion; not execution consent".into(),
    }
}
fn ok() -> BoolReturn {
    BoolReturn {
        returned: 1,
        error: None,
    }
}
fn raws() -> [Raw; 5] {
    [
        Raw::Version {
            status: 0,
            major: 10,
            minor: 0,
            build: 26100,
            platform: 2,
        },
        Raw::Architecture {
            call: ok(),
            native: 0x8664,
            process_machine: 0,
            process_x64: true,
        },
        Raw::Elevation {
            open: ok(),
            query: Some(ok()),
            returned_bytes: 4,
            elevated: 0,
            close: Some(ok()),
        },
        Raw::PowerStatus {
            call: ok(),
            ac: 1,
            battery_flags: 128,
            battery_percent: 255,
            saver: 0,
            lifetime: u32::MAX,
            full_lifetime: u32::MAX,
        },
        Raw::PowerScheme {
            status: 0,
            pointer_present: true,
            guid: Some([0x12; 16]),
            released: Some(true),
            release_error: None,
        },
    ]
}
struct Fake {
    raw: [Raw; 5],
    calls: usize,
    pressure: Option<(Budget, Reservation)>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            raw: raws(),
            calls: 0,
            pressure: None,
        }
    }
}
impl Reader for Fake {
    fn read(&mut self, _: Operation) -> Raw {
        let raw = self.raw[self.calls];
        self.calls += 1;
        raw
    }
}
fn collect(s: &Scope, c: Conditions, f: &mut impl Reader, b: &Budget) -> Snapshot {
    let mut n = 1000;
    Snapshot::collect_fixture(
        s,
        c,
        f,
        || {
            n += 100;
            Some(a(n))
        },
        || false,
        b,
    )
    .unwrap()
}
fn reported(s: &Scope, snapshot: &Snapshot, b: &Budget) -> Reported {
    Reported::read(s, snapshot.canonical().as_bytes(), snapshot.digest(), b).unwrap()
}
/// Model untrusted peer source claims without executing the Windows path.
fn claimed(s: &Scope, snapshot: &Snapshot, b: &Budget) -> Reported {
    let bytes = snapshot.canonical().replace(
        "\"source\":\"simulated_fixture\"",
        "\"source\":\"windows_api\"",
    );
    Reported::read(
        s,
        bytes.as_bytes(),
        Digest::of(Domain::RawArtifact, bytes.as_bytes()),
        b,
    )
    .unwrap()
}
fn pilot(s: &Scope, env: Digest, starting: Digest) -> PilotDraft {
    let q = s.draft();
    PilotDraft {
        session: q.session,
        runs: [[11; 16], [12; 16], [13; 16]],
        clock_token: q.clock_token,
        raw_uuid: q.raw_uuid.clone(),
        raw_pci: q.raw_pci.clone(),
        source_commit: q.source_commit.clone(),
        artifacts: q.artifacts,
        starting_evidence: starting,
        environment_evidence: env,
        approval_ref: "declaration only".into(),
        output_root: "D:\\capture\\pilot".into(),
    }
}
#[test]
fn raw_host_facts_roundtrip_without_native_provenance_or_consent() {
    let b = budget();
    let s = scope(&b);
    let mut f = Fake::default();
    let snap = collect(&s, conditions(), &mut f, &b);
    assert_eq!(f.calls, 5);
    assert_eq!(snap.halt(), None);
    assert_eq!(snap.host_facts(), Ok(()));
    assert!(snap.canonical().contains("\"lifetime\":\"4294967295\""));
    assert!(snap.canonical().contains("\"source\":\"owner_assertion\""));
    let read = reported(&s, &snap, &b);
    assert_eq!(read.snapshot().canonical(), snap.canonical());
    assert_eq!(
        read.snapshot()
            .observations()
            .map(|o| o.raw.unwrap())
            .collect::<Vec<_>>(),
        raws()
    );
    assert_eq!(
        snap.check_manifest(&pilot(&s, snap.digest(), d(20)), s.draft().frequency, a(0)),
        Err(Fault::Simulated)
    );
}
#[test]
fn api_failures_and_uncertain_cleanup_remain_raw_and_never_become_complete_facts() {
    let failed = BoolReturn {
        returned: 0,
        error: Some(5),
    };
    let cases = [
        (
            0,
            Raw::Version {
                status: -1073741823,
                major: 0,
                minor: 0,
                build: 0,
                platform: 0,
            },
            Fault::Missing,
        ),
        (
            1,
            Raw::Architecture {
                call: ok(),
                native: 0xaa64,
                process_machine: 0x8664,
                process_x64: true,
            },
            Fault::Architecture,
        ),
        (
            1,
            Raw::Architecture {
                call: failed,
                native: 0x8664,
                process_machine: 0,
                process_x64: true,
            },
            Fault::Architecture,
        ),
        (
            1,
            Raw::Architecture {
                call: ok(),
                native: 0x8664,
                process_machine: 0x14c,
                process_x64: false,
            },
            Fault::Architecture,
        ),
        (
            2,
            Raw::Elevation {
                open: failed,
                query: None,
                returned_bytes: 0,
                elevated: 0,
                close: None,
            },
            Fault::Missing,
        ),
        (
            2,
            Raw::Elevation {
                open: ok(),
                query: Some(failed),
                returned_bytes: 4,
                elevated: 0,
                close: Some(ok()),
            },
            Fault::Missing,
        ),
        (
            2,
            Raw::Elevation {
                open: ok(),
                query: Some(ok()),
                returned_bytes: 3,
                elevated: 0,
                close: Some(ok()),
            },
            Fault::Missing,
        ),
        (
            2,
            Raw::Elevation {
                open: ok(),
                query: Some(ok()),
                returned_bytes: 4,
                elevated: 1,
                close: Some(ok()),
            },
            Fault::Elevated,
        ),
        (
            2,
            Raw::Elevation {
                open: ok(),
                query: Some(ok()),
                returned_bytes: 4,
                elevated: 0,
                close: Some(failed),
            },
            Fault::Missing,
        ),
        (
            3,
            Raw::PowerStatus {
                call: failed,
                ac: 1,
                battery_flags: 0,
                battery_percent: 0,
                saver: 0,
                lifetime: 0,
                full_lifetime: 0,
            },
            Fault::Missing,
        ),
        (
            3,
            Raw::PowerStatus {
                call: ok(),
                ac: 255,
                battery_flags: 255,
                battery_percent: 255,
                saver: 0,
                lifetime: 0,
                full_lifetime: 0,
            },
            Fault::Missing,
        ),
        (
            3,
            Raw::PowerStatus {
                call: ok(),
                ac: 1,
                battery_flags: 128,
                battery_percent: 255,
                saver: 255,
                lifetime: u32::MAX,
                full_lifetime: u32::MAX,
            },
            Fault::Missing,
        ),
        (
            4,
            Raw::PowerScheme {
                status: 5,
                pointer_present: false,
                guid: None,
                released: None,
                release_error: None,
            },
            Fault::Missing,
        ),
        (
            4,
            Raw::PowerScheme {
                status: 0,
                pointer_present: false,
                guid: None,
                released: None,
                release_error: None,
            },
            Fault::Missing,
        ),
        (
            4,
            Raw::PowerScheme {
                status: 0,
                pointer_present: true,
                guid: Some([1; 16]),
                released: Some(false),
                release_error: Some(6),
            },
            Fault::Missing,
        ),
    ];
    for (index, raw, expected) in cases {
        let b = budget();
        let s = scope(&b);
        let mut f = Fake::default();
        f.raw[index] = raw;
        let snap = collect(&s, conditions(), &mut f, &b);
        assert_eq!(f.calls, 5);
        assert_eq!(snap.host_facts(), Err(expected));
        assert_eq!(snap.observations().nth(index).unwrap().raw, Some(raw));
        assert_eq!(
            reported(&s, &snap, &b).snapshot().host_facts(),
            Err(expected)
        );
    }
}
#[test]
fn every_clock_boundary_retains_the_failed_anchor_and_blocks_successor_reads() {
    for fail_at in 0usize..10 {
        for fault in 0..4 {
            let b = budget();
            let s = scope(&b);
            let mut f = Fake::default();
            let mut index = 0;
            let snap = Snapshot::collect_fixture(
                &s,
                conditions(),
                &mut f,
                || {
                    let i = index;
                    index += 1;
                    if i == fail_at {
                        match fault {
                            0 => None,
                            1 => Some(a(10_000_000_000)),
                            2 => {
                                let mut v = a(0);
                                v.qpc_before = 1;
                                Some(v)
                            }
                            _ => {
                                let mut v = a(2000);
                                v.clock_token = [9; 16];
                                Some(v)
                            }
                        }
                    } else {
                        Some(a(1000 + i as u64 * 100))
                    }
                },
                || false,
                &b,
            )
            .unwrap();
            let expected = if fault == 1 {
                Fault::Deadline
            } else {
                Fault::Clock
            };
            assert_eq!(snap.halt(), Some(expected));
            assert_eq!(f.calls, fail_at.div_ceil(2));
            let last = snap.observations().last().unwrap();
            assert_eq!(last.raw.is_some(), fail_at % 2 == 1);
            assert_eq!(reported(&s, &snap, &b).snapshot().halt(), snap.halt());
        }
    }
}
#[test]
fn stop_at_each_pre_or_post_read_boundary_never_loses_return_or_starts_successor() {
    for stop_at in 0usize..10 {
        let b = budget();
        let s = scope(&b);
        let mut f = Fake::default();
        let mut n = 1000;
        let mut count = 0;
        let snap = Snapshot::collect_fixture(
            &s,
            conditions(),
            &mut f,
            || {
                n += 100;
                Some(a(n))
            },
            || {
                let i = count;
                count += 1;
                i == stop_at
            },
            &b,
        )
        .unwrap();
        assert_eq!(snap.halt(), Some(Fault::Stopped));
        assert_eq!(f.calls, stop_at.div_ceil(2));
        assert_eq!(
            snap.observations().last().unwrap().raw.is_some(),
            stop_at % 2 == 1
        );
        assert_eq!(
            reported(&s, &snap, &b).snapshot().halt(),
            Some(Fault::Stopped)
        );
    }
}
#[test]
fn reservation_failure_precedes_reads_and_mid_read_pressure_cannot_erase_artifact() {
    let b = budget();
    let s = scope(&b);
    let mut f = Fake::default();
    let tiny = Budget::new(1).unwrap();
    assert!(matches!(
        Snapshot::collect_fixture(
            &s,
            conditions(),
            &mut f,
            || panic!("no clock before reservation"),
            || panic!("no stop before reservation"),
            &tiny
        ),
        Err(Fault::Capacity)
    ));
    assert_eq!(f.calls, 0);
    assert_eq!(tiny.used(), 0);
    struct Pressure<'a> {
        reader: &'a mut Fake,
        budget: Budget,
    }
    impl Reader for Pressure<'_> {
        fn read(&mut self, o: Operation) -> Raw {
            if self.reader.pressure.is_none() {
                let r = self.budget.reserve(LIMIT - self.budget.used()).unwrap();
                self.reader.pressure = Some((self.budget.clone(), r));
            }
            self.reader.read(o)
        }
    }
    let snap = collect(
        &s,
        conditions(),
        &mut Pressure {
            reader: &mut f,
            budget: b.clone(),
        },
        &b,
    );
    assert_eq!(f.calls, 5);
    assert_eq!(snap.host_facts(), Ok(()));
    assert!(!snap.canonical().is_empty());
}
#[test]
fn assertions_are_not_sensor_reads_and_unknown_or_competing_conditions_block() {
    for mode in 0..5 {
        let b = budget();
        let s = scope(&b);
        let mut c = conditions();
        match mode {
            0 => c.background_load = None,
            1 => c.competing_tuner = Some(true),
            2 => c.reference.clear(),
            3 => c.asserted_at = a(5000),
            _ => c.asserted_at = a(10_000_000_000),
        }
        let snap = collect(&s, c, &mut Fake::default(), &b);
        assert!(snap.host_facts().is_err());
        assert!(snap.canonical().contains("owner_assertion"));
    }
}
#[test]
fn strict_reader_rejects_wrong_scope_digest_unknown_fields_order_and_integer_overflow() {
    let b = budget();
    let s = scope(&b);
    let snap = collect(&s, conditions(), &mut Fake::default(), &b);
    assert!(matches!(
        Reported::read(&s, snap.canonical().as_bytes(), d(99), &b),
        Err(Fault::Binding)
    ));
    let mut foreign = s.draft().clone();
    foreign.capture_nonce = [99; 16];
    let foreign = Scope::freeze(foreign, &b).unwrap();
    assert!(Reported::read(&foreign, snap.canonical().as_bytes(), snap.digest(), &b).is_err());
    for bytes in [
        snap.canonical()
            .replacen("\"version\":\"1\"", "\"extra\":null,\"version\":\"1\"", 1),
        snap.canonical().replacen(
            "\"major\":\"10\",\"minor\":\"0\"",
            "\"minor\":\"0\",\"major\":\"10\"",
            1,
        ),
        snap.canonical()
            .replace("\"build\":\"26100\"", "\"build\":\"4294967296\""),
        snap.canonical()
            .replacen("\"version\":\"1\"", "\"version\":1", 1),
        snap.canonical().replacen(
            "\"operation\":\"rtl_get_version\"",
            "\"operation\":\"power_get_active_scheme\"",
            1,
        ),
    ] {
        assert_ne!(bytes, snap.canonical());
        assert!(Reported::read(
            &s,
            bytes.as_bytes(),
            Digest::of(Domain::RawArtifact, bytes.as_bytes()),
            &b
        )
        .is_err());
    }
}
#[test]
fn reported_claims_bind_exact_manifest_references_without_issuing_admission() {
    let b = budget();
    let s = scope(&b);
    let snap = collect(&s, conditions(), &mut Fake::default(), &b);
    let read = claimed(&s, &snap, &b);
    let draft = pilot(&s, read.snapshot().digest(), d(20));
    assert_eq!(
        read.snapshot()
            .check_manifest(&draft, s.draft().frequency, a(0)),
        Ok(())
    );
    for mode in 0..8 {
        let mut d = draft.clone();
        let mut frequency = s.draft().frequency;
        let mut epoch = a(0);
        match mode {
            0 => d.environment_evidence = Digest::of(Domain::RawArtifact, b"other"),
            1 => d.session = [99; 16],
            2 => d.raw_pci = "0000:01:03.0".into(),
            3 => d.artifacts.worker = Digest::of(Domain::RawArtifact, b"other"),
            4 => d.runs[0] = s.draft().capture_nonce,
            5 => frequency += 1,
            6 => epoch = a(100),
            _ => d.source_commit = "b".repeat(40),
        }
        assert_eq!(
            read.snapshot().check_manifest(&d, frequency, epoch),
            Err(Fault::Binding)
        );
    }
}
fn starting(s: &Scope, b: &Budget) -> ReportedSnapshot {
    let draft = Draft {
        raw_uuid: s.draft().raw_uuid.clone(),
        raw_pci: s.draft().raw_pci.clone(),
        observations: SettingMetric::ALL.map(|kind| Observation {
            kind,
            before: Some(a(3000 + kind as u64 * 200)),
            after: Some(a(3100 + kind as u64 * 200)),
            raw: Some(Field::available(
                kind.unit(),
                if kind == SettingMetric::PowerLimit {
                    200000
                } else {
                    0
                },
            )),
        }),
        halt: None,
    };
    let snap = Starting::fixture(s, a(2900), &draft, b).unwrap();
    let bytes = snap
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
fn preflight_artifacts_compose_with_parent_and_keep_checks_one_shot() {
    let b = budget();
    let s = scope(&b);
    let snap = collect(&s, conditions(), &mut Fake::default(), &b);
    let env = claimed(&s, &snap, &b);
    let start = starting(&s, &b);
    let draft = pilot(&s, env.snapshot().digest(), start.snapshot().digest());
    let manifest = env
        .freeze_manifest(&start, draft, s.draft().frequency, a(0), &b)
        .unwrap();
    let run = manifest.bind_run(0, 5000).unwrap();
    let mut parent = ParentSampler::new(
        &run,
        s.draft().frequency,
        a(0),
        a(5000),
        a(5100),
        b.clone(),
        b.partition(2 * 1024 * 1024).unwrap(),
    )
    .unwrap();
    assert_eq!(parent.check_environment(&env, &run, a(0)), Ok(()));
    assert_eq!(
        parent.environment_check(),
        Some((env.snapshot().digest(), Ok(())))
    );
    assert!(!parent.native_admission_available());
    assert_eq!(
        parent.check_environment(&env, &run, a(0)),
        Err(ParentFault::Protocol)
    );
    assert_eq!(
        parent.environment_check(),
        Some((env.snapshot().digest(), Ok(())))
    );
    assert!(!parent.native_admission_available());
}
#[test]
fn parent_keeps_raw_environment_rejection_and_original_stop() {
    let b = budget();
    let s = scope(&b);
    let snap = collect(&s, conditions(), &mut Fake::default(), &b);
    let env = reported(&s, &snap, &b);
    let manifest = PilotManifest::freeze(pilot(&s, env.snapshot().digest(), d(20)), &b).unwrap();
    let run = manifest.bind_run(0, 5000).unwrap();
    let mut parent = ParentSampler::new(
        &run,
        s.draft().frequency,
        a(0),
        a(5000),
        a(5100),
        b.clone(),
        b.partition(2 * 1024 * 1024).unwrap(),
    )
    .unwrap();
    assert_eq!(
        parent.check_environment(&env, &run, a(0)),
        Err(ParentFault::Environment)
    );
    assert_eq!(
        parent.environment_check(),
        Some((env.snapshot().digest(), Err(Fault::Simulated)))
    );
    let first = parent.stop();
    assert_eq!(
        parent.check_environment(&env, &run, a(0)),
        Err(ParentFault::Protocol)
    );
    assert_eq!(parent.stop(), first);
    assert_eq!(parent.fault(), Some(ParentFault::Environment));
}
#[test]
fn out_of_order_raw_kind_stops_after_retaining_return() {
    let b = budget();
    let s = scope(&b);
    let mut f = Fake::default();
    f.raw.swap(0, 4);
    let snap = collect(&s, conditions(), &mut f, &b);
    assert_eq!(snap.halt(), Some(Fault::Protocol));
    assert_eq!(f.calls, 1);
    assert!(snap.observations().next().unwrap().raw.is_some());
    let read = reported(&s, &snap, &b);
    assert_eq!(
        read.snapshot().observations().next().unwrap().raw,
        snap.observations().next().unwrap().raw
    );
    assert_eq!(read.snapshot().host_facts(), Err(Fault::Protocol));
}
#[test]
fn capture_does_not_move_preflight_or_run_origins() {
    let b = budget();
    let s = scope(&b);
    let snap = collect(&s, conditions(), &mut Fake::default(), &b);
    let env = claimed(&s, &snap, &b);
    let manifest = PilotManifest::freeze(pilot(&s, env.snapshot().digest(), d(20)), &b).unwrap();
    let early = manifest.bind_run(0, 1000).unwrap();
    assert_eq!(
        env.snapshot().check_run(&early, s.draft().frequency, a(0)),
        Err(Fault::Deadline)
    );
    let run = manifest.bind_run(0, 2000).unwrap();
    assert_eq!(
        env.snapshot().check_run(&run, s.draft().frequency, a(0)),
        Ok(())
    );
    assert_eq!(run.origin_ns(), 2000);
}
