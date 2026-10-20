use autotune_power_trial::engine::{Engine, Event, Failure, Operation, Report, State};
use autotune_power_trial::journal::{Fault, FaultPhase, Journal, ReconcileObservation};
use autotune_power_trial::manifest::{Consent, Reading, Rejection};
use autotune_power_trial::mock::{device, manifest, plan, run, SCENARIOS};

fn arm() -> Engine {
    let m = manifest(240_000);
    Engine::arm(
        m.clone(),
        Some(Consent::simulated(&m)),
        &mut Journal::default(),
        device(&m),
        500,
        100_500,
    )
    .unwrap()
}
fn finish(e: &mut Engine) {
    for _ in 0..64 {
        e.advance(e.now + 250);
        e.event(Event::Heartbeat);
        if e.is_terminal() && !e.has_pending() {
            break;
        }
    }
}
fn observation(current: Reading<u32>) -> ReconcileObservation {
    let m = manifest(240_000);
    ReconcileObservation {
        identities: vec![m.identity],
        bounds: Reading::Available(m.bounds),
        current,
    }
}
fn setters(r: &Report) -> Vec<&autotune_power_trial::engine::Trace> {
    r.trace
        .iter()
        .filter(|t| t.event == "native_entry" && t.operation.unwrap().is_setter())
        .collect()
}
fn invariants(r: &Report) {
    assert_eq!(r.provenance, "simulated");
    assert_eq!(r.manifest_digest, r.manifest.digest());
    let mut pending = None;
    let mut candidate = 0;
    let mut restores = 0;
    let mut closed = false;
    for (n, t) in r.trace.iter().enumerate() {
        if t.event.starts_with("admission_closed") {
            closed = true;
        }
        if t.event == "native_entry" {
            assert!(pending.is_none(), "overlapping calls: {}", r.scenario);
            pending = t.operation;
            assert_eq!(t.uuid, r.manifest.identity.uuid);
            assert_eq!(t.pci, r.manifest.identity.pci);
            assert_eq!(t.handle.as_deref(), Some("synthetic-handle-7"));
            match t.operation.unwrap() {
                Operation::Candidate => {
                    candidate += 1;
                    assert!(!closed);
                    assert_eq!(t.value_mw, Some(r.manifest.target_mw));
                    assert!(r.trace[..n]
                        .iter()
                        .any(|t| t.event == "durable:mutation_possible"));
                    assert!(r.trace[..n]
                        .iter()
                        .any(|t| t.event == "durable:intent:Candidate"));
                    assert!(r.journal.consumed_nonces.contains(&r.manifest.nonce));
                }
                Operation::Restore => {
                    restores += 1;
                    assert!(closed);
                    if restores == 2 {
                        let previous = r.trace[..n]
                            .iter()
                            .rev()
                            .find(|t| t.event == "native_return")
                            .unwrap();
                        assert!(matches!(
                            previous.operation,
                            Some(Operation::VerifyFirst | Operation::VerifySecond)
                        ));
                        assert!(
                            previous.value_mw != Some(r.manifest.baseline_mw)
                                || previous.status != Some(0)
                        );
                    }
                    assert_eq!(t.value_mw, Some(r.manifest.baseline_mw));
                    assert!(
                        r.trace[..n]
                            .iter()
                            .any(|t| t.event == "durable:intent:Restore")
                            || r.journal.storage_failed
                    );
                }
                _ => {}
            }
        } else if t.event == "native_return" {
            assert_eq!(pending.take(), t.operation);
        }
    }
    assert!(candidate <= 1 && restores <= 2);
    if r.observed_restored {
        assert!(restores >= 1 && pending.is_none());
        let v = &r.verification_reads[r.verification_reads.len() - 2..];
        assert!(v[1].0 >= v[0].0 + 250);
        assert!(v
            .iter()
            .all(|(_, v)| *v == Reading::Available(r.manifest.baseline_mw)));
    }
    if r.overall == "success" {
        assert!(r.observed_restored && r.evidence_durable && !r.journal.unresolved);
        assert!(r.failures.is_empty());
        assert_eq!(r.candidate_api, Some(0));
        assert_eq!(
            r.candidate_readback,
            Some(Reading::Available(r.manifest.target_mw))
        );
    }
    if r.journal.unresolved || !r.journal.validate() {
        let mut j = r.journal.clone();
        let mut m = r.manifest.clone();
        m.nonce.push_str("-new");
        assert!(Engine::arm(
            m.clone(),
            Some(Consent::simulated(&m)),
            &mut j,
            device(&m),
            500,
            100_500
        )
        .is_err());
    }
}

