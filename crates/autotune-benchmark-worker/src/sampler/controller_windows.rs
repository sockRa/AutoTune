//! Fixed sampler-only controller. This source is compile-only under current
//! verification authority; real invocation needs exact separately retained consent.
use super::{
    archive::{Artifact, IndexBinding},
    manifest::{profile_bytes, CaptureManifest},
    producer::{build_envelope, raw_events, BoundText},
    runtime::{ExecutionRequest, Operation, ResourceObservation, Watchdog},
    supervisor::{Effect, Fault, Input, Supervisor},
    windows::{FailedLaunch, SamplerLaunch, SamplerProcess},
    wire::Message,
    writer::{DirectorySink, Failure as WriterFailure, IncompleteCapture, Writer},
};
use crate::{
    evidence::Provenance,
    monitor::{windows_clock::WindowsClock, Anchor},
};
use sha2::{Digest as _, Sha256};
use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::{self, Read},
    mem::{size_of, zeroed},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::FILE_SHARE_READ,
    System::{
        Console::*, ProcessStatus::*, Threading::*, WindowsProgramming::QueryInterruptTimePrecise,
    },
};
static USER_STOP: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn cancellation(kind: u32) -> i32 {
    if matches!(kind, CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT) {
        USER_STOP.store(true, Ordering::Release);
        1
    } else {
        0
    }
}
fn failed() -> io::Error {
    io::Error::other("capture binding/capacity/observation failed")
}
fn code(result: io::Result<()>) -> Result<(), u32> {
    result.map_err(|e| {
        e.raw_os_error()
            .map(|n| n as u32)
            .unwrap_or(ERROR_INVALID_DATA)
    })
}
fn raw_interrupt() -> u64 {
    let mut raw = 0;
    unsafe { QueryInterruptTimePrecise(&mut raw) };
    raw
}
fn cpu_time(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}
fn resources() -> ResourceObservation {
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = zeroed();
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let memory = K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            counters.cb,
        );
        let memory_error = (memory == 0).then(|| GetLastError());
        let (mut created, mut exit, mut kernel, mut user) =
            (zeroed(), zeroed(), zeroed(), zeroed());
        let time = GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exit,
            &mut kernel,
            &mut user,
        );
        let time_error = (time == 0).then(|| GetLastError());
        ResourceObservation {
            commit_bytes: (memory != 0).then_some(counters.PrivateUsage as u64),
            user_100ns: (time != 0).then_some(cpu_time(user)),
            kernel_100ns: (time != 0).then_some(cpu_time(kernel)),
            memory_error,
            time_error,
        }
    }
}
fn measured_controller() -> io::Result<(File, [u8; 32])> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut block = [0; 32768];
    loop {
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        hash.update(&block[..n]);
    }
    Ok((file, hash.finalize().into()))
}
fn anchor(clock: &WindowsClock) -> Option<Anchor> {
    clock.capture().ok()
}
fn incomplete(
    manifest: &CaptureManifest,
    s: &mut Supervisor,
    clock: &WindowsClock,
    writer: &mut Writer,
    failure: Option<WriterFailure>,
) -> io::Error {
    let cancellation = writer.request_cancel();
    let finished = writer.thread_finished();
    let _ = s.advance(
        anchor(clock),
        Input::WriterShutdown {
            cancellation,
            finished,
        },
    );
    // A failed/blocked sink cannot promise durable failure artifacts. Return
    // bounded owned diagnostics, preserving storage/cancellation facts even if
    // raw serialization no longer fits the remaining RAM.
    match IncompleteCapture::retain(
        manifest.digest(),
        failure,
        cancellation,
        finished,
        || raw_events(s).ok(),
        &s.failure_budget(),
    ) {
        Ok(evidence) => io::Error::other(evidence),
        Err(error) => error,
    }
}
struct Host<'a> {
    clock: &'a WindowsClock,
    process: Option<&'a SamplerProcess>,
    failed_launch: Option<&'a FailedLaunch>,
}
impl super::runtime::HostSource for Host<'_> {
    fn resources(&mut self) -> super::runtime::ResourceObservation {
        resources()
    }
    fn process(&mut self) -> Option<super::supervisor::ProcessObservation> {
        self.process
            .map(SamplerProcess::observe)
            .or_else(|| self.failed_launch.map(FailedLaunch::observe))
    }
    fn sampler_resources(&mut self) -> Option<ResourceObservation> {
        self.process.map(SamplerProcess::resources)
    }
    fn watchdog(&mut self) -> u64 {
        raw_interrupt()
    }
    fn anchor(&mut self) -> Option<Anchor> {
        anchor(self.clock)
    }
}
fn drive(
    effects: Vec<Effect>,
    s: &mut Supervisor,
    clock: &WindowsClock,
    process: &mut Option<SamplerProcess>,
    launch: &Option<SamplerLaunch>,
    failed_launch: &mut Option<Box<FailedLaunch>>,
) -> bool {
    let mut effects = VecDeque::from(effects);
    let mut finalize = false;
    let mut actions = 0;
    while let Some(effect) = effects.pop_front() {
        actions += 1;
        if actions > 16 {
            let _ = s.advance(anchor(clock), Input::Fault(Fault::Protocol));
            return true;
        }
        let operation = match effect {
            Effect::Finalize => {
                finalize = true;
                continue;
            }
            Effect::SignalStop => Some((
                Operation::StopEvent,
                if let Some(p) = process.as_ref() {
                    p.request_stop()
                } else if let Some(l) = launch.as_ref() {
                    l.request_stop()
                } else {
                    Ok(())
                },
            )),
            Effect::Terminate => Some((
                Operation::JobTerminate,
                if let Some(p) = process.as_mut() {
                    p.request_termination()
                } else if let Some(failed) = failed_launch.as_mut() {
                    failed.request_termination()
                } else if let Some(l) = launch.as_ref() {
                    l.request_termination()
                } else {
                    Ok(())
                },
            )),
            Effect::Send(message) => {
                let kind = if matches!(message, Message::Query(_)) {
                    Operation::QuerySend
                } else {
                    Operation::StopSend
                };
                Some((
                    kind,
                    if let Some(p) = process.as_mut() {
                        p.send(&message)
                    } else {
                        Err(io::Error::from_raw_os_error(ERROR_NOT_READY as i32))
                    },
                ))
            }
        };
        if let Some((operation, result)) = operation {
            effects.extend(s.advance(
                anchor(clock),
                Input::Operation {
                    operation,
                    result: code(result),
                },
            ));
        }
    }
    finalize
}
pub fn controller_main() -> io::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let request = ExecutionRequest::parse(&args).map_err(|_| failed())?;
    // Bounded manifest and measured self-image checks precede any native sampler
    // launch. The original measurement epoch is created once after this admission
    // input is known; it is never rebased during writer/startup/getter activity.
    let mut bytes = Vec::new();
    File::open(&request.manifest_path)?
        .take(16385)
        .read_to_end(&mut bytes)?;
    let manifest = CaptureManifest::parse(&bytes).map_err(|_| failed())?;
    let (_held_controller, digest) = measured_controller()?;
    request.bind(&manifest, digest).map_err(|_| failed())?;
    if unsafe { SetConsoleCtrlHandler(Some(cancellation), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let clock = WindowsClock::new(manifest.draft().clock_token).map_err(|_| failed())?;
    let origin = clock.capture().map_err(|_| failed())?;
    let binding = manifest
        .binding(clock.qpc_frequency())
        .map_err(|_| failed())?;
    let mut s = Supervisor::new(binding, origin, manifest.selection()).map_err(|_| failed())?;
    let mut watchdog = Watchdog::new(origin.interrupt_100ns).map_err(|_| failed())?;
    let output_root = manifest.draft().output_root.clone();
    let mut writer = Writer::start_factory(
        move || DirectorySink::open(Path::new(&output_root)),
        s.writer_budget(),
    )?;
    let initial_result = (|| -> io::Result<()> {
        writer.enqueue(
            Artifact::Manifest,
            BoundText::copy(manifest.canonical(), &s.writer_budget()).map_err(|_| failed())?,
        )?;
        writer.enqueue(
            Artifact::Profile,
            BoundText::copy(&profile_bytes(), &s.writer_budget()).map_err(|_| failed())?,
        )?;
        Ok(())
    })();
    if let Err(error) = initial_result {
        let failure = writer
            .poll()
            .and_then(Result::err)
            .unwrap_or_else(|| WriterFailure::new("initialize_artifacts", error));
        let _ = s.advance(anchor(&clock), Input::WriterFailure(failure));
        return Err(incomplete(
            &manifest,
            &mut s,
            &clock,
            &mut writer,
            Some(failure),
        ));
    }
    let mut launch = None;
    let mut launch_attempted = false;
    let mut process = None;
    let mut failed_launch = None;
    let mut capture_closed = false;
    let mut writer_failure = None;
    let mut inputs_drained = false;
    let mut transport_fault_reported = false;
    let mut watchdog_forced = false;
    loop {
        let capture = super::runtime::observe_host(&mut Host {
            clock: &clock,
            process: process.as_ref(),
            failed_launch: failed_launch.as_deref(),
        });
        let raw = capture.watchdog_100ns;
        let watchdog_ok = watchdog.observe(raw).is_ok();
        let observed = capture.anchor;
        let host = Input::Host {
            watchdog_100ns: raw,
            resources: capture.resources,
            sampler_resources: capture.sampler_resources,
            process: capture.process,
        };
        let effects = s.advance(observed, host);
        capture_closed |= drive(
            effects,
            &mut s,
            &clock,
            &mut process,
            &launch,
            &mut failed_launch,
        );
        if !watchdog_ok {
            let effects = s.advance(None, Input::ObservationExpired);
            capture_closed |= drive(
                effects,
                &mut s,
                &clock,
                &mut process,
                &launch,
                &mut failed_launch,
            );
        }
        if USER_STOP.load(Ordering::Acquire) {
            let effects = s.advance(anchor(&clock), Input::Stop);
            capture_closed |= drive(
                effects,
                &mut s,
                &clock,
                &mut process,
                &launch,
                &mut failed_launch,
            );
        }
        if let Some(t) = s.terminal() {
            watchdog.stop(t.at_ns);
        }
        if let Some(Err(failure)) = writer.poll() {
            // No completion is expected before the final four artifacts/index.
            // Failure closes sampling even while the storage thread was blocked.
            if writer_failure.is_none() {
                writer_failure = Some(failure);
                let effects = s.advance(anchor(&clock), Input::WriterFailure(failure));
                capture_closed |= drive(
                    effects,
                    &mut s,
                    &clock,
                    &mut process,
                    &launch,
                    &mut failed_launch,
                );
            }
        }
        if !launch_attempted && s.terminal().is_none() && writer.written_artifacts() >= 2 {
            launch_attempted = true;
            match SamplerLaunch::start(manifest.draft().sampler, binding, s.budget()) {
                Ok(l) => launch = Some(l),
                Err(error) => {
                    let effects = s.advance(
                        anchor(&clock),
                        Input::Operation {
                            operation: Operation::Launch,
                            result: code(Err(error)),
                        },
                    );
                    capture_closed |= drive(
                        effects,
                        &mut s,
                        &clock,
                        &mut process,
                        &launch,
                        &mut failed_launch,
                    );
                }
            }
        }
        if process.is_none() {
            if let Some(result) = launch.as_mut().and_then(SamplerLaunch::poll) {
                match result {
                    Ok(mut child) => {
                        if s.terminal().is_some() {
                            // A launch can return after admission closed. Retain
                            // both raw operation results; neither is exit proof.
                            let result = child.request_stop();
                            let _ = s.advance(
                                anchor(&clock),
                                Input::Operation {
                                    operation: Operation::StopEvent,
                                    result: code(result),
                                },
                            );
                            let result = child.request_termination();
                            let _ = s.advance(
                                anchor(&clock),
                                Input::Operation {
                                    operation: Operation::JobTerminate,
                                    result: code(result),
                                },
                            );
                        }
                        let effects = s.advance(
                            anchor(&clock),
                            Input::Launched {
                                creation: child.creation_time(),
                            },
                        );
                        if s.terminal().is_none() {
                            let result = child.send(&Message::Bootstrap {
                                origin,
                                raw_uuid: manifest.draft().raw_uuid.clone(),
                                raw_pci: manifest.draft().raw_pci.clone(),
                                manifest: manifest.canonical().into(),
                            });
                            process = Some(child);
                            capture_closed |= drive(
                                effects,
                                &mut s,
                                &clock,
                                &mut process,
                                &launch,
                                &mut failed_launch,
                            );
                            let effects = s.advance(
                                anchor(&clock),
                                Input::Operation {
                                    operation: Operation::BootstrapSend,
                                    result: code(result),
                                },
                            );
                            capture_closed |= drive(
                                effects,
                                &mut s,
                                &clock,
                                &mut process,
                                &launch,
                                &mut failed_launch,
                            );
                        } else {
                            process = Some(child);
                            capture_closed |= drive(
                                effects,
                                &mut s,
                                &clock,
                                &mut process,
                                &launch,
                                &mut failed_launch,
                            );
                        }
                    }
                    Err(error) => {
                        let error = match FailedLaunch::from_error(error) {
                            Ok(failure) => {
                                let io_shutdown = failure.io_shutdown();
                                let evidence = failure.evidence();
                                failed_launch = Some(failure);
                                let effects = s.advance(anchor(&clock), evidence);
                                capture_closed |= drive(
                                    effects,
                                    &mut s,
                                    &clock,
                                    &mut process,
                                    &launch,
                                    &mut failed_launch,
                                );
                                for observation in io_shutdown {
                                    let effects =
                                        s.advance(anchor(&clock), Input::IoShutdown(observation));
                                    capture_closed |= drive(
                                        effects,
                                        &mut s,
                                        &clock,
                                        &mut process,
                                        &launch,
                                        &mut failed_launch,
                                    );
                                }
                                io::Error::other("retained failed sampler launch")
                            }
                            Err(error) => error,
                        };
                        let effects = s.advance(
                            anchor(&clock),
                            Input::Operation {
                                operation: Operation::Launch,
                                result: code(Err(error)),
                            },
                        );
                        capture_closed |= drive(
                            effects,
                            &mut s,
                            &clock,
                            &mut process,
                            &launch,
                            &mut failed_launch,
                        );
                    }
                }
            }
        }
        if let Some(child) = process.as_mut() {
            match super::drain::poll(child) {
                super::drain::Step::Item(received) => {
                    let receipt = anchor(&clock);
                    let effects = received.apply(&mut s, receipt);
                    capture_closed |= drive(
                        effects,
                        &mut s,
                        &clock,
                        &mut process,
                        &launch,
                        &mut failed_launch,
                    );
                }
                super::drain::Step::Waiting => {}
                super::drain::Step::Finished(error) => {
                    inputs_drained = true;
                    if let Some(error) = error.filter(|_| !transport_fault_reported) {
                        transport_fault_reported = true;
                        let effects = s.advance(
                            anchor(&clock),
                            Input::Operation {
                                operation: Operation::TransportReceive,
                                result: code(Err(error)),
                            },
                        );
                        capture_closed |= drive(
                            effects,
                            &mut s,
                            &clock,
                            &mut process,
                            &launch,
                            &mut failed_launch,
                        );
                    }
                }
                super::drain::Step::Failed(error) if !transport_fault_reported => {
                    transport_fault_reported = true;
                    let effects = s.advance(
                        anchor(&clock),
                        Input::Operation {
                            operation: Operation::TransportReceive,
                            result: code(Err(error)),
                        },
                    );
                    capture_closed |= drive(
                        effects,
                        &mut s,
                        &clock,
                        &mut process,
                        &launch,
                        &mut failed_launch,
                    );
                }
                super::drain::Step::Failed(_) => {}
            }
        }
        if s.terminal().is_some() && watchdog.force_due() && !watchdog_forced && !s.exit_confirmed()
        {
            watchdog_forced = true;
            let result = if let Some(child) = process.as_mut() {
                child.request_termination()
            } else if let Some(failed) = failed_launch.as_mut() {
                failed.request_termination()
            } else if let Some(l) = launch.as_ref() {
                l.request_termination()
            } else {
                Err(io::Error::from_raw_os_error(ERROR_NOT_READY as i32))
            };
            let effects = s.advance(
                anchor(&clock),
                Input::Operation {
                    operation: Operation::JobTerminate,
                    result: code(result),
                },
            );
            capture_closed |= drive(
                effects,
                &mut s,
                &clock,
                &mut process,
                &launch,
                &mut failed_launch,
            );
        }
        if s.terminal().is_some() && watchdog.observation_expired() && !capture_closed {
            let effects = s.advance(anchor(&clock), Input::ObservationExpired);
            capture_closed |= drive(
                effects,
                &mut s,
                &clock,
                &mut process,
                &launch,
                &mut failed_launch,
            );
        }
        if s.terminal().is_some() && !launch_attempted {
            capture_closed = true;
        }
        if (capture_closed
            && (inputs_drained || process.is_none() || watchdog.observation_expired()))
            || watchdog.finalization_expired()
        {
            break;
        }
        std::thread::sleep(Duration::from_nanos(p.controller_tick_ns));
    }
    let mut io_shutdown = None;
    if let Some(child) = process.as_ref() {
        let observations = child.shutdown_transport();
        for observation in observations {
            let _ = s.advance(anchor(&clock), Input::IoShutdown(observation));
        }
        io_shutdown = Some(observations);
    }
    // Cancellation does not mean a reader finished. Retain queued and partial
    // packets until both readers are observed done plus an empty queue, or the
    // ORIGINAL finalization deadline. Never join a worker on this path.
    if let Some(child) = process.as_mut() {
        while !inputs_drained {
            let _ = watchdog.observe(raw_interrupt());
            if watchdog.finalization_expired() {
                break;
            }
            match super::drain::poll(child) {
                super::drain::Step::Item(received) => {
                    let _ = received.apply(&mut s, anchor(&clock));
                }
                super::drain::Step::Finished(error) => {
                    inputs_drained = true;
                    if let Some(error) = error.filter(|_| !transport_fault_reported) {
                        transport_fault_reported = true;
                        let _ = s.advance(
                            anchor(&clock),
                            Input::Operation {
                                operation: Operation::TransportReceive,
                                result: code(Err(error)),
                            },
                        );
                    }
                }
                super::drain::Step::Failed(error) => {
                    if !transport_fault_reported {
                        transport_fault_reported = true;
                        let _ = s.advance(
                            anchor(&clock),
                            Input::Operation {
                                operation: Operation::TransportReceive,
                                result: code(Err(error)),
                            },
                        );
                    }
                    std::thread::sleep(Duration::from_nanos(p.controller_tick_ns));
                }
                super::drain::Step::Waiting => {
                    std::thread::sleep(Duration::from_nanos(p.controller_tick_ns));
                }
            }
        }
        if let Some(observations) = io_shutdown {
            // Keep initial cancellation and later completion distinct. A worker
            // that still has not finished remains explicitly unresolved.
            for (mut observation, finished) in
                observations.into_iter().zip(child.transport_completion())
            {
                if observation.finished != finished {
                    observation.finished = finished;
                    let _ = s.advance(anchor(&clock), Input::IoShutdown(observation));
                }
            }
        }
    }
    if writer_failure.is_some() || watchdog.finalization_expired() {
        return Err(incomplete(
            &manifest,
            &mut s,
            &clock,
            &mut writer,
            writer_failure,
        ));
    }
    let finalize_result = (|| -> io::Result<()> {
        let raw = raw_events(&s).map_err(|_| failed())?;
        let mut envelope =
            build_envelope(&manifest, &s, Provenance::LiveObserved).map_err(|_| failed())?;
        envelope
            .account_artifacts(manifest.canonical(), &profile_bytes(), &raw)
            .map_err(|_| failed())?;
        writer.enqueue(Artifact::Raw, raw)?;
        let session = envelope.encode().map_err(|_| failed())?;
        let session_digest = session.session_digest().ok_or_else(failed)?;
        writer.enqueue(Artifact::Session, session)?;
        writer.finish(IndexBinding {
            manifest: *manifest.digest().as_bytes(),
            profile: *manifest.profile_digest().as_bytes(),
            source_tree: manifest.draft().source_tree,
            controller: manifest.draft().controller,
            sampler: manifest.draft().sampler,
            protocol: manifest.draft().protocol,
            session_digest: *session_digest.as_bytes(),
        })
    })();
    if let Err(error) = finalize_result {
        let failure = writer
            .poll()
            .and_then(Result::err)
            .unwrap_or_else(|| WriterFailure::new("finalize_artifacts", error));
        let _ = s.advance(anchor(&clock), Input::WriterFailure(failure));
        return Err(incomplete(
            &manifest,
            &mut s,
            &clock,
            &mut writer,
            Some(failure),
        ));
    }
    loop {
        let _ = watchdog.observe(raw_interrupt());
        if watchdog.finalization_expired() {
            return Err(incomplete(
                &manifest,
                &mut s,
                &clock,
                &mut writer,
                writer_failure,
            ));
        }
        if let Some(result) = writer.poll() {
            if let Err(failure) = result {
                let _ = s.advance(anchor(&clock), Input::WriterFailure(failure));
                return Err(incomplete(
                    &manifest,
                    &mut s,
                    &clock,
                    &mut writer,
                    Some(failure),
                ));
            }
            // A complete sampler artifact set still never qualifies a workload
            // baseline. Process/native uncertainty remains explicit in session.
            if !s.exit_confirmed() {
                return Err(io::Error::other(
                    "capture written; sampler exit remains unknown",
                ));
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_nanos(p.controller_tick_ns));
    }
}
