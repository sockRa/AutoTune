use crate::engine::{CallPlan, Device, Engine, Event, Failure, Operation, Report, State};
use crate::journal::{Fault, FaultPhase, Journal};
use crate::manifest::{Bounds, Consent, Identity, Manifest, Reading, Timers};

pub const SCENARIOS: &[&str] = &[
    "success",
    "baseline-230000",
    "missing-consent",
    "changed-manifest",
    "stale-consent",
    "replayed-consent",
    "ambiguous-device",
    "drift",
    "default-unavailable",
    "invalid-bounds",
    "missing-symbol",
    "guardian-unready",
    "error-before-change",
    "error-after-change",
    "clamped-readback",
    "unavailable-readback",
    "restore-retry",
    "restore-error-observed",
    "restore-failed",
    "verification-drift",
    "candidate-hang",
    "late-completion",
    "getter-hang",
    "restore-hang",
    "cancel-before-entry",
    "cancel-inflight",
    "parent-death",
    "heartbeat-loss",
    "lease-expiry",
    "guardian-death",
    "identity-change",
    "bounds-change",
    "ownership-loss",
    "storage-before",
    "storage-after",
    "storage-close",
    "corrupt-journal",
    "clock-discontinuity",
    "restart",
];

pub fn manifest(baseline: u32) -> Manifest {
    Manifest {
        schema: 1, experiment: "synthetic-experiment-001".into(), nonce: "synthetic-nonce-not-secret-001".into(),
        protocol_digest: "synthetic-protocol-v1".into(), executable_digest: "synthetic-executable".into(),
        guardian_digest: "synthetic-guardian".into(), worker_digest: "synthetic-worker".into(),
        approval_contract_digest: "synthetic-exact-single-use-contract-v1".into(), approval_expiry_utc_ms: 160_500,
        operator: "synthetic-operator".into(),
        identity: Identity { uuid: "GPU-SYNTHETIC-001".into(), pci: "0000:01:00.0".into(), vendor: 0x10de, device: 0x1e84, subsystem: 1, model: "Synthetic GPU (not hardware evidence)".into() },
        windows_build: "simulated-windows-build".into(), driver_version: "simulated-driver".into(),
        nvml_version: "simulated-nvml".into(), header_source: "documented API contract; no native resolver".into(),
        trusted_dll_path: "simulated:System32/nvml.dll (never loaded)".into(), privilege: "simulated-normal-user".into(),
        baseline_readings_mw: [Reading::Available(baseline), Reading::Available(baseline)],
        capture_mono_ms: [0, 500], capture_utc_ms: [100_000, 100_500], baseline_mw: baseline,
        bounds: Bounds { min_mw: 107_500, max_mw: 240_000 }, default_mw: Reading::Available(215_000),
        target_mw: baseline.saturating_sub(5_000), delta_mw: 5_000,
        setter: "nvmlDeviceSetPowerManagementLimit".into(), getter: "nvmlDeviceGetPowerManagementLimit".into(),
        candidate_attempts: 1, restore_attempts: 2, timers: Timers::default(),
        guardian_identity: "synthetic-guardian:creation-1".into(), parent_identity: "synthetic-parent:creation-2".into(),
        worker_identity: "synthetic-worker:creation-3".into(), boot_identity: "synthetic-boot-1".into(),
        journal_location: "simulated:protected-local-journal (memory only)".into(), acl_expectations: "simulated operator/guardian only; no redirected paths".into(),
        ownership_inspection: "simulated per-device lock and external-tool inspection".into(), other_tools_stopped: true,
        risk_acknowledgement: "simulated acceptance: display disruption, GPU loss, baseline unqualified, unsuccessful restoration and no guaranteed crash/reboot recovery".into(),
        supervised_escalation: "simulation only; live procedure requires separate human approval".into(), provenance: "simulated".into(),
    }
}

pub fn device(m: &Manifest) -> Device {
    Device {
        devices: vec![(m.identity.clone(), "synthetic-handle-7".into())],
        bounds: m.bounds.clone(),
        current: Reading::Available(m.baseline_mw),
        runtime: m.driver_version.clone(),
        owned: true,
        symbols_available: true,
        guardian_ready: true,
        guardian_alive: true,
        plans: Default::default(),
    }
}

pub fn plan(
    operation: Operation,
    duration_ms: Option<u64>,
    status: i32,
    change_to: Option<u32>,
    reading: Option<Reading<u32>>,
) -> CallPlan {
    CallPlan {
        operation,
        duration_ms,
        status,
        change_to,
        reading,
    }
}

