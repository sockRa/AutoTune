#![allow(dead_code)]
use autotune_recovery_contract::*;

pub fn preparation() -> Preparation {
    Preparation {
        version: VERSION,
        session: [1; 32],
        nonce: [2; 32],
        device: Device {
            uuid: "SYNTHETIC-test-device".into(),
            pci: "0000:01:00.0".into(),
            vendor: 1,
            device: 2,
            subsystem: 3,
            model: "fake".into(),
        },
        protocol: [3; 32],
        executables: [[4; 32]; 3],
        operator: "simulated-operator".into(),
        boot: [5; 32],
        created_ms: 0,
        synthetic: true,
    }
}
pub fn runtime() -> Runtime {
    Runtime {
        windows_build: "synthetic-build".into(),
        driver: "synthetic-driver".into(),
        api_version: "synthetic-v1".into(),
        header: "synthetic-header".into(),
        module_path: "synthetic:no-dll".into(),
        privilege: "unelevated-synthetic".into(),
        setter: "synthetic-set-configured-limit".into(),
        getter: "synthetic-get-configured-limit".into(),
    }
}
pub fn manifest(baseline: u32) -> Manifest {
    let preparation = preparation();
    let process = ProcessIdentity {
        pid: 1,
        creation_100ns: 2,
        binary: [4; 32],
        logon_sid: "synthetic-logon".into(),
        integrity: 1,
    };
    Manifest {
        preparation_digest: preparation.digest().unwrap(),
        preparation,
        capture: Capture {
            readings_mw: [baseline; 2],
            sampled_ms: [0, 500],
            trace: [6; 32],
            minimum_mw: 100_000,
            maximum_mw: 300_000,
            default_mw: Some(215_000),
        },
        target_mw: baseline - 5000,
        delta_mw: 5000,
        runtime: runtime(),
        supervision: Supervision {
            guardian: process.clone(),
            coordinator: process.clone(),
            worker: process,
            root: "synthetic:test-root".into(),
            acl_expectation: [7; 32],
            lock_identity: [8; 32],
            other_tools_stopped_simulated: true,
            escalation: "report unresolved; never replay".into(),
            risks: "synthetic only; no physical restoration claim".into(),
        },
        approval_expiry_utc_ms: 60_000,
        approval_challenge: [9; 32],
        policy_version: VERSION,
    }
}
pub fn approval(m: &Manifest) -> SimulatedApproval {
    SimulatedApproval {
        manifest: m.digest().unwrap(),
        nonce: m.preparation.nonce,
        challenge: m.approval_challenge,
        operator: m.preparation.operator.clone(),
        full_and_summary_presented: true,
    }
}
pub fn frame() -> Frame {
    Frame {
        version: VERSION,
        from: Role::Guardian,
        to: Role::Worker,
        session: [1; 32],
        digest: [2; 32],
        generation: 1,
        sequence: 1,
        operation_id: 0,
        message: Message::Heartbeat,
    }
}
