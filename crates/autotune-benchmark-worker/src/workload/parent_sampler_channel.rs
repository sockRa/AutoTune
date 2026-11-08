//! One retained parent-to-sampler sender and its matching process poller.
//!
//! The adapter binds ParentSampler's inbound and outbound path to the same
//! retained sampler process. It does not launch, wait for exit, or grant compute.
use super::{
    launch::CancelQueue,
    parent_sampler::{self, ParentSampler, PreflightFailure, PreflightFault, ReportedPreflight},
    parent_worker_channel::{self, ParentWorkerChannel},
    starting_bootstrap,
    windows_process::{Process, ProcessQueue},
    worker_setup_delivery::ReadyDelivery,
};
use crate::{
    monitor::{windows_clock::WindowsClock, Anchor},
    sampler::budget::Budget,
    workload::{manifest::BoundRun, transport::Sender},
};
use std::io;

pub enum InitFault {
    Parent(parent_sampler::Fault),
    Sender(crate::workload::transport::SendFailure),
}

#[allow(clippy::large_enum_variant)] // Keep original stop/controller owners inline on failure.
pub enum ControllerStartFault {
    Parent(parent_sampler::Fault),
    SamplerStopOwner(io::Error),
    WorkerStopOwner {
        sampler: super::windows_process::StopOwner,
        error: io::Error,
    },
    Controller(super::parent_watch_windows::StartFailure),
    Attach {
        handle: super::parent_watch_windows::Handle,
        fault: super::parent_watch::Fault,
    },
}

/// Owns the sampler protocol state and the unique sender from one process.
pub struct ParentSamplerChannel<'a> {
    process: &'a Process,
    parent: ParentSampler,
    sender: Sender<CancelQueue<ProcessQueue<'a>>>,
}

