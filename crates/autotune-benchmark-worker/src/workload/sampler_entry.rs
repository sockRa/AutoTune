//! Fixed sampler entry composition. Execution scope/consent remain external;
//! bootstrap/readiness neither grant worker compute nor qualify a baseline.
use super::sampler_native::NativeOwner;
use super::{
    child_windows::{Context, Failure},
    control_windows::{Handle, Report},
    launch::{Arguments, Role},
    sampler_dispatch::Dispatch,
    sampler_runtime::{route, Fault, Operation, Route, Trace},
    telemetry::{Collector, Output},
    transport::{RawReceived, WorkerReceived},
    windows_transport::Receive,
    wire::{Channel, Direction, EncodedFrame, Message},
};
use crate::{
    evidence::Reason,
    monitor::{
        windows_clock::{CaptureError, WindowsClock},
        Anchor,
    },
    sampler::{budget::Budget, wire::Lane},
};
use autotune_probe::{
    nvml::{NvmlSampler, SamplerInfo},
    report::Failure as NativeFailure,
};
use std::{mem::ManuallyDrop, thread, time::Duration};
pub struct Outcome {
    pub context: Context,
    pub trace: Trace,
    pub native_calls: super::nvml_trace::Ledger,
    pub control: Option<Handle>,
    pub report: Option<Report>,
    pub settings: Option<Dispatch>,
    pub telemetry: Option<Collector>,
    pub info: Option<SamplerInfo>,
    pub ready: Option<Message>,
    pub unsent: Option<EncodedFrame>,
    pub native_failure: Option<NativeFailure>,
    pub unreported: Option<Message>,
    pub rejected: Option<RawReceived>,
    pub rejected_worker: Option<WorkerReceived>,
    pub transport_fault: Option<(Option<i32>, Option<Anchor>)>,
    pub clock_error: Option<CaptureError>,
    pub control_error: Option<super::control::Fault>,
    native: NativeOwner,
}
impl Outcome {
    /// Child orderly-completion status only, never qualification or GPU fence.
    pub fn exit_code(&self) -> i32 {
        if self.trace.fault.is_none()
            && self.trace.output_drained
            && self.trace.control_finished
            && self
                .report
                .as_ref()
                .is_some_and(|r| r.state.finished() && r.state.fault().is_none())
        {
            0
        } else {
            2
        }
    }
    pub fn initialized_session_retained(&self) -> bool {
        self.native.initialized()
    }
}
pub struct EntryFailure {
    pub fault: Fault,
    pub startup: Option<Failure>,
    pub context: Option<Context>,
}
fn capture(clock: &WindowsClock, error: &mut Option<CaptureError>) -> Option<Anchor> {
    match clock.capture() {
        Ok(a) => Some(a),
        Err(e) => {
            error.get_or_insert(e);
            None
        }
    }
}
#[allow(clippy::result_large_err)]
/// # Safety
/// Call once at actual child entry with exclusive ownership of the inherited
/// endpoints supplied by the fixed pilot launcher, before any other adoption.
/// Exact native execution scope and consent remain caller prerequisites.
pub unsafe fn entry() -> Result<Outcome, EntryFailure> {
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let budget = Budget::new(p.evidence_ram_bytes as usize).map_err(|_| EntryFailure {
        fault: Fault::Capacity,
        startup: None,
        context: None,
    })?;
    let args_charge = budget.reserve(8192).map_err(|_| EntryFailure {
        fault: Fault::Capacity,
        startup: None,
        context: None,
    })?;
    let mut raw = Vec::with_capacity(17);
    for arg in std::env::args_os().skip(1).take(17) {
        let arg = arg.into_string().map_err(|_| EntryFailure {
            fault: Fault::Arguments,
            startup: None,
            context: None,
        })?;
        if arg.len() > 128 {
            return Err(EntryFailure {
                fault: Fault::Arguments,
                startup: None,
                context: None,
            });
        }
        raw.push(arg);
    }
    let args = Arguments::parse(&raw).map_err(|_| EntryFailure {
        fault: Fault::Arguments,
        startup: None,
        context: None,
    })?;
    // SAFETY: the fixed executable calls entry once, before any other owner can
    // adopt these inherited endpoints. The fixed launcher supplies its allowlist.
    let context = unsafe { Context::from_inherited(&args, Role::Sampler, budget.clone()) }
        .map_err(|startup| EntryFailure {
            fault: Fault::Startup,
            startup: Some(startup),
            context: None,
        })?;
    let bootstrap = context.bootstrap();
    let trace = match Trace::new(
        bootstrap.binding(),
        bootstrap.setup_entry(),
        bootstrap.receipt(),
        &budget,
    ) {
        Ok(trace) => trace,
        Err(fault) => {
            return Err(EntryFailure {
                fault,
                startup: None,
                context: Some(context),
            })
        }
    };
    let binding = bootstrap.binding();
    let native_calls = match super::nvml_trace::Ledger::new(
        binding.monitor().qpc_frequency,
        binding.epoch,
        bootstrap.receipt(),
        bootstrap.deadline_ns(),
        &budget,
    ) {
        Ok(l) => l,
        Err(_) => {
            return Err(EntryFailure {
                fault: Fault::Capacity,
                startup: None,
                context: Some(context),
            })
        }
    };
    drop(raw);
    drop(args_charge);
    let mut outcome = Outcome {
        context,
        trace,
        native_calls,
        control: None,
        report: None,
        settings: None,
        telemetry: None,
        info: None,
        ready: None,
        unsent: None,
        native_failure: None,
        unreported: None,
        rejected: None,
        rejected_worker: None,
        transport_fault: None,
        clock_error: None,
        control_error: None,
        native: NativeOwner::Released,
    };
    run(&mut outcome);
    Ok(outcome)
}
fn run(out: &mut Outcome) {
    let (bootstrap, transport, clock, event, budget) = out.context.parts();
    let binding = bootstrap.binding();
    let run = bootstrap.run();
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let port = match transport.split_control() {
        Ok(v) => v,
        Err(_) => {
            out.trace.fault = Some(Fault::Control);
            return;
        }
    };
    let event = match event.try_clone() {
        Ok(v) => v,
        Err(_) => {
            out.trace.fault = Some(Fault::Control);
            return;
        }
    };
    let first = capture(clock, &mut out.clock_error);
    let startup_gate = crate::sampler::child::NativeAdmission::default();
    if out.trace.observe(first, &startup_gate).is_err() {
        return;
    }
    let Some(first) = first else {
        out.trace.fault = Some(Fault::Clock);
        return;
    };
    let control = match Handle::start(port, event, binding, Role::Sampler, first, budget.clone()) {
        Ok(v) => v,
        Err(e) => {
            out.control_error = Some(e);
            out.trace.fault = Some(Fault::Control);
            return;
        }
    };
    let shared = control.shared().clone();
    let gate = shared.admission();
    let configured = shared
        .configure_setup(binding, bootstrap.deadline_ns())
        .is_ok();
    let work = (|| -> Result<(), Fault> {
        if !configured {
            return Err(Fault::Control);
        }
        while !control.ready() {
            let a = capture(clock, &mut out.clock_error);
            if !out.trace.setup(a, gate) || control.thread_finished() {
                return Err(Fault::Control);
            }
            thread::sleep(Duration::from_nanos(p.controller_tick_ns));
        }
        let mut sender = transport
            .sender(binding, Direction::FromWorker, budget)
            .map_err(|_| Fault::Control)?;
        // One output channel owns the entire Ready/telemetry sequence.
        let mut tx = Channel::sampler(
            binding,
            Lane::Ordinary,
            Direction::FromWorker,
            budget.clone(),
        )
        .map_err(|_| Fault::Capacity)?;
        let mut rx = Channel::sampler(binding, Lane::Ordinary, Direction::ToWorker, budget.clone())
            .map_err(|_| Fault::Capacity)?;
        let before = capture(clock, &mut out.clock_error);
        if !out
            .trace
            .begin(Operation::Open, before, shared.cooperative_raw(), gate)
        {
            return Err(Fault::Runtime);
        }
        let opened = NvmlSampler::open_observed(
            run.raw_uuid(),
            run.raw_pci(),
            &mut out
                .native_calls
                .observer(gate, || capture(clock, &mut out.clock_error)),
        );
        out.native = match opened {
            Ok(s) => NativeOwner::Selected(ManuallyDrop::new(s)),
            Err(f) => {
                out.native_failure = Some(f.failure.clone());
                NativeOwner::Failed(ManuallyDrop::new(f))
            }
        };
        let after = capture(clock, &mut out.clock_error);
        out.trace.returned(Operation::Open, after, gate);
        if out.native_calls.fault() == Some(super::nvml_trace::Fault::Clock) {
            shared.close_at(binding, None);
        }
        if out.native.selected().is_none() {
            return Err(Fault::Runtime);
        }
        if out.trace.fault.is_some() {
            return Err(Fault::Clock);
        }
        let before = capture(clock, &mut out.clock_error);
        if !out
            .trace
            .begin(Operation::Describe, before, shared.cooperative_raw(), gate)
        {
            return Err(Fault::Runtime);
        }
        out.info = Some(
            out.native
                .selected()
                .expect("selected above")
                .describe_observed(
                    &mut out
                        .native_calls
                        .observer(gate, || capture(clock, &mut out.clock_error)),
                ),
        );
        let after = capture(clock, &mut out.clock_error);
        out.trace.returned(Operation::Describe, after, gate);
        if out.native_calls.fault() == Some(super::nvml_trace::Fault::Clock) {
            shared.close_at(binding, None);
        }
        if out.trace.fault.is_some() {
            return Err(Fault::Clock);
        }
        let info = out.info.as_ref().expect("description retained");
        if out.native_calls.fault().is_some()
            || [
                &info.runtime.driver_version,
                &info.runtime.nvml_version,
                &info.runtime.cuda_driver_version,
            ]
            .iter()
            .any(|f| {
                matches!(
                    f.reading,
                    autotune_probe::report::Reading::Unavailable { .. }
                )
            })
        {
            return Err(Fault::Runtime);
        }
        let runtime = serde_json::to_string(&info.runtime).map_err(|_| Fault::Ready)?;
        let before = before.ok_or(Fault::Clock)?;
        let after = after.ok_or(Fault::Clock)?;
        out.ready = Some(Message::SamplerReady {
            before,
            after,
            raw_uuid: info.raw_uuid.clone(),
            raw_pci: info.raw_pci.clone(),
            sampler_digest: run.sampler_digest(),
            runtime,
        });
        let frame = tx
            .encode(out.ready.as_ref().expect("ready retained"))
            .map_err(|_| Fault::Ready)?;
        if let Err(frame) = sender.send_worker(frame) {
            out.unsent = Some(frame);
            return Err(Fault::Ready);
        }
        out.settings = Some(
            Dispatch::new(
                &run,
                binding.monitor().qpc_frequency,
                binding.epoch,
                bootstrap.setup_entry(),
                budget.clone(),
            )
            .map_err(|_| Fault::Settings)?,
        );
        out.telemetry =
            Some(Collector::new(binding, after, budget.clone()).map_err(|_| Fault::Telemetry)?);
        while !gate.stopped() {
            if control.thread_finished() {
                return Err(Fault::Control);
            }
            if transport.inputs_finished() {
                return Err(Fault::Protocol);
            }
            let received = transport.receive(|| capture(clock, &mut out.clock_error));
            let anchor = match &received {
                Receive::Packet(r) => r.receipt(),
                Receive::Idle { anchor } | Receive::Fault { anchor, .. } => *anchor,
            };
            if out.trace.observe(anchor, gate).is_err() {
                shared.close_at(binding, None);
            }
            match received {
                Receive::Packet(raw) => {
                    let route = if raw.packet().rejection().is_some() {
                        Err(Fault::Protocol)
                    } else {
                        route(raw.packet().bytes(), budget)
                    };
                    match route {
                        Ok(Route::Settings) => {
                            let result =
                                out.settings.as_mut().expect("dispatcher retained").native(
                                    &run,
                                    raw,
                                    out.native.selected_mut().expect("selected retained"),
                                    gate,
                                    || {
                                        let a = capture(clock, &mut out.clock_error);
                                        if out.trace.observe(a, gate).is_err() {
                                            shared.close_at(binding, None);
                                        }
                                        a
                                    },
                                    &mut sender,
                                );
                            if let Err(raw) = result {
                                out.rejected = Some(raw);
                                return Err(Fault::Settings);
                            }
                            if out.settings.as_ref().expect("retained").fault().is_some() {
                                return Err(Fault::Settings);
                            }
                            let receipt = capture(clock, &mut out.clock_error);
                            out.trace.setup_complete = receipt;
                            out.trace.observe(receipt, gate)?;
                            shared
                                .complete_setup(binding, receipt.ok_or(Fault::Clock)?)
                                .map_err(|_| Fault::Settings)?;
                        }
                        Ok(Route::Telemetry) => {
                            let result =
                                out.telemetry.as_mut().expect("collector retained").native(
                                    &run,
                                    raw.decode_worker(&mut rx),
                                    out.native.selected().expect("selected retained"),
                                    gate,
                                    || {
                                        let a = capture(clock, &mut out.clock_error);
                                        if out.trace.observe(a, gate).is_err() {
                                            shared.close_at(binding, None);
                                        }
                                        a
                                    },
                                    Output {
                                        channel: &mut tx,
                                        sender: &mut sender,
                                    },
                                );
                            if let Err(raw) = result {
                                out.rejected_worker = Some(raw);
                                return Err(Fault::Telemetry);
                            }
                            if out.telemetry.as_ref().expect("retained").fault().is_some() {
                                return Err(Fault::Telemetry);
                            }
                        }
                        Err(e) => {
                            out.rejected = Some(raw);
                            return Err(e);
                        }
                    }
                }
                Receive::Idle { anchor } => {
                    out.telemetry.as_mut().expect("retained").idle(anchor, gate);
                    if !out.settings.as_ref().expect("retained").response_queued()
                        && anchor.is_none_or(|a| {
                            binding
                                .elapsed(a)
                                .map_or(true, |n| n >= bootstrap.deadline_ns())
                        })
                    {
                        out.trace.fail(Fault::Deadline, gate);
                        return Err(Fault::Settings);
                    }
                }
                Receive::Fault { code, anchor } => {
                    out.transport_fault = Some((code, anchor));
                    return Err(Fault::Protocol);
                }
            }
            if out.trace.fault.is_some() {
                return Err(Fault::Clock);
            }
            thread::sleep(Duration::from_nanos(p.controller_tick_ns));
        }
        Ok(())
    })();
    let trigger = capture(clock, &mut out.clock_error);
    out.trace.stop_trigger = trigger;
    if out.trace.observe(trigger, gate).is_err() {
        shared.close_at(binding, None);
    }
    if let Err(f) = work {
        out.trace.fail(f, gate);
    }
    if let Err(e) = control.request_stop(trigger) {
        out.control_error = Some(e);
        out.trace.fail(Fault::Control, gate);
    }
    if let Some(raw) = &out.native_failure {
        let message = Message::NativeFailure {
            stage: Reason::new("nvml-open").expect("fixed reason"),
            before: out.trace.calls[0].before,
            after: out.trace.calls[0].after,
            reason: raw.reason,
            vendor_status: raw.vendor_status,
            detail: raw.detail.clone(),
        };
        if let Err(message) = control.failure(message) {
            out.unreported = Some(message);
        }
    }
    if out.native.initialized() {
        let before = capture(clock, &mut out.clock_error);
        if out
            .trace
            .begin(Operation::Cleanup, before, shared.cooperative_raw(), gate)
        {
            let status = out.native.shutdown();
            out.trace.cleanup_status = status;
            // The status is retained before unloading the closed library owner.
            out.native.release();
            let after = capture(clock, &mut out.clock_error);
            out.trace
                .cleanup_returned(after, status, shared.cooperative_raw(), gate);
        }
    } else {
        out.native.release();
    }
    if let Err(e) = control.complete(out.trace.calls[2].attempted, out.trace.cleanup_status) {
        out.control_error = Some(e);
        out.trace.fail(Fault::Control, gate);
    }
    let mut control = control;
    loop {
        let drained = transport.output_drained();
        let finished = control.thread_finished();
        let a = capture(clock, &mut out.clock_error);
        if out
            .trace
            .drain(a, drained, finished, shared.cooperative_raw(), gate)
        {
            break;
        }
        if a.is_none_or(|v| v.interrupt_100ns >= shared.cooperative_raw())
            || out.trace.history.missing()
            || out.trace.history.clock_error().is_some()
            || out.trace.history.overflow().is_some()
        {
            break;
        }
        thread::sleep(Duration::from_nanos(p.controller_tick_ns));
    }
    match control.take_report() {
        Ok(Some(report)) => {
            if report.state.fault().is_some() || !report.state.finished() {
                out.trace.fail(Fault::Control, gate);
            }
            out.report = Some(report);
        }
        Ok(None) => {
            out.trace.fail(Fault::Control, gate);
        }
        Err(e) => {
            out.control_error = Some(e);
            out.trace.fail(Fault::Control, gate);
        }
    }
    out.control = Some(control);
}
