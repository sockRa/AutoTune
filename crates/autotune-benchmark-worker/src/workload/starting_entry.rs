//! Actual pre-manifest native entry composition. No executable routing or final
//! response/export yet; returned in-memory facts are not delivered evidence.
use super::{
    child_windows::Context,
    sampler_native::NativeOwner,
    settings::Block,
    starting::Snapshot,
    starting_bootstrap::Bound,
    starting_control::{Completion, Fault as ControlFault},
    starting_control_windows::{Handle, Report},
    starting_delivery::{Chunks, Delivery, DeliveryFault},
    starting_export::{Binding, ClockFault, ControlFacts, Facts, Response},
    starting_runtime::{Fault, Operation, Trace},
};
use crate::monitor::{
    windows_clock::{CaptureError, WindowsClock},
    Anchor,
};
use crate::sampler::{budget::Budget, wire::Lane};
use autotune_probe::{
    nvml::{NvmlSampler, SamplerInfo},
    report::Failure as NativeFailure,
};
use std::{mem::ManuallyDrop, thread, time::Duration};
pub struct Outcome {
    pub context: Context<Bound>,
    pub trace: Trace,
    pub native_calls: super::nvml_trace::Ledger,
    pub control: Option<Handle>,
    pub report: Option<Report>,
    pub info: Option<SamplerInfo>,
    pub snapshot: Option<Snapshot>,
    pub snapshot_error: Option<Block>,
    pub native_failure: Option<NativeFailure>,
    pub clock_error: Option<CaptureError>,
    pub control_error: Option<ControlFault>,
    pub event_clone_error: Option<u32>,
    pub unreported_completion: Option<Completion>,
    pub export_error: Option<Block>,
    pub delivery: Option<Delivery>,
    pub delivery_error: Option<DeliveryFault>,
    native: NativeOwner,
}
impl Outcome {
    pub fn runtime_file(&self) -> Option<&autotune_probe::runtime_file::windows::Image> {
        self.native.runtime_file()
    }
    pub fn runtime_load_error(&self) -> Option<&autotune_probe::nvml::RuntimeLoadError> {
        self.native.load_error()
    }
    pub fn initialization_status(&self) -> Option<i32> {
        self.native.initialization()
    }
    /// Child orderly-completion status only, never qualification or GPU fence.
    /// Requires the complete bounded export to have been handed to the queue.
    pub fn exit_code(&self) -> i32 {
        if self.trace.fault.is_none()
            && self.trace.output_drained
            && self.trace.control_finished
            && self.export_error.is_none()
            && self.delivery_error.is_none()
            && self.delivery.as_ref().is_some_and(Delivery::drained)
            && self
                .report
                .as_ref()
                .is_some_and(|r| r.state.finished() && r.state.shared().fault().is_none())
        {
            0
        } else {
            2
        }
    }
    pub fn initialized_session_retained(&self) -> bool {
        self.native.initialized()
    }
    /// Own one bounded canonical export of the retained raw facts. This neither
    /// delivers it to a peer nor proves native execution/cleanup/GPU quiescence.
    pub fn response(&self, budget: &Budget) -> Result<Response, Block> {
        let bootstrap = self.context.bootstrap();
        let control = ControlFacts {
            completion: self.report.as_ref().and_then(|r| r.state.completion()),
            completion_receipt: self
                .report
                .as_ref()
                .and_then(|r| r.state.completion_receipt()),
            finished: self.report.as_ref().map(|r| r.state.finished()),
            fault: self.report.as_ref().and_then(|r| r.state.shared().fault()),
            window: self.report.as_ref().and_then(|r| r.state.shared().window()),
            trigger: self
                .report
                .as_ref()
                .and_then(|r| r.state.shared().trigger()),
            event_status: self.report.as_ref().and_then(|r| r.event_status),
            event_error: self.report.as_ref().and_then(|r| r.event_error),
            transport_error: self.report.as_ref().and_then(|r| r.transport_error),
            observed_frequency: self.report.as_ref().and_then(|r| r.observed_frequency),
            rejected_receipt: self
                .report
                .as_ref()
                .and_then(|r| r.rejected.as_ref().and_then(|x| x.receipt())),
        };
        Response::build_runtime(
            Binding {
                scope: bootstrap.scope(),
                declaration: bootstrap.declaration(),
                first_entry: bootstrap.first_entry(),
                receipt: bootstrap.receipt(),
                deadline_ns: bootstrap.deadline_ns(),
            },
            Facts {
                info: self.info.as_ref(),
                ledger: &self.native_calls,
                snapshot: self.snapshot.as_ref(),
                snapshot_error: self.snapshot_error,
                trace: &self.trace,
                control,
                native_failure: self.native_failure.as_ref(),
                clock_error: self.clock_error.map(ClockFault::from).or_else(|| {
                    self.report
                        .as_ref()
                        .and_then(|r| r.clock_error)
                        .map(ClockFault::from)
                }),
                control_error: self.control_error,
                event_clone_error: self.event_clone_error,
                unreported_completion: self.unreported_completion,
            },
            super::runtime_evidence::Input::Native {
                file: self.runtime_file(),
                load_error: self.runtime_load_error(),
                initialization: self.initialization_status(),
            },
            budget,
        )
    }
}
pub struct EntryFailure {
    pub context: Context<Bound>,
    pub fault: Fault,
}
/// Entry-level rejection before or around the actual retained context/draft.
pub enum EntryError {
    Arguments,
    Capacity,
    Startup(Box<super::child_windows::Failure>),
    Entry(Box<EntryFailure>),
}
/// The fixed executable dispatches only on this exact first token.
pub fn is_starting(args: &[std::ffi::OsString]) -> bool {
    args.first()
        .is_some_and(|a| a.to_str() == Some(super::starting_bootstrap::FLAG))
}
/// Parse the fixed starting argument mode, adopt the exclusively inherited
/// endpoints and compose the actual entry.
/// # Safety
/// Invoke once at the fixed sampler's process entry before any other adoption.
/// Exact native-read execution scope/consent remain caller prerequisites.
pub unsafe fn entry() -> Result<Outcome, EntryError> {
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let budget = Budget::new(p.evidence_ram_bytes as usize).map_err(|_| EntryError::Capacity)?;
    let args_charge = budget.reserve(8192).map_err(|_| EntryError::Capacity)?;
    let mut raw = Vec::with_capacity(14);
    // Read one extra token so a launcher that supplies more than the fixed 14
    // arguments is rejected by the strict parser instead of silently truncated.
    for arg in std::env::args_os().skip(1).take(15) {
        let arg = arg.into_string().map_err(|_| EntryError::Arguments)?;
        if arg.len() > 128 {
            return Err(EntryError::Arguments);
        }
        raw.push(arg);
    }
    let args =
        super::starting_bootstrap::Arguments::parse(&raw).map_err(|_| EntryError::Arguments)?;
    drop(raw);
    drop(args_charge);
    // SAFETY: the fixed executable calls entry once, before any other owner can
    // adopt these inherited endpoints. The fixed launcher supplies its allowlist.
    let context = unsafe { Context::from_starting_inherited(&args, budget.clone()) }
        .map_err(|f| EntryError::Startup(Box::new(f)))?;
    execute(context).map_err(|f| EntryError::Entry(Box::new(f)))
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
/// # Safety
/// Invoke once with the fixed sampler's exclusively adopted starting context,
/// only under separate exact native-read execution scope and parent supervision.
/// This function actually opens NVML/reads current settings when invoked. Scope
/// hashes and approval-reference strings do not prove that authorization.
#[allow(clippy::result_large_err)]
pub unsafe fn execute(mut context: Context<Bound>) -> Result<Outcome, EntryFailure> {
    let (bootstrap, _, _, _, budget) = context.parts();
    let trace = match Trace::new(bootstrap.scope(), bootstrap.receipt(), budget) {
        Ok(t) => t,
        Err(fault) => return Err(EntryFailure { context, fault }),
    };
    let d = bootstrap.scope().draft();
    let native_calls = match super::nvml_trace::Ledger::new(
        d.frequency,
        d.epoch,
        bootstrap.receipt(),
        bootstrap.deadline_ns(),
        budget,
    ) {
        Ok(l) => l,
        Err(_) => {
            return Err(EntryFailure {
                context,
                fault: Fault::Capacity,
            })
        }
    };
    let mut out = Outcome {
        context,
        trace,
        native_calls,
        control: None,
        report: None,
        info: None,
        snapshot: None,
        snapshot_error: None,
        native_failure: None,
        clock_error: None,
        control_error: None,
        event_clone_error: None,
        unreported_completion: None,
        export_error: None,
        delivery: None,
        delivery_error: None,
        native: NativeOwner::Released(Default::default()),
    };
    run(&mut out);
    deliver(&mut out);
    Ok(out)
}
/// Send one complete bounded export over the already-owned ordinary lane. A
/// response-build or queue failure is retained; it never becomes success.
fn deliver(out: &mut Outcome) {
    let budget = {
        let (_, _, _, _, budget) = out.context.parts();
        budget.clone()
    };
    let response = match out.response(&budget) {
        Ok(response) => response,
        Err(block) => {
            out.export_error = Some(block);
            return;
        }
    };
    let chunks = match Chunks::new(&response, &budget) {
        Ok(chunks) => chunks,
        Err(_) => {
            out.export_error = Some(Block::Capacity);
            return;
        }
    };
    let (_, transport, _, _, _) = out.context.parts();
    match chunks.send(
        |bytes| transport.queue_copy(Lane::Ordinary, bytes),
        || transport.output_drained(),
    ) {
        Ok(delivery) => out.delivery = Some(delivery),
        Err(fault) => out.delivery_error = Some(fault),
    }
}
fn run(out: &mut Outcome) {
    let (bootstrap, transport, clock, event, budget) = out.context.parts();
    let scope = bootstrap.scope();
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let port = match transport.split_control() {
        Ok(p) => p,
        Err(_) => {
            out.trace.fault = Some(Fault::Control);
            return;
        }
    };
    let event = match event.try_clone() {
        Ok(e) => e,
        Err(e) => {
            out.event_clone_error = e.raw_os_error().map(|v| v as u32);
            out.trace.fault = Some(Fault::Control);
            return;
        }
    };
    let Some(first) = capture(clock, &mut out.clock_error) else {
        out.trace.fault = Some(Fault::Clock);
        return;
    };
    let mut control = match Handle::start(port, event, scope, first, budget) {
        Ok(c) => c,
        Err(e) => {
            out.control_error = Some(e);
            out.trace.fault = Some(Fault::Control);
            return;
        }
    };
    let shared = control.shared().clone();
    let gate = shared.admission();
    let work = (|| -> Result<(), Fault> {
        // Retain the main's actual startup capture separately from the control
        // thread's own first tick. Neither can rebase the original preflight.
        out.trace.observe(Some(first), &shared)?;
        while !control.ready() {
            let a = capture(clock, &mut out.clock_error);
            if !out.trace.preflight(a, &shared) || control.thread_finished() {
                return Err(Fault::Control);
            }
            thread::sleep(Duration::from_nanos(p.controller_tick_ns));
        }
        let before = capture(clock, &mut out.clock_error);
        if !out.trace.begin(Operation::Open, before, &shared) {
            return Err(Fault::Runtime);
        }
        let result = NvmlSampler::open_observed(
            &scope.draft().raw_uuid,
            &scope.draft().raw_pci,
            &mut out
                .native_calls
                .observer(gate, || capture(clock, &mut out.clock_error)),
        );
        out.native = match result {
            Ok(s) => NativeOwner::Selected(ManuallyDrop::new(s)),
            Err(f) => {
                out.native_failure = Some(f.failure.clone());
                NativeOwner::Failed(ManuallyDrop::new(f))
            }
        };
        let after = capture(clock, &mut out.clock_error);
        out.trace.returned(Operation::Open, after, &shared);
        if out.native_calls.fault() == Some(super::nvml_trace::Fault::Clock) {
            shared.fail(ControlFault::Clock);
        }
        if out.native.selected().is_none() {
            return Err(Fault::Runtime);
        }
        if out.trace.fault.is_some() {
            return Err(Fault::Control);
        }
        let before = capture(clock, &mut out.clock_error);
        if !out.trace.begin(Operation::Describe, before, &shared) {
            return Err(Fault::Runtime);
        }
        out.info = Some(
            out.native
                .selected()
                .expect("retained selected owner")
                .describe_observed(
                    &mut out
                        .native_calls
                        .observer(gate, || capture(clock, &mut out.clock_error)),
                ),
        );
        let after = capture(clock, &mut out.clock_error);
        out.trace.returned(Operation::Describe, after, &shared);
        if out.native_calls.fault() == Some(super::nvml_trace::Fault::Clock) {
            shared.fail(ControlFault::Clock);
        }
        if out.trace.fault.is_some() {
            return Err(Fault::Control);
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
        if info.raw_uuid != scope.draft().raw_uuid || info.raw_pci != scope.draft().raw_pci {
            return Err(Fault::Runtime);
        }
        let before = capture(clock, &mut out.clock_error);
        if !out.trace.begin(Operation::Read, before, &shared) {
            return Err(Fault::Read);
        }
        let result = Snapshot::collect_native(
            scope,
            before.ok_or(Fault::Clock)?,
            out.native.selected().expect("selected retained"),
            gate,
            || {
                let a = capture(clock, &mut out.clock_error);
                let _ = out.trace.observe(a, &shared);
                a
            },
            budget,
        );
        match result {
            Ok(snapshot) => out.snapshot = Some(snapshot),
            Err(e) => out.snapshot_error = Some(e),
        }
        let after = capture(clock, &mut out.clock_error);
        out.trace.returned(Operation::Read, after, &shared);
        if out.trace.fault.is_some() {
            return Err(Fault::Read);
        }
        let snapshot = out.snapshot.as_ref().ok_or(Fault::Read)?;
        if let Err(e) = snapshot.current() {
            out.snapshot_error = Some(e);
            return Err(Fault::Read);
        }
        Ok(())
    })();
    let trigger = capture(clock, &mut out.clock_error);
    out.trace.stop_trigger = trigger;
    let _ = out.trace.observe(trigger, &shared);
    if let Err(f) = work {
        out.trace.fail(f, &shared);
    }
    if let Err(e) = control.request_stop(trigger) {
        out.control_error = Some(e);
        out.trace.fail(Fault::Control, &shared);
    }
    let initialized = out.native.initialized();
    if initialized {
        let before = capture(clock, &mut out.clock_error);
        if out.trace.begin(Operation::Cleanup, before, &shared) {
            let status = out.native.shutdown();
            out.trace.cleanup_status = status;
            // No implicit native cleanup during unwinding/drop. Release only a
            // noninitialized owner or one whose explicit shutdown has returned.
            out.native.release();
            let after = capture(clock, &mut out.clock_error);
            out.trace.cleanup_returned(after, status, &shared);
        }
    } else {
        out.native.release();
    }
    let a = capture(clock, &mut out.clock_error);
    let _ = out.trace.observe(a, &shared);
    if let Some(anchor) = a {
        let command = Completion {
            anchor,
            initialized_session: initialized,
            cleanup_attempted: out.trace.calls[3].attempted,
            cleanup_status: out.trace.cleanup_status,
        };
        if let Err(c) = control.complete(command) {
            out.unreported_completion = Some(c);
            out.trace.fail(Fault::Control, &shared);
        }
    } else {
        out.trace.fail(Fault::Clock, &shared);
    }
    loop {
        let output = transport.output_drained();
        let finished = control.thread_finished();
        let a = capture(clock, &mut out.clock_error);
        if out.trace.drain(a, output, finished, &shared) {
            break;
        }
        if a.is_none_or(|a| a.interrupt_100ns >= shared.cooperative_raw())
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
            if report.state.shared().fault().is_some() || !report.state.finished() {
                out.trace.fail(Fault::Control, &shared);
            }
            out.report = Some(report);
        }
        Ok(None) => out.trace.fail(Fault::Control, &shared),
        Err(e) => {
            out.control_error = Some(e);
            out.trace.fail(Fault::Control, &shared);
        }
    }
    out.control = Some(control);
}
