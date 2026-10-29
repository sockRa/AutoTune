use autotune_benchmark_worker::sampler::runtime::{ResourceObservation, Watchdog};
#[test]
fn shutdown_watchdog_never_extends_first_stop_or_absolute_capture_cap() {
    let mut w = Watchdog::new(100).unwrap();
    assert_eq!(w.observe(110).unwrap(), 1_000);
    w.stop(1_000);
    w.stop(2_000_000_000);
    assert!(!w.force_due());
    w.observe(10_000_110).unwrap();
    assert!(w.force_due());
    assert!(!w.observation_expired());
    w.observe(30_000_110).unwrap();
    assert!(w.observation_expired());
    assert!(!w.finalization_expired());
    w.observe(80_000_110).unwrap();
    assert!(w.finalization_expired());
    let mut w = Watchdog::new(100).unwrap();
    w.observe(100_000_100).unwrap();
    assert!(w.capture_due());
    w.stop(10_000_000_000);
    w.observe(180_000_100).unwrap();
    assert!(w.finalization_expired());
}
#[test]
fn raw_clock_regression_overflow_or_untrustworthy_time_closes_all_waits() {
    for values in [[100, 99], [0, u64::MAX]] {
        let mut w = Watchdog::new(values[0]).unwrap();
        assert!(w.observe(values[1]).is_err());
        assert!(w.force_due());
        assert!(w.observation_expired());
        assert!(w.finalization_expired());
        assert!(w.observe(values[0]).is_err());
    }
}
#[test]
fn controller_resources_preserve_unknowns_and_fail_closed_on_exhaustion() {
    let good = ResourceObservation {
        commit_bytes: Some(100),
        user_100ns: Some(100),
        kernel_100ns: Some(100),
        memory_error: None,
        time_error: None,
    };
    assert!(good.within_limits());
    for bad in [
        ResourceObservation {
            commit_bytes: None,
            memory_error: Some(5),
            ..good
        },
        ResourceObservation {
            user_100ns: Some(u64::MAX),
            ..good
        },
        ResourceObservation {
            user_100ns: Some(600_000_000),
            ..good
        },
        ResourceObservation {
            commit_bytes: Some(512 * 1024 * 1024),
            ..good
        },
    ] {
        assert!(!bad.within_limits());
    }
}

#[test]
fn execution_request_requires_exact_manifest_digest_and_controller_artifact() {
    use autotune_benchmark_worker::sampler::{
        manifest::{CaptureManifest, Draft},
        runtime::ExecutionRequest,
    };
    let m = CaptureManifest::freeze(Draft {
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
    .unwrap();
    let args = vec![
        "controller".into(),
        "--execute-readonly-manifest".into(),
        m.digest().to_hex(),
        "manifest.json".into(),
    ];
    let request = ExecutionRequest::parse(&args).unwrap();
    assert!(request.bind(&m, [6; 32]).is_ok());
    assert!(request.bind(&m, [9; 32]).is_err());
    let mut wrong = args.clone();
    wrong[2] = "0".repeat(64);
    assert!(ExecutionRequest::parse(&wrong)
        .unwrap()
        .bind(&m, [6; 32])
        .is_err());
    let mut implicit = args.clone();
    implicit[1] = "--approved".into();
    assert!(ExecutionRequest::parse(&implicit).is_err());
}
#[test]
fn host_exit_observation_is_timestamped_after_calls_and_cannot_pass_at_deadline() {
    use autotune_benchmark_worker::{
        monitor::{Anchor, Binding},
        sampler::{
            runtime::{observe_host, HostSource},
            supervisor::{Input, ProcessObservation, Selection, Supervisor},
            wire::WireBinding,
        },
        worker::DiscoveryBinding,
    };
    fn a(ns: u64) -> Anchor {
        Anchor {
            clock_token: [4; 16],
            qpc_before: ns,
            qpc_after: ns,
            interrupt_100ns: ns / 100,
            unbiased_100ns: ns / 100,
        }
    }
    struct Delayed {
        now: u64,
    }
    impl HostSource for Delayed {
        fn resources(&mut self) -> ResourceObservation {
            ResourceObservation {
                commit_bytes: Some(100),
                user_100ns: Some(1),
                kernel_100ns: Some(1),
                memory_error: None,
                time_error: None,
            }
        }
        fn process(&mut self) -> Option<ProcessObservation> {
            self.now = 3_000_000_000;
            Some(ProcessObservation {
                creation: 123,
                wait_status: 0,
                wait_error: None,
                exit_code: Some(0),
                exit_error: None,
                termination: None,
                active_processes: Some(0),
                accounting_error: None,
            })
        }
        fn watchdog(&mut self) -> u64 {
            self.now / 100
        }
        fn anchor(&mut self) -> Option<Anchor> {
            Some(a(self.now))
        }
    }
    let uuid = "GPU-03030303-0303-0303-0303-030303030303";
    let pci = "0000:01:02.0";
    let binding = WireBinding {
        monitor: Binding {
            session: [1; 16],
            run: [2; 16],
            device: DiscoveryBinding::parse(uuid, pci).unwrap().identity,
            clock_token: [4; 16],
            qpc_frequency: 1_000_000_000,
        },
        manifest_digest: [5; 32],
    };
    let mut s = Supervisor::new(
        binding,
        a(0),
        Selection {
            raw_uuid: uuid.into(),
            raw_pci: pci.into(),
            sampler_digest: [6; 32],
        },
    )
    .unwrap();
    s.advance(Some(a(0)), Input::Launched { creation: 123 });
    s.advance(Some(a(0)), Input::Stop);
    let capture = observe_host(&mut Delayed { now: 2_999_999_900 });
    s.advance(
        capture.anchor,
        Input::Host {
            watchdog_100ns: capture.watchdog_100ns,
            resources: capture.resources,
            sampler_resources: capture.sampler_resources,
            process: capture.process,
        },
    );
    assert!(!s.exit_confirmed());
}

#[test]
fn sampler_cpu_observation_counts_kernel_time_and_fails_at_exact_ceiling() {
    let mut r = ResourceObservation {
        commit_bytes: Some(1),
        user_100ns: Some(1),
        kernel_100ns: Some(1),
        memory_error: None,
        time_error: None,
    };
    assert!(r.within_sampler_limits());
    r.user_100ns = Some(1);
    r.kernel_100ns = Some(300_000_000 - 1);
    assert!(!r.within_sampler_limits());
    assert!(r.within_limits());
    r.kernel_100ns = None;
    assert!(!r.within_sampler_limits());
}
