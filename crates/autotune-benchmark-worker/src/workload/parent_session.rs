//! Fixed parent session owner for the retained pilot process pair.
//!
//! This composes process-bound admission construction with one bounded
//! setup/preflight/Start/NativeGrant/terminal poll per call. Process owners stay
//! borrowed from `AdmissionPair`; after this owner is consumed, callers can
//! transfer the returned terminal/fault owners together with that same pair to
//! retirement and archive completion.
use super::{
    manifest::BoundRun,
    parent_admission::{
        Owners, ParentAdmission, PilotTurn, PreflightInput, ProcessAdmissionFailure, Stage,
    },
    pilot_launch::AdmissionPair,
    starting_parent::CompletedStarting,
    worker_setup_delivery::Receiver,
};
use crate::{
    monitor::{windows_clock::WindowsClock, Anchor},
    sampler::budget::Budget,
};

#[must_use = "advance the fixed parent session or transfer its terminal/fault owners"]
pub struct FixedParentSession<'a, 'p, 'r, 'm> {
    admission: ParentAdmission<'a, 'p, 'p>,
    run: &'r BoundRun<'m>,
}

impl<'a, 'p, 'r, 'm> FixedParentSession<'a, 'p, 'r, 'm> {
    /// Construct the parent channels and supervised admission from the exact
    /// retained pilot pair and completed starting response owner.
    #[allow(clippy::result_large_err)] // Construction failure returns every process/channel/controller owner.
    #[allow(clippy::too_many_arguments)] // Keep run, epoch and process-owner bindings explicit.
    pub fn start(
        pair: &'p mut AdmissionPair,
        run: &'r BoundRun<'m>,
        setup_entry: Anchor,
        first: Anchor,
        starting: &'a CompletedStarting,
        setup_receiver: Option<Receiver>,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> Result<Self, ProcessAdmissionFailure<'a, 'p, 'p>> {
        let admission = pair.admit(
            run,
            setup_entry,
            first,
            starting.process(),
            setup_receiver,
            clock,
            budget,
        )?;
        Ok(Self { admission, run })
    }

    pub fn stage(&self) -> Stage {
        self.admission.stage()
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

    /// Release channel borrows only after the success terminal predicate.
    /// The returned Owners can be paired with the original AdmissionPair for
    /// finalizer/archive transfer.
    #[allow(clippy::result_large_err)] // Return the live session unchanged until the terminal gate passes.
    pub fn into_terminal_owners(self) -> Result<Owners, Self> {
        let run = self.run;
        match self.admission.into_terminal_owners() {
            Ok(owners) => Ok(owners),
            Err(admission) => Err(Self { admission, run }),
        }
    }

    /// Release channel borrows for failure finalization only after a parent
    /// fault has been retained.
    #[allow(clippy::result_large_err)] // Return the live session unchanged until failure is evidenced.
    pub fn into_faulted_owners(self) -> Result<Owners, Self> {
        let run = self.run;
        match self.admission.into_faulted_owners() {
            Ok(owners) => Ok(owners),
            Err(admission) => Err(Self { admission, run }),
        }
    }
}
