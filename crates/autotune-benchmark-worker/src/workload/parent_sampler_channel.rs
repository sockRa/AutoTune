//! One retained parent-to-sampler sender and its matching process poller.
//!
//! The adapter binds ParentSampler's inbound and outbound path to the same
//! retained sampler process. It does not launch, wait for exit, or grant compute.
use super::{
    launch::CancelQueue,
    parent_sampler::{self, ParentSampler},
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
}
