//! Retain session evidence until retirement and every owned control thread end.
//! Polling is nonblocking; it joins only handles already observed finished.
use super::{
    parent_watch_windows::Report,
    retirement_collector::{Assets, Collector, Fault as CollectorFault, Outcome},
};
use crate::sampler::budget::Budget;
use crate::sampler::{
    archive::{Artifact, Receipt as ArchiveReceipt},
    producer::BoundText,
    writer::Writer,
};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartFault {
    OwnerBinding,
    Collector(CollectorFault),
}
pub struct StartFailure<E> {
    pub fault: StartFault,
    pub assets: Assets,
    pub evidence: E,
}

/// Concrete pilot-session owners that have no slot in ParentSampler or the
/// retirement process pair. Retaining these together prevents the starting
/// process, local consent, and original artifacts from disappearing while the
/// final controller outcome is unresolved. `additional` holds other bounded
/// caller evidence, such as workload exports, with its existing reservations.
pub struct PilotOwners<E> {
    pub manifest: super::manifest::PilotManifest,
    pub scope: super::starting::Scope,
    pub starting_process: super::windows_process::Process<super::starting_bootstrap::Declaration>,
    pub starting_handoff: super::starting_launch::ClosedHandoff,
    pub starting_consent: super::consent::Record,
    pub starting_read: super::starting_read::Receipt,
    pub starting_evidence: super::starting::ReportedSnapshot,
    pub host_environment: super::host_environment::Reported,
    pub worker_setup_delivery: super::worker_setup_delivery::ReadyDelivery,
    pub worker_report_evidence: super::parent_worker_channel::WorkerReportEvidence,
    pub additional: E,
}
impl<E> PilotOwners<E> {
    /// Require one cross-role-consistent set of immutable evidence before
    /// starting normal retirement collection. This checks ownership/binding
    /// links only; source claims remain untrusted and confer no execution right.
    pub fn validate(&self, assets: &Assets) -> Result<(), StartFault> {
        let scope = self.scope.draft();
        let manifest = self.manifest.manifest_draft();
        let starting_declaration = self.starting_process.declaration();
        let start_origin = self.starting_process.origin();
        let parent = &assets.parent;
        let start_was_queued = parent
            .start_attempt()
            .is_some_and(|attempt| attempt.queued && attempt.result.is_ok());
        let planned_stop_matches = parent
            .stop()
            .zip(assets.controller.requested_window())
            .is_some_and(|(parent, controller)| parent == controller);
        let start_response_matches = matches!(
            self.starting_handoff.session.outcome(),
            Some(super::starting_session::Outcome::Complete { digest, .. })
                if digest == self.starting_read.digest()
        );
        let parent_refs_match = assets.parent.context_check().is_some_and(|record| {
            record.result.is_ok_and(|summary| {
                let refs = summary.references;
                refs.manifest == self.manifest.digest()
                    && refs.scope == self.scope.digest()
                    && refs.host == Some(self.host_environment.snapshot().digest())
                    && refs.starting
                        == self
                            .starting_read
                            .snapshot()
                            .map(|snapshot| snapshot.snapshot().digest())
                    && refs.starting_response == Some(self.starting_read.digest())
                    && refs.worker_setup == self.worker_setup_delivery.receipt().digest()
            })
        });
        let worker_control_finished = self
            .worker_report_evidence
            .control_audit()
            .is_some_and(|audit| audit.role_finished() && audit.fault().is_none());
        if scope.session != manifest.session
            || scope.clock_token != manifest.clock_token
            || scope.raw_uuid != manifest.raw_uuid
            || scope.raw_pci != manifest.raw_pci
            || scope.source_commit != manifest.source_commit
            || scope.artifacts != manifest.artifacts
            || starting_declaration.scope_digest() != self.scope.digest()
            || self.starting_read.scope_digest() != self.scope.digest()
            || self.starting_read.declaration() != starting_declaration.tag()
            || self
                .starting_read
                .snapshot()
                .is_none_or(|snapshot| snapshot.snapshot().scope().digest() != self.scope.digest())
            || self.starting_evidence.snapshot().scope().digest() != self.scope.digest()
            || self.starting_evidence.snapshot().digest() != manifest.starting_evidence
            || self.host_environment.snapshot().scope().digest() != self.scope.digest()
            || self.host_environment.snapshot().digest() != manifest.environment_evidence
            || self.starting_consent.request().scope() != self.scope.digest()
            || self.starting_consent.result().is_err()
            || self.starting_consent.capture_ref() != start_origin.consent()
            || self.starting_handoff.session.origin().ok() != Some(start_origin)
            || !start_response_matches
            || self.worker_setup_delivery.receipt().binding() != assets.parent.context_binding()
            || !matches!(self.worker_setup_delivery.assessment(), Some(Ok(_)))
            || self.worker_report_evidence.binding() != assets.parent.context_binding()
            || self.worker_report_evidence.origin() != assets.worker.origin()
            || !worker_control_finished
            || !parent_refs_match
            || parent.fault().is_some()
            || !parent.is_finishing()
            || !parent.reported_finished()
            || !start_was_queued
            || !planned_stop_matches
            || assets.controller.abort_fault().is_some()
            || assets.controller.retired()
        {
            return Err(StartFault::OwnerBinding);
        }
        let peers = assets
            .parent
            .peer_check()
            .filter(|record| record.result.is_ok())
            .ok_or(StartFault::OwnerBinding)?;
        let expected = [
            start_origin,
            assets.sampler.origin(),
            assets.worker.origin(),
        ];
        if peers.peers.each_ref().map(|peer| peer.expected) != expected
            || peers.peers.each_ref().map(|peer| peer.received.ok()) != expected.map(Some)
            || self.worker_setup_delivery.origin().ok() != Some(expected[2])
            || assets.parent.consent().is_none_or(|delivery| {
                delivery.assessment.result.is_err()
                    || delivery.assessment.captures[0] != self.starting_consent.capture_ref()
            })
        {
            return Err(StartFault::OwnerBinding);
        }
        Ok(())
    }
}

