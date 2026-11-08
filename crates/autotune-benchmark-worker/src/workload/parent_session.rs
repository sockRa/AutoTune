//! Fixed parent session owner for the retained pilot process pair.
//!
//! This composes process-bound admission construction with one bounded
//! setup/preflight/Start/NativeGrant/terminal poll per call. Process owners stay
//! borrowed from `AdmissionPair`; after this owner is consumed, callers can
//! transfer the returned terminal/fault owners together with that same pair to
//! retirement and archive completion.
use super::{
    consent::{Decisions, Record as ConsentRecord},
    host_environment::Reported as HostEnvironment,
    manifest::BoundRun,
    parent_admission::{
        OwnerPairCompletionStartFailure, Owners, ParentAdmission, PilotOwnerInputs, PilotTurn,
        PreflightInput, PreflightInputFailure, ProcessAdmissionFailure, Stage,
    },
    pilot_launch::AdmissionPair,
    starting::Scope,
    starting_parent::CompletedStarting,
    worker_setup_delivery::Receiver,
};
use crate::{
    monitor::{windows_clock::WindowsClock, Anchor},
    sampler::budget::Budget,
};
use std::{io, path::PathBuf};

#[must_use = "advance the fixed parent session or transfer its terminal/fault owners"]
pub struct FixedParentSession<'a, 'p, 'r, 'm> {
    admission: ParentAdmission<'a, 'p, 'p>,
    run: &'r BoundRun<'m>,
    scope: &'a Scope,
    starting: &'a CompletedStarting,
    writer_started: bool,
}

pub struct PreflightContext<'a> {
    pub environment: &'a HostEnvironment,
    pub starting_decision: &'a ConsentRecord,
    pub decisions: Decisions,
    pub now: Anchor,
}

pub enum PilotWriterStartFailure {
    ManifestBinding,
    AlreadyStarted,
    Start(io::Error),
}

#[allow(clippy::large_enum_variant)] // Keep the session, process pair and archive context recoverable.
pub enum PilotCompletionStartFailure<'session, 'pair, 'run, 'manifest, 'archive, E> {
    SessionNotTerminal {
        session: FixedParentSession<'session, 'pair, 'run, 'manifest>,
        pair: AdmissionPair,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
        writer: &'archive mut crate::sampler::writer::Writer,
        provenance: crate::evidence::Provenance,
    },
    Finalizer(OwnerPairCompletionStartFailure<'archive, E>),
}

#[allow(clippy::large_enum_variant)] // Keep the session, process pair and archive context recoverable.
pub enum PilotFailureCompletionStartFailure<'session, 'pair, 'run, 'manifest, 'archive, E> {
    SessionNotFaulted {
        session: FixedParentSession<'session, 'pair, 'run, 'manifest>,
        pair: AdmissionPair,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
        writer: &'archive mut crate::sampler::writer::Writer,
        provenance: crate::evidence::Provenance,
    },
    Finalizer(OwnerPairCompletionStartFailure<'archive, E>),
}

impl<'a, 'p, 'r, 'm> FixedParentSession<'a, 'p, 'r, 'm> {
    /// Construct the parent channels and supervised admission from the exact
    /// retained pilot pair and completed starting response owner.
    #[allow(clippy::result_large_err)] // Construction failure returns every process/channel/controller owner.
    #[allow(clippy::too_many_arguments)] // Keep run, epoch and process-owner bindings explicit.
    pub fn start(
        pair: &'p mut AdmissionPair,
        run: &'r BoundRun<'m>,
        scope: &'a Scope,
        setup_entry: Anchor,
        first: Anchor,
        starting: &'a CompletedStarting,
        setup_receiver: Option<Receiver>,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> Result<Self, ProcessAdmissionFailure<'a, 'p, 'p>> {
        if starting.receipt().scope_digest() != scope.digest()
            || starting.process().process().declaration().scope_digest() != scope.digest()
            || starting
                .receipt()
                .snapshot()
                .is_none_or(|snapshot| snapshot.snapshot().scope().digest() != scope.digest())
        {
            return Err(ProcessAdmissionFailure::Binding { setup_receiver });
        }
        let admission = pair.admit(
            run,
            setup_entry,
            first,
            starting.process(),
            setup_receiver,
            clock,
            budget,
        )?;
        Ok(Self {
            admission,
            run,
            scope,
            starting,
            writer_started: false,
        })
    }

    pub fn stage(&self) -> Stage {
        self.admission.stage()
    }

    /// Start the one pilot-bound writer for this exact run. Its sink opens on
    /// the dedicated writer thread using the manifest's preapproved empty
    /// output directory, not on the parent poll path.
    pub fn start_pilot_writer(
        &mut self,
        manifest: &super::manifest::PilotManifest,
        budget: &Budget,
    ) -> Result<crate::sampler::writer::Writer, PilotWriterStartFailure> {
        if self.run.manifest_digest() != manifest.digest() {
            return Err(PilotWriterStartFailure::ManifestBinding);
        }
        if self.writer_started {
            return Err(PilotWriterStartFailure::AlreadyStarted);
        }
        self.writer_started = true;
        let output_root = PathBuf::from(&manifest.manifest_draft().output_root);
        crate::sampler::writer::Writer::start_pilot_workload_factory(
            move || crate::sampler::writer::DirectorySink::open(&output_root),
            budget.clone(),
            manifest,
        )
        .map_err(PilotWriterStartFailure::Start)
    }

