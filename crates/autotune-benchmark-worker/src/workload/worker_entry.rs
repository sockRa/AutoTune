//! Actual fixed-role entry composition. The feature-gated fixed worker binary
//! issues NativeAdmission only from the retained, setup-bound parent grant.
use super::{
    bootstrap::EntryClock,
    child_windows::{Context, Failure as StartupFailure},
    control_windows::{Handle, Report},
    launch::{Arguments, Role},
    producer::End,
    transport::WorkerReceived,
    wire::Direction,
    worker_finish::{ClockChain, Fault as FinishFault, State as Finish},
    worker_native::{self, Preparation, Prepared},
    worker_runtime::Session,
};
use crate::{
    cuda::CudaBackend,
    evidence::Reason,
    monitor::{
        windows_clock::{CaptureError, WindowsClock},
        Anchor,
    },
    sampler::{
        budget::{Budget, Reservation},
        runtime::IoObservation,
    },
};
use std::{
    cell::{Ref, RefCell},
    rc::Rc,
    thread,
    time::Duration,
};
type ClockFn = Box<dyn FnMut() -> Option<Anchor>>;
type StopFn = Box<dyn FnMut() -> Option<super::gate::StopWindow>>;
pub type NativeSession = Session<CudaBackend, ClockFn, StopFn>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Startup,
    Clock,
    Control,
    Native,
    Ready,
    Protocol,
    Admission,
    Worker,
    Export,
    Finish,
}

