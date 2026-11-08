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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitFault {
    SamplerAlreadyFaulted,
    WorkerAlreadyFaulted,
    MissingSetupReceiver,
}

/// One bounded, nonblocking parent loop round. Each process lane is polled at
/// most once here; all packet owners remain in their channel or setup owner.
pub struct Turn {
    pub sampler: parent_sampler::Poll,
    pub setup: Option<Result<bool, worker_setup_delivery::Fault>>,
    pub worker_control: parent_worker_channel::ControlPoll,
    pub worker_result: Option<parent_worker_channel::ResultPoll>,
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
    starting: &'a Process<starting_bootstrap::Declaration>,
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
        starting: &'a Process<starting_bootstrap::Declaration>,
        setup_receiver: Option<SetupReceiver>,
    ) -> Result<Self, InitFailure> {
        let fault = if sampler.parent().fault().is_some() {
            Some(InitFault::SamplerAlreadyFaulted)
        } else if worker.assessment_fault().is_some() || worker.overflowed() {
            Some(InitFault::WorkerAlreadyFaulted)
        } else if setup_receiver.is_none() {
            Some(InitFault::MissingSetupReceiver)
        } else {
            None
        };
        if let Some(fault) = fault {
            return Err(InitFailure {
                fault,
                setup: setup_receiver,
            });
        }
        Ok(Self {
            sampler,
            worker,
            starting,
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
            starting: self.starting,
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
            self.starting,
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
            self.starting,
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
}
