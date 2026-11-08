//! Retain session evidence until retirement and every owned control thread end.
//! Polling is nonblocking; it joins only handles already observed finished.
use super::{
    parent_watch_windows::{self, Report},
    retirement_collector::{self, Assets, Collector, Fault as CollectorFault, Outcome},
};
use crate::sampler::budget::Budget;

pub struct StartFailure<E> {
    pub collector: retirement_collector::StartFailure,
    pub evidence: E,
}

/// One terminal handoff. `complete()` means only that host retirement was
/// acknowledged and all retained control threads returned cleanly. It is not
/// workload, GPU, correctness or profile acceptance.
pub struct Finalized<E> {
    pub retirement: Outcome,
    pub evidence: E,
    pub controller: Result<Report, parent_watch_windows::Fault>,
    /// Results for operations which were actually started and joined.
    pub operations: [Option<Result<(), parent_watch_windows::Fault>>; 4],
    pub retirement_acknowledged: bool,
}
impl<E> Finalized<E> {
    pub fn complete(&self) -> bool {
        self.retirement.result.is_ok()
            && self
                .retirement
                .assets
                .progress()
                .is_some_and(|frame| frame.result().is_ok())
            && self.retirement_acknowledged
            && self.controller.is_ok()
            && self.operations.iter().flatten().all(Result::is_ok)
    }
}

/// Owns session evidence and the retirement collector until a terminal
/// controller report and all four request-operation handles have been joined.
/// A dropped finalizer is abandonment/fault containment, never successful
/// session completion.
#[must_use]
pub struct SessionFinalizer<E> {
    collector: Option<Collector>,
    retirement: Option<Outcome>,
    evidence: Option<E>,
    controller: Option<Result<Report, parent_watch_windows::Fault>>,
}
impl<E> SessionFinalizer<E> {
    /// `evidence` is the caller's owned starting-read/artifact/session record;
    /// all of its own budget reservations remain alive in that value.
    pub fn start(assets: Assets, evidence: E, budget: &Budget) -> Result<Self, StartFailure<E>> {
        match Collector::start(assets, budget) {
            Ok(collector) => Ok(Self {
                collector: Some(collector),
                retirement: None,
                evidence: Some(evidence),
                controller: None,
            }),
            Err(collector) => Err(StartFailure {
                collector,
                evidence,
            }),
        }
    }

    /// Returns `None` while collection, acknowledgment, controller shutdown or
    /// any operation handle remains pending. A returned value transfers all
    /// evidence and original owners exactly once, including failures.
    pub fn poll(&mut self) -> Option<Finalized<E>> {
        if self.retirement.is_none() {
            let collected = self.collector.as_mut()?.poll()?;
            self.collector.take();
            self.retirement = Some(collected);
        }

        let assets: &mut Assets = &mut self.retirement.as_mut()?.assets;
        let operations = assets.controller.poll_operations();
        if self.controller.is_none() {
            self.controller = assets.controller.poll();
        }
        if assets
            .controller
            .operation_threads_finished()
            .iter()
            .any(Option::is_some)
            || self.controller.is_none()
        {
            return None;
        }

        let retirement_acknowledged = assets.controller.retired();
        Some(Finalized {
            retirement: self.retirement.take().expect("retirement was retained"),
            evidence: self.evidence.take().expect("session evidence was retained"),
            controller: self.controller.take().expect("controller was observed"),
            operations,
            retirement_acknowledged,
        })
    }

    pub fn collection_finished(&self) -> bool {
        self.retirement.is_some() || self.collector.as_ref().is_some_and(Collector::finished)
    }

    /// Request fail-closed cancellation through the independent controller
    /// reference before or after collector completion. This is not an OS
    /// acknowledgment and does not discard any retained evidence.
    pub fn request_cancellation(&self) -> bool {
        if let Some(collector) = &self.collector {
            collector.cancellation().cancel();
            true
        } else if let Some(outcome) = &self.retirement {
            outcome.assets.controller.cancellation().cancel();
            true
        } else {
            false
        }
    }

    pub fn collector_fault(&self) -> Option<CollectorFault> {
        self.retirement.as_ref().and_then(|o| o.result.err())
    }
}
