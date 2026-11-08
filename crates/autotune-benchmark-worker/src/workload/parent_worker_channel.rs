//! One retained parent-to-worker sender for the fixed Windows pilot process.
//!
//! Start and NativeGrant share this owner and therefore the same ordinary
//! channel sequence. The adapter binds the process-state reads to the worker
//! whose inherited pipe owns that sender. It does not grant permission by
//! itself; ParentSampler still applies its original-context, safety, deadline
//! and one-shot checks.
use super::{
    launch::CancelQueue,
    parent_sampler::{Fault, ParentSampler},
    parent_start,
    process_state::Processes,
    starting_bootstrap, windows_process,
    worker_setup_delivery::{self, ReadyDelivery, Receiver},
};
use crate::{monitor::windows_clock::WindowsClock, sampler::budget::Budget};

/// Owns the unique ordinary sender derived from one retained worker process.
/// Keep this value alive from Start through the optional one-shot NativeGrant.
pub struct ParentWorkerChannel<'a> {
    worker: &'a windows_process::Process,
    sender: crate::workload::transport::Sender<CancelQueue<windows_process::ProcessQueue<'a>>>,
}

impl<'a> ParentWorkerChannel<'a> {
    /// Takes the worker's single sender. A second channel cannot be created
    /// from this process owner, preventing sequence restarts between frames.
    pub fn new(
        worker: &'a windows_process::Process,
        budget: &Budget,
    ) -> Result<Self, crate::workload::transport::SendFailure> {
        let sender = worker.sender(budget)?;
        Ok(Self { worker, sender })
    }

    /// Poll one packet from the same retained worker process and feed the
    /// original strict setup receiver. Idle polls still tick its frozen
    /// deadline. On failure `receiver` remains owned by the caller with its
    /// first fault and rejected/raw packet intact.
    pub fn poll_setup(
        &self,
        receiver: &mut Option<Receiver>,
        clock: &WindowsClock,
    ) -> Result<Option<ReadyDelivery>, worker_setup_delivery::Fault> {
        use super::windows_transport::Receive;
        let Some(current) = receiver.as_mut() else {
            return Err(worker_setup_delivery::Fault::Protocol);
        };
        match self.worker.receive(|| clock.capture().ok()) {
            Receive::Packet(raw) => current.accept(raw)?,
            Receive::Idle { anchor } => current.tick(anchor)?,
            Receive::Fault { code, anchor } => {
                current.transport_failed(code, anchor);
                return Err(current
                    .fault()
                    .unwrap_or(worker_setup_delivery::Fault::Transport));
            }
        }
        if current.ready().is_none() || current.receipt().is_none() {
            return Ok(None);
        }
        let completed = receiver.take().expect("setup receiver is retained");
        match completed.into_ready() {
            Ok(ready) => Ok(Some(ready)),
            Err(rejected) => {
                let fault = rejected
                    .fault()
                    .unwrap_or(worker_setup_delivery::Fault::Protocol);
                *receiver = Some(rejected);
                Err(fault)
            }
        }
    }

    /// Make the one Start attempt using the exact worker that owns this pipe.
    pub fn queue_start(
        &mut self,
        parent: &mut ParentSampler,
        starting: &windows_process::Process<starting_bootstrap::Declaration>,
        sampler: &windows_process::Process,
        setup: &mut ReadyDelivery,
        clock: &WindowsClock,
    ) -> Result<(), parent_start::Fault> {
        parent.queue_start(
            Processes {
                starting,
                sampler,
                worker: self.worker,
                setup,
            },
            clock,
            &mut self.sender,
        )
    }

    /// Collect a fresh retained-process frame and queue NativeGrant on the
    /// same sender/sequence as Start. The caller must keep this blocking OS
    /// observation off the independent deadline-controller lane.
    pub fn collect_and_queue_native_grant(
        &mut self,
        parent: &mut ParentSampler,
        starting: &windows_process::Process<starting_bootstrap::Declaration>,
        sampler: &windows_process::Process,
        setup: &mut ReadyDelivery,
        clock: &WindowsClock,
    ) -> Result<(), Fault> {
        parent.collect_and_queue_native_grant(
            Processes {
                starting,
                sampler,
                worker: self.worker,
                setup,
            },
            clock,
            &mut self.sender,
        )
    }
}