#[test]
fn all_scenarios_are_deterministic_and_obey_trace_invariants() {
    for s in SCENARIOS {
        let a = run(s).unwrap();
        let b = run(s).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "{s}"
        );
        invariants(&a);
        assert_eq!(
            a.overall == "success",
            ["success", "baseline-230000", "default-unavailable"].contains(s),
            "{s}"
        );
    }
}

#[test]
fn m01_missing_generic_wrong_operator_replay_and_expiry() {
    for kind in 0..7 {
        let m = manifest(240_000);
        let mut c = Consent::simulated(&m);
        let mut j = Journal::default();
        let mut now = 500;
        match kind {
            1 => c.simulated_exact_interaction = false,
            2 => c.manifest_digest = "wrong".into(),
            3 => c.operator = "other".into(),
            4 => {
                j.consumed_nonces.insert(m.nonce.clone());
            }
            5 => now = 60_500,
            6 => c.nonce = "other".into(),
            _ => {}
        }
        assert!(Engine::arm(
            m.clone(),
            if kind == 0 { None } else { Some(c) },
            &mut j,
            device(&m),
            now,
            100_000 + now
        )
        .is_err());
    }
}

#[test]
fn m02_every_serialized_manifest_field_is_bound_to_consent() {
    fn leaves(value: &serde_json::Value, path: String, out: &mut Vec<(String, serde_json::Value)>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    leaves(value, format!("{path}/{key}"), out);
                }
            }
            serde_json::Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    leaves(value, format!("{path}/{index}"), out);
                }
            }
            serde_json::Value::String(s) => {
                out.push((path, serde_json::json!(format!("{s}-changed"))))
            }
            serde_json::Value::Number(n) => out.push((
                path,
                serde_json::json!(if n.as_u64().unwrap() == 0 { 1 } else { 0 }),
            )),
            serde_json::Value::Bool(b) => out.push((path, serde_json::json!(!b))),
            serde_json::Value::Null => panic!("unexpected null manifest leaf: {path}"),
        }
    }
    let m = manifest(240_000);
    let original = m.digest();
    let object = serde_json::to_value(&m).unwrap();
    let mut mutations = Vec::new();
    leaves(&object, String::new(), &mut mutations);
    for prefix in [
        "/identity/",
        "/bounds/",
        "/timers/",
        "/baseline_readings_mw/0/",
        "/baseline_readings_mw/1/",
        "/capture_mono_ms/0",
        "/capture_mono_ms/1",
        "/capture_utc_ms/0",
        "/capture_utc_ms/1",
    ] {
        assert!(
            mutations.iter().any(|(path, _)| path.starts_with(prefix)),
            "{prefix}"
        );
    }
    for (path, replacement) in mutations {
        let mut changed = object.clone();
        *changed.pointer_mut(&path).unwrap() = replacement;
        let changed = serde_json::from_value::<autotune_power_trial::manifest::Manifest>(changed)
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        assert_ne!(original, changed.digest(), "{path}");
        assert!(Consent::simulated(&m).validate(&changed).is_err(), "{path}");
    }
}

#[test]
fn m03_capture_boundaries_and_clock_jumps_never_extend_consent() {
    let mut m = manifest(240_000);
    m.approval_expiry_utc_ms = 200_000;
    assert!(m.fresh(60_499, 160_499).is_ok());
    assert_eq!(m.fresh(60_500, 160_500), Err(Rejection::ExpiredConsent));
    assert_eq!(m.fresh(500, 100_499), Err(Rejection::ClockDiscontinuity));
    assert_eq!(m.fresh(500, 100_501), Err(Rejection::ClockDiscontinuity));
    assert_eq!(m.fresh(499, 100_499), Err(Rejection::ClockDiscontinuity));
    let mut e = arm();
    e.event(Event::ClockDiscontinuity);
    e.submit_candidate(&manifest(240_000).digest(), 1);
    assert!(setters(&e.report("sleep-before")).is_empty());
    invariants(&run("clock-discontinuity").unwrap());
}

