#![cfg(all(windows, feature = "sampler-process-fixture"))]
use autotune_benchmark_worker::sampler::windows::{FixtureMode, SamplerProcess};

// Compile-time API contract only under default verification. OS acceptance
// requires a separately approved exact fixture artifact/source matrix.
#[test]
#[ignore = "requires separately approved Windows sampler-process execution"]
fn fixed_hung_sampler_requires_observed_exit_after_force() {
    let mut process = SamplerProcess::launch_fixture(
        SamplerProcess::fixture_digest().unwrap(),
        FixtureMode::Hang,
    )
    .unwrap();
    process.request_stop().unwrap();
    assert_ne!(process.observe().wait_status, 0);
    process.request_termination().unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let observation = process.observe();
        assert!(std::time::Instant::now() < until);
        if observation.wait_status == 0 {
            assert!(observation.exit_code.is_some());
            assert_eq!(observation.active_processes, Some(0));
            break;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires separately approved Windows sampler-process execution"]
fn cancelled_async_launch_never_allows_a_late_sampler_to_continue() {
    use autotune_benchmark_worker::sampler::windows::SamplerLaunch;
    let mut launch =
        SamplerLaunch::start_fixture(SamplerProcess::fixture_digest().unwrap(), FixtureMode::Hang)
            .unwrap();
    launch.request_stop().unwrap();
    launch.request_termination().unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let result = launch.poll();
        assert!(std::time::Instant::now() < until);
        if let Some(result) = result {
            match result {
                Ok(mut process) => {
                    process.request_termination().unwrap();
                    loop {
                        let observed = process.observe();
                        assert!(std::time::Instant::now() < until);
                        if observed.wait_status == 0 {
                            assert!(observed.wait_error.is_none());
                            assert!(observed.exit_code.is_some());
                            assert!(observed.exit_error.is_none());
                            assert_eq!(observed.active_processes, Some(0));
                            assert!(observed.accounting_error.is_none());
                            break;
                        }
                        assert!(std::time::Instant::now() < until);
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                Err(error) => {
                    // After-spawn errors carry the original child handle. A
                    // termination request cannot pass this case without exit
                    // observation inside the ORIGINAL three-second bound.
                    if let Ok(mut failed) =
                        autotune_benchmark_worker::sampler::windows::FailedLaunch::from_error(error)
                    {
                        failed.request_termination().unwrap();
                        loop {
                            let observed = failed.observe();
                            assert!(std::time::Instant::now() < until);
                            if observed.wait_status == 0 {
                                assert!(observed.wait_error.is_none());
                                assert!(observed.exit_code.is_some());
                                assert!(observed.exit_error.is_none());
                                break;
                            }
                            assert!(std::time::Instant::now() < until);
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                    // Other errors originate before successful CreateProcess;
                    // no child handle exists in those source paths.
                }
            }
            break;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

// These use the production child protocol/control loop with a fixed fake
// backend. They do not load NVML. Each executable/source hash still requires
// separate owner approval before execution; default verification only links.
fn wire_round_trip(mode: u8) {
    use autotune_benchmark_worker::{
        evidence::{Provenance, Verdict},
        monitor::windows_clock::WindowsClock,
        sampler::{
            drain::{poll, Step},
            manifest::{CaptureManifest, Draft},
            producer::{build_envelope, raw_events},
            runtime::{Operation, ResourceObservation},
            supervisor::{Effect, Fault, Input, Supervisor},
            windows::{SamplerLaunch, WireFixtureMode},
            wire::Message,
        },
    };
    let digest = SamplerProcess::fixture_digest().unwrap();
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
        sampler: digest,
        protocol: [8; 32],
        approval_ref: "separately approved fixture execution only".into(),
        output_root: "D:\\unused-fixture-output".into(),
    })
    .unwrap();
    let clock = WindowsClock::new([3; 16]).unwrap();
    let origin = clock.capture().unwrap();
    let binding = m.binding(clock.qpc_frequency()).unwrap();
    let mut s = Supervisor::new(binding, origin, m.selection()).unwrap();
    let mut launch = SamplerLaunch::start_bound_fixture(
        digest,
        binding,
        s.budget(),
        match mode {
            0 => WireFixtureMode::Cooperative,
            1 => WireFixtureMode::BlockedGetter,
            _ => WireFixtureMode::WrongIdentity,
        },
    )
    .unwrap();
    let absolute = std::time::Instant::now() + std::time::Duration::from_secs(18);
    let mut child = loop {
        if let Some(result) = launch.poll() {
            break result.unwrap();
        }
        s.advance(Some(clock.capture().unwrap()), Input::Tick);
        assert!(
            s.terminal().is_none(),
            "startup exceeded unchanged approved limits"
        );
        assert!(std::time::Instant::now() < absolute);
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    s.advance(
        Some(clock.capture().unwrap()),
        Input::Launched {
            creation: child.creation_time(),
        },
    );
    child
        .send(&Message::Bootstrap {
            origin,
            raw_uuid: m.draft().raw_uuid.clone(),
            raw_pci: m.draft().raw_pci.clone(),
            manifest: m.canonical().into(),
        })
        .unwrap();
    let mut force = false;
    let mut drained = false;
    loop {
        // Receipt follows the actual observed OS calls, just as in the fixed
        // controller. Fixture controller usage is explicit synthetic evidence.
        let observation = child.observe();
        let sampler_resources = child.resources();
        let receipt = clock.capture().unwrap();
        let mut effects = s.advance(
            Some(receipt),
            Input::Host {
                watchdog_100ns: receipt.interrupt_100ns,
                resources: ResourceObservation {
                    commit_bytes: Some(1),
                    user_100ns: Some(0),
                    kernel_100ns: Some(0),
                    memory_error: None,
                    time_error: None,
                },
                sampler_resources: Some(sampler_resources),
                process: Some(observation),
            },
        );
        match poll(&mut child) {
            Step::Item(received) => {
                effects.extend(received.apply(&mut s, Some(clock.capture().unwrap())))
            }
            Step::Finished(_) => drained = true,
            Step::Waiting | Step::Failed(_) => {}
        }
        if mode == 0 && s.samples().count() >= 6 && s.terminal().is_none() {
            effects.extend(s.advance(Some(clock.capture().unwrap()), Input::Stop));
        }
        for effect in effects {
            match effect {
                Effect::Send(message) => {
                    let operation = if matches!(message, Message::Query(_)) {
                        Operation::QuerySend
                    } else {
                        Operation::StopSend
                    };
                    let result = child.send(&message);
                    assert!(result.is_ok());
                    s.advance(
                        Some(clock.capture().unwrap()),
                        Input::Operation {
                            operation,
                            result: Ok(()),
                        },
                    );
                }
                Effect::SignalStop => child.request_stop().unwrap(),
                Effect::Terminate => {
                    force = true;
                    child.request_termination().unwrap();
                }
                Effect::Finalize => {}
            }
        }
        if s.exit_confirmed() && drained {
            break;
        }
        assert!(
            std::time::Instant::now() < absolute,
            "fixture termination/drain unresolved at original bound"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for observation in child.shutdown_transport() {
        s.advance(
            Some(clock.capture().unwrap()),
            Input::IoShutdown(observation),
        );
    }
    let raw = raw_events(&s).unwrap();
    let mut e = build_envelope(&m, &s, Provenance::SimulatedFixture).unwrap();
    e.account_artifacts(m.canonical(), "fixture profile", &raw)
        .unwrap();
    assert!(e.encode().is_ok());
    assert!(s.exit_confirmed());
    match mode {
        0 => {
            assert!(s.samples().count() >= 6);
            assert_eq!(s.terminal().unwrap().reason, Fault::Requested);
            assert_eq!(e.result.verdict, Verdict::Blocked);
        }
        1 => {
            assert!(force);
            assert!(s.terminal().unwrap().pending.is_some());
            assert!(raw.contains("\"pending\":true"));
            assert_eq!(e.result.verdict, Verdict::Invalid);
        }
        _ => {
            assert_eq!(s.terminal().unwrap().reason, Fault::Identity);
            assert_eq!(s.samples().count(), 0);
            assert_eq!(e.result.verdict, Verdict::Invalid);
        }
    }
}
#[test]
#[ignore = "requires exact source/artifact-bound Windows wire fixture execution approval"]
fn shared_wire_fixture_samples_six_metrics_and_drains_after_stop() {
    wire_round_trip(0);
}
#[test]
#[ignore = "requires exact source/artifact-bound Windows wire fixture execution approval"]
fn shared_wire_fixture_blocked_getter_keeps_control_responsive_until_force() {
    wire_round_trip(1);
}
#[test]
#[ignore = "requires exact source/artifact-bound Windows wire fixture execution approval"]
fn shared_wire_fixture_wrong_ready_identity_never_admits_a_query() {
    wire_round_trip(2);
}