impl<'a> ParentSamplerChannel<'a> {
    /// Exact sampler process whose sender and ParentSampler receive state this
    /// channel retains. Exposed for fixed parent composition, not replacement.
    pub fn process(&self) -> &'a Process {
        self.process
    }

    /// Constructs ParentSampler from this process declaration and takes its
    /// unique sender. A second channel cannot restart either lane sequence.
    pub fn new(
        process: &'a mut Process,
        run: &BoundRun<'_>,
        setup_entry: Anchor,
        first: Anchor,
        budget: &Budget,
    ) -> Result<Self, InitFault> {
        let parent = ParentSampler::for_process(run, process, setup_entry, first, budget.clone())
            .map_err(InitFault::Parent)?;
        let sender = process.sender(budget).map_err(InitFault::Sender)?;
        Ok(Self {
            process,
            parent,
            sender,
        })
    }

    pub fn parent(&self) -> &ParentSampler {
        &self.parent
    }

    pub fn parent_mut(&mut self) -> &mut ParentSampler {
        &mut self.parent
    }

    /// Run the ordered reported-evidence gate only when its sampler peer is
    /// this channel's process. A mismatch closes setup's reference path and
    /// returns untouched consent decisions to the caller.
    #[allow(clippy::result_large_err)] // Preserve the original consent owner without allocating.
    pub fn verify_reported_preflight(
        &mut self,
        input: ReportedPreflight<'_, '_>,
    ) -> Result<(), PreflightFailure> {
        if !std::ptr::eq(input.processes.sampler, self.process) {
            input.processes.setup.close_reference_path();
            self.parent.reject_process_owner_binding();
            return Err(PreflightFailure {
                fault: PreflightFault::Peers(super::parent_peers::Fault::Binding),
                decisions: Some(input.decisions),
            });
        }
        self.parent.verify_reported_preflight(input)
    }

    /// Start and attach the independent controller only after every retained
    /// reported preflight result succeeded. Original process owners and any
    /// partial controller remain in the error value on construction failure.
    #[allow(clippy::result_large_err)] // Return original OS/controller owners without allocating.
    pub fn start_and_attach_watch(
        &mut self,
        worker: &Process,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> Result<super::parent_watch_windows::Handle, ControllerStartFault> {
        let parent_fault = self.parent.fault();
        if let Some(fault) = parent_fault {
            return Err(ControllerStartFault::Parent(fault));
        }
        if !self
            .parent
            .environment_check()
            .is_some_and(|(_, result)| result.is_ok())
            || !self
                .parent
                .starting_check()
                .is_some_and(|(_, result)| result.is_ok())
            || !self
                .parent
                .context_check()
                .is_some_and(|record| record.result.is_ok())
            || !self
                .parent
                .peer_check()
                .is_some_and(|record| record.result.is_ok())
            || !self
                .parent
                .consent()
                .is_some_and(|delivery| delivery.assessment.result.is_ok())
        {
            return Err(ControllerStartFault::Parent(parent_sampler::Fault::Context));
        }

        self.start_watch(worker, clock, budget)
    }

    /// Start safety supervision while the original context remains open,
    /// without granting Start or requiring the later reported preflight. This
    /// lets the original setup deadline and parent faults trigger retained
    /// stop/termination requests while context and other evidence are checked.
    #[allow(clippy::result_large_err)]
    pub fn start_and_attach_safety_watch(
        &mut self,
        worker: &Process,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> Result<super::parent_watch_windows::Handle, ControllerStartFault> {
        if self.parent.fault().is_some() || !self.parent.context_open() {
            return Err(ControllerStartFault::Parent(parent_sampler::Fault::Context));
        }

        self.start_watch(worker, clock, budget)
    }

    #[allow(clippy::result_large_err)]
    fn start_watch(
        &mut self,
        worker: &Process,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> Result<super::parent_watch_windows::Handle, ControllerStartFault> {
        let sampler_owner = self
            .process
            .stop_owner(budget)
            .map_err(ControllerStartFault::SamplerStopOwner)?;
        let worker_owner = match worker.stop_owner(budget) {
            Ok(owner) => owner,
            Err(error) => {
                return Err(ControllerStartFault::WorkerStopOwner {
                    sampler: sampler_owner,
                    error,
                })
            }
        };
        let handle = match super::parent_watch_windows::Handle::start(
            &self.parent,
            [sampler_owner, worker_owner],
            clock,
            budget.clone(),
        ) {
            Ok(handle) => handle,
            Err(failure) => return Err(ControllerStartFault::Controller(failure)),
        };
        match self.parent.attach_watch(&handle) {
            Ok(()) => Ok(handle),
            Err(fault) => Err(ControllerStartFault::Attach { handle, fault }),
        }
    }

    /// Queue the worker's one-shot Start through its retained channel only
    /// after this ParentSampler's original controller reports ready. The
    /// process peer is fixed to this channel's sampler owner.
    pub fn queue_worker_start_when_watch_ready(
        &mut self,
        worker: &mut ParentWorkerChannel<'_>,
        starting: &Process<starting_bootstrap::Declaration>,
        setup: &mut ReadyDelivery,
        clock: &WindowsClock,
    ) -> Result<parent_worker_channel::StartProgress, super::parent_start::Fault> {
        worker.queue_start_when_watch_ready(&mut self.parent, starting, self.process, setup, clock)
    }

    /// Collect fresh process/safety/re-entry evidence and queue the one-shot
    /// NativeGrant on the worker's existing Start sequence, using the same
    /// sampler process that feeds this ParentSampler.
    pub fn collect_and_queue_native_grant(
        &mut self,
        worker: &mut ParentWorkerChannel<'_>,
        starting: &Process<starting_bootstrap::Declaration>,
        setup: &mut ReadyDelivery,
        clock: &WindowsClock,
    ) -> Result<(), parent_sampler::Fault> {
        worker.collect_and_queue_native_grant(
            &mut self.parent,
            starting,
            self.process,
            setup,
            clock,
        )
    }

    /// Performs one nonblocking process poll and any resulting bounded query
    /// or stop handoff, always using this channel's retained process sender.
    pub fn poll(
        &mut self,
        run: &BoundRun<'_>,
        clock: impl FnMut() -> Option<Anchor>,
    ) -> parent_sampler::Poll {
        self.parent
            .drive_process(run, self.process, clock, &mut self.sender)
    }

    /// Retains ParentSampler's planned finish/stop result through the same
    /// unique sender used during polling.
    pub fn finish(&mut self, anchor: Option<Anchor>) -> Result<(), parent_sampler::Fault> {
        self.parent.finish(anchor, &mut self.sender)
    }

    /// Issue an immediate fail-closed stop when another parent lane fails.
    /// The request is retained on the original ParentSampler/process owner and
    /// the bounded Stop frame uses the same sender; no new time window is made.
    pub fn abort_for_parent_fault(&mut self, anchor: Option<Anchor>) {
        self.parent.abort_for_parent_fault(anchor);
        if self.parent.stop_event().is_none() {
            self.parent.retain_abort_stop_event(
                self.process
                    .request_stop()
                    .map_err(|error| error.raw_os_error()),
            );
        }
        self.parent.queue_stop(&mut self.sender);
    }

    /// Release the process-borrowing sender and transfer the accumulated
    /// ParentSampler owner to retirement/finalization. The caller can then
    /// move the same process owner into `retirement_collector::Assets`.
    pub fn into_parent(self) -> ParentSampler {
        let Self {
            process: _,
            parent,
            sender,
        } = self;
        drop(sender);
        parent
    }
}