#[test]
fn m04_m05_exact_identity_resolution_not_index_or_name() {
    for kind in 0..4 {
        let m = manifest(240_000);
        let mut d = device(&m);
        match kind {
            0 => d.devices.push(d.devices[0].clone()),
            1 => d.devices.clear(),
            2 => d.devices[0].0.pci.clear(),
            _ => d.devices[0].0.uuid = "other".into(),
        }
        assert!(Engine::arm(
            m.clone(),
            Some(Consent::simulated(&m)),
            &mut Journal::default(),
            d,
            500,
            100_500
        )
        .is_err());
    }
    let mut e = arm();
    let mut other = e.device.devices[0].clone();
    other.0.uuid = "other-same-model".into();
    other.1 = "other-handle".into();
    e.device.devices.insert(0, other);
    e.submit_candidate(&manifest(240_000).digest(), 1);
    finish(&mut e);
    invariants(&e.report("reordered"));
    assert_eq!(e.report("").overall, "success");
    let mut e = arm();
    e.device.devices[0].0.uuid = "replaced".into();
    e.submit_candidate(&manifest(240_000).digest(), 1);
    assert!(setters(&e.report("")).is_empty());
}

#[test]
fn m06_capture_and_pre_entry_drift_reject_without_rebase() {
    let mut m = manifest(240_000);
    m.baseline_readings_mw[0] = Reading::Available(239_999);
    assert_eq!(m.validate(), Err(Rejection::InvalidCapture));
    for bounds in [false, true] {
        let mut e = arm();
        if bounds {
            e.device.bounds.max_mw -= 1;
        } else {
            e.device.current = Reading::Available(239_999);
        }
        e.submit_candidate(&manifest(240_000).digest(), 1);
        assert!(setters(&e.report("")).is_empty());
    }
}

#[test]
fn m07_m08_fresh_baselines_and_optional_default() {
    for s in ["success", "baseline-230000", "default-unavailable"] {
        let r = run(s).unwrap();
        invariants(&r);
        assert_eq!(r.overall, "success");
        assert_ne!(setters(&r)[1].value_mw, Some(215_000));
    }
    for reading in [
        Reading::Available(0),
        Reading::Available(u32::MAX),
        Reading::Unavailable("failed".into()),
    ] {
        let mut m = manifest(240_000);
        m.baseline_readings_mw[1] = reading;
        assert!(m.validate().is_err());
    }
}

#[test]
fn m08_m09_checked_bounds_and_reductions() {
    for (b, min, max, delta, t) in [
        (240_000, 0, 240_000, 5000, 235_000),
        (240_000, 240_000, 240_000, 5000, 235_000),
        (240_000, 240_001, 250_000, 5000, 235_000),
        (240_000, 107_500, u32::MAX, 5000, 235_000),
        (240_000, 107_500, 240_000, 999, 239_001),
        (240_000, 107_500, 240_000, 5001, 234_999),
        (240_000, 107_500, 240_000, 1000, 240_000),
        (240_000, 107_500, 250_000, 1000, 241_000),
        (500, 1, 1000, 1000, u32::MAX),
        (107_500, 107_500, 240_000, 1000, 106_500),
    ] {
        let mut m = manifest(240_000);
        m.baseline_mw = b;
        m.baseline_readings_mw = [Reading::Available(b), Reading::Available(b)];
        m.bounds.min_mw = min;
        m.bounds.max_mw = max;
        m.delta_mw = delta;
        m.target_mw = t;
        assert!(m.validate().is_err());
    }
    for delta in [1000, 5000] {
        let mut m = manifest(240_000);
        m.delta_mw = delta;
        m.target_mw = m.baseline_mw - delta;
        assert!(m.validate().is_ok());
    }
}

#[test]
fn m10_m20_m21_m22_distinct_candidate_api_and_exact_readback() {
    for s in [
        "error-before-change",
        "error-after-change",
        "clamped-readback",
        "unavailable-readback",
    ] {
        let r = run(s).unwrap();
        invariants(&r);
        assert_eq!(r.overall, "failed_restored");
        assert!(r.observed_restored);
        assert_eq!(setters(&r).len(), 2);
        if s.starts_with("error") {
            assert_eq!(r.candidate_api, Some(4));
        }
    }
    for v in [
        Reading::Available(240_000),
        Reading::Available(0),
        Reading::Available(u32::MAX),
        Reading::Available(234_999),
    ] {
        let mut e = arm();
        e.device
            .plans
            .push_back(plan(Operation::CandidateRead, Some(0), 0, None, Some(v)));
        e.submit_candidate(&manifest(240_000).digest(), 1);
        finish(&mut e);
        assert_eq!(e.report("").overall, "failed_restored");
        invariants(&e.report(""));
    }
}

