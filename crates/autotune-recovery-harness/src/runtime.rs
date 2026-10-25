//! Fixed fake roles. Diagnostic capture is read-only; no arm bypass or native backend.
use autotune_recovery_contract::{process::*, storage::*, *};
use autotune_recovery_core::{worker::WorkerPolicy, Phase};
use autotune_recovery_windows::{
    ipc::{Channel, RpcLane},
    process::{Result, *},
    storage::{Store, TestRoot},
    storage_executor::StorageExecutor,
};
use std::{os::windows::io::AsHandle, path::Path, thread, time::Duration};

pub fn fake_profile() -> FakeProfile {
    FakeProfile {
        device: Device {
            uuid: "SYNTHETIC-owned-worker".into(),
            pci: "0000:01:00.0".into(),
            vendor: 1,
            device: 2,
            subsystem: 3,
            model: "owned fake device".into(),
        },
        runtime: Runtime {
            windows_build: "synthetic-build".into(),
            driver: "synthetic-driver".into(),
            api_version: "synthetic-v1".into(),
            header: "synthetic-header".into(),
            module_path: "synthetic:no-dll".into(),
            privilege: "unelevated-synthetic".into(),
            setter: "synthetic-set-configured-limit".into(),
            getter: "synthetic-get-configured-limit".into(),
        },
        baseline_mw: 247_123,
        minimum_mw: 100_000,
        maximum_mw: 300_000,
        default_mw: Some(215_000),
        behavior: FakeBehavior::Normal,
    }
}
pub fn ipc_check() -> Result<RoleStatus> {
    let image = Executable::isolated_current_copy()?;
    let child = launch_verifier(image, rng()?, fake_profile(), false, None)?;
    let mut channel = match child.server.accept(
        &child.bootstrap,
        child.process.as_handle(),
        boot_ms() + 5000,
    ) {
        Ok(c) => c,
        Err(e) => {
            child.process.terminate_owned()?;
            child.process.wait(5000)?;
            return Err(e);
        }
    };
    let Message::Status(initial) = channel.receive(Some(boot_ms() + 5000))?.message else {
        return Err(ProcessError::Policy("verifier status"));
    };
    if initial.process != child.process.identity || initial.role != Role::Verifier {
        return Err(ProcessError::Policy("verifier identity"));
    }
    channel.send(Message::Control(Control::Stop), 0, 0, boot_ms() + 1000)?;
    let Message::Status(final_status) = channel.receive(Some(boot_ms() + 1000))?.message else {
        return Err(ProcessError::Policy("verifier stop"));
    };
    if !child.process.wait(5000)? {
        child.process.terminate_owned()?;
        return Err(ProcessError::Policy("verifier shutdown"));
    }
    Ok(*final_status)
}
pub fn process_case(case: ProcessCase) -> Result<ProcessCheckReport> {
    let mut report = ProcessCheckReport {
        case,
        passed: false,
        blocker: None,
        rejection: None,
        host_in_job: in_any_job(current_process_handle())?,
        host_job_flags: current_job_limits()?,
        child: None,
        sibling: None,
        zero_device_permissions: true,
    };
    let image = Executable::isolated_current_copy()?;
    if case == ProcessCase::PipeSquatter {
        let identity = current_identity()?;
        let server = autotune_recovery_windows::ipc::Server::new(&identity)?;
        server.audit_security(&identity)?;
        match autotune_recovery_windows::ipc::Server::with_name(server.name.clone(), &identity) {
            Err(ProcessError::Win32(5)) => {
                report.passed = true;
                report.rejection = Some("first-instance pipe squatter rejected (5)".into());
            }
            _ => return Err(ProcessError::Policy("pipe squatter not rejected")),
        }
        return Ok(report);
    }
    if case == ProcessCase::IsolationGate {
        let root = TestRoot::provision_fixture()?;
        match launch(
            image,
            LaunchRequest {
                parent_role: Role::Launcher,
                role: Role::Guardian,
                session: rng()?,
                profile: fake_profile(),
                root: Some(&root),
                custody: None,
                job: None,
            },
        ) {
            Err(ProcessError::Win32(5))
                if report.host_in_job && report.host_job_flags.is_some_and(|f| f & 0x1800 == 0) =>
            {
                report.passed = true;
                report.rejection =
                    Some("CREATE_BREAKAWAY_FROM_JOB denied (5); zero readiness/permissions".into());
                report.blocker=Some("host enclosing job forbids breakaway; independent guardian/worker acceptance cannot run here".into());
                return Ok(report);
            }
            Ok(child) => {
                child.process.terminate_owned()?;
                child.process.wait(5000)?;
                report.passed = true;
                return Ok(report);
            }
            Err(e) => return Err(e),
        }
    }
    let job = if case == ProcessCase::JobClose {
        Some(OwnedJob::new(false)?)
    } else {
        None
    };
    let child = launch_verifier(
        image.clone(),
        rng()?,
        fake_profile(),
        case == ProcessCase::ConsoleClose,
        job.as_ref(),
    )?;
    report.child = Some(child.process.identity.clone());
    let mut bootstrap = child.bootstrap.clone();
    if case == ProcessCase::WrongPeer {
        bootstrap.context.child.process.pid = current_identity()?.process.pid;
    }
    if case == ProcessCase::WrongMac {
        bootstrap.key[0] ^= 1;
    }
    let channel = child
        .server
        .accept(&bootstrap, child.process.as_handle(), boot_ms() + 5000);
    if matches!(case, ProcessCase::WrongPeer | ProcessCase::WrongMac) {
        match channel {
            Err(e) => {
                report.rejection = Some(format!("{e:?}"));
                report.passed = true;
            }
            Ok(_) => {
                child.process.terminate_owned()?;
                return Err(ProcessError::Policy("hostile peer/MAC accepted"));
            }
        }
        if !child.process.wait(5000)? {
            child.process.terminate_owned()?;
            child.process.wait(5000)?;
        }
        return Ok(report);
    }
    let mut channel = match channel {
        Ok(c) => c,
        Err(e) => {
            child.process.terminate_owned()?;
            child.process.wait(5000)?;
            return Err(e);
        }
    };
    let Message::Status(initial) = channel.receive(Some(boot_ms() + 5000))?.message else {
        return Err(ProcessError::Policy("measured verifier status"));
    };
    if initial.process != child.process.identity || initial.role != Role::Verifier {
        return Err(ProcessError::Policy("retained process identity mismatch"));
    }
    if let ProcessCase::Wire(fault) = case {
        channel.send(
            Message::Control(Control::Fault(fault)),
            0,
            0,
            boot_ms() + 1000,
        )?;
        match channel.receive(Some(boot_ms() + 1500)) {
            Err(e) => {
                report.rejection = Some(format!("{e:?}"));
                report.passed = true;
            }
            Ok(_) => return Err(ProcessError::Policy("hostile frame accepted")),
        }
        if channel
            .send(Message::Control(Control::Observe), 0, 0, boot_ms() + 1000)
            .is_ok()
        {
            return Err(ProcessError::Policy("failed channel reconnected"));
        }
        drop(channel);
        if !child.process.wait(5000)? {
            child.process.terminate_owned()?;
            child.process.wait(5000)?;
        }
        return Ok(report);
    }
    if case == ProcessCase::JobClose {
        let sibling = launch_verifier(image, rng()?, fake_profile(), false, None)?;
        let mut sc = sibling.server.accept(
            &sibling.bootstrap,
            sibling.process.as_handle(),
            boot_ms() + 5000,
        )?;
        let _ = sc.receive(Some(boot_ms() + 5000))?;
        report.sibling = Some(sibling.process.identity.clone());
        if !job.as_ref().unwrap().contains(child.process.as_handle())?
            || job
                .as_ref()
                .unwrap()
                .contains(sibling.process.as_handle())?
        {
            return Err(ProcessError::Policy("owned job membership"));
        }
        drop(job);
        if !child.process.wait(5000)? || !sibling.process.alive() {
            return Err(ProcessError::Policy("job closure did not isolate sibling"));
        }
        sc.send(Message::Control(Control::Stop), 0, 0, boot_ms() + 1000)?;
        let _ = sc.receive(Some(boot_ms() + 1000))?;
        if !sibling.process.wait(5000)? {
            sibling.process.terminate_owned()?;
            return Err(ProcessError::Policy("sibling shutdown"));
        }
        report.passed = true;
        return Ok(report);
    }
    if case == ProcessCase::ConsoleClose {
        match close_owned_console(&child.process, &initial) {
            Ok(()) => report.passed = true,
            Err(e) => {
                report.blocker = Some(format!("owned console closure unavailable: {e:?}"));
                child.process.terminate_owned()?;
                child.process.wait(5000)?;
            }
        }
        return Ok(report);
    }
    // Verifier cannot prepare/arm, even though authentication succeeded.
    channel.send(
        Message::Control(Control::ProbeIsolation),
        0,
        0,
        boot_ms() + 1000,
    )?;
    let response = channel.receive(Some(boot_ms() + 1000))?;
    if report.host_in_job && !matches!(response.message, Message::Rejected { .. }) {
        return Err(ProcessError::Policy(
            "job-contained process reported independent readiness",
        ));
    }
    channel.send(Message::Control(Control::Stop), 0, 0, boot_ms() + 1000)?;
    let _ = channel.receive(Some(boot_ms() + 1000))?;
    if !child.process.wait(5000)? {
        child.process.terminate_owned()?;
        return Err(ProcessError::Policy("verifier shutdown"));
    }
    report.passed = true;
    Ok(report)
}
fn status(role: Role) -> Result<RoleStatus> {
    let process = current_identity()?;
    let (no_console, console_window, console_processes) = console_state()?;
    Ok(RoleStatus {
        role,
        no_job: !in_any_job(current_process_handle())?,
        process,
        no_console,
        console_window,
        console_processes,
        worker: None,
        coordinator: None,
        candidate_entries: 0,
        restore_entries: 0,
        pending: None,
        recovery: None,
    })
}
fn rng() -> Result<Hash> {
    autotune_recovery_windows::random().map_err(|_| ProcessError::Policy("CNG RNG"))
}
// Opt-in diagnostics are bounded observations, not crash-persistent evidence.
thread_local! {
    static DIAGNOSTICS: std::cell::RefCell<(Vec<String>, usize, usize)> = const {
        std::cell::RefCell::new((Vec::new(), 0, 0))
    };
}
fn diag(message: &str) {
    if std::env::var_os("AUTOTUNE_RECOVERY_DIAG").is_none() {
        return;
    }
    // Diagnostics must not open/write files between a publication request and
    // its 100 ms acknowledgement deadline. They are optional observations, not
    // journal records: bounded RAM capture is flushed only after the role ends.
    DIAGNOSTICS.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        if buffer.0.len() < 1024 && buffer.1.saturating_add(message.len()) <= 256 * 1024 {
            buffer.0.push(message.to_owned());
            buffer.1 += message.len();
        } else {
            buffer.2 += 1;
        }
    });
}
fn flush_diagnostics() {
    if let Ok(base) = std::env::var("AUTOTUNE_RECOVERY_DIAG") {
        use std::io::Write;
        let path = format!("{base}.{}.log", std::process::id());
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let mut file = std::io::BufWriter::new(file);
            DIAGNOSTICS.with(|buffer| {
                let mut buffer = buffer.borrow_mut();
                for message in buffer.0.drain(..) {
                    let _ = writeln!(file, "{message}");
                }
                if buffer.2 != 0 {
                    let _ = writeln!(file, "diagnostics_truncated_events={}", buffer.2);
                }
                buffer.1 = 0;
                buffer.2 = 0;
            });
            let _ = file.flush();
        }
    }
}
fn publish(storage: &mut StorageExecutor, envelope: JournalEnvelope) -> Result<JournalEnvelope> {
    storage.submit(envelope, None)?;
    loop {
        match storage.poll() {
            Ok(Some(scan)) => return Ok(scan.active),
            Ok(None) => thread::yield_now(),
            Err(e) => return Err(e.into()),
        }
    }
}
fn next(
    previous: &JournalEnvelope,
    edit: impl FnOnce(&mut StorageSnapshot),
) -> Result<JournalEnvelope> {
    let JournalPayload::Snapshot(mut s) = previous.payload.clone() else {
        return Err(ProcessError::Policy("snapshot expected"));
    };
    s.sequence = s
        .sequence
        .checked_add(1)
        .ok_or(ProcessError::Policy("record overflow"))?;
    s.previous = previous.checksum;
    edit(&mut s);
    Ok(JournalEnvelope::new(JournalPayload::Snapshot(s))?)
}
fn rpc(lane: &mut RpcLane, message: Message, generation: u64, id: u64) -> Result<Frame> {
    lane.submit(message, generation, id)?;
    let deadline = boot_ms() + 6000;
    loop {
        if let Some(reply) = lane.poll() {
            return reply;
        }
        if boot_ms() >= deadline {
            return Err(ProcessError::Policy(
                "RPC unresolved; no successor permitted",
            ));
        }
        thread::yield_now();
    }
}