/// One terminal handoff. `complete()` means only that host retirement was
/// acknowledged and all retained control threads returned cleanly. It is not
/// workload, GPU, correctness or profile acceptance.
pub struct Finalized<E> {
    pub retirement: Outcome,
    pub evidence: E,
    pub controller: Result<Report, super::parent_watch::Fault>,
    /// Results for operations which were actually started and joined.
    pub operations: [Option<Result<(), super::parent_watch::Fault>>; 4],
    pub retirement_acknowledged: bool,
}
impl<E> Finalized<E> {
    pub fn complete(&self) -> bool {
        self.retirement.result.is_ok()
            && self.retirement.operation == Some(Ok(()))
            && self.retirement.panics.iter().all(Option::is_none)
            && self
                .retirement
                .assets
                .progress()
                .is_some_and(|frame| frame.result().is_ok())
            && self.retirement_acknowledged
            && self.controller.as_ref().is_ok_and(|report| {
                report.state.retired()
                    && report.state.fault().is_none()
                    && report.state.overflow().is_none()
                    && report.state.capture_error().is_none()
                    && report.pending.is_empty()
            })
            && self.operations.iter().flatten().all(Result::is_ok)
    }
}
impl<E> Finalized<PilotOwners<E>> {
    /// Full host and worker-reported completion. This remains only reported
    /// host/child lifecycle evidence, not OS process-exit or GPU-quiescence
    /// proof, and does not imply candidate correctness or acceptance.
    pub fn complete_pilot(&self) -> bool {
        self.complete()
            && self
                .evidence
                .worker_report_evidence
                .control_audit()
                .is_some_and(|audit| {
                    audit.role_finished() && !audit.native_failure() && audit.fault().is_none()
                })
            && self.evidence.worker_report_evidence.reports_complete()
            && self
                .evidence
                .worker_report_evidence
                .assessment_fault()
                .is_none()
            && !self.evidence.worker_report_evidence.overflowed()
    }