#[test]
fn m11_m13_every_publication_fault_preserves_cleanup_and_exclusion() {
    for n in 1..=14 {
        for phase in [
            FaultPhase::Before,
            FaultPhase::PartialWrite,
            FaultPhase::Flush,
            FaultPhase::Replace,
            FaultPhase::Readback,
            FaultPhase::After,
        ] {
            let m = manifest(240_000);
            let mut j = Journal {
                fault: Some(Fault {
                    publication: n,
                    phase: phase.clone(),
                }),
                ..Default::default()
            };
            let result = Engine::arm(
                m.clone(),
                Some(Consent::simulated(&m)),
                &mut j,
                device(&m),
                500,
                100_500,
            );
            if n <= 2 {
                assert!(result.is_err());
                assert!(j.unresolved);
                assert!(j.consumed_nonces.contains(&m.nonce));
                continue;
            }
            let mut e = result.unwrap();
            e.submit_candidate(&manifest(240_000).digest(), 1);
            finish(&mut e);
            let r = e.report(&format!("publication-{n}-{phase:?}"));
            invariants(&r);
            assert_ne!(r.overall, "success");
            assert!(r.journal.unresolved);
            assert!(!r.evidence_durable);
            if n >= 5 {
                assert!(r.observed_restored);
                assert_eq!(setters(&r).len(), 2);
            } else {
                assert!(setters(&r).is_empty());
            }
        }
    }
}

#[test]
fn m12_m14_m29_m30_restart_is_read_only_never_authority() {
    let mut e = arm();
    e.submit_candidate(&manifest(240_000).digest(), 1);
    // Crash after durable intent and entry, before any result.
    assert!(e.journal.unresolved);
    for value in [
        Reading::Available(215_000),
        Reading::Available(240_000),
        Reading::Unavailable("lost".into()),
    ] {
        let before = serde_json::to_string(&e.journal).unwrap();
        let r = e.journal.reconcile(observation(value));
        assert_eq!(r.setters_executed, 0);
        assert!(r.unresolved && r.human_reconciliation_required);
        assert!(r.identity_matches && r.bounds_match && r.baseline_in_bounds);
        assert_eq!(before, serde_json::to_string(&e.journal).unwrap());
    }
    for corruption in 0..5 {
        let mut j = e.journal.clone();
        match corruption {
            0 => j.invalid = true,
            1 => j.records[0].event.push_str("tampered"),
            2 => j.manifest.as_mut().unwrap().baseline_mw = 215_000,
            3 => j.manifest.as_mut().unwrap().schema = 99,
            _ => {
                j.records.remove(0);
            }
        }
        assert!(!j.validate());
        assert_eq!(
            j.reconcile(observation(Reading::Available(240_000)))
                .setters_executed,
            0
        );
    }
    let mut j = e.journal.clone();
    let m = manifest(240_000);
    assert_eq!(
        Engine::arm(
            m.clone(),
            Some(Consent::simulated(&m)),
            &mut j,
            device(&m),
            500,
            100_500
        )
        .err(),
        Some(Rejection::JournalBlocked)
    );
}

#[test]
fn m15_m19_cancel_parent_ipc_and_clock_interleavings_at_every_phase() {
    for at in 500..=770 {
        for event in [
            Event::Cancel,
            Event::ParentDeath,
            Event::IpcLoss,
            Event::ClockDiscontinuity,
        ] {
            let mut e = arm();
            for op in [
                Operation::Candidate,
                Operation::CandidateRead,
                Operation::Restore,
                Operation::VerifyFirst,
                Operation::VerifySecond,
            ] {
                e.device.plans.push_back(plan(
                    op,
                    Some(1),
                    0,
                    match op {
                        Operation::Candidate => Some(235_000),
                        Operation::Restore => Some(240_000),
                        _ => None,
                    },
                    None,
                ));
            }
            if at == 500 {
                e.event(event);
                e.submit_candidate(&manifest(240_000).digest(), 1);
            } else {
                e.submit_candidate(&manifest(240_000).digest(), 1);
                e.advance(at);
                e.event(event);
            }
            e.submit_candidate(&manifest(240_000).digest(), 1); // stale queued command
            finish(&mut e);
            let r = e.report("interleaving");
            invariants(&r);
            if at == 500 {
                assert!(setters(&r).is_empty());
            } else {
                assert!(r.observed_restored);
            }
        }
    }
}