/// Drain reducer effects through the single storage publisher. A returned
/// dispatch is a request to call the worker; it is never itself an ack.
fn execute_expected(
    actor: &mut autotune_recovery_core::journal::JournaledGuardian,
    storage: &mut StorageExecutor,
    lane: &mut RpcLane,
    heartbeats: &std::sync::mpsc::Receiver<Frame>,
    operation: Operation,
) -> Result<Completion> {
    let outcome = execute_fake(actor, storage, lane, heartbeats, operation)?;
    if matches!(outcome, Completion::NotEntered { .. }) {
        return Err(ProcessError::Policy(
            "unexpected fake entry rejection during the round trip",
        ));
    }
    Ok(outcome)
}

fn drive_effects(
    actor: &mut autotune_recovery_core::journal::JournaledGuardian,
    storage: &mut StorageExecutor,
    effects: Vec<autotune_recovery_core::journal::JournalEffect>,
    dispatch: &mut Option<DispatchProof>,
) -> Result<()> {
    use autotune_recovery_core::journal::JournalEffect;
    let mut queue: std::collections::VecDeque<JournalEffect> = effects.into();
    while let Some(effect) = queue.pop_front() {
        match effect {
            JournalEffect::Publish(envelope) => {
                diag(&format!("publish seq {}", envelope.sequence()));
                let cleanup = actor.guardian().cleanup_deadline();
                let submitted = storage.submit((*envelope).clone(), cleanup).is_ok();
                let reply = if submitted {
                    loop {
                        match storage.poll() {
                            Ok(Some(scan)) => break Some(scan),
                            Ok(None) => thread::sleep(Duration::from_millis(1)),
                            Err(_) => break None,
                        }
                    }
                } else {
                    None
                };
                let (now, utc) = sample_time()?;
                match reply {
                    Some(scan) => {
                        let effects = actor.acknowledge(scan.active.checksum, now, utc)?;
                        // A storage completion is not necessarily a timely actor
                        // acknowledgement. Retain the actual policy disposition.
                        diag(&format!("publication completion at {now}: seq={} phase={:?} storage_failed={} faults={:?}",
                            scan.active.sequence(), actor.guardian().phase(), actor.publication_failed(), actor.guardian().evidence().faults));
                        queue.extend(effects)
                    }
                    None => {
                        diag("publish failed/timeout");
                        queue.extend(actor.storage_failure(now, utc)?)
                    }
                }
            }
            JournalEffect::Dispatch(proof) => {
                if dispatch.is_some() {
                    return Err(ProcessError::Policy("overlapping dispatch effect"));
                }
                *dispatch = Some(*proof);
            }
            JournalEffect::Alert(_) => (),
        }
    }
    Ok(())
}

