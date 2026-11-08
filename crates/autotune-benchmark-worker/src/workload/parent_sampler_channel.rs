//! One retained parent-to-sampler sender and its matching process poller.
//!
//! The adapter binds ParentSampler's inbound and outbound path to the same
//! retained sampler process. It does not launch, wait for exit, or grant compute.
use super::{
    launch::CancelQueue,
    parent_sampler::{self, ParentSampler, PreflightFailure, PreflightFault, ReportedPreflight},
    windows_process::{Process, ProcessQueue},
};
use crate::{
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{manifest::BoundRun, transport::Sender},
};

pub enum InitFault {
    Parent(parent_sampler::Fault),
    Sender(crate::workload::transport::SendFailure),
}

/// Owns the sampler protocol state and the unique sender from one process.
pub struct ParentSamplerChannel<'a> {
    process: &'a Process,
    parent: ParentSampler,
    sender: Sender<CancelQueue<ProcessQueue<'a>>>,
}

impl<'a> ParentSamplerChannel<'a> {
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
            return Err(PreflightFailure {
                fault: PreflightFault::Peers(super::parent_peers::Fault::Binding),
                decisions: Some(input.decisions),
            });
        }
        self.parent.verify_reported_preflight(input)
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