#[test]
fn m16_fixed_lease_heartbeat_and_closed_generation() {
    let r = run("lease-expiry").unwrap();
    assert!(setters(&r).is_empty());
    assert!(r.failures.contains(&Failure::LeaseExpired));
    let mut e = arm();
    e.advance(1_500); // exact heartbeat-loss boundary wins
    e.event(Event::Heartbeat);
    e.submit_candidate(&manifest(240_000).digest(), 1);
    assert!(setters(&e.report("")).is_empty());
    let mut e = arm();
    for _ in 0..8 {
        e.advance(e.now + 250);
        e.event(Event::Heartbeat);
    }
    e.submit_candidate(&manifest(240_000).digest(), 1);
    assert!(setters(&e.report("")).is_empty());
}

#[test]
fn m17_m18_unknown_native_completion_never_races_a_restore() {
    for s in [
        "candidate-hang",
        "getter-hang",
        "restore-hang",
        "late-completion",
        "guardian-death",
    ] {
        let r = run(s).unwrap();
        invariants(&r);
        assert_eq!(r.overall, "recovery_required");
        assert!(!r.observed_restored && r.journal.unresolved);
        if [
            "candidate-hang",
            "getter-hang",
            "late-completion",
            "guardian-death",
        ]
        .contains(&s)
        {
            assert_eq!(setters(&r).len(), 1);
        }
    }
    // Completion at watchdog boundary is uncertain, but proves quiescence before cleanup.
    let mut e = arm();
    e.device.plans.push_back(plan(
        Operation::Candidate,
        Some(1000),
        0,
        Some(235_000),
        None,
    ));
    e.submit_candidate(&manifest(240_000).digest(), 1);
    finish(&mut e);
    let r = e.report("watchdog-tie");
    invariants(&r);
    assert!(r.observed_restored);
    assert_ne!(r.overall, "success");
}

#[test]
fn m23_m24_m25_m26_restoration_is_exact_bounded_and_independent_of_api_success() {
    for (s, observed, attempts) in [
        ("success", true, 1),
        ("restore-retry", true, 2),
        ("restore-error-observed", true, 1),
        ("restore-failed", false, 2),
        ("verification-drift", false, 2),
    ] {
        let r = run(s).unwrap();
        invariants(&r);
        assert_eq!(r.observed_restored, observed);
        assert_eq!(r.restore_api.len(), attempts);
        if s != "success" {
            assert_ne!(r.overall, "success");
        }
    }
    // A call completing beyond the recovery budget must not claim restoration.
    let mut e = arm();
    e.device
        .plans
        .push_back(plan(Operation::Restore, Some(5000), 0, Some(240_000), None));
    e.submit_candidate(&manifest(240_000).digest(), 1);
    finish(&mut e);
    let r = e.report("expired-cleanup");
    invariants(&r);
    assert!(!r.observed_restored);
    assert_eq!(r.restore_api.len(), 1);
}

#[test]
fn m27_m28_no_blind_restore_after_identity_runtime_bounds_or_owner_change() {
    for s in ["identity-change", "bounds-change", "ownership-loss"] {
        let r = run(s).unwrap();
        invariants(&r);
        assert_eq!(setters(&r).len(), 1);
        assert!(r.journal.unresolved);
    }
    let mut e = arm();
    e.device
        .plans
        .push_back(plan(Operation::Candidate, Some(1), 0, Some(235_000), None));
    e.submit_candidate(&manifest(240_000).digest(), 1);
    e.device.runtime = "changed".into();
    finish(&mut e);
    assert_eq!(setters(&e.report("")).len(), 1);
    let m = manifest(240_000);
    let mut d = device(&m);
    d.owned = false;
    assert!(Engine::arm(
        m.clone(),
        Some(Consent::simulated(&m)),
        &mut Journal::default(),
        d,
        500,
        100_500
    )
    .is_err());
}

