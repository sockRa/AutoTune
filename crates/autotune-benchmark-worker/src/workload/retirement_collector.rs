//! One owned final host-exit observation off the independent deadline path.
//! Success queues evidence; retain the returned assets until the controller
//! acknowledges retirement and every controller/operation thread is joined.
pub use super::collector_task::Fault;
use super::{
    collector_task,
    parent_sampler::ParentSampler,
    parent_watch_retirement::Frame,
    parent_watch_windows::{Cancellation, Handle},
    windows_process::Process,
};
use crate::{
    monitor::windows_clock::WindowsClock,
    sampler::budget::{Budget, Reservation},
};
use std::{any::Any, sync::Arc};

/// Moving these owners requires ending the process-borrowing sender first.
/// Starting-read/artifact/consent owners outside ParentSampler still belong to
/// the enclosing session; this is not complete session orchestration.
pub struct Assets {
    pub parent: ParentSampler,
    pub controller: Handle,
    pub sampler: Process,
    pub worker: Process,
    clock: WindowsClock,
    progress: Option<Frame>,
}
impl Assets {
    pub fn new(
        parent: ParentSampler,
        controller: Handle,
        sampler: Process,
        worker: Process,
        clock: WindowsClock,
    ) -> Self {
        Self {
            parent,
            controller,
            sampler,
            worker,
            clock,
            progress: None,
        }
    }
    pub fn progress(&self) -> Option<&Frame> {
        self.progress.as_ref()
    }
}
pub struct Outcome {
    pub assets: Assets,
    pub result: Result<(), Fault>,
    pub operation: Option<Result<(), Fault>>,
    /// Separate operation and outer thread unwind payloads, without erasure.
    pub panics: [Option<Box<dyn Any + Send>>; 2],
    _allocation: Arc<Reservation>,
}
pub struct StartFailure {
    pub fault: Fault,
    pub error: Option<std::io::Error>,
    pub assets: Assets,
}
/// Dropping an uncollected task requests cancellation and abandons observation;
/// it is fault containment, never successful finalization. Keep this owner until
/// poll returns, then keep its assets through controller acknowledgement/join.
#[must_use]
pub struct Collector {
    task: collector_task::Task<Assets, Cancellation>,
    cancellation: Cancellation,
}
fn collect(assets: &mut Assets) -> Result<(), super::parent_watch_retirement::Fault> {
    assets.controller.retire_into(
        &assets.parent,
        &assets.sampler,
        &assets.worker,
        &assets.clock,
        &mut assets.progress,
    )
}
impl Collector {
    /// No new epoch, retries, grace period or native admission is created.
    #[allow(clippy::result_large_err)] // Return original owners without allocating on failure.
    pub fn start(assets: Assets, budget: &Budget) -> Result<Self, StartFailure> {
        let cancellation = assets.controller.cancellation();
        match collector_task::Task::start(assets, cancellation.clone(), budget, collect) {
            Ok(task) => Ok(Self { task, cancellation }),
            Err(e) => Err(StartFailure {
                fault: e.fault,
                error: e.error,
                assets: e.owned,
            }),
        }
    }
    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }
    pub fn finished(&self) -> bool {
        self.task.finished()
    }
    /// Nonblocking: observes thread completion before acquiring retained data.
    /// Returns owners once; success is not retirement acknowledgement or GPU completion.
    pub fn poll(&mut self) -> Option<Outcome> {
        self.task.poll().map(|o| Outcome {
            assets: o.owned,
            result: o.result,
            operation: o.operation,
            panics: o.panics,
            _allocation: o.allocation,
        })
    }
}