pub struct Captures {
    clock: WindowsClock,
    chain: RefCell<ClockChain>,
    os_error: RefCell<Option<CaptureError>>,
}
impl Captures {
    fn capture(&self) -> Option<Anchor> {
        {
            let c = self.chain.borrow();
            if c.fault().is_some() || c.missing() {
                return None;
            }
        }
        let raw = match self.clock.capture() {
            Ok(a) => Some(a),
            Err(e) => {
                self.os_error.borrow_mut().get_or_insert(e);
                None
            }
        };
        self.chain.borrow_mut().observe(raw)
    }
    pub fn chain(&self) -> Ref<'_, ClockChain> {
        self.chain.borrow()
    }
    pub fn os_error(&self) -> Option<CaptureError> {
        *self.os_error.borrow()
    }
}
pub struct Outcome {
    pub context: Context,
    pub startup_history: EntryClock,
    pub captures: Rc<Captures>,
    pub control: Option<Handle>,
    pub report: Option<Report>,
    pub prepared: Option<Prepared<ClockFn, StopFn>>,
    pub setup_report: Option<super::worker_setup::Artifact>,
    pub setup_report_error: Option<super::worker_setup::Fault>,
    pub prepare_failure: Option<worker_native::Failure<ClockFn, StopFn>>,
    pub surplus: Option<WorkerReceived>,
    pub admission_rejection: Option<Reason>,
    pub fault: Option<Fault>,
    pub control_error: Option<super::control::Fault>,
    pub split_error: Option<super::transport::SendFailure>,
    pub sender_error: Option<super::transport::SendFailure>,
    pub event_clone_error: Option<std::io::Error>,
    pub stop_trigger: Option<Anchor>,
    pub finish_first: Option<Anchor>,
    pub finish: Option<Finish>,
    pub finish_error: Option<FinishFault>,
    pub completion_result: Option<Result<(), super::control::Fault>>,
    pub io_shutdown: Option<[IoObservation; 4]>,
    pub io_completion: Option<[bool; 4]>,
    pub io_receipt: Option<Anchor>,
    _metadata: Reservation,
}
impl Outcome {
    /// Child completion only. No process/job exit, qualification or GPU fence.
    pub fn exit_code(&self) -> i32 {
        let completed = matches!(self.prepared.as_ref().map(|p|&p.result),Some(Preparation::Ready{session,..}) if session.fault().is_none()&&session.producer().end()==Some(End::ReportsComplete));
        if self.fault.is_none()
            && completed
            && self.finish.as_ref().is_some_and(Finish::complete)
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
}
pub struct Failure {
    pub fault: Fault,
    pub startup: Option<StartupFailure>,
    pub context: Option<Context>,
    pub metadata: Option<Reservation>,
}
#[allow(clippy::result_large_err)] // Retain original inherited owners without error allocation.
/// # Safety
/// Call once at the actual fixed worker entry, before any other adoption of its
/// inherited endpoints. Exact native setup/manifest/device/artifact consent and
/// independent supervision are prerequisites. A distinct post-Start
/// NativeGrant must match the setup artifact and original deadline before
/// the entry issues NativeAdmission from the exact accepted grant and worker
/// setup artifact. The fixed parent sender validates the complete parent
/// facts/consent/safety chain; fixtures and bootstrap/Ready/Start cannot issue it.
pub unsafe fn entry(args: &Arguments) -> Result<Outcome, Failure> {
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let budget = Budget::new(p.evidence_ram_bytes as usize).map_err(|_| Failure {
        fault: Fault::Capacity,
        startup: None,
        context: None,
        metadata: None,
    })?;
    let metadata = budget
        .reserve(std::mem::size_of::<Outcome>() + 4096)
        .map_err(|_| Failure {
            fault: Fault::Capacity,
            startup: None,
            context: None,
            metadata: None,
        })?;
    let mut context = match unsafe { Context::from_inherited(args, Role::Worker, budget.clone()) } {
        Ok(c) => c,
        Err(startup) => {
            return Err(Failure {
                fault: Fault::Startup,
                startup: Some(startup),
                context: None,
                metadata: Some(metadata),
            })
        }
    };
    let bootstrap = context.bootstrap();
    let binding = bootstrap.binding();
    let first = bootstrap.receipt();
    let chain = match ClockChain::new(binding, first) {
        Ok(v) => v,
        Err(_) => {
            return Err(Failure {
                fault: Fault::Clock,
                startup: None,
                context: Some(context),
                metadata: Some(metadata),
            })
        }
    };
    let history = match EntryClock::new(
        binding.monitor().qpc_frequency,
        binding.epoch,
        first,
        &budget,
    ) {
        Ok(v) => v,
        Err(_) => {
            return Err(Failure {
                fault: Fault::Capacity,
                startup: None,
                context: Some(context),
                metadata: Some(metadata),
            })
        }
    };
    // A clone copies immutable token/frequency only; epoch validation stays in
    // this original ClockChain and every retained stage ledger.
    let clock = context.parts().2.clone();
    let captures = Rc::new(Captures {
        clock,
        chain: RefCell::new(chain),
        os_error: RefCell::new(None),
    });
    let mut out = Outcome {
        context,
        startup_history: history,
        captures,
        control: None,
        report: None,
        prepared: None,
        setup_report: None,
        setup_report_error: None,
        prepare_failure: None,
        surplus: None,
        admission_rejection: None,
        fault: None,
        control_error: None,
        split_error: None,
        sender_error: None,
        event_clone_error: None,
        stop_trigger: None,
        finish_first: None,
        finish: None,
        finish_error: None,
        completion_result: None,
        io_shutdown: None,
        io_completion: None,
        io_receipt: None,
        _metadata: metadata,
    };
    run(&mut out);
    Ok(out)
}
fn run(out: &mut Outcome) {
    let captures = out.captures.clone();
    let (bootstrap, port, _, event, budget) = out.context.parts();
    let binding = bootstrap.binding();
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let control_port = match port.split_control() {
        Ok(v) => v,
        Err(e) => {
            out.split_error = Some(e);
            out.fault = Some(Fault::Control);
            out.io_shutdown = Some(port.shutdown());
            return;
        }
    };
    let event = match event.try_clone() {
        Ok(v) => v,
        Err(e) => {
            out.event_clone_error = Some(e);
            out.fault = Some(Fault::Control);
            out.io_shutdown = Some(port.shutdown());
            return;
        }
    };
    let first = captures.capture();
    let _ = out.startup_history.observe(first);
    let Some(first) = first else {
        out.fault = Some(Fault::Clock);
        out.io_shutdown = Some(port.shutdown());
        return;
    };
    let mut control = match Handle::start(
        control_port,
        event,
        binding,
        Role::Worker,
        first,
        budget.clone(),
    ) {
        Ok(v) => v,
        Err(e) => {
            out.fault = Some(Fault::Control);
            out.control_error = Some(e);
            out.io_shutdown = Some(port.shutdown());
            return;
        }
    };
    let shared = control.shared().clone();
    let configured = shared.configure_setup(binding, bootstrap.deadline_ns());
    let work = (|| -> Result<(), Fault> {
        configured.map_err(|e| {
            out.control_error = Some(e);
            Fault::Control
        })?;
        while !control.ready() {
            let a = captures.capture();
            let n = out.startup_history.observe(a).map_err(|_| Fault::Clock)?;
            if n >= bootstrap.deadline_ns()
                || shared.admission().stopped()
                || control.thread_finished()
            {
                return Err(Fault::Control);
            }
            thread::sleep(Duration::from_nanos(p.controller_tick_ns));
        }
        let mut sender = port
            .sender(binding, Direction::FromWorker, budget)
            .map_err(|e| {
                out.sender_error = Some(e);
                Fault::Control
            })?;
        let clock_owner = captures.clone();
        let clock: ClockFn = Box::new(move || clock_owner.capture());
        let stop: StopFn = Box::new(|| None);
        match unsafe { worker_native::prepare(bootstrap, shared.clone(), clock, stop, budget) } {
            Ok(v) => out.prepared = Some(v),
            Err(f) => {
                out.prepare_failure = Some(f);
                return Err(Fault::Native);
            }
        }
        match out
            .prepared
            .as_ref()
            .expect("preparation owner")
            .setup_artifact(budget)
        {
            Ok(report) => out.setup_report = Some(report),
            Err(error) => {
                out.setup_report_error = Some(error);
                return Err(Fault::Export);
            }
        }
        let Preparation::Ready { session, facts } =
            &mut out.prepared.as_mut().expect("preparation owner").result
        else {
            return Err(Fault::Native);
        };
        session
            .send_setup(
                out.setup_report
                    .as_ref()
                    .expect("setup report before Ready"),
                &mut sender,
                budget,
            )
            .map_err(|_| Fault::Export)?;
        session
            .announce(*facts, &mut sender)
            .map_err(|_| Fault::Ready)?;
        while session.start_packet().is_none() {
            if control.thread_finished() {
                return Err(Fault::Control);
            }
            if let Err(raw) = session.poll_native(port) {
                out.surplus = Some(raw);
                return Err(Fault::Protocol);
            }
            if session.fault().is_some() {
                return Err(Fault::Protocol);
            }
            if session.start_packet().is_none() {
                thread::sleep(Duration::from_nanos(p.controller_tick_ns));
            }
        }
        while session.native_grant_packet().is_none() {
            if control.thread_finished() {
                return Err(Fault::Control);
            }
            if let Err(raw) = session.poll_native(port) {
                out.surplus = Some(raw);
                return Err(Fault::Protocol);
            }
            if session.fault().is_some() {
                return Err(Fault::Protocol);
            }
            if session.native_grant_packet().is_none() {
                thread::sleep(Duration::from_nanos(p.controller_tick_ns));
            }
        }
        let admission = match session.native_admission(
            out.setup_report
                .as_ref()
                .expect("setup report before Ready"),
        ) {
            Ok(v) => v,
            Err(_) => {
                out.admission_rejection =
                    Some(Reason::new("native_grant_rejected").expect("fixed reason"));
                return Err(Fault::Admission);
            }
        };
        session
            .start_native(admission)
            .map_err(|_| Fault::Admission)?;
        while session.producer().end().is_none() {
            if control.thread_finished() {
                return Err(Fault::Control);
            }
            if let Err(raw) = session.poll_native(port) {
                out.surplus = Some(raw);
                return Err(Fault::Protocol);
            }
            if session.fault().is_some() {
                return Err(Fault::Worker);
            }
            session.advance(&mut sender);
            if session.producer().end().is_none() {
                thread::sleep(Duration::from_nanos(p.controller_tick_ns));
            }
        }
        if !session.producer().retained_output().is_empty()
            && session
                .producer()
                .last_record()
                .is_some_and(|r| r.record().mismatches != 0)
        {
            loop {
                match session.export_failure_step(&mut sender) {
                    Ok(true) => break,
                    Ok(false) => (),
                    Err(_) => return Err(Fault::Export),
                }
            }
        }
        if matches!(session.producer().end(), Some(End::Fault(_))) {
            return Err(Fault::Worker);
        }
        Ok(())
    })();
    if let Err(f) = work {
        out.fault.get_or_insert(f);
    }
    let stopped = matches!(out.prepared.as_ref().map(|p|&p.result),Some(Preparation::Ready{session,..}) if session.producer().end()==Some(End::Stopped));
    if work.is_err() || stopped {
        let trigger = captures.capture();
        out.stop_trigger = trigger;
        if let Err(e) = control.request_stop(trigger) {
            out.control_error.get_or_insert(e);
            out.fault.get_or_insert(Fault::Control);
        }
    }
    let producer_deadline = match out.prepared.as_ref().map(|p| &p.result) {
        Some(Preparation::Ready { session, .. }) => session.producer().exit_deadline_ns(),
        _ => None,
    };
    let window_deadline = shared
        .stop_window(binding)
        .ok()
        .flatten()
        .map(|w| w.exit_ns);
    let deadline = match (producer_deadline, window_deadline) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    out.finish_first = captures.capture();
    if let (Some(a), Some(d)) = (out.finish_first, deadline) {
        match Finish::new(binding, shared.clone(), a, d, budget) {
            Ok(v) => out.finish = Some(v),
            Err(e) => out.finish_error = Some(e),
        }
    } else {
        out.finish_error = Some(if out.finish_first.is_none() {
            FinishFault::Clock
        } else {
            FinishFault::Protocol
        });
    }
    if let Some(prepared) = out.prepared.as_mut() {
        prepared.cleanup_unprepared(shared.clone());
    }
    let cleanup = match out.prepared.as_mut() {
        Some(prepared) if matches!(prepared.result, Preparation::PrepareFailed(_)) => {
            let raw = prepared
                .prestart
                .requested()
                .then_some(prepared.prestart.cleanup);
            if let Some(f) = &mut out.finish {
                f.record_cleanup(raw, prepared.prestart.ledger.fault());
            }
            raw
        }
        Some(Prepared {
            result: Preparation::Ready { session, .. },
            ..
        }) => {
            session.cleanup();
            let raw = session.cleanup_observation();
            let fault = session.teardown_fault();
            if let Some(f) = &mut out.finish {
                f.record_cleanup(raw, fault);
            }
            raw
        }
        _ => {
            if let Some(f) = &mut out.finish {
                f.record_cleanup(None, None);
            }
            None
        }
    };
    let status = cleanup.and_then(|c| c.result).map(|r| match r {
        Ok(()) => 0,
        Err(e) => e.raw_status,
    });
    let completion = control.complete(cleanup.is_some_and(|c| c.attempted), status);
    out.completion_result = Some(completion);
    if let Some(f) = &mut out.finish {
        f.completion_queued(completion);
    }
    if let Err(e) = completion {
        out.control_error.get_or_insert(e);
        out.fault.get_or_insert(Fault::Control);
    }
    if let Some(f) = &mut out.finish {
        loop {
            let output = port.output_drained();
            let done = control.thread_finished();
            let a = captures.capture();
            if f.drain(a, output, done) {
                break;
            }
            if a.is_none_or(|v| {
                binding
                    .elapsed(v)
                    .map_or(true, |n| n >= f.original_deadline_ns())
                    || v.interrupt_100ns >= shared.cooperative_raw()
            }) || f.history.missing()
                || f.history.clock_error().is_some()
                || f.history.overflow().is_some()
            {
                break;
            }
            thread::sleep(Duration::from_nanos(p.controller_tick_ns));
        }
    }
    match control.take_report() {
        Ok(Some(r)) => {
            if !r.state.finished() || r.state.fault().is_some() {
                out.fault.get_or_insert(Fault::Control);
            }
            out.report = Some(r);
        }
        Ok(None) => {
            out.fault.get_or_insert(Fault::Control);
        }
        Err(e) => {
            out.control_error.get_or_insert(e);
            out.fault.get_or_insert(Fault::Control);
        }
    }
    out.control = Some(control);
    let io = port.shutdown();
    out.io_shutdown = Some(io);
    let a = captures.capture();
    out.io_receipt = a;
    if let Some(f) = &mut out.finish {
        f.io_started(io, a);
        loop {
            let done = port.io_completion();
            let a = captures.capture();
            out.io_completion = Some(done);
            out.io_receipt = a;
            if f.io_poll(done, a) {
                break;
            }
            if a.is_none_or(|v| {
                binding
                    .elapsed(v)
                    .map_or(true, |n| n >= f.original_deadline_ns())
                    || v.interrupt_100ns >= shared.cooperative_raw()
            }) || f.history.missing()
                || f.history.clock_error().is_some()
                || f.history.overflow().is_some()
            {
                break;
            }
            thread::sleep(Duration::from_nanos(p.controller_tick_ns));
        }
    } else {
        out.io_completion = Some(port.io_completion());
    }
    if !out.finish.as_ref().is_some_and(Finish::complete) {
        out.fault.get_or_insert(Fault::Finish);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_fixed_entry_is_monomorphized_and_linked_without_invocation() {
        let pointer: unsafe fn(&Arguments) -> Result<Outcome, Failure> = entry;
        // Force code generation of the complete real entry. Never call the
        // pointer: this test performs no adoption, thread/DLL load or GPU work.
        std::hint::black_box(pointer);
    }
}