    /// Advance one bounded parent round while keeping the run binding fixed.
    pub fn advance_once<'preflight, 'context>(
        &mut self,
        clock: &WindowsClock,
        budget: &Budget,
        preflight: Option<PreflightInput<'preflight, 'context>>,
    ) -> PilotTurn<'preflight, 'context> {
        self.admission
            .advance_pilot_once(self.run, clock, budget, preflight)
    }

    /// Construct preflight from this session's exact starting receipt and
    /// response session, then advance one parent round. If construction fails,
    /// both consent decisions are returned by `PreflightInputFailure`.
    #[allow(clippy::result_large_err)] // Preserve consent decision owners on a source-binding fault.
    pub fn advance_with_context<'ctx>(
        &mut self,
        clock: &WindowsClock,
        budget: &Budget,
        context: Option<PreflightContext<'ctx>>,
    ) -> Result<PilotTurn<'ctx, 'm>, PreflightInputFailure>
    where
        'a: 'ctx,
        'r: 'ctx,
    {
        let run = self.run;
        let scope = self.scope;
        let starting = self.starting;
        let preflight = match context {
            Some(context) => Some(starting.preflight_input(
                run,
                scope,
                context.environment,
                context.starting_decision,
                context.decisions,
                context.now,
            )?),
            None => None,
        };
        Ok(self
            .admission
            .advance_pilot_once(run, clock, budget, preflight))
    }

    /// Release channel borrows only after the success terminal predicate.
    /// The returned Owners can be paired with the original AdmissionPair for
    /// finalizer/archive transfer.
    #[allow(clippy::result_large_err)] // Return the live session unchanged until the terminal gate passes.
    pub fn into_terminal_owners(self) -> Result<Owners, Self> {
        let run = self.run;
        let scope = self.scope;
        let starting = self.starting;
        let writer_started = self.writer_started;
        match self.admission.into_terminal_owners() {
            Ok(owners) => Ok(owners),
            Err(admission) => Err(Self {
                admission,
                run,
                scope,
                starting,
                writer_started,
            }),
        }
    }

    /// Release channel borrows for failure finalization only after a parent
    /// fault has been retained.
    #[allow(clippy::result_large_err)] // Return the live session unchanged until failure is evidenced.
    pub fn into_faulted_owners(self) -> Result<Owners, Self> {
        let run = self.run;
        let scope = self.scope;
        let starting = self.starting;
        let writer_started = self.writer_started;
        match self.admission.into_faulted_owners() {
            Ok(owners) => Ok(owners),
            Err(admission) => Err(Self {
                admission,
                run,
                scope,
                starting,
                writer_started,
            }),
        }
    }

    /// Transfer a clean terminal session directly to retirement, finalization
    /// and archive polling while retaining all caller inputs on a rejected gate.
    #[allow(clippy::result_large_err)] // Return the entire live owner graph on either rejection path.
    #[allow(clippy::too_many_arguments)] // Keep every process, clock and evidence binding explicit.
    pub fn start_completion<'archive, E>(
        self,
        pair: AdmissionPair,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
        writer: &'archive mut crate::sampler::writer::Writer,
        provenance: crate::evidence::Provenance,
        budget: &Budget,
    ) -> Result<
        crate::workload::session_finalizer::PilotCompletionDriver<'archive, E>,
        PilotCompletionStartFailure<'a, 'p, 'r, 'm, 'archive, E>,
    > {
        let owners = match self.into_terminal_owners() {
            Ok(owners) => owners,
            Err(session) => {
                return Err(PilotCompletionStartFailure::SessionNotTerminal {
                    session,
                    pair,
                    clock,
                    inputs,
                    writer,
                    provenance,
                });
            }
        };
        owners
            .start_pilot_completion(pair, clock, inputs, writer, provenance, budget)
            .map_err(PilotCompletionStartFailure::Finalizer)
    }

    /// Transfer a retained parent fault directly to failure finalization and
    /// archive polling while retaining all caller inputs on a rejected gate.
    #[allow(clippy::result_large_err)] // Return the entire live owner graph on either rejection path.
    #[allow(clippy::too_many_arguments)] // Keep every process, clock and evidence binding explicit.
    pub fn start_failure_completion<'archive, E>(
        self,
        pair: AdmissionPair,
        clock: WindowsClock,
        inputs: PilotOwnerInputs<E>,
        writer: &'archive mut crate::sampler::writer::Writer,
        provenance: crate::evidence::Provenance,
        budget: &Budget,
    ) -> Result<
        crate::workload::session_finalizer::PilotCompletionDriver<'archive, E>,
        PilotFailureCompletionStartFailure<'a, 'p, 'r, 'm, 'archive, E>,
    > {
        let owners = match self.into_faulted_owners() {
            Ok(owners) => owners,
            Err(session) => {
                return Err(PilotFailureCompletionStartFailure::SessionNotFaulted {
                    session,
                    pair,
                    clock,
                    inputs,
                    writer,
                    provenance,
                });
            }
        };
        owners
            .start_pilot_failure_completion(pair, clock, inputs, writer, provenance, budget)
            .map_err(PilotFailureCompletionStartFailure::Finalizer)
    }
}
