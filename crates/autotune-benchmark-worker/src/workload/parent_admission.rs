//! Ordered parent admission loop over the retained sampler and worker channels.
//!
//! This composes setup, reported preflight, the independent watch, Start and
//! NativeGrant. Process launch, final retirement and workload qualification
//! remain outside this admission owner.
use super::{
    consent::{self, Record as ConsentRecord},
    host_environment::Reported as HostEnvironment,
    manifest::BoundRun,
    parent_sampler::{self, PreflightFailure, ReportedPreflight},
    parent_sampler_channel::{self, ParentSamplerChannel},
    parent_watch_windows::Handle as ControllerHandle,
    parent_worker_channel::{self, ParentWorkerChannel},
    starting::{ReportedSnapshot, Scope},
    starting_bootstrap,
    starting_read::Receipt as StartingReceipt,
    starting_session::Session as StartingSession,
    windows_process::Process,
    worker_setup_delivery::{self, ReadyDelivery, Receiver as SetupReceiver},
};
use crate::{
    monitor::{windows_clock::WindowsClock, Anchor},
    sampler::budget::Budget,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    SetupPending,
    SetupReady,
    PreflightAccepted,
    ControllerAttached,
    StartQueued,
    NativeGrantQueued,
    Failed,
}

pub struct InitFailure {
    pub fault: InitFault,
    pub setup: Option<SetupReceiver>,
    pub starting_launch: super::starting_launch::LaunchFacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitFault {
    SamplerAlreadyFaulted,
    WorkerAlreadyFaulted,
    MissingSetupReceiver,
    StartingLaunchIneligible,
}

/// One bounded, nonblocking parent loop round. Each process lane is polled at
/// most once here; all packet owners remain in their channel or setup owner.
pub struct Turn {
    pub sampler: parent_sampler::Poll,
    pub setup: Option<Result<bool, worker_setup_delivery::Fault>>,
    pub worker_control: parent_worker_channel::ControlPoll,
    pub worker_result: Option<parent_worker_channel::ResultPoll>,
}

#[allow(clippy::large_enum_variant)] // Keep unconsumed consent decisions inline on rejection.
pub enum AdmissionProgress {
    SetupPending,
    AwaitingPreflight,
    PreflightAccepted,
    ControllerAttached,
    WaitingForWatch,
    StartQueued,
    NativeGrantQueued,
    PreflightRejected(PreflightFailure),
    ControllerStartFailed(parent_sampler_channel::ControllerStartFault),
    StartFailed(super::parent_start::Fault),
    NativeGrantFailed(parent_sampler::Fault),
    Failed,
}

pub struct AdmissionTurn<'r, 's> {
    pub observed: Turn,
    pub progress: AdmissionProgress,
    /// Remains owned by the caller until required peers/setup are ready.
    pub preflight: Option<PreflightInput<'r, 's>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalProgress {
    AdmissionIncomplete,
    AwaitingWorkerResults,
    PlannedStopQueued,
    AwaitingRoleCompletion,
    TerminalEvidenceReady,
    Failed,
    StopFailed(parent_sampler::Fault),
    ControlAuditFailed(parent_worker_channel::ControlFault),
}

pub struct TerminalTurn {
    pub observed: Turn,
    pub progress: TerminalProgress,
}

#[allow(clippy::large_enum_variant)] // Keep unconsumed caller evidence inline; boxing would add an unbudgeted transfer.
pub enum PilotProgress {
    Admission(AdmissionProgress),
    Shutdown(TerminalProgress),
}

/// One nonblocking round across the complete parent admission and successful
/// terminal handshake. The caller still owns process launch, failure cleanup,
/// retirement polling and archive completion.
pub struct PilotTurn<'r, 'p> {
    pub observed: Turn,
    pub progress: PilotProgress,
    /// A prepared preflight bundle remains caller-owned until admission uses it.
    pub preflight: Option<PreflightInput<'r, 'p>>,
}

pub struct Owners {
    pub stage: Stage,
    pub parent: parent_sampler::ParentSampler,
    pub worker_report: parent_worker_channel::WorkerReportEvidence,
    pub setup_receiver: Option<SetupReceiver>,
    pub setup: Option<ReadyDelivery>,
    pub controller: Option<ControllerHandle>,
}

pub struct RetirementOwners {
    pub assets: super::retirement_collector::Assets,
    pub stage: Stage,
    pub worker_report: parent_worker_channel::WorkerReportEvidence,
    pub setup_receiver: Option<SetupReceiver>,
    pub setup: Option<ReadyDelivery>,
}

pub struct PilotOwnerInputs<E> {
    pub manifest: super::manifest::PilotManifest,
    pub scope: super::starting::Scope,
    pub starting_process: Process<starting_bootstrap::Declaration>,
    pub starting_launch: super::starting_launch::LaunchFacts,
    pub starting_handoff: super::starting_launch::ClosedHandoff,
    pub starting_consent: super::consent::Record,
    pub starting_read: super::starting_read::Receipt,
    pub starting_evidence: super::starting::ReportedSnapshot,
    pub host_environment: super::host_environment::Reported,
    pub additional: E,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalizerFailureKind {
    SetupIncomplete,
    Pilot(super::session_finalizer::StartFault),
}

pub struct FinalizerFailure<E> {
    pub kind: FinalizerFailureKind,
    pub owners: RetirementOwners,
    pub inputs: PilotOwnerInputs<E>,
}

#[allow(clippy::large_enum_variant)] // Keep all original Windows/process owners inline on failure.
pub enum AdmissionFinalizerFailure<'a, 's, 'w, E> {
    StartingLaunchBinding {
        admission: ParentAdmission<'a, 's, 'w>,
        sampler: Process,
        worker: Process,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
    },
    TerminalIncomplete {
        admission: ParentAdmission<'a, 's, 'w>,
        sampler: Process,
        worker: Process,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
    },
    MissingController {
        owners: Owners,
        sampler: Process,
        worker: Process,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
    },
    Finalizer(FinalizerFailure<E>),
}

impl RetirementOwners {
    /// Reassemble the exact starting/pilot evidence and run the existing
    /// three-role finalizer validation. Every failure returns both retirement
    /// and starting owners for continued cleanup or explicit fault handling.
    #[allow(clippy::result_large_err)] // Keep the complete session owner graph on failure.
    pub fn start_pilot_finalizer<E>(
        self,
        inputs: PilotOwnerInputs<E>,
        budget: &Budget,
    ) -> Result<
        super::session_finalizer::SessionFinalizer<super::session_finalizer::PilotOwners<E>>,
        FinalizerFailure<E>,
    > {
        let Self {
            assets,
            stage,
            worker_report,
            setup_receiver,
            setup,
        } = self;
        let setup = match (setup_receiver, setup) {
            (None, Some(setup)) => setup,
            (setup_receiver, setup) => {
                return Err(FinalizerFailure {
                    kind: FinalizerFailureKind::SetupIncomplete,
                    owners: Self {
                        assets,
                        stage,
                        worker_report,
                        setup_receiver,
                        setup,
                    },
                    inputs,
                });
            }
        };

        let PilotOwnerInputs {
            manifest,
            scope,
            starting_process,
            starting_launch,
            starting_handoff,
            starting_consent,
            starting_read,
            starting_evidence,
            host_environment,
            additional,
        } = inputs;
        let evidence = super::session_finalizer::PilotOwners {
            manifest,
            scope,
            starting_process,
            starting_launch,
            starting_handoff,
            starting_consent,
            starting_read,
            starting_evidence,
            host_environment,
            worker_setup_delivery: setup,
            worker_report_evidence: worker_report,
            additional,
        };
        match super::session_finalizer::SessionFinalizer::<
            super::session_finalizer::PilotOwners<E>,
        >::start_pilot(assets, evidence, budget)
        {
            Ok(finalizer) => Ok(finalizer),
            Err(failure) => {
                let super::session_finalizer::PilotOwners {
                    manifest,
                    scope,
                    starting_process,
                    starting_launch,
                    starting_handoff,
                    starting_consent,
                    starting_read,
                    starting_evidence,
                    host_environment,
                    worker_setup_delivery,
                    worker_report_evidence,
                    additional,
                } = failure.evidence;
                Err(FinalizerFailure {
                    kind: FinalizerFailureKind::Pilot(failure.fault),
                    owners: Self {
                        assets: failure.assets,
                        stage,
                        worker_report: worker_report_evidence,
                        setup_receiver: None,
                        setup: Some(worker_setup_delivery),
                    },
                    inputs: PilotOwnerInputs {
                        manifest,
                        scope,
                        starting_process,
                        starting_launch,
                        starting_handoff,
                        starting_consent,
                        starting_read,
                        starting_evidence,
                        host_environment,
                        additional,
                    },
                })
            }
        }
    }
}

pub struct MissingController {
    pub owners: Owners,
    pub sampler: Process,
    pub worker: Process,
    pub clock: WindowsClock,
}

impl Owners {
    /// Move the exact sampler/worker processes into retirement Assets after
    /// their channel borrows have been consumed. If the controller was never
    /// created, return every original process and evidence owner unchanged.
    #[allow(clippy::result_large_err)] // Preserve the full owner graph without allocating.
    pub fn into_retirement(
        self,
        sampler: Process,
        worker: Process,
        clock: WindowsClock,
    ) -> Result<RetirementOwners, MissingController> {
        let (stage, parent, worker_report, setup_receiver, setup, controller) = match self {
            Self {
                stage,
                parent,
                worker_report,
                setup_receiver,
                setup,
                controller: Some(controller),
            } => (
                stage,
                parent,
                worker_report,
                setup_receiver,
                setup,
                controller,
            ),
            owners => {
                return Err(MissingController {
                    owners,
                    sampler,
                    worker,
                    clock,
                })
            }
        };
        Ok(RetirementOwners {
            assets: super::retirement_collector::Assets::new(
                parent, controller, sampler, worker, clock,
            ),
            stage,
            worker_report,
            setup_receiver,
            setup,
        })
    }
}

pub struct PreflightInput<'a, 'r> {
    pub run: &'a BoundRun<'r>,
    pub scope: &'a Scope,
    pub environment: &'a HostEnvironment,
    pub starting: &'a ReportedSnapshot,
    pub starting_response: &'a StartingReceipt,
    pub starting_decision: &'a ConsentRecord,
    pub decisions: consent::Decisions,
    pub now: Anchor,
    pub starting_session: &'a StartingSession,
}

#[must_use = "retain the admission owner and controller through session shutdown"]
pub struct ParentAdmission<'a, 's, 'w> {
    sampler: ParentSamplerChannel<'s>,
    worker: ParentWorkerChannel<'w>,
    starting: &'a super::starting_launch::AdmissionProcess,
    starting_launch: super::starting_launch::LaunchFacts,
    setup_receiver: Option<SetupReceiver>,
    setup: Option<ReadyDelivery>,
    controller: Option<ControllerHandle>,
    stage: Stage,
}

impl<'a, 's, 'w> ParentAdmission<'a, 's, 'w> {
    #[allow(clippy::result_large_err)] // Return the unconsumed setup reader without allocating.
    pub fn new(
        sampler: ParentSamplerChannel<'s>,
        worker: ParentWorkerChannel<'w>,
        starting: &'a super::starting_launch::AdmissionProcess,
        setup_receiver: Option<SetupReceiver>,
    ) -> Result<Self, InitFailure> {
        let starting_launch = starting.facts();
        let fault = if sampler.parent().fault().is_some() {
            Some(InitFault::SamplerAlreadyFaulted)
        } else if worker.assessment_fault().is_some() || worker.overflowed() {
            Some(InitFault::WorkerAlreadyFaulted)
        } else if setup_receiver.is_none() {
            Some(InitFault::MissingSetupReceiver)
        } else if !starting_launch.admission_eligible() {
            Some(InitFault::StartingLaunchIneligible)
        } else {
            None
        };
        if let Some(fault) = fault {
            return Err(InitFailure {
                fault,
                setup: setup_receiver,
                starting_launch,
            });
        }
        Ok(Self {
            sampler,
            worker,
            starting,
            starting_launch,
            setup_receiver,
            setup: None,
            controller: None,
            stage: Stage::SetupPending,
        })
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn setup(&self) -> Option<&ReadyDelivery> {
        self.setup.as_ref()
    }

    pub fn controller(&self) -> Option<&ControllerHandle> {
        self.controller.as_ref()
    }

    pub fn preflight_ready(&self) -> bool {
        self.stage == Stage::SetupReady
            && self.setup.is_some()
            && self.sampler.parent().ready().is_some()
            && self.sampler.parent().runtime().is_some()
            && self.sampler.parent().current().is_some()
    }

    /// Poll sampler telemetry and worker setup/control once each. Once Start
    /// has established the result audit, ordinary worker packets go to that
    /// audit instead of being offered to the completed setup reader again.
    pub fn poll_once(&mut self, run: &BoundRun<'_>, clock: &WindowsClock, budget: &Budget) -> Turn {
        let sampler = self.sampler.poll(run, || clock.capture().ok());

        let setup = if self.setup_receiver.is_some() {
            let result = self.worker.poll_setup(&mut self.setup_receiver, clock);
            match result {
                Ok(Some(delivery)) => {
                    self.setup = Some(delivery);
                    if self.stage == Stage::SetupPending {
                        self.stage = Stage::SetupReady;
                    }
                    Some(Ok(true))
                }
                Ok(None) => Some(Ok(false)),
                Err(fault) => {
                    self.stage = Stage::Failed;
                    Some(Err(fault))
                }
            }
        } else {
            None
        };

        let worker_control = self.worker.poll_control(clock);
        let worker_result = match (self.setup.as_mut(), self.worker.result_progress()) {
            (Some(setup), Some(_)) => Some(self.worker.poll_result(setup, clock, budget)),
            _ => None,
        };

        if sampler.transport_fault
            || sampler.rejected.is_some()
            || self.sampler.parent().fault().is_some()
            || matches!(
                &worker_control,
                parent_worker_channel::ControlPoll::Fault { .. }
                    | parent_worker_channel::ControlPoll::Overflow(_)
            )
            || self.worker.assessment_fault().is_some()
            || self.worker.overflowed()
            || worker_result.as_ref().is_some_and(|result| {
                matches!(
                    result,
                    parent_worker_channel::ResultPoll::Fault { .. }
                        | parent_worker_channel::ResultPoll::Overflow(_)
                        | parent_worker_channel::ResultPoll::Unavailable(_)
                )
            })
        {
            self.stage = Stage::Failed;
        }

        Turn {
            sampler,
            setup,
            worker_control,
            worker_result,
        }
    }

    /// Poll once, then perform at most one ordered admission transition. The
    /// caller may prepare preflight evidence in advance, but it is returned
    /// untouched until all required peer/setup reports have arrived.
    pub fn advance_admission_once<'r, 'p>(
        &mut self,
        run: &BoundRun<'_>,
        clock: &WindowsClock,
        budget: &Budget,
        preflight: Option<PreflightInput<'r, 'p>>,
    ) -> AdmissionTurn<'r, 'p> {
        let observed = self.poll_once(run, clock, budget);
        let mut unused_preflight = preflight;
        let progress = match self.stage {
            Stage::SetupPending => AdmissionProgress::SetupPending,
            Stage::SetupReady if !self.preflight_ready() => AdmissionProgress::SetupPending,
            Stage::SetupReady => match unused_preflight.take() {
                None => AdmissionProgress::AwaitingPreflight,
                Some(input) => match self.verify_reported_preflight(input) {
                    Ok(()) => AdmissionProgress::PreflightAccepted,
                    Err(failure) => AdmissionProgress::PreflightRejected(failure),
                },
            },
            Stage::PreflightAccepted => match self.start_and_attach_watch(clock, budget) {
                Ok(()) => AdmissionProgress::ControllerAttached,
                Err(fault) => AdmissionProgress::ControllerStartFailed(fault),
            },
            Stage::ControllerAttached => match self.queue_start_when_watch_ready(clock) {
                Ok(parent_worker_channel::StartProgress::WaitingForWatch) => {
                    AdmissionProgress::WaitingForWatch
                }
                Ok(parent_worker_channel::StartProgress::Queued) => AdmissionProgress::StartQueued,
                Err(fault) => AdmissionProgress::StartFailed(fault),
            },
            Stage::StartQueued => match self.collect_and_queue_native_grant(clock) {
                Ok(()) => AdmissionProgress::NativeGrantQueued,
                Err(fault) => AdmissionProgress::NativeGrantFailed(fault),
            },
            Stage::NativeGrantQueued => AdmissionProgress::NativeGrantQueued,
            Stage::Failed => AdmissionProgress::Failed,
        };
        AdmissionTurn {
            observed,
            progress,
            preflight: unused_preflight,
        }
    }

    /// Drive the fixed pilot's ordered parent loop one round at a time. Before
    /// NativeGrant this delegates to the admission transition driver; once the
    /// grant is queued it continues the worker-result/StopAck/RoleFinished
    /// handshake. Exactly one poll round occurs per call. Any unused preflight
    /// evidence is returned intact during shutdown.
    pub fn advance_pilot_once<'r, 'p>(
        &mut self,
        run: &BoundRun<'_>,
        clock: &WindowsClock,
        budget: &Budget,
        preflight: Option<PreflightInput<'r, 'p>>,
    ) -> PilotTurn<'r, 'p> {
        if self.stage == Stage::NativeGrantQueued {
            let observed = self.poll_once(run, clock, budget);
            let progress = self.advance_successful_shutdown_after_poll(clock, budget);
            PilotTurn {
                observed,
                progress: PilotProgress::Shutdown(progress),
                preflight,
            }
        } else {
            let admission = self.advance_admission_once(run, clock, budget, preflight);
            PilotTurn {
                observed: admission.observed,
                progress: PilotProgress::Admission(admission.progress),
                preflight: admission.preflight,
            }
        }
    }

    /// Poll once and drive the successful terminal handshake. Only complete
    /// worker result assessment can trigger the planned stop; the subsequent
    /// control audit is bound to that same original ParentSampler stop window.
    pub fn advance_successful_shutdown_once(
        &mut self,
        run: &BoundRun<'_>,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> TerminalTurn {
        let observed = self.poll_once(run, clock, budget);
        let progress = self.advance_successful_shutdown_after_poll(clock, budget);
        TerminalTurn { observed, progress }
    }

    fn advance_successful_shutdown_after_poll(
        &mut self,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> TerminalProgress {
        let progress = if self.stage == Stage::Failed {
            TerminalProgress::Failed
        } else if self.stage != Stage::NativeGrantQueued {
            TerminalProgress::AdmissionIncomplete
        } else if self.sampler.parent().is_finishing() {
            if self.worker.control_audit().is_none() {
                match self.begin_worker_control_audit(budget) {
                    Ok(()) => TerminalProgress::PlannedStopQueued,
                    Err(fault) => {
                        self.stage = Stage::Failed;
                        TerminalProgress::ControlAuditFailed(fault)
                    }
                }
            } else if self.terminal_evidence_ready() {
                TerminalProgress::TerminalEvidenceReady
            } else {
                TerminalProgress::AwaitingRoleCompletion
            }
        } else if !self.worker.reports_complete() {
            TerminalProgress::AwaitingWorkerResults
        } else {
            match self.sampler.finish(clock.capture().ok()) {
                Err(fault) => {
                    self.stage = Stage::Failed;
                    TerminalProgress::StopFailed(fault)
                }
                Ok(()) => match self.begin_worker_control_audit(budget) {
                    Ok(()) => TerminalProgress::PlannedStopQueued,
                    Err(fault) => {
                        self.stage = Stage::Failed;
                        TerminalProgress::ControlAuditFailed(fault)
                    }
                },
            }
        };
        progress
    }

    fn terminal_evidence_ready(&self) -> bool {
        self.stage == Stage::NativeGrantQueued
            && self.worker.assessment_fault().is_none()
            && !self.worker.overflowed()
            && self.sampler.parent().fault().is_none()
            && self.sampler.parent().reported_finished()
            && self.worker.reports_complete()
            && self.worker.control_audit().is_some_and(|audit| {
                audit.stop_acknowledged()
                    && audit.role_finished()
                    && !audit.native_failure()
                    && audit.fault().is_none()
            })
    }

    #[allow(clippy::result_large_err)] // Return unconsumed consent and setup owners intact.
    pub fn verify_reported_preflight(
        &mut self,
        input: PreflightInput<'_, '_>,
    ) -> Result<(), PreflightFailure> {
        let PreflightInput {
            run,
            scope,
            environment,
            starting,
            starting_response,
            starting_decision,
            decisions,
            now,
            starting_session,
        } = input;
        if !self.preflight_ready() {
            return Err(PreflightFailure {
                fault: parent_sampler::PreflightFault::Peers(super::parent_peers::Fault::Missing),
                decisions: Some(decisions),
            });
        }
        let processes = super::parent_peers::Processes {
            starting: self.starting.process(),
            sampler: self.sampler.process(),
            worker: self.worker.process(),
            starting_session,
            setup: self.setup.as_mut().expect("setup checked"),
        };
        let result = self.sampler.verify_reported_preflight(ReportedPreflight {
            run,
            scope,
            environment,
            starting,
            starting_response,
            starting_decision,
            decisions,
            now,
            processes,
        });
        self.stage = if result.is_ok() {
            Stage::PreflightAccepted
        } else {
            Stage::Failed
        };
        result
    }

    #[allow(clippy::result_large_err)] // Preserve partial controller and process-stop owners.
    pub fn start_and_attach_watch(
        &mut self,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> Result<(), parent_sampler_channel::ControllerStartFault> {
        if self.stage != Stage::PreflightAccepted {
            self.stage = Stage::Failed;
            return Err(parent_sampler_channel::ControllerStartFault::Parent(
                parent_sampler::Fault::Context,
            ));
        }
        match self
            .sampler
            .start_and_attach_watch(self.worker.process(), clock, budget)
        {
            Ok(handle) => {
                self.controller = Some(handle);
                self.stage = Stage::ControllerAttached;
                Ok(())
            }
            Err(fault) => {
                self.stage = Stage::Failed;
                Err(fault)
            }
        }
    }

    pub fn queue_start_when_watch_ready(
        &mut self,
        clock: &WindowsClock,
    ) -> Result<parent_worker_channel::StartProgress, super::parent_start::Fault> {
        if self.stage != Stage::ControllerAttached {
            self.stage = Stage::Failed;
            return Err(super::parent_start::Fault::Context);
        }
        let setup = self
            .setup
            .as_mut()
            .ok_or(super::parent_start::Fault::Context)?;
        match self.sampler.queue_worker_start_when_watch_ready(
            &mut self.worker,
            self.starting.process(),
            setup,
            clock,
        ) {
            Ok(parent_worker_channel::StartProgress::WaitingForWatch) => {
                Ok(parent_worker_channel::StartProgress::WaitingForWatch)
            }
            Ok(parent_worker_channel::StartProgress::Queued) => {
                self.stage = Stage::StartQueued;
                Ok(parent_worker_channel::StartProgress::Queued)
            }
            Err(fault) => {
                self.stage = Stage::Failed;
                Err(fault)
            }
        }
    }

    pub fn collect_and_queue_native_grant(
        &mut self,
        clock: &WindowsClock,
    ) -> Result<(), parent_sampler::Fault> {
        if self.stage != Stage::StartQueued {
            self.stage = Stage::Failed;
            return Err(parent_sampler::Fault::Context);
        }
        let setup = self.setup.as_mut().ok_or(parent_sampler::Fault::Context)?;
        match self.sampler.collect_and_queue_native_grant(
            &mut self.worker,
            self.starting.process(),
            setup,
            clock,
        ) {
            Ok(()) => {
                self.stage = Stage::NativeGrantQueued;
                Ok(())
            }
            Err(fault) => {
                self.stage = Stage::Failed;
                Err(fault)
            }
        }
    }

    /// Shutdown request remains available after a failed admission stage so
    /// callers can still perform the retained fail-closed stop path.
    pub fn finish_sampler(&mut self, anchor: Option<Anchor>) -> Result<(), parent_sampler::Fault> {
        self.sampler.finish(anchor)
    }

    /// Begin the worker terminal-control audit using the ParentSampler's
    /// retained original StopWindow. Call after the planned sampler stop has
    /// been recorded; subsequent control polls update this same audit.
    pub fn begin_worker_control_audit(
        &mut self,
        budget: &Budget,
    ) -> Result<(), parent_worker_channel::ControlFault> {
        self.worker
            .begin_control_audit(self.sampler.parent(), budget)
    }

    /// Consume both channels, transfer their retained protocol/evidence owners
    /// and release the process borrows so the same handles can move into
    /// retirement/finalizer assets.
    pub fn into_parts(self) -> Owners {
        let Self {
            sampler,
            worker,
            setup_receiver,
            setup,
            controller,
            stage,
            starting: _,
            starting_launch: _,
        } = self;
        Owners {
            stage,
            parent: sampler.into_parent(),
            worker_report: worker.into_evidence(),
            setup_receiver,
            setup,
            controller,
        }
    }

    /// Transfer a terminally audited admission owner into the pilot finalizer.
    /// Before the exact parent/worker completion gate is met, every owner is
    /// returned unchanged so polling and shutdown evidence collection can continue.
    #[allow(clippy::result_large_err)] // Preserve every session owner inline on failure.
    pub fn into_pilot_finalizer<E>(
        self,
        sampler: Process,
        worker: Process,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
        budget: &Budget,
    ) -> Result<
        super::session_finalizer::SessionFinalizer<super::session_finalizer::PilotOwners<E>>,
        AdmissionFinalizerFailure<'a, 's, 'w, E>,
    > {
        if self.starting_launch != inputs.starting_launch
            || !std::ptr::eq(self.starting.process(), &inputs.starting_process)
        {
            return Err(AdmissionFinalizerFailure::StartingLaunchBinding {
                admission: self,
                sampler,
                worker,
                clock,
                inputs,
            });
        }
        if !self.terminal_evidence_ready() {
            return Err(AdmissionFinalizerFailure::TerminalIncomplete {
                admission: self,
                sampler,
                worker,
                clock,
                inputs,
            });
        }
        let owners = self.into_parts();
        let retirement = match owners.into_retirement(sampler, worker, clock) {
            Ok(retirement) => retirement,
            Err(missing) => {
                return Err(AdmissionFinalizerFailure::MissingController {
                    owners: missing.owners,
                    sampler: missing.sampler,
                    worker: missing.worker,
                    clock: missing.clock,
                    inputs,
                });
            }
        };
        retirement
            .start_pilot_finalizer(inputs, budget)
            .map_err(AdmissionFinalizerFailure::Finalizer)
    }
}