#[test]
fn native_model_faults_at_each_entry_return_and_timeout_boundary() {
    for op in [
        Operation::Candidate,
        Operation::CandidateRead,
        Operation::Restore,
        Operation::VerifyFirst,
        Operation::VerifySecond,
    ] {
        for duration in [None, Some(0), Some(999), Some(1000), Some(1001), Some(6000)] {
            for status in [0, 4, 6] {
                let mut e = arm();
                e.device.plans.push_back(plan(
                    op,
                    duration,
                    status,
                    None,
                    if op.is_setter() {
                        None
                    } else {
                        Some(Reading::Unavailable("injected".into()))
                    },
                ));
                e.submit_candidate(&manifest(240_000).digest(), 1);
                finish(&mut e);
                invariants(&e.report("native-fault"));
                assert_ne!(e.report("").overall, "success");
            }
        }
    }
}

#[test]
fn m31_m32_mock_source_has_no_live_dependency_or_execution_path() {
    let cargo = include_str!("../Cargo.toml");
    assert!(!cargo.contains("libloading") && !cargo.contains("autotune-probe"));
    for source in [
        include_str!("../src/engine.rs"),
        include_str!("../src/journal.rs"),
        include_str!("../src/manifest.rs"),
        include_str!("../src/mock.rs"),
        include_str!("../src/main.rs"),
    ] {
        for forbidden in [
            "unsafe {",
            "extern \"",
            "Command::new",
            "Library::new",
            "std::thread",
            "std::fs::",
        ] {
            assert!(!source.contains(forbidden), "{forbidden}");
        }
    }
    assert_eq!(run("success").unwrap().state, State::RestoredVerified);
}

#[test]
fn manifest_bound_ingress_rejects_foreign_session_and_stale_generation() {
    let mut e = arm();
    e.event(Event::Candidate {
        generation: 1,
        manifest_digest: "foreign-session".into(),
    });
    e.submit_candidate("foreign-session", 1);
    e.submit_candidate(&manifest(240_000).digest(), 0);
    assert!(setters(&e.report("foreign-command")).is_empty());
    e.submit_candidate(&manifest(240_000).digest(), 1);
    e.submit_candidate(&manifest(240_000).digest(), 1);
    finish(&mut e);
    let r = e.report("duplicate-command");
    invariants(&r);
    assert_eq!(setters(&r).len(), 2);
    assert!(r.failures.contains(&Failure::InvalidCommand));
}

#[test]
fn crash_at_every_retained_publication_is_read_only_and_blocks_reentry() {
    let r = run("success").unwrap();
    for count in 0..r.journal.records.len() {
        let mut j = r.journal.clone();
        j.records.truncate(count);
        j.unresolved = true;
        let before = serde_json::to_string(&j).unwrap();
        let reconciliation = j.reconcile(observation(Reading::Available(240_000)));
        assert!(reconciliation.unresolved);
        assert_eq!(reconciliation.setters_executed, 0);
        assert_eq!(before, serde_json::to_string(&j).unwrap());
        let mut m = manifest(240_000);
        m.nonce.push_str("-after-crash");
        assert!(Engine::arm(
            m.clone(),
            Some(Consent::simulated(&m)),
            &mut j,
            device(&m),
            500,
            100_500
        )
        .is_err());
    }
    let mut changed = observation(Reading::Available(215_000));
    changed.identities[0].pci = "0000:02:00.0".into();
    changed.bounds = Reading::Available(autotune_power_trial::manifest::Bounds {
        min_mw: 1,
        max_mw: 220_000,
    });
    let reconciliation = r.journal.reconcile(changed);
    assert!(
        !reconciliation.identity_matches
            && !reconciliation.bounds_match
            && !reconciliation.baseline_in_bounds
    );
    assert_eq!(reconciliation.setters_executed, 0);
}