fn service(
    actor: &mut autotune_recovery_core::journal::JournaledGuardian,
    storage: &mut StorageExecutor,
    heartbeats: &std::sync::mpsc::Receiver<Frame>,
) -> Result<()> {
    for _ in 0..8 {
        match heartbeats.try_recv() {
            Ok(_) => {
                let (now, utc) = sample_time()?;
                let effects = actor.heartbeat(now, utc)?;
                let mut none = None;
                drive_effects(actor, storage, effects, &mut none)?;
                if none.is_some() {
                    return Err(ProcessError::Policy("heartbeat produced a device dispatch"));
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }
    Ok(())
}

fn wait_serviced(
    actor: &mut autotune_recovery_core::journal::JournaledGuardian,
    storage: &mut StorageExecutor,
    heartbeats: &std::sync::mpsc::Receiver<Frame>,
    ms: u64,
) -> Result<()> {
    let deadline = boot_ms()
        .checked_add(ms)
        .ok_or(ProcessError::Policy("wait overflow"))?;
    while boot_ms() < deadline {
        service(actor, storage, heartbeats)?;
        thread::sleep(Duration::from_millis(5));
    }
    service(actor, storage, heartbeats)
}

/// One explicit fake operation: persist intent, dispatch only after the storage
/// receipt, wait for the authenticated atomic completion, persist it, then send
/// the exact receipt. There is no hidden getter inside this call.
fn execute_fake(
    actor: &mut autotune_recovery_core::journal::JournaledGuardian,
    storage: &mut StorageExecutor,
    lane: &mut RpcLane,
    heartbeats: &std::sync::mpsc::Receiver<Frame>,
    operation: Operation,
) -> Result<Completion> {
    service(actor, storage, heartbeats)?;
    let (now, utc) = sample_time()?;
    diag(&format!("reserve {operation:?} at {now}"));
    let effects = actor.reserve(operation, now, utc)?;
    let mut dispatch = None;
    drive_effects(actor, storage, effects, &mut dispatch)?;
    let proof = dispatch.ok_or_else(|| {
        diag(&format!("dispatch refused at {}: operation={operation:?} phase={:?} storage_failed={} pending={} faults={:?}",
            boot_ms(), actor.guardian().phase(), actor.publication_failed(), actor.guardian().pending().is_some(), actor.guardian().evidence().faults));
        ProcessError::Policy("intent publication did not authorize dispatch")
    })?;
    notify_worker(actor, lane)?;
    diag(&format!(
        "dispatch {operation:?} id {}",
        proof.permit.operation_id
    ));
    let generation = proof.permit.generation;
    let id = proof.permit.operation_id;
    lane.submit(Message::Dispatch(Box::new(proof.clone())), generation, id)?;
    let mut notification = actor.guardian().recovery_window().map(|window| {
        (
            actor.guardian().generation(),
            window,
            actor.publication_failed(),
        )
    });
    let mut expected_acks = lane.control_acknowledgements();
    let reply = loop {
        // Storage and worker I/O run off-loop. Watchdog/heartbeat checks continue
        // while a call is blocked; none of them retire its pending permit.
        service(actor, storage, heartbeats)?;
        let (now, utc) = sample_time()?;
        let effects = actor.tick(now, utc)?;
        let mut unexpected = None;
        drive_effects(actor, storage, effects, &mut unexpected)?;
        if unexpected.is_some() {
            return Err(ProcessError::Policy("tick overlapped pending call"));
        }
        if let Some(window) = actor.guardian().recovery_window() {
            let state = (
                actor.guardian().generation(),
                window.clone(),
                actor.publication_failed(),
            );
            if notification.as_ref() != Some(&state) {
                let count = lane.notify_pending(state.0, window, state.2)?;
                if count != 0 {
                    expected_acks += count;
                    notification = Some(state);
                }
            }
            // The blocked-call fixture requires actual authenticated worker
            // acknowledgement before releasing its late effect. The control
            // acknowledgement itself never retires the device permit.
            if notification.is_some() && lane.control_acknowledgements() >= expected_acks {
                lane.release_fake_barrier();
            }
        }
        if let Some(reply) = lane.poll() {
            break reply?;
        }
        if now >= proof.permit.watchdog_ms.saturating_add(CLEANUP_MS) {
            return Err(ProcessError::Policy(
                "RPC unresolved; pending and ownership retained",
            ));
        }
        thread::sleep(Duration::from_millis(1));
    };
    diag(&format!("returned {operation:?} id {id}"));
    let Message::ReturnAndIdle { permit, outcome } = reply.message else {
        return Err(ProcessError::Policy(
            "worker returned a non-completion frame",
        ));
    };
    diag(&format!("outcome {operation:?}: {:?}", *outcome));
    if permit != proof.permit {
        return Err(ProcessError::Policy(
            "worker retired the wrong exact permit",
        ));
    }
    let (now, utc) = sample_time()?;
    let effects = actor.completed(&permit, (*outcome).clone(), now, utc)?;
    let mut unexpected = None;
    drive_effects(actor, storage, effects, &mut unexpected)?;
    if unexpected.is_some() {
        return Err(ProcessError::Policy(
            "completion publication scheduled another device call",
        ));
    }
    notify_worker(actor, lane)?;
    {
        // RAM-only receipts preserve the exact ledger ordering after a failed
        // publisher; they never masquerade as publication acknowledgements.
        let ledger = if actor.publication_failed() {
            actor.working()
        } else {
            actor.published()
        };
        let JournalPayload::Snapshot(snapshot) = &ledger.payload else {
            return Err(ProcessError::Policy("published snapshot expected"));
        };
        let completion = snapshot.sessions[0]
            .calls
            .last()
            .and_then(|call| call.completion.as_ref())
            .ok_or(ProcessError::Policy("published completion expected"))?;
        if completion.sequence == 0 {
            return Err(ProcessError::Policy("durable completion sequence missing"));
        }
        let receipt = ResultReceipt {
            operation_id: permit.operation_id,
            sequence: completion.sequence,
            received_utc_ms: completion.received_utc_ms,
            publication: (!actor.publication_failed()).then_some(actor.published().checksum),
        };
        let reply = rpc(
            lane,
            Message::Recorded(receipt),
            permit.generation,
            permit.operation_id,
        )?;
        if !matches!(reply.message, Message::Heartbeat) {
            if let Message::Rejected { reason } = reply.message {
                return Err(ProcessError::Context(
                    "worker receipt acknowledgement",
                    Box::new(ProcessError::RemoteRejection(reason)),
                ));
            }
            return Err(ProcessError::Policy("worker receipt acknowledgement"));
        }
    }
    Ok((*outcome).clone())
}

/// Deliver the guardian's frozen epoch before any successor operation or result
/// receipt. Failure is uncertainty, never permission to reconnect or replace a lane.
fn notify_worker(
    actor: &autotune_recovery_core::journal::JournaledGuardian,
    lane: &mut RpcLane,
) -> Result<()> {
    let Some(window) = actor.guardian().recovery_window() else {
        return Ok(());
    };
    let generation = actor.guardian().generation();
    let reply = rpc(
        lane,
        Message::Revoke {
            generation,
            trigger_ms: window.trigger_ms,
            deadline_ms: window.deadline_ms,
        },
        0,
        0,
    )?;
    if !matches!(reply.message, Message::Heartbeat) {
        return Err(ProcessError::Policy("worker revoke acknowledgement"));
    }
    if actor.publication_failed() {
        let reply = rpc(lane, Message::StorageFailed(window), 0, 0)?;
        if !matches!(reply.message, Message::Heartbeat) {
            return Err(ProcessError::Policy(
                "worker storage-failure acknowledgement",
            ));
        }
    }
    Ok(())
}

fn observed_acl(store: &Store) -> Result<Hash> {
    let observed = store.ancestry_security()?;
    let text = observed
        .iter()
        .map(|entry| {
            format!(
                "{}:{}:{}",
                entry.owner_sid, entry.dacl_protected, entry.ace_count
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    Ok(digest(b"observed-ancestry-v1", text.as_bytes()))
}

const CHAIN: [Operation; 5] = [
    Operation::Enumerate,
    Operation::Identity,
    Operation::Runtime,
    Operation::Bounds,
    Operation::Current,
];

#[allow(clippy::too_many_arguments)]
fn full_fake_roundtrip(
    actor: &mut autotune_recovery_core::journal::JournaledGuardian,
    storage: &mut StorageExecutor,
    lane: &mut RpcLane,
    heartbeats: &std::sync::mpsc::Receiver<Frame>,
    preparation: &Preparation,
    profile: &FakeProfile,
    guardian_identity: MeasuredProcess,
    worker_identity: MeasuredProcess,
    coordinator_identity: MeasuredProcess,
    coordinator_process: &OwnedProcess,
    custody: Hash,
    acl: Hash,
    root: &str,
    storage_fault: &std::sync::atomic::AtomicBool,
    witness: &std::sync::Mutex<crate::witness::Witness>,
) -> Result<RoundTripReport> {
    let baseline = profile.baseline_mw;
    let delta = 5000u32;
    diag("full fake round trip begin");
    let target = baseline
        .checked_sub(delta)
        .ok_or(ProcessError::Policy("fixture target underflow"))?;
    // Capture: explicit getter chain plus a second, separated current reading.
    for operation in CHAIN {
        execute_expected(actor, storage, lane, heartbeats, operation)?;
    }
    wait_serviced(actor, storage, heartbeats, 510)?;
    execute_expected(actor, storage, lane, heartbeats, Operation::Current)?;

    let trace = actor.capture_trace()?;
    let JournalPayload::Snapshot(captured) = &actor.working().payload else {
        return Err(ProcessError::Policy("captured snapshot expected"));
    };
    let samples: Vec<(u64, u32)> = captured.sessions[0]
        .calls
        .iter()
        .filter_map(|call| match &call.completion {
            Some(StoredCompletion {
                outcome:
                    Completion::Returned {
                        return_ms,
                        result: RawResult::Value(value),
                        ..
                    },
                ..
            }) if call.permit.operation == Operation::Current => Some((*return_ms, *value)),
            _ => None,
        })
        .collect();
    if samples.len() != 2 || samples[0].1 != baseline || samples[1].1 != baseline {
        return Err(ProcessError::Policy("fresh stable baseline required"));
    }
    let (_, utc_now) = sample_time()?;
    let manifest = Manifest {
        preparation: preparation.clone(),
        preparation_digest: preparation.digest()?,
        capture: Capture {
            readings_mw: [samples[0].1, samples[1].1],
            sampled_ms: [samples[0].0, samples[1].0],
            trace,
            minimum_mw: profile.minimum_mw,
            maximum_mw: profile.maximum_mw,
            default_mw: profile.default_mw,
        },
        target_mw: target,
        delta_mw: delta,
        runtime: profile.runtime.clone(),
        supervision: Supervision {
            guardian: guardian_identity.process.clone(),
            coordinator: coordinator_identity.process.clone(),
            worker: worker_identity.process.clone(),
            root: root.to_owned(),
            acl_expectation: acl,
            lock_identity: custody,
            other_tools_stopped_simulated: true,
            escalation: "simulated: report unresolved; never replay or escalate privileges".into(),
            risks: "synthetic device only; no physical restoration or stability claim".into(),
        },
        approval_expiry_utc_ms: utc_now
            .checked_add(CAPTURE_FRESH_MS)
            .ok_or(ProcessError::Policy("approval expiry overflow"))?,
        approval_challenge: rng()?,
        policy_version: VERSION,
    };
    manifest.validate()?;
    witness
        .lock()
        .map_err(|_| ProcessError::Policy("witness poisoned"))?
        .seal(manifest.clone())?;

    let sealed = rpc(lane, Message::Seal(Box::new(manifest.clone())), 0, 0)?;
    let Message::Sealed {
        digest: worker_digest,
    } = sealed.message
    else {
        return Err(ProcessError::Policy("worker seal acknowledgement"));
    };
    if worker_digest != manifest.digest()? {
        return Err(ProcessError::Policy("worker manifest digest mismatch"));
    }
    diag("seal acknowledged");
    lane.bind_digest(worker_digest)?;
    let effects = actor.seal(manifest.clone(), worker_digest)?;
    let mut none = None;
    drive_effects(actor, storage, effects, &mut none)?;
    if none.is_some() {
        return Err(ProcessError::Policy("seal dispatch is impossible"));
    }

    let approval = SimulatedApproval {
        manifest: manifest.digest()?,
        nonce: preparation.nonce,
        challenge: manifest.approval_challenge,
        operator: preparation.operator.clone(),
        full_and_summary_presented: true,
    };
    let (now, utc) = sample_time()?;
    let effects = actor.approve(approval.clone(), now, utc)?;
    drive_effects(actor, storage, effects, &mut none)?;
    let JournalPayload::Snapshot(armed) = &actor.published().payload else {
        return Err(ProcessError::Policy("armed snapshot expected"));
    };
    let commit = armed.sessions[0]
        .arm_commit
        .clone()
        .ok_or(ProcessError::Policy("committed arm epoch"))?;
    let capsule = ArmCapsule {
        approval,
        commit: commit.clone(),
        nonce_publication: actor.published().checksum,
    };
    let armed_reply = rpc(lane, Message::Arm(Box::new(capsule)), 0, 0)?;
    if !matches!(armed_reply.message, Message::Armed { .. }) {
        return Err(ProcessError::Policy("worker arm acknowledgement"));
    }
    diag("armed");
    if profile.behavior == FakeBehavior::HeartbeatLossDuringBlockedCandidate {
        // Freeze this owned coordinator before the validation chain, not at
        // candidate entry: heartbeat loss must precede the dispatch-origin
        // watchdog rather than race an already-buffered final heartbeat.
        coordinator_process.suspend_owned()?;
        wait_serviced(actor, storage, heartbeats, 300)?;
    }
    for operation in CHAIN {
        execute_expected(actor, storage, lane, heartbeats, operation)?;
    }
    let now = sample_time()?.0;
    if now
        >= commit
            .boot_ms
            .checked_add(CANDIDATE_MS)
            .ok_or(ProcessError::Policy("candidate window overflow"))?
    {
        return Err(ProcessError::Policy("candidate admission window missed"));
    }
    let candidate = execute_fake(actor, storage, lane, heartbeats, Operation::CandidateOnce);
    if profile.behavior == FakeBehavior::WorkerKilledDuringCandidate {
        if candidate.is_ok() {
            return Err(ProcessError::Policy("killed worker supplied a completion"));
        }
        if !witness
            .lock()
            .map_err(|_| ProcessError::Policy("witness poisoned"))?
            .calls()
            .last()
            .is_some_and(|c| {
                c.worker_lost_before_effect
                    && c.invocation.permit.operation == Operation::CandidateOnce
                    && c.result.is_some()
            })
        {
            return Err(ProcessError::Policy(
                "candidate failed without the owned worker-kill barrier",
            ));
        }
        let (now, utc) = sample_time()?;
        let effects = actor.fault(autotune_recovery_core::Fault::WorkerLost, now, utc)?;
        let mut dispatch = None;
        drive_effects(actor, storage, effects, &mut dispatch)?;
        if dispatch.is_some() || actor.guardian().pending().is_none() || actor.close().is_ok() {
            return Err(ProcessError::Policy("worker loss cleared uncertainty"));
        }
        return Ok(RoundTripReport {
            in_call_controls_acknowledged: lane.control_acknowledgements() as u64,
            guardian: guardian_identity,
            worker: worker_identity,
            coordinator: coordinator_identity,
            baseline_mw: baseline,
            target_mw: target,
            delta_mw: delta,
            candidate_entries: 1,
            restore_entries: 0,
            candidate_api: None,
            candidate_readback: None,
            restore_api: vec![],
            baseline_reads: vec![],
            observed_restored: false,
            publication_verified: false,
            final_state: StoredState::RecoveryRequired,
            journal_sequence: actor.published().sequence(),
            cold_read_only: false,
            cold_state: StoredState::RecoveryRequired,
            fixture_root: root.to_owned(),
            pending: actor.guardian().pending().cloned(),
            recovery: actor.guardian().recovery_window(),
            witness_calls: vec![],
            witness_current_mw: baseline,
        });
    }
    candidate?;
    if matches!(
        profile.behavior,
        FakeBehavior::CancelAfterCandidate | FakeBehavior::StorageFailureAfterCandidate
    ) {
        // A real publisher fault at the next publication barrier, after the
        // candidate has returned. No storage acknowledgement is fabricated.
        storage_fault.store(
            profile.behavior == FakeBehavior::StorageFailureAfterCandidate,
            std::sync::atomic::Ordering::SeqCst,
        );
        let (now, utc) = sample_time()?;
        let effects = actor.fault(autotune_recovery_core::Fault::Cancel, now, utc)?;
        let mut none = None;
        drive_effects(actor, storage, effects, &mut none)?;
        if none.is_some() {
            return Err(ProcessError::Policy("cancellation dispatched a successor"));
        }
        notify_worker(actor, lane)?;
    }
    // A candidate error or an entry rejection starts recovery immediately; a
    // successful candidate return requires the immediate configured-limit readback.
    match actor.guardian().next_operation() {
        Some(Operation::Current) if actor.guardian().phase() == Phase::Armed => {
            execute_expected(actor, storage, lane, heartbeats, Operation::Current)?;
        }
        Some(_) if actor.guardian().phase() == Phase::Recovering => (),
        other => {
            return Err(ProcessError::Policy(match other {
                None => "candidate outcome left no recovery path",
                Some(_) => "unexpected post-candidate operation",
            }))
        }
    }

    actor
        .guardian()
        .recovery_window()
        .ok_or(ProcessError::Policy("recovery epoch missing"))?;
    diag("candidate readback complete; recovery epoch frozen");
    notify_worker(actor, lane)?;
    for operation in CHAIN {
        execute_expected(actor, storage, lane, heartbeats, operation)?;
    }
    execute_expected(actor, storage, lane, heartbeats, Operation::RestoreBaseline)?;
    execute_expected(actor, storage, lane, heartbeats, Operation::Current)?;
    wait_serviced(actor, storage, heartbeats, 300)?;
    execute_expected(actor, storage, lane, heartbeats, Operation::Current)?;
    if !actor.publication_failed() {
        let effects = actor.close()?;
        let mut none = None;
        drive_effects(actor, storage, effects, &mut none)?;
        if none.is_some() || !actor.guardian().evidence().publication_verified {
            return Err(ProcessError::Policy(
                "terminal publication was not durably acknowledged",
            ));
        }
    } else if actor.close().is_ok() {
        return Err(ProcessError::Policy(
            "failed storage allowed terminal closure",
        ));
    }

    let status = rpc(lane, Message::Control(Control::Observe), 0, 0)?;
    let Message::Status(status) = status.message else {
        return Err(ProcessError::Policy("worker evidence status"));
    };
    let evidence = actor.guardian().evidence();
    let JournalPayload::Snapshot(final_snapshot) = &actor.published().payload else {
        return Err(ProcessError::Policy("terminal snapshot expected"));
    };
    let final_state = if actor.publication_failed() {
        StoredState::RecoveryRequired
    } else {
        final_snapshot.sessions[0].state
    };
    if (!actor.publication_failed() && final_state != StoredState::RestoredVerified)
        || status.pending.is_some()
        || status.candidate_entries > 1
        || status.restore_entries > 2
        || evidence.restore_api.is_empty()
        || !evidence.observed_restoration
    {
        return Err(ProcessError::Policy("round-trip evidence incomplete"));
    }
    Ok(RoundTripReport {
        in_call_controls_acknowledged: lane.control_acknowledgements() as u64,
        guardian: guardian_identity,
        worker: worker_identity,
        coordinator: coordinator_identity,
        baseline_mw: baseline,
        target_mw: target,
        delta_mw: delta,
        candidate_entries: status.candidate_entries,
        restore_entries: status.restore_entries,
        candidate_api: evidence.candidate_api,
        candidate_readback: evidence.candidate_readback,
        restore_api: evidence.restore_api.clone(),
        baseline_reads: evidence.baseline_reads.clone(),
        observed_restored: evidence.observed_restoration,
        publication_verified: evidence.publication_verified,
        final_state,
        journal_sequence: actor.published().sequence(),
        cold_read_only: false,
        cold_state: final_state,
        fixture_root: root.to_owned(),
        pending: status.pending,
        recovery: status.recovery,
        witness_calls: vec![],
        witness_current_mw: baseline,
    })
}
pub fn launch_check(action: Control, exit_after_ready: bool) -> Result<RoleStatus> {
    let image = Executable::isolated_current_copy()
        .map_err(|e| ProcessError::Context("protected role copy", Box::new(e)))?;
    let root = TestRoot::provision_fixture()?;
    let launch_role = if action == Control::CloseCoordinatorConsole {
        launch_console_test
    } else {
        launch
    };
    let launch = launch_role(
        image,
        LaunchRequest {
            parent_role: Role::Launcher,
            role: Role::Guardian,
            session: rng()?,
            profile: fake_profile(),
            root: Some(&root),
            custody: None,
            job: None,
        },
    )
    .map_err(|e| ProcessError::Context("suspended detached guardian launch", Box::new(e)))?;
    let mut channel = match launch.server.accept(
        &launch.bootstrap,
        launch.process.as_handle(),
        boot_ms() + 5000,
    ) {
        Ok(c) => c,
        Err(e) => {
            launch.process.terminate_owned()?;
            launch.process.wait(5000)?;
            return Err(e);
        }
    };
    let ready = channel.receive(Some(boot_ms() + 5000))?;
    let Message::Status(ready) = ready.message else {
        launch.process.terminate_owned()?;
        return Err(ProcessError::Policy("guardian readiness"));
    };
    println!(
        "{}",
        String::from_utf8(canonical(&*ready)?).map_err(|_| ProcessError::Policy("UTF8"))?
    );
    if exit_after_ready {
        return Ok(*ready);
    } // owned launcher exits; guardian must clean without it.
    channel.send(Message::Control(action), 0, 0, boot_ms() + 1000)?;
    let result = channel.receive(Some(boot_ms() + 8000));
    match result {
        Ok(frame) => match frame.message {
            Message::Status(report) => {
                if !launch.process.wait(5000)? {
                    launch.process.terminate_owned()?;
                    return Err(ProcessError::Policy(
                        "guardian failed bounded diagnostic shutdown",
                    ));
                }
                Ok(*report)
            }
            Message::Rejected { .. } => {
                launch.process.wait(5000)?;
                Err(ProcessError::Policy(
                    "owned console/isolation diagnostic rejected",
                ))
            }
            _ => Err(ProcessError::Policy("guardian final report")),
        },
        Err(e) => {
            launch.process.terminate_owned()?;
            launch.process.wait(5000)?;
            Err(e)
        }
    }
}
pub fn roundtrip_check(behavior: FakeBehavior) -> Result<RoundTripReport> {
    let mut profile = fake_profile();
    profile.behavior = behavior;
    roundtrip_profile(profile, false)
}
pub fn alternate_baseline_check() -> Result<RoundTripReport> {
    let mut profile = fake_profile();
    profile.baseline_mw = 231_457;
    roundtrip_profile(profile, false)
}
/// Fixed isolated startup fault: another live owner retains the same root lock.
/// No worker is launched and no device permission can be issued.
pub fn startup_owner_conflict_check() -> Result<RoundTripReport> {
    roundtrip_profile(fake_profile(), true)
}
fn roundtrip_profile(profile: FakeProfile, owner_conflict: bool) -> Result<RoundTripReport> {
    let image = Executable::isolated_current_copy()
        .map_err(|e| ProcessError::Context("protected role copy", Box::new(e)))?;
    let root = TestRoot::provision_fixture()?;
    let _competing_owner = if owner_conflict {
        Some(Store::open(&root)?)
    } else {
        None
    };
    let launch = launch(
        image,
        LaunchRequest {
            parent_role: Role::Launcher,
            role: Role::Guardian,
            session: rng()?,
            profile,
            root: Some(&root),
            custody: None,
            job: None,
        },
    )
    .map_err(|e| ProcessError::Context("suspended detached guardian launch", Box::new(e)))?;
    let mut channel = match launch.server.accept(
        &launch.bootstrap,
        launch.process.as_handle(),
        boot_ms() + 5000,
    ) {
        Ok(c) => c,
        Err(e) => {
            launch.process.terminate_owned()?;
            launch.process.wait(5000)?;
            return Err(ProcessError::Context(
                "guardian bootstrap/handshake",
                Box::new(e),
            ));
        }
    };
    let ready = channel
        .receive(Some(boot_ms() + 5000))
        .map_err(|e| ProcessError::Context("guardian readiness receive", Box::new(e)))?;
    match ready.message {
        Message::Status(_) => (),
        Message::Rejected { reason } => {
            launch.process.wait(5000)?;
            return Err(ProcessError::RemoteRejection(reason));
        }
        _ => {
            launch.process.terminate_owned()?;
            launch.process.wait(5000)?;
            return Err(ProcessError::Policy("guardian readiness"));
        }
    }
    channel
        .send(
            Message::Control(Control::FullFakeRoundTrip),
            0,
            0,
            boot_ms() + 1000,
        )
        .map_err(|e| ProcessError::Context("guardian roundtrip request", Box::new(e)))?;
    let result = channel.receive(Some(boot_ms() + 30_000));
    let report = match result {
        Ok(frame) => match frame.message {
            Message::RoundTrip(report) => *report,
            Message::Rejected { reason } => {
                launch.process.wait(5000)?;
                return Err(ProcessError::RemoteRejection(reason));
            }
            _ => {
                launch.process.terminate_owned()?;
                return Err(ProcessError::Policy("round trip report"));
            }
        },
        Err(e) => {
            launch.process.terminate_owned()?;
            launch.process.wait(5000)?;
            return Err(ProcessError::Context(
                "guardian final report receive",
                Box::new(e),
            ));
        }
    };
    if !launch.process.wait(5000)? {
        launch.process.terminate_owned()?;
        return Err(ProcessError::Policy(
            "guardian failed bounded round-trip shutdown",
        ));
    }
    Ok(report)
}
pub fn internal_role(role: Role) -> Result<()> {
    let bootstrap = inherited_bootstrap(role)?;
    let mut channel = Channel::connect(&bootstrap, boot_ms() + 5000)?;
    let result = (|| match role {
        Role::Guardian => {
            // Cover failures before readiness too. A fail-closed guardian must
            // attempt to retain its original reason, not silently drop the pipe.
            let result = guardian(bootstrap, &mut channel);
            if let Err(error) = &result {
                let _ = channel.send(
                    Message::Rejected {
                        reason: format!("{error:?}"),
                    },
                    0,
                    0,
                    boot_ms() + 1000,
                );
            }
            result
        }
        Role::Worker => worker(bootstrap, channel),
        Role::Coordinator => {
            channel.send(
                Message::Status(Box::new(status(role)?)),
                0,
                0,
                boot_ms() + 1000,
            )?;
            while alive(bootstrap.supervisor.as_handle()) {
                channel.send(Message::Heartbeat, 0, 0, boot_ms() + 1000)?;
                thread::sleep(Duration::from_millis(250));
            }
            Ok(())
        }
        Role::Verifier => {
            // Diagnostic endpoint has NO device policy, lock custody, preparation,
            // manifest or arm state. Host-job membership never becomes readiness.
            channel.send(
                Message::Status(Box::new(status(role)?)),
                0,
                0,
                boot_ms() + 1000,
            )?;
            loop {
                let frame = channel.receive(None)?;
                let stopping = matches!(frame.message, Message::Control(Control::Stop));
                match frame.message {
                    Message::Control(Control::Observe | Control::Stop) => channel.send(
                        Message::Status(Box::new(status(role)?)),
                        0,
                        0,
                        boot_ms() + 1000,
                    )?,
                    Message::Control(Control::Fault(fault)) => {
                        channel.send_invalid_fixture(fault, boot_ms() + 1000)?
                    }
                    Message::Control(Control::ProbeIsolation) => {
                        let response = match require_independent() {
                            Ok(()) => Message::Status(Box::new(status(role)?)),
                            Err(e) => Message::Rejected {
                                reason: format!("{e:?}"),
                            },
                        };
                        channel.send(response, 0, 0, boot_ms() + 1000)?;
                    }
                    _ => {
                        channel.send(
                            Message::Rejected {
                                reason: "diagnostic verifier has zero execution authority".into(),
                            },
                            0,
                            0,
                            boot_ms() + 1000,
                        )?;
                    }
                }
                if stopping {
                    return Ok(());
                }
            }
        }
        _ => Err(ProcessError::Policy("internal role")),
    })();
    flush_diagnostics();
    result
}
fn worker(bootstrap: ChildBootstrap, mut channel: Channel) -> Result<()> {
    let custody = bootstrap
        .envelope
        .handles
        .custody
        .as_ref()
        .ok_or(ProcessError::Policy("worker custody"))?
        .identity
        .digest()?;
    let mut policy = WorkerPolicy::new(
        bootstrap.envelope.profile.clone(),
        bootstrap.envelope.context.session,
        boot_identity()?,
        custody,
        format!("simulated:{}", bootstrap.envelope.context.child.user_sid),
    )?;
    channel.send(
        Message::Status(Box::new(status(Role::Worker)?)),
        0,
        0,
        boot_ms() + 1000,
    )?;
    loop {
        let frame = channel.receive(None)?;
        let (now, utc) = sample_time()?;
        let generation = frame.generation;
        let id = frame.operation_id;
        let stopping = matches!(&frame.message, Message::Control(Control::Stop));
        let response = (|| -> Result<(Message, Option<Hash>)> {
            Ok(match frame.message {
                Message::Prepare {
                    capsule,
                    publication,
                } => {
                    let digest = policy.prepare(*capsule, publication)?;
                    (
                        Message::Prepared {
                            digest,
                            custody: policy.custody(),
                        },
                        Some(digest),
                    )
                }
                Message::Seal(manifest) => {
                    if manifest.supervision.worker != bootstrap.envelope.context.child.process
                        || manifest.supervision.guardian
                            != bootstrap.envelope.context.supervisor.process
                        || manifest.preparation.executables
                            != [bootstrap.envelope.context.child.process.binary; 3]
                    {
                        return Err(ProcessError::Policy("sealed peer measurement mismatch"));
                    }
                    let digest = policy.seal(*manifest)?;
                    (Message::Sealed { digest }, Some(digest))
                }
                Message::Arm(a) => {
                    require_independent()?;
                    policy.arm(&a, now, utc, alive(bootstrap.supervisor.as_handle()))?;
                    (
                        Message::Armed {
                            commit_ms: a.commit.boot_ms,
                        },
                        None,
                    )
                }
                Message::Dispatch(proof) => {
                    if proof.permit.generation != generation || proof.permit.operation_id != id {
                        return Err(ProcessError::Policy("frame/permit mismatch"));
                    }
                    let permit = proof.permit.clone();
                    let outcome = match policy.admit(
                        *proof,
                        now,
                        utc,
                        alive(bootstrap.supervisor.as_handle()),
                    )? {
                        Err(outcome) => outcome,
                        Ok(entry) => {
                            let (entry_ms, _) = sample_time()?;
                            if let Err(reason) = policy.revalidate_entry(
                                &entry,
                                entry_ms,
                                alive(bootstrap.supervisor.as_handle()),
                            ) {
                                policy.not_entered(entry, format!("{reason:?}"), entry_ms)?
                            } else {
                                let invocation = policy.external_call(&entry, entry_ms)?;
                                channel.send(
                                    Message::FakeCall(Box::new(invocation)),
                                    generation,
                                    id,
                                    boot_ms() + 1000,
                                )?;
                                let reply = loop {
                                    let reply = channel.receive(None)?;
                                    match reply.message {
                                        Message::Revoke {
                                            generation,
                                            trigger_ms,
                                            deadline_ms,
                                        } if reply.generation == 0 && reply.operation_id == 0 => {
                                            policy.revoke(
                                                generation,
                                                RecoveryWindow {
                                                    trigger_ms,
                                                    deadline_ms,
                                                },
                                            )?;
                                        }
                                        Message::StorageFailed(window)
                                            if reply.generation == 0 && reply.operation_id == 0 =>
                                        {
                                            policy.storage_failed(window)?;
                                        }
                                        _ => break reply,
                                    }
                                    channel.send(Message::Heartbeat, 0, 0, boot_ms() + 1000)?;
                                };
                                let Message::FakeResult {
                                    operation_id,
                                    result,
                                } = reply.message
                                else {
                                    return Err(ProcessError::Policy("fake oracle reply"));
                                };
                                if operation_id != id
                                    || reply.operation_id != id
                                    || reply.generation != generation
                                {
                                    return Err(ProcessError::Policy(
                                        "fake oracle operation mismatch",
                                    ));
                                }
                                let returned = boot_ms();
                                policy.returned(entry, entry_ms, returned, result)?
                            }
                        }
                    };
                    (
                        Message::ReturnAndIdle {
                            permit,
                            outcome: Box::new(outcome),
                        },
                        None,
                    )
                }
                Message::Recorded(r) => {
                    if r.operation_id != id {
                        return Err(ProcessError::Policy("receipt frame mismatch"));
                    }
                    policy.recorded(&r).map_err(|e| {
                        diag(&format!("worker receipt refused: id={} sequence={} published={} phase={:?} pending={} error={e:?}",
                            r.operation_id, r.sequence, r.publication.is_some(), policy.phase(), policy.pending().is_some()));
                        ProcessError::Context("worker result receipt", Box::new(e.into()))
                    })?;
                    (Message::Heartbeat, None)
                }
                Message::Revoke {
                    generation,
                    trigger_ms,
                    deadline_ms,
                } => {
                    policy.revoke(
                        generation,
                        RecoveryWindow {
                            trigger_ms,
                            deadline_ms,
                        },
                    )?;
                    (Message::Heartbeat, None)
                }
                Message::StorageFailed(window) => {
                    policy.storage_failed(window)?;
                    (Message::Heartbeat, None)
                }
                Message::Control(Control::Observe | Control::Stop) => {
                    let mut s = status(Role::Worker)?;
                    s.candidate_entries = policy.candidate_entries();
                    s.restore_entries = policy.restore_entries();
                    s.pending = policy.pending().cloned();
                    s.recovery = policy.recovery_window();
                    (Message::Status(Box::new(s)), None)
                }
                _ => {
                    return Err(ProcessError::Policy(
                        "worker rejected unsupported authority",
                    ))
                }
            })
        })();
        match response {
            Ok((response, binding)) => {
                channel.send(response, generation, id, boot_ms() + 1000)?;
                if let Some(binding) = binding {
                    channel.bind_digest(binding);
                }
                if stopping {
                    return Ok(());
                }
            }
            Err(e) => {
                let _ = channel.send(
                    Message::Rejected {
                        reason: format!("{e:?}"),
                    },
                    generation,
                    id,
                    boot_ms() + 1000,
                );
                return Err(e);
            }
        }
    }
}
fn guardian(bootstrap: ChildBootstrap, parent: &mut Channel) -> Result<()> {
    require_independent()?;
    diag("guardian ready: independent");
    let root = bootstrap
        .envelope
        .fixture_root
        .as_ref()
        .ok_or(ProcessError::Policy("missing isolated root"))?;
    let store = Store::open(Path::new(root))?;
    if store.reconciliation_only() {
        return Err(ProcessError::Policy("startup reconciliation only"));
    }
    let acl_digest = observed_acl(&store)?;
    let initial = store.scan().active.clone();
    let custody = store.duplicate_custody()?;
    let image = Executable::current_role_image()?;
    let session = bootstrap.envelope.context.session;
    let profile = bootstrap.envelope.profile.clone();
    let worker_launch = launch(
        image.clone(),
        LaunchRequest {
            parent_role: Role::Guardian,
            role: Role::Worker,
            session,
            profile: profile.clone(),
            root: None,
            custody: Some(&custody),
            job: None,
        },
    )?;
    let mut wc = worker_launch.server.accept(
        &worker_launch.bootstrap,
        worker_launch.process.as_handle(),
        boot_ms() + 5000,
    )?;
    let worker_ready = wc.receive(Some(boot_ms() + 5000))?;
    if !matches!(worker_ready.message, Message::Status(_)) {
        return Err(ProcessError::Policy("worker readiness"));
    }
    let job = OwnedJob::new(false)?;
    let launch_coordinator = if bootstrap.envelope.console_test {
        launch_console_test
    } else {
        launch
    };
    let coordinator = launch_coordinator(
        image,
        LaunchRequest {
            parent_role: Role::Guardian,
            role: Role::Coordinator,
            session,
            profile: profile.clone(),
            root: None,
            custody: None,
            job: Some(&job),
        },
    )?;
    let mut cc = coordinator.server.accept(
        &coordinator.bootstrap,
        coordinator.process.as_handle(),
        boot_ms() + 5000,
    )?;
    let Message::Status(coordinator_ready) = cc.receive(Some(boot_ms() + 5000))?.message else {
        return Err(ProcessError::Policy("coordinator readiness"));
    };
    if !in_any_job(coordinator.process.as_handle())?
        || coordinator_ready.no_console == bootstrap.envelope.console_test
    {
        return Err(ProcessError::Policy(
            "coordinator console policy/job not established",
        ));
    }
    // Heartbeat decoding occurs off-loop; separate worker RPC/completion capacity.
    let (heartbeat_tx, heartbeat_rx) = std::sync::mpsc::sync_channel(64);
    let feed = thread::spawn(move || {
        while let Ok(frame) = cc.receive(None) {
            if heartbeat_tx.try_send(frame).is_err() {
                break;
            }
        }
    });
    let coordinator_process = std::sync::Arc::new(coordinator.process);
    let (now, _) = sample_time()?;
    let preparation = Preparation {
        version: VERSION,
        session,
        nonce: rng()?,
        device: profile.device.clone(),
        protocol: digest(b"protocol", b"windows-fake-process-v1"),
        executables: [bootstrap.envelope.context.child.process.binary; 3],
        operator: format!("simulated:{}", bootstrap.envelope.context.child.user_sid),
        boot: boot_identity()?,
        created_ms: now,
        synthetic: true,
    };
    let opening = JournalEnvelope::new(JournalPayload::Snapshot(StorageSnapshot {
        version: STORAGE_VERSION,
        store: initial.store(),
        sequence: 1,
        previous: initial.checksum,
        consumed_nonces: vec![],
        sessions: vec![StoredSession {
            preparation: preparation.clone(),
            manifest: None,
            previous_terminal: None,
            nonce_consumed: false,
            consent: None,
            arm_commit: None,
            recovery: None,
            state: StoredState::Preparing,
            notices: vec![],
            calls: vec![],
        }],
    }))?;
    let storage_fault = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let publisher_fault = storage_fault.clone();
    let publisher_behavior = profile.behavior;
    let mut storage = StorageExecutor::start_with(store, move |step| {
        if step == autotune_recovery_windows::storage::PublicationStep::Closed
            && publisher_fault.load(std::sync::atomic::Ordering::SeqCst)
        {
            if publisher_behavior == FakeBehavior::StorageStallDuringBlockedCandidate {
                // The actor's 100 ms ceiling must expire independently of this
                // real publisher. Only the in-call notification/return barrier
                // releases the test stall; no second publisher is started.
                while publisher_fault.load(std::sync::atomic::Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                return Ok(());
            }
            Err(autotune_recovery_windows::storage::StorageError::Invalid(
                "injected post-candidate storage failure",
            ))
        } else {
            Ok(())
        }
    })?;
    let mut journal = publish(&mut storage, opening)?;
    wc.send(
        Message::Prepare {
            capsule: Box::new(preparation.clone()),
            publication: journal.checksum,
        },
        0,
        0,
        boot_ms() + 1000,
    )?;
    let Message::Prepared {
        digest,
        custody: lock,
    } = wc.receive(Some(boot_ms() + 1000))?.message
    else {
        return Err(ProcessError::Policy("capture readiness"));
    };
    if digest != preparation.digest()? || lock != custody.identity.digest()? {
        return Err(ProcessError::Policy("worker capsule/custody digest"));
    }
    wc.bind_digest(digest);
    let worker_process = std::sync::Arc::new(worker_launch.process);
    let witness = std::sync::Arc::new(std::sync::Mutex::new(crate::witness::Witness::new(
        profile.clone(),
    )?));
    let oracle = witness.clone();
    let oracle_worker = worker_process.clone();
    let behavior = profile.behavior;
    let oracle_storage_fault = storage_fault.clone();
    let mut worker = RpcLane::start_with_oracle(wc, move |proof, invocation, release| {
        let kill = behavior == FakeBehavior::WorkerKilledDuringCandidate
            && invocation.permit.operation == Operation::CandidateOnce;
        let block = matches!(
            behavior,
            FakeBehavior::BlockedCandidateReturns
                | FakeBehavior::StorageFailureDuringBlockedCandidate
                | FakeBehavior::HeartbeatLossDuringBlockedCandidate
                | FakeBehavior::StorageStallDuringBlockedCandidate
        ) && invocation.permit.operation == Operation::CandidateOnce;
        let mut witness = oracle
            .lock()
            .map_err(|_| ProcessError::Policy("witness poisoned"))?;
        witness.enter(proof, invocation)?;
        if block
            && matches!(
                behavior,
                FakeBehavior::StorageFailureDuringBlockedCandidate
                    | FakeBehavior::StorageStallDuringBlockedCandidate
            )
        {
            oracle_storage_fault.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if block {
            while !release.load(std::sync::atomic::Ordering::SeqCst) {
                if boot_ms() >= proof.permit.watchdog_ms.saturating_add(CLEANUP_MS) {
                    return Err(ProcessError::Policy(
                        "fake entry barrier never released by guardian fault",
                    ));
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        if kill {
            oracle_worker.terminate_owned()?;
            if !oracle_worker.wait(5000)? {
                return Err(ProcessError::Policy(
                    "owned worker did not exit at entry barrier",
                ));
            }
        }
        if block && behavior == FakeBehavior::StorageStallDuringBlockedCandidate {
            oracle_storage_fault.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(witness.release(boot_ms(), kill)?)
    })?;
    let mut report = status(Role::Guardian)?;
    report.worker = Some(worker_process.identity.clone());
    report.coordinator = Some(coordinator_process.identity.clone());
    parent.send(
        Message::Status(Box::new(report.clone())),
        0,
        0,
        boot_ms() + 1000,
    )?;
    let command = parent.receive(Some(boot_ms() + 5000));
    let mut notice = None;
    match command {
        Ok(frame) => {
            if let Message::Control(action) = frame.message {
                match action {
                    Control::TerminateCoordinator => {
                        coordinator_process.terminate_owned()?;
                        if !coordinator_process.wait(5000)? {
                            return Err(ProcessError::Policy("coordinator still live"));
                        }
                        notice = Some(NoticeCode::ParentLost);
                    }
                    Control::CloseCoordinatorJob => {
                        drop(job);
                        if !coordinator_process.wait(5000)? {
                            return Err(ProcessError::Policy(
                                "last job handle did not close coordinator",
                            ));
                        }
                        notice = Some(NoticeCode::ParentLost);
                    }
                    Control::CloseCoordinatorConsole => {
                        if let Err(e) =
                            close_owned_console(&coordinator_process, &coordinator_ready)
                        {
                            let _ = parent.send(
                                Message::Rejected {
                                    reason: format!("{e:?}"),
                                },
                                0,
                                0,
                                boot_ms() + 1000,
                            );
                            coordinator_process.terminate_owned()?;
                            let _ = rpc(&mut worker, Message::Control(Control::Stop), 0, 0);
                            worker_process.wait(5000)?;
                            storage.shutdown()?;
                            return Err(e);
                        }
                        notice = Some(NoticeCode::ParentLost);
                    }
                    Control::FullFakeRoundTrip => {
                        let (now, utc) = sample_time()?;
                        let mut actor = autotune_recovery_core::journal::JournaledGuardian::resume(
                            preparation.clone(),
                            journal.clone(),
                            now,
                            utc,
                        )?;
                        let guardian_identity = current_identity()?;
                        diag("roundtrip call entered");
                        let result = full_fake_roundtrip(
                            &mut actor,
                            &mut storage,
                            &mut worker,
                            &heartbeat_rx,
                            &preparation,
                            &profile,
                            guardian_identity,
                            worker_process.identity.clone(),
                            coordinator_process.identity.clone(),
                            &coordinator_process,
                            custody.identity.digest()?,
                            acl_digest,
                            root,
                            &storage_fault,
                            &witness,
                        );
                        diag(&format!(
                            "roundtrip result error: {:?}",
                            result.as_ref().err()
                        ));
                        let mut report = result?;
                        {
                            let witness = witness
                                .lock()
                                .map_err(|_| ProcessError::Policy("witness poisoned"))?;
                            report.witness_calls = witness.calls().to_vec();
                            report.witness_current_mw = witness.current_mw();
                        }
                        diag("stopping worker");
                        if worker_process.alive() {
                            if let Ok(frame) =
                                rpc(&mut worker, Message::Control(Control::Stop), 0, 0)
                            {
                                if let Message::Status(s) = frame.message {
                                    report.candidate_entries = s.candidate_entries;
                                    report.restore_entries = s.restore_entries;
                                }
                            }
                        }
                        diag("waiting worker");
                        if !worker_process.wait(5000)? {
                            return Err(ProcessError::Policy(
                                "worker did not stop after verified round trip",
                            ));
                        }
                        diag("stopping coordinator");
                        if coordinator_process.alive() {
                            coordinator_process.terminate_owned()?;
                        }
                        coordinator_process.wait(5000)?;
                        diag("joining coordinator feed");
                        let _ = feed.join();
                        diag("stopping worker lane");
                        worker.shutdown()?;
                        diag("stopping storage");
                        storage.shutdown()?;
                        // Release the guardian's lifetime duplicate; the worker's copy
                        // ended with its process. Only then can a cold scan acquire it.
                        drop(custody);
                        diag("cold open");
                        let cold = Store::open(Path::new(root))?;
                        report.cold_read_only = cold.reconciliation_only();
                        if let JournalPayload::Snapshot(snapshot) = &cold.scan().active.payload {
                            report.cold_state = snapshot.sessions[0].state;
                        }
                        diag("sending report");
                        let _ = parent.send(
                            Message::RoundTrip(Box::new(report)),
                            0,
                            0,
                            boot_ms() + 1000,
                        );
                        diag("report sent");
                        return Ok(());
                    }
                    Control::Observe | Control::ExpiredGetter => {
                        if action == Control::ExpiredGetter {
                            worker_process.suspend_owned()?;
                        }
                        let (start, utc) = sample_time()?;
                        let permit = Permission {
                            session,
                            boot: preparation.boot,
                            digest,
                            generation: 1,
                            operation_id: 1,
                            operation: Operation::Enumerate,
                            dispatched_ms: start,
                            watchdog_ms: start + 1000,
                            expiry_ms: start + 1000,
                            recovery_trigger_ms: None,
                        };
                        journal = publish(
                            &mut storage,
                            next(&journal, |s| {
                                s.sessions[0].calls.push(StoredCall {
                                    permit: permit.clone(),
                                    intent_sequence: s.sequence,
                                    intent_utc_ms: utc,
                                    completion: None,
                                })
                            })?,
                        )?;
                        let intent_sequence = journal.sequence();
                        let proof = DispatchProof {
                            permit: permit.clone(),
                            intent_sequence,
                            intent_utc_ms: utc,
                            publication: Some(journal.checksum),
                        };
                        worker.submit(Message::Dispatch(Box::new(proof)), 1, 1)?;
                        if action == Control::ExpiredGetter {
                            while boot_ms() < permit.expiry_ms {
                                thread::yield_now();
                            }
                            worker_process.resume_owned()?;
                            notice = Some(NoticeCode::Watchdog);
                        }
                        let reply = loop {
                            if let Some(reply) = worker.poll() {
                                break reply?;
                            }
                            for _ in 0..8 {
                                if heartbeat_rx.try_recv().is_err() {
                                    break;
                                }
                            }
                            thread::yield_now();
                        };
                        let Message::ReturnAndIdle {
                            permit: returned,
                            outcome,
                        } = reply.message
                        else {
                            return Err(ProcessError::Policy("atomic worker completion"));
                        };
                        if returned != permit {
                            return Err(ProcessError::Policy("wrong retired permit"));
                        }
                        if action == Control::ExpiredGetter
                            && !matches!(*outcome, Completion::NotEntered { .. })
                        {
                            return Err(ProcessError::Policy("worker entered expired grant"));
                        }
                        let (_, utc) = sample_time()?;
                        journal = publish(
                            &mut storage,
                            next(&journal, |s| {
                                s.sessions[0].calls[0].completion = Some(StoredCompletion {
                                    sequence: s.sequence,
                                    received_utc_ms: utc,
                                    outcome: *outcome,
                                })
                            })?,
                        )?;
                        rpc(
                            &mut worker,
                            Message::Recorded(ResultReceipt {
                                operation_id: 1,
                                sequence: journal.sequence(),
                                received_utc_ms: utc,
                                publication: Some(journal.checksum),
                            }),
                            1,
                            1,
                        )?;
                    }
                    Control::DeniedCandidate => {
                        let (start, utc) = sample_time()?;
                        let proof = DispatchProof {
                            permit: Permission {
                                session,
                                boot: preparation.boot,
                                digest,
                                generation: 1,
                                operation_id: 1,
                                operation: Operation::CandidateOnce,
                                dispatched_ms: start,
                                watchdog_ms: start + 1000,
                                expiry_ms: start + 1000,
                                recovery_trigger_ms: None,
                            },
                            intent_sequence: 2,
                            intent_utc_ms: utc,
                            publication: None,
                        };
                        let rejected = rpc(&mut worker, Message::Dispatch(Box::new(proof)), 1, 1)?;
                        if !matches!(rejected.message, Message::Rejected { .. }) {
                            return Err(ProcessError::Policy(
                                "pre-manifest candidate was not rejected",
                            ));
                        }
                        notice = Some(NoticeCode::Protocol);
                    }
                    Control::Stop => (),
                    Control::Fault(_) => {
                        return Err(ProcessError::Policy(
                            "wire fault not admitted on guardian authority channel",
                        ))
                    }
                    Control::ProbeIsolation => {
                        return Err(ProcessError::Policy(
                            "diagnostic isolation probe not an authority request",
                        ))
                    }
                }
            } else {
                return Err(ProcessError::Policy("launcher control authority"));
            }
        }
        Err(_) => notice = Some(NoticeCode::ParentLost), // launcher loss never owns cleanup progress
    }
    require_independent()?;
    if !worker_process.alive() && notice != Some(NoticeCode::Protocol) {
        return Err(ProcessError::Policy(
            "worker did not survive coordinator/launcher loss",
        ));
    }
    if worker_process.alive() {
        if let Ok(frame) = rpc(&mut worker, Message::Control(Control::Stop), 0, 0) {
            if let Message::Status(s) = frame.message {
                report.candidate_entries = s.candidate_entries;
                report.restore_entries = s.restore_entries;
            }
        }
    }
    if !worker_process.wait(5000)? {
        return Err(ProcessError::Policy(
            "worker did not stop without pending authority",
        ));
    }
    if coordinator_process.alive() {
        coordinator_process.terminate_owned()?;
    }
    coordinator_process.wait(5000)?;
    let _ = feed.join();
    worker.shutdown()?;
    let (now, utc) = sample_time()?;
    journal = publish(
        &mut storage,
        next(&journal, |s| {
            s.sessions[0].state = StoredState::AbortedNoWrite;
            if let Some(code) = notice {
                s.sessions[0].notices.push(StoredNotice {
                    sequence: s.sequence,
                    boot_ms: now,
                    utc_ms: utc,
                    code,
                });
            }
        })?,
    )?;
    if journal.read_only_required() {
        return Err(ProcessError::Policy("diagnostic terminal invalid"));
    }
    storage.shutdown()?;
    let _ = parent.send(Message::Status(Box::new(report)), 0, 0, boot_ms() + 1000);
    Ok(())
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn optional_diagnostics_are_bounded_and_do_not_write_inside_the_actor_path() {
        let base = TestRoot::approved_parent().unwrap().join(format!(
            "fake-diagnostic-buffer-{}",
            rng()
                .unwrap()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ));
        let path =
            std::path::PathBuf::from(format!("{}.{}.log", base.display(), std::process::id()));
        let previous = std::env::var_os("AUTOTUNE_RECOVERY_DIAG");
        std::env::set_var("AUTOTUNE_RECOVERY_DIAG", &base);
        diag("synthetic actor observation");
        diag(&"x".repeat(256 * 1024 + 1));
        let before_flush = path.exists();
        let buffered = DIAGNOSTICS.with(|buffer| {
            let buffer = buffer.borrow();
            (buffer.0.len(), buffer.2)
        });
        flush_diagnostics();
        match previous {
            Some(value) => std::env::set_var("AUTOTUNE_RECOVERY_DIAG", value),
            None => std::env::remove_var("AUTOTUNE_RECOVERY_DIAG"),
        }
        assert!(
            !before_flush,
            "diagnostics must not perform deadline-sensitive file I/O"
        );
        assert_eq!(buffered, (1, 1));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "synthetic actor observation\ndiagnostics_truncated_events=1\n"
        );
    }
}