pub fn run(scenario: &str) -> Option<Report> {
    if !SCENARIOS.contains(&scenario) {
        return None;
    }
    let mut m = manifest(if scenario == "baseline-230000" {
        230_000
    } else {
        240_000
    });
    let mut consent = Some(Consent::simulated(&m));
    let mut d = device(&m);
    let mut j = Journal::default();
    let mut now = 500;
    match scenario {
        "missing-consent" => consent = None,
        "changed-manifest" => m.privilege = "changed-after-consent".into(),
        "stale-consent" => now = 60_500,
        "replayed-consent" => {
            j.consumed_nonces.insert(m.nonce.clone());
        }
        "ambiguous-device" => d.devices.push(d.devices[0].clone()),
        "drift" => d.current = Reading::Available(m.baseline_mw - 1),
        "default-unavailable" => {
            m.default_mw = Reading::Unavailable("unsupported".into());
            consent = Some(Consent::simulated(&m));
        }
        "invalid-bounds" => m.bounds.min_mw = 0,
        "missing-symbol" => d.symbols_available = false,
        "guardian-unready" => d.guardian_ready = false,
        "error-before-change" => {
            d.plans
                .push_back(plan(Operation::Candidate, Some(0), 4, None, None))
        }
        "error-after-change" => d.plans.push_back(plan(
            Operation::Candidate,
            Some(0),
            4,
            Some(m.target_mw),
            None,
        )),
        "clamped-readback" => d.plans.push_back(plan(
            Operation::Candidate,
            Some(0),
            0,
            Some(m.target_mw + 1),
            None,
        )),
        "unavailable-readback" => d.plans.push_back(plan(
            Operation::CandidateRead,
            Some(0),
            0,
            None,
            Some(Reading::Unavailable("denied".into())),
        )),
        "restore-retry" => d
            .plans
            .push_back(plan(Operation::Restore, Some(0), 4, None, None)),
        "restore-error-observed" => d.plans.push_back(plan(
            Operation::Restore,
            Some(0),
            4,
            Some(m.baseline_mw),
            None,
        )),
        "restore-failed" => {
            d.plans
                .push_back(plan(Operation::Restore, Some(0), 4, None, None));
            d.plans
                .push_back(plan(Operation::Restore, Some(0), 4, None, None));
        }
        "verification-drift" => {
            d.plans.push_back(plan(
                Operation::VerifySecond,
                Some(0),
                0,
                Some(m.baseline_mw - 1),
                None,
            ));
            d.plans.push_back(plan(
                Operation::VerifySecond,
                Some(0),
                0,
                Some(m.baseline_mw - 1),
                None,
            ));
        }
        "candidate-hang" | "guardian-death" | "heartbeat-loss" => {
            d.plans
                .push_back(plan(Operation::Candidate, None, 0, Some(m.target_mw), None))
        }
        "late-completion" => d.plans.push_back(plan(
            Operation::Candidate,
            Some(7_000),
            0,
            Some(m.target_mw),
            None,
        )),
        "getter-hang" => d
            .plans
            .push_back(plan(Operation::CandidateRead, None, 0, None, None)),
        "restore-hang" => {
            d.plans
                .push_back(plan(Operation::Restore, None, 0, Some(m.baseline_mw), None))
        }
        "cancel-inflight"
        | "parent-death"
        | "identity-change"
        | "bounds-change"
        | "ownership-loss"
        | "clock-discontinuity" => d.plans.push_back(plan(
            Operation::Candidate,
            Some(100),
            0,
            Some(m.target_mw),
            None,
        )),
        "storage-before" => {
            j.fault = Some(Fault {
                publication: 3,
                phase: FaultPhase::Flush,
            })
        }
        "storage-after" => {
            j.fault = Some(Fault {
                publication: 5,
                phase: FaultPhase::Replace,
            })
        }
        "storage-close" => {
            j.fault = Some(Fault {
                publication: 14,
                phase: FaultPhase::Flush,
            })
        }
        "corrupt-journal" => j.invalid = true,
        "restart" => {
            j.manifest = Some(m.clone());
            j.unresolved = true;
        }
        _ => {}
    }
    let mut e = match Engine::arm(m.clone(), consent, &mut j, d, now, 100_000 + now) {
        Ok(e) => e,
        Err(reason) => {
            return Some(Report {
                schema: 1,
                provenance: "simulated".into(),
                scenario: scenario.into(),
                manifest_digest: m.digest(),
                manifest: m,
                state: State::AbortedNoWrite,
                candidate_api: None,
                candidate_readback: None,
                restore_api: vec![],
                verification_reads: vec![],
                observed_restored: false,
                evidence_durable: false,
                overall: "rejected".into(),
                failures: vec![Failure::Rejected(reason)],
                trace: vec![],
                journal: j,
            })
        }
    };
    if scenario == "cancel-before-entry" {
        e.event(Event::Cancel);
    } else if scenario == "lease-expiry" {
        for _ in 0..40 {
            e.advance(e.now + 250);
            e.event(Event::Heartbeat);
        }
    } else {
        e.submit_candidate(&m.digest(), 1);
        match scenario {
            "cancel-inflight" => e.event(Event::Cancel),
            "parent-death" => e.event(Event::ParentDeath),
            "guardian-death" => e.event(Event::GuardianDeath),
            "clock-discontinuity" => e.event(Event::ClockDiscontinuity),
            "identity-change" => e.device.devices[0].0.pci = "0000:02:00.0".into(),
            "bounds-change" => e.device.bounds.max_mw -= 1,
            "ownership-loss" => e.device.owned = false,
            _ => {}
        }
    }
    // Finite deterministic event loop; heartbeat cannot renew the lease.
    for _ in 0..64 {
        e.advance(e.now + 250);
        if scenario != "heartbeat-loss" {
            e.event(Event::Heartbeat);
        }
        if e.is_terminal() && !e.has_pending() {
            break;
        }
    }
    Some(e.report(scenario))
}