    /// Queue raw/session leaves and retain every finalizer and source-text owner
    /// by borrow until the asynchronous workload writer has actually stopped.
    /// The writer must have been created with `Writer::start_pilot_workload`.
    pub fn queue_workload_archive<'a>(
        &'a self,
        writer: &'a mut Writer,
        raw: &'a BoundText,
        session: &'a BoundText,
        budget: &Budget,
    ) -> io::Result<PilotArchivePending<'a, E>> {
        let raw_copy = BoundText::copy(raw, budget)
            .map_err(|_| io::Error::other("workload raw evidence capacity"))?;
        writer.enqueue(Artifact::Raw, raw_copy)?;
        let session_copy = BoundText::copy(session, budget)
            .map_err(|_| io::Error::other("workload session evidence capacity"))?;
        writer.enqueue_pilot_session(&self.evidence.manifest, session_copy)?;
        writer.finish_pilot_workload(&self.evidence.manifest, session)?;
        Ok(PilotArchivePending {
            _finalized: self,
            _raw: raw,
            _session: session,
            writer,
            result: None,
            terminal: false,
        })
    }
}

/// Owns the borrows needed to keep finalized process/consent/evidence state and
/// source buffers alive until the archive writer has returned. Polling is
/// nonblocking; an unfinished writer remains pending, even after an error has
/// arrived, until its retained thread handle is signalled finished.
#[must_use = "retain this handle and poll it until the writer has finished"]
pub struct PilotArchivePending<'a, E> {
    _finalized: &'a Finalized<PilotOwners<E>>,
    _raw: &'a BoundText,
    _session: &'a BoundText,
    writer: &'a mut Writer,
    result: Option<Result<ArchiveReceipt, crate::sampler::writer::Failure>>,
    terminal: bool,
}
impl<E> PilotArchivePending<'_, E> {
    pub fn poll(&mut self) -> Option<Result<ArchiveReceipt, crate::sampler::writer::Failure>> {
        if self.terminal {
            return None;
        }
        if self.result.is_none() {
            self.result = self.writer.poll();
        }
        if !self.writer.thread_finished() {
            return None;
        }
        self.terminal = true;
        Some(
            self.result
                .take()
                .unwrap_or(Err(crate::sampler::writer::Failure {
                    operation: "writer_exit_without_result",
                    os_code: None,
                    kind: io::ErrorKind::BrokenPipe,
                })),
        )
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
    controller: Option<Result<Report, super::parent_watch::Fault>>,
}
impl<E> SessionFinalizer<E> {
    /// `evidence` is the caller's owned starting-read/artifact/session record;
    /// all of its own budget reservations remain alive in that value.
    #[allow(clippy::result_large_err)] // Failure returns all original assets/evidence without allocating.
    pub fn start(assets: Assets, evidence: E, budget: &Budget) -> Result<Self, StartFailure<E>> {
        match Collector::start(assets, budget) {
            Ok(collector) => Ok(Self {
                collector: Some(collector),
                retirement: None,
                evidence: Some(evidence),
                controller: None,
            }),
            Err(collector) => Err(StartFailure {
                fault: StartFault::Collector(collector.fault),
                assets: collector.assets,
                evidence,
            }),
        }
    }

    /// Typed construction path for the three-role pilot. The sampler and
    /// worker process/control owners stay in `Assets`; the original starting
    /// process/reception/consent and parsed artifacts stay in `PilotOwners`.
    #[allow(clippy::result_large_err)] // Failure returns all original assets/evidence without allocating.
    pub fn start_pilot<X>(
        assets: Assets,
        evidence: PilotOwners<X>,
        budget: &Budget,
    ) -> Result<SessionFinalizer<PilotOwners<X>>, StartFailure<PilotOwners<X>>> {
        if evidence.validate(&assets).is_err() {
            return Err(StartFailure {
                fault: StartFault::OwnerBinding,
                assets,
                evidence,
            });
        }
        SessionFinalizer::<PilotOwners<X>>::start(assets, evidence, budget)
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