#[test]
fn publication_faults_cross_cancellation_and_native_completion_boundaries() {
    for publication in 1..=14 {
        for phase in [
            FaultPhase::Before,
            FaultPhase::PartialWrite,
            FaultPhase::Flush,
            FaultPhase::Replace,
            FaultPhase::Readback,
            FaultPhase::After,
        ] {
            for at in [500, 501, 502, 503, 504, 753, 754, 755] {
                let m = manifest(240_000);
                let mut j = Journal {
                    fault: Some(Fault {
                        publication,
                        phase: phase.clone(),
                    }),
                    ..Default::default()
                };
                let mut d = device(&m);
                for op in [
                    Operation::Candidate,
                    Operation::CandidateRead,
                    Operation::Restore,
                    Operation::VerifyFirst,
                    Operation::VerifySecond,
                ] {
                    d.plans.push_back(plan(
                        op,
                        Some(1),
                        0,
                        match op {
                            Operation::Candidate => Some(m.target_mw),
                            Operation::Restore => Some(m.baseline_mw),
                            _ => None,
                        },
                        None,
                    ));
                }
                let Ok(mut e) = Engine::arm(
                    m.clone(),
                    Some(Consent::simulated(&m)),
                    &mut j,
                    d,
                    500,
                    100_500,
                ) else {
                    continue;
                };
                if at == 500 {
                    e.event(Event::Cancel);
                }
                e.submit_candidate(&m.digest(), 1);
                e.advance(at);
                e.event(Event::Cancel);
                finish(&mut e);
                invariants(&e.report("fault-cancel-cross-product"));
            }
        }
    }
}

#[test]
fn pre_entry_guardian_symbols_and_journal_revalidation_fail_closed() {
    for kind in 0..4 {
        let mut e = arm();
        match kind {
            0 => e.device.guardian_alive = false,
            1 => e.device.guardian_ready = false,
            2 => e.device.symbols_available = false,
            _ => e.journal.invalid = true,
        }
        e.submit_candidate(&manifest(240_000).digest(), 1);
        assert!(setters(&e.report("pre-entry-invalid")).is_empty());
    }
    let mut e = arm();
    e.advance(u64::MAX);
    e.submit_candidate(&manifest(240_000).digest(), 1);
    assert!(setters(&e.report("overflow-clock")).is_empty());
}

#[test]
fn repeated_cleanup_triggers_do_not_retry_successful_pending_verification() {
    let mut e = arm();
    e.submit_candidate(&manifest(240_000).digest(), 1);
    e.advance(500); // first restore and first baseline read have completed
    for _ in 0..10 {
        e.event(Event::Cancel);
        e.event(Event::ParentDeath);
        e.event(Event::IpcLoss);
    }
    e.advance(750);
    let r = e.report("cleanup-idempotence");
    invariants(&r);
    assert_eq!(r.restore_api.len(), 1);
    assert!(r.observed_restored);
    assert_eq!(
        r.verification_reads
            .iter()
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        vec![500, 750]
    );
}

#[test]
fn guardian_death_preserves_first_recovery_deadline_and_ignores_terminal_sessions() {
    for cancel_first in [false, true] {
        let mut e = arm();
        e.device
            .plans
            .push_back(plan(Operation::Candidate, None, 0, None, None));
        e.submit_candidate(&manifest(240_000).digest(), 1);
        if cancel_first {
            e.event(Event::Cancel);
        }
        e.advance(750);
        e.event(Event::GuardianDeath);
        let deadline = if cancel_first { 5500 } else { 5750 };
        e.advance(deadline - 1);
        e.event(Event::GuardianDeath);
        assert!(!e.is_terminal());
        e.advance(deadline);
        assert!(e.is_terminal());
        assert!(e.report("").failures.contains(&Failure::RecoveryDeadline));
        e.advance(deadline + 1000);
        e.event(Event::GuardianDeath);
        assert!(e.is_terminal());
        assert!(e.journal.unresolved);
    }
    for restored in [false, true] {
        let mut e = arm();
        if restored {
            e.submit_candidate(&manifest(240_000).digest(), 1);
            finish(&mut e);
        } else {
            e.event(Event::Cancel);
        }
        let before = serde_json::to_string(&e.report("terminal")).unwrap();
        e.event(Event::GuardianDeath);
        e.event(Event::GuardianDeath);
        assert_eq!(
            before,
            serde_json::to_string(&e.report("terminal")).unwrap()
        );
    }
}

#[test]
fn incomplete_prefixes_and_historical_nonce_deletion_fail_journal_validation() {
    let first = run("success").unwrap();
    for count in 0..first.journal.records.len() {
        let mut j = first.journal.clone();
        j.records.truncate(count);
        j.unresolved = false;
        assert!(!j.validate(), "prefix {count}");
        let mut m = manifest(240_000);
        m.nonce.push_str("-next");
        assert_eq!(j.begin(&m, 500, 100_500), Err(Rejection::JournalBlocked));
    }
    let mut j = first.journal;
    let mut m = manifest(240_000);
    m.nonce.push_str("-second");
    let mut e = Engine::arm(
        m.clone(),
        Some(Consent::simulated(&m)),
        &mut j,
        device(&m),
        500,
        100_500,
    )
    .unwrap();
    e.submit_candidate(&m.digest(), 1);
    finish(&mut e);
    let mut j = e.report("second").journal;
    assert!(j.validate() && !j.unresolved);
    assert!(j.consumed_nonces.remove(&manifest(240_000).nonce));
    assert!(!j.validate());
    assert_eq!(
        j.begin(&manifest(240_000), 500, 100_500),
        Err(Rejection::JournalBlocked)
    );
}

