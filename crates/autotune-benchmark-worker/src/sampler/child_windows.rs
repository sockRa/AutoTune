//! Fixed production sampler entry. Compilation is not native execution approval.
use super::{
    budget::Budget,
    child::{ChildGate, ChildShutdown, NativeAdmission},
    windows_io::Transport,
    wire::{Channel, Lane, Message, WireBinding},
};
use crate::{
    monitor::{windows_clock::WindowsClock, Binding},
    worker::Identity,
};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{self, Read},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::{GetFileType, FILE_SHARE_READ, FILE_TYPE_PIPE},
    System::{Threading::*, WindowsProgramming::QueryInterruptTimePrecise},
};
fn failure() -> io::Error {
    io::Error::other("sampler binding/protocol failure")
}
fn decode_hex<const N: usize>(s: &str) -> io::Result<[u8; N]> {
    super::wire_model::unhex(&crate::evidence::Value::Text(s.into())).map_err(|_| failure())
}
fn arguments() -> io::Result<(Vec<OwnedHandle>, WireBinding, u32)> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 17 || args[1] != "--sampler-v1" || args.iter().any(|s| s.len() > 32768) {
        return Err(failure());
    }
    let mut numbers = Vec::new();
    for value in &args[2..7] {
        let raw = value.parse::<usize>().map_err(|_| failure())?;
        if raw == 0 || raw == usize::MAX || numbers.contains(&raw) {
            return Err(failure());
        }
        numbers.push(raw);
    }
    // Validate all handles before adopting ownership of any of them.
    for (i, n) in numbers.iter().enumerate() {
        let mut flags = 0;
        unsafe {
            if GetHandleInformation(*n as HANDLE, &mut flags) == 0 {
                return Err(io::Error::last_os_error());
            }
            if i < 4 && GetFileType(*n as HANDLE) != FILE_TYPE_PIPE {
                return Err(failure());
            }
        }
    }
    let binding = WireBinding {
        monitor: Binding {
            session: decode_hex(&args[7])?,
            run: decode_hex(&args[8])?,
            clock_token: decode_hex(&args[10])?,
            qpc_frequency: args[11].parse().map_err(|_| failure())?,
            device: Identity {
                uuid: decode_hex(&args[12])?,
                domain: args[13].parse().map_err(|_| failure())?,
                bus: args[14].parse().map_err(|_| failure())?,
                device: args[15].parse().map_err(|_| failure())?,
            },
        },
        manifest_digest: decode_hex(&args[9])?,
    };
    Channel::new(binding, Lane::Ordinary).map_err(|_| failure())?;
    let mode = args[16].parse().map_err(|_| failure())?;
    // SAFETY: unique current-process handles were all checked above. The fixed
    // launcher passes exclusively these explicitly inherited child endpoints.
    let handles = numbers
        .into_iter()
        .map(|n| unsafe { OwnedHandle::from_raw_handle(n as HANDLE) })
        .collect();
    Ok((handles, binding, mode))
}
fn own_digest() -> io::Result<(std::fs::File, [u8; 32])> {
    let mut image = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 32768];
    loop {
        let n = image.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok((image, hash.finalize().into()))
}
pub fn sampler_main() -> io::Result<()> {
    sampler_run(false)
}
fn sampler_run(fixture: bool) -> io::Result<()> {
    let (handles, binding, mode) = arguments()?;
    if (!fixture && mode != 0)
        || (fixture && (!cfg!(feature = "sampler-process-fixture") || !matches!(mode, 3..=5)))
    {
        return Err(failure());
    }
    let [r0, r1, w0, w1, stop]: [OwnedHandle; 5] = handles.try_into().map_err(|_| failure())?;
    let profile = autotune_fma_reference::profile::Profile::approved_design();
    let budget = Budget::new(profile.limits.evidence_ram_bytes as usize).map_err(|_| failure())?;
    let mut transport = Transport::new([r0, r1], [w0, w1], budget.clone())?;
    let mut rx = Channel::new(binding, Lane::Ordinary).map_err(|_| failure())?;
    let mut tx = Channel::new(binding, Lane::Ordinary).map_err(|_| failure())?;
    let mut gate = ChildGate::new(binding);
    let stopped = Arc::new(AtomicBool::new(false));
    let admission = Arc::new(NativeAdmission::default());
    let first_stop = Arc::new(AtomicU64::new(u64::MAX));
    let first = first_stop.clone();
    let (finish_send, finish_receive) = mpsc::sync_channel(1);
    let control = transport.take_control();
    let event = stop.try_clone()?;
    let c = stopped.clone();
    let q = admission.clone();
    let control_thread = std::thread::Builder::new()
        .name("sampler-control".into())
        .spawn(move || {
            let result = (|| -> io::Result<()> {
                let clock =
                    WindowsClock::new(binding.monitor.clock_token).map_err(|_| failure())?;
                if clock.qpc_frequency() != binding.monitor.qpc_frequency {
                    return Err(failure());
                }
                let mut rx = Channel::new(binding, Lane::Control).map_err(|_| failure())?;
                let mut tx = Channel::new(binding, Lane::Control).map_err(|_| failure())?;
                let mut shutdown = ChildShutdown::new();
                loop {
                    let event_result = unsafe { WaitForSingleObject(event.as_raw_handle(), 0) };
                    let requested = match control.receive()? {
                        Some(frame) => {
                            if frame.rejection.is_some() {
                                return Err(failure());
                            }
                            let _temporary = control.codec_reservation()?;
                            match rx.decode(&frame.bytes).map_err(|_| failure())? {
                                Message::Stop { .. } => true,
                                _ => return Err(failure()),
                            }
                        }
                        None => false,
                    };
                    if event_result == WAIT_FAILED {
                        return Err(io::Error::last_os_error());
                    }
                    if requested || event_result == WAIT_OBJECT_0 {
                        q.stop();
                        c.store(true, Ordering::Release);
                        if shutdown.first_stop().is_none() {
                            let anchor = clock.capture().map_err(|_| failure())?;
                            first.fetch_min(anchor.interrupt_100ns, Ordering::AcqRel);
                            let message = shutdown.stop(anchor, q.pending()).ok_or_else(failure)?;
                            let _temporary = control.codec_reservation()?;
                            control.send(tx.encode(&message).map_err(|_| failure())?)?;
                        }
                    }
                    match finish_receive.try_recv() {
                        Ok(()) => {
                            let anchor = clock.capture().map_err(|_| failure())?;
                            let message = shutdown
                                .finished(anchor, q.pending())
                                .map_err(|_| failure())?;
                            let _temporary = control.codec_reservation()?;
                            control.send(tx.encode(&message).map_err(|_| failure())?)?;
                            return Ok(());
                        }
                        Err(mpsc::TryRecvError::Disconnected) => return Err(failure()),
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    std::thread::sleep(Duration::from_nanos(profile.limits.controller_tick_ns));
                }
            })();
            // Transport/control errors also close the execution lane. The parent
            // independently observes channel loss; never replace a blocked getter.
            if result.is_err() {
                q.stop();
                c.store(true, Ordering::Release);
            }
            result
        })?;
    let receive = |transport: &Transport,
                   rx: &mut Channel|
     -> io::Result<Option<(Message, super::budget::Reservation)>> {
        match transport.receive()? {
            Some((Lane::Ordinary, frame)) => {
                if frame.rejection.is_some() {
                    return Err(failure());
                }
                let _temporary = transport.codec_reservation(Lane::Ordinary)?;
                rx.decode(&frame.bytes)
                    .map(|message| Some((message, _temporary)))
                    .map_err(|_| failure())
            }
            Some(_) => Err(failure()),
            None => Ok(None),
        }
    };
    let bootstrap = loop {
        if stopped.load(Ordering::Acquire) {
            return Err(failure());
        }
        if let Some(message) = receive(&transport, &mut rx)? {
            break message;
        }
        std::thread::sleep(Duration::from_nanos(profile.limits.controller_tick_ns));
    };
    let (bootstrap, _bootstrap_reservation) = bootstrap;
    let (manifest, _origin) = gate.bootstrap(bootstrap).map_err(|_| failure())?;
    let (_held_image, digest) = own_digest()?;
    if digest != manifest.draft().sampler {
        return Err(failure());
    }
    let clock = WindowsClock::new(binding.monitor.clock_token).map_err(|_| failure())?;
    if clock.qpc_frequency() != binding.monitor.qpc_frequency {
        return Err(failure());
    }
    // Exactly one installed trusted NVML session. All native calls, including
    // initialization/runtime description/shutdown, stay on this serial lane.
    // Reserve bounded native description and serialization temporaries before
    // the getter/description APIs construct any variable metadata.
    let native_metadata = transport.codec_reservation(Lane::Ordinary)?;
    admission.begin().map_err(|_| failure())?;
    let sampler = if fixture {
        None
    } else {
        Some(
            autotune_probe::nvml::NvmlSampler::open(
                &manifest.draft().raw_uuid,
                &manifest.draft().raw_pci,
            )
            .map_err(|_| failure())?,
        )
    };
    admission.complete();
    admission.begin().map_err(|_| failure())?;
    let (raw_uuid, raw_pci, runtime) = if let Some(sampler) = &sampler {
        let info = sampler.describe();
        (
            info.raw_uuid,
            info.raw_pci,
            serde_json::to_string(&info.runtime).map_err(|_| failure())?,
        )
    } else {
        (
            manifest.draft().raw_uuid.clone(),
            manifest.draft().raw_pci.clone(),
            "simulated_fixture".into(),
        )
    };
    admission.complete();
    let anchor = clock.capture().map_err(|_| failure())?;
    let ready_temporary = transport.codec_reservation(Lane::Ordinary)?;
    transport.send(
        Lane::Ordinary,
        tx.encode(&Message::Ready {
            raw_uuid,
            raw_pci,
            sampler_digest: if fixture && mode == 5 {
                [0x99; 32]
            } else {
                digest
            },
            frequency: clock.qpc_frequency(),
            anchor,
            runtime,
        })
        .map_err(|_| failure())?,
    )?;
    drop(ready_temporary);
    drop(native_metadata);
    loop {
        if stopped.load(Ordering::Acquire)
            || unsafe { WaitForSingleObject(stop.as_raw_handle(), 0) } == WAIT_OBJECT_0
        {
            first_stop.fetch_min(
                clock.capture().map_err(|_| failure())?.interrupt_100ns,
                Ordering::AcqRel,
            );
            gate.stop();
            admission.stop();
            break;
        }
        if let Some((message, _message_reservation)) = receive(&transport, &mut rx)? {
            let Message::Query(ticket) = message else {
                return Err(failure());
            };
            gate.begin(ticket).map_err(|_| failure())?;
            let start = clock.capture().map_err(|_| failure())?;
            if admission.begin().is_err() {
                gate.stop();
                break;
            }
            let raw = if let Some(sampler) = &sampler {
                sampler.sample(super::sample_metric(ticket.metric))
            } else {
                // Fixed fault injection on the same execution lane and wire
                // engine. The blocked getter deliberately ignores stop; only
                // the independently supervised owned job can end this fixture.
                if mode == 4 {
                    loop {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                let (unit, value) = match ticket.metric {
                    crate::monitor::Metric::Temperature => ("degC", 42),
                    crate::monitor::Metric::Power => ("mW", 100_000),
                    crate::monitor::Metric::GraphicsClock | crate::monitor::Metric::MemoryClock => {
                        ("MHz", 1500)
                    }
                    _ => ("%", 10),
                };
                autotune_probe::report::Field::available(unit, value)
            };
            let end = clock.capture().map_err(|_| failure())?;
            admission.complete();
            gate.complete().map_err(|_| failure())?;
            // A completed in-flight getter is retained even after stop; it
            // confers no permission for a successor or terminal upgrade.
            let _temporary = transport.codec_reservation(Lane::Ordinary)?;
            transport.send(
                Lane::Ordinary,
                tx.encode(&Message::MetricRecord {
                    ticket,
                    start,
                    end,
                    raw,
                })
                .map_err(|_| failure())?,
            )?;
        } else {
            std::thread::sleep(Duration::from_nanos(profile.limits.controller_tick_ns));
        }
    }
    drop(sampler); // May block in native shutdown; parent deadlines remain independent.
    finish_send.try_send(()).map_err(|_| failure())?;
    while !control_thread.is_finished() || !transport.output_drained() {
        let mut now = 0;
        unsafe { QueryInterruptTimePrecise(&mut now) };
        let first = first_stop.load(Ordering::Acquire);
        if first == u64::MAX
            || now
                .checked_sub(first)
                .and_then(|n| n.checked_mul(100))
                .is_none_or(|n| n >= profile.limits.cooperative_stop_ns)
        {
            break;
        }
        std::thread::sleep(Duration::from_nanos(profile.limits.controller_tick_ns));
    }
    let control_done = control_thread.is_finished();
    let output_done = transport.output_drained();
    if control_done {
        control_thread.join().map_err(|_| failure())??;
    } else {
        drop(control_thread);
    }
    let _ = transport.shutdown();
    // No unbounded join. Unfinished control/I/O threads are not exit evidence.
    if control_done && output_done {
        Ok(())
    } else {
        Err(failure())
    }
}
#[cfg(feature = "sampler-process-fixture")]
pub fn fixture_main() -> io::Result<()> {
    // Inspect the fixed mode before adopting inherited handles; sampler_run
    // owns them exactly once when using the shared protocol engine.
    let mode = std::env::args()
        .nth(16)
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or_else(failure)?;
    if matches!(mode, 3..=5) {
        return sampler_run(true);
    }
    let (handles, _binding, mode) = arguments()?;
    if !matches!(mode, 1 | 2) {
        return Err(failure());
    }
    let event = &handles[4];
    loop {
        let status = unsafe { WaitForSingleObject(event.as_raw_handle(), 10) };
        if status == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        if mode == 1 && status == WAIT_OBJECT_0 {
            return Ok(());
        }
        if mode == 2 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