#[test]
fn symbol_loss_after_candidate_or_before_final_verification_stops_native_entries() {
    for final_verification in [false, true] {
        let mut e = arm();
        if !final_verification {
            e.device
                .plans
                .push_back(plan(Operation::Candidate, Some(1), 0, Some(235_000), None));
        }
        e.submit_candidate(&manifest(240_000).digest(), 1);
        if final_verification {
            e.advance(500);
        }
        let entries = e
            .report("")
            .trace
            .iter()
            .filter(|t| t.event == "native_entry")
            .count();
        e.device.symbols_available = false;
        finish(&mut e);
        let r = e.report("symbol-loss");
        assert_eq!(
            entries,
            r.trace.iter().filter(|t| t.event == "native_entry").count()
        );
        assert!(r.failures.contains(&Failure::Rejected(Rejection::Symbols)));
        assert!(r.journal.unresolved && !r.observed_restored);
        invariants(&r);
    }
}

#[test]
fn candidate_api_or_result_storage_failure_skips_blocked_optional_readback() {
    for storage_failure in [false, true] {
        let mut e = arm();
        let status = if storage_failure { 0 } else { 4 };
        e.device.plans.push_back(plan(
            Operation::Candidate,
            Some(0),
            status,
            Some(235_000),
            None,
        ));
        e.device
            .plans
            .push_back(plan(Operation::CandidateRead, None, 0, None, None));
        if storage_failure {
            e.journal.fault = Some(Fault {
                publication: 5,
                phase: FaultPhase::Flush,
            });
        }
        e.submit_candidate(&manifest(240_000).digest(), 1);
        e.advance(750);
        let r = e.report("immediate-cleanup");
        assert_eq!(r.candidate_api, Some(status));
        assert!(r.candidate_readback.is_none());
        assert!(!r
            .trace
            .iter()
            .any(|t| t.operation == Some(Operation::CandidateRead)));
        assert_eq!(setters(&r)[1].mono_ms, 500);
        assert!(r.observed_restored);
        assert_eq!(r.evidence_durable, !storage_failure);
        invariants(&r);
    }
}

#[test]
fn reconciliation_selects_exact_identity_among_unrelated_devices_and_rejects_duplicates() {
    let j = run("success").unwrap().journal;
    let mut o = observation(Reading::Available(240_000));
    let mut unrelated = o.identities[0].clone();
    unrelated.uuid = "unrelated".into();
    o.identities.insert(0, unrelated);
    assert!(j.reconcile(o.clone()).identity_matches);
    let mut duplicate = o.clone();
    duplicate.identities.push(o.identities[1].clone());
    assert!(!j.reconcile(duplicate).identity_matches);
    o.identities[1].pci = "0000:02:00.0".into();
    assert!(!j.reconcile(o).identity_matches);
}

#[test]
fn low_baseline_fixtures_are_invalid_without_subtraction_panics() {
    for baseline in [0, 1, 999, 4999] {
        let m = manifest(baseline);
        assert_eq!(m.target_mw, 0);
        assert!(m.validate().is_err());
    }
}

#[test]
fn explicit_utc_approval_expiry_blocks_entry_while_capture_and_admission_are_fresh() {
    let mut m = manifest(240_000);
    m.approval_expiry_utc_ms = 100_750;
    let mut e = Engine::arm(
        m.clone(),
        Some(Consent::simulated(&m)),
        &mut Journal::default(),
        device(&m),
        500,
        100_500,
    )
    .unwrap();
    e.advance(750);
    e.submit_candidate(&m.digest(), 1);
    let r = e.report("utc-expiry");
    assert!(r
        .failures
        .contains(&Failure::Rejected(Rejection::ExpiredConsent)));
    assert!(setters(&r).is_empty());
    assert_eq!(r.state, State::AbortedNoWrite);
}
