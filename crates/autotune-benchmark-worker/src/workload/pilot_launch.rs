//! Deadline-bound parent launch for the fixed sampler and worker processes.
//!
//! This keeps process creation on its existing background owner while the
//! caller polls the same run clock. A late or clock-uncertain launch remains a
//! cleanup owner and cannot enter ordinary admission.
use super::{
    launch::Declaration,
    manifest::BoundRun,
    starting_launch::RequestAttempt,
    windows_process::{Launch, Process},
    wire::WorkerBinding,
};
use crate::{
    monitor::{
        clock::Clock,
        windows_clock::{CaptureError, WindowsClock},
        Anchor,
    },
    sampler::budget::{Budget, Reservation},
    workload::consent::Record,
};
use autotune_fma_reference::profile::Phase;
use std::io;

#[derive(Debug)]
pub enum StartFault {
    Binding,
    Clock(CaptureError),
    ClockDomain,
    Deadline,
    Capacity,
    Launch(io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    ProcessObserved,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub polls: u64,
    pub elapsed_ns: Option<u64>,
    pub deadline_exceeded: bool,
    pub clock_capture_error: Option<CaptureError>,
    pub clock_domain_fault: bool,
    pub stop_request: Option<RequestAttempt>,
    pub termination_request: Option<RequestAttempt>,
}

impl Facts {
    /// Eligibility to transfer a launched process into normal parent admission.
    pub fn admission_eligible(self) -> bool {
        self.polls > 0
            && self.elapsed_ns.is_some()
            && !self.deadline_exceeded
            && self.clock_capture_error.is_none()
            && !self.clock_domain_fault
            && self.stop_request.is_none()
            && self.termination_request.is_none()
    }
}

/// Process handle and its exact successful launch observations. Only the
/// clock-clean guard can construct this owner.
#[must_use = "retain the launch facts with their process through finalization"]
pub struct AdmissionProcess {
    process: Process,
    facts: Facts,
}

impl AdmissionProcess {
    pub fn process(&self) -> &Process {
        &self.process
    }

    pub fn process_mut(&mut self) -> &mut Process {
        &mut self.process
    }

    pub fn facts(&self) -> Facts {
        self.facts
    }

    pub fn into_parts(self) -> (Process, Facts) {
        (self.process, self.facts)
    }
}

/// Failure while creating the fixed sampler/worker launch pair. If worker
/// launch setup fails, the already-started sampler guard remains owned here
/// for explicit stop/termination handling.
#[allow(clippy::large_enum_variant)] // Keep the already-started sampler owner inline on worker-start failure.
pub enum PairStartFailure {
    RoleBinding,
    Capacity,
    Sampler(StartFault),
    Worker {
        sampler: Guard,
        fault: StartFault,
        _metadata: Reservation,
    },
}

/// The two fixed pilot processes launched against one manifest run and one
/// original clock observation. Both guards remain available until both
/// launches are eligible for admission or the caller takes the failure path.
#[must_use = "poll both guarded launches and retain them through cleanup or admission"]
pub struct Pair {
    sampler: Guard,
    worker: Guard,
    _metadata: Reservation,
}

/// Successfully launched sampler/worker owners retained together so their
/// exact handles and facts can open parent admission as one handoff.
pub struct AdmissionPair {
    sampler: AdmissionProcess,
    worker: AdmissionProcess,
}

impl AdmissionPair {
    pub fn sampler_facts(&self) -> Facts {
        self.sampler.facts()
    }

    pub fn worker_facts(&self) -> Facts {
        self.worker.facts()
    }

    /// Compose the exact guarded process pair into the supervised parent
    /// admission coordinator while retaining both wrappers in the caller.
    #[allow(clippy::result_large_err)] // Keep constructed channel/controller owners inline on failure.
    #[allow(clippy::too_many_arguments)] // Preserve the exact run, epoch and starting owner inputs.
    pub fn admit<'a, 'p>(
        &'p mut self,
        run: &BoundRun<'_>,
        setup_entry: Anchor,
        first: Anchor,
        starting: &'a super::starting_launch::AdmissionProcess,
        setup_receiver: Option<super::worker_setup_delivery::Receiver>,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> Result<
        super::parent_admission::ParentAdmission<'a, 'p, 'p>,
        super::parent_admission::ProcessAdmissionFailure<'a, 'p, 'p>,
    > {
        super::parent_admission::ParentAdmission::from_processes(
            &mut self.sampler,
            &mut self.worker,
            run,
            setup_entry,
            first,
            starting,
            setup_receiver,
            clock,
            budget,
        )
    }

    /// Release the exact process/facts pairs after parent admission has been
    /// consumed and its process borrows have ended.
    pub fn into_parts(self) -> ((Process, Facts), (Process, Facts)) {
        (self.sampler.into_parts(), self.worker.into_parts())
    }
}

impl Pair {
    #[allow(clippy::result_large_err)] // Preserve an already-started sampler if worker startup fails.
    #[allow(clippy::too_many_arguments)] // Keep the exact shared run/clock/consent inputs visible at launch.
    pub fn start(
        run: &BoundRun<'_>,
        sampler_declaration: Declaration,
        worker_declaration: Declaration,
        first_entry: Anchor,
        sampler_consent: &mut Record,
        worker_consent: &mut Record,
        clock: WindowsClock,
        budget: &Budget,
    ) -> Result<Self, PairStartFailure> {
        if sampler_declaration.role() != super::launch::Role::Sampler
            || worker_declaration.role() != super::launch::Role::Worker
        {
            return Err(PairStartFailure::RoleBinding);
        }
        let metadata = budget
            .reserve(std::mem::size_of::<Self>() + 512)
            .map_err(|_| PairStartFailure::Capacity)?;
        let sampler = Guard::start(
            run,
            sampler_declaration,
            first_entry,
            sampler_consent,
            clock.clone(),
            budget,
        )
        .map_err(PairStartFailure::Sampler)?;
        let worker = match Guard::start(
            run,
            worker_declaration,
            first_entry,
            worker_consent,
            clock,
            budget,
        ) {
            Ok(worker) => worker,
            Err(fault) => {
                return Err(PairStartFailure::Worker {
                    sampler,
                    fault,
                    _metadata: metadata,
                });
            }
        };
        Ok(Self {
            sampler,
            worker,
            _metadata: metadata,
        })
    }

    /// Poll each original launch guard exactly once; callers must preserve both
    /// guards even when one side fails so cleanup can account for both owners.
    /// A failed or clock-ineligible side requests stop on its companion at
    /// once; request failures trigger that guard's existing termination path.
    pub fn poll_once(&mut self) -> (Poll, Poll) {
        let sampler = self.sampler.poll();
        let worker = self.worker.poll();
        let sampler_failed = sampler == Poll::Failed || !self.sampler.facts().admission_eligible();
        let worker_failed = worker == Poll::Failed || !self.worker.facts().admission_eligible();
        if sampler_failed {
            self.worker.request_stop_once();
        }
        if worker_failed {
            self.sampler.request_stop_once();
        }
        (sampler, worker)
    }

    pub fn sampler_facts(&self) -> Facts {
        self.sampler.facts()
    }

    pub fn worker_facts(&self) -> Facts {
        self.worker.facts()
    }

    /// Transfer both processes together only after each launch thread has
    /// finished and both original-clock records remain admission-eligible.
    #[allow(clippy::result_large_err)] // Return both guards together while either launch is pending/faulted.
    pub fn into_admission_processes(self) -> Result<(AdmissionProcess, AdmissionProcess), Self> {
        if !self.sampler.can_transfer_to_admission() || !self.worker.can_transfer_to_admission() {
            return Err(self);
        }
        let Self {
            sampler,
            worker,
            _metadata: _,
        } = self;
        let sampler = match sampler.into_admission_process() {
            Ok(process) => process,
            Err(_) => unreachable!("sampler transfer precondition was checked"),
        };
        let worker = match worker.into_admission_process() {
            Ok(process) => process,
            Err(_) => unreachable!("worker transfer precondition was checked"),
        };
        Ok((sampler, worker))
    }

    #[allow(clippy::result_large_err)] // Return both launch guards inline if either is not transferable.
    pub fn into_admission_pair(self) -> Result<AdmissionPair, Self> {
        self.into_admission_processes()
            .map(|(sampler, worker)| AdmissionPair { sampler, worker })
    }
}

/// One bounded observation of an asynchronous pilot process launch.
#[must_use = "retain and poll the launch guard until its process or failure is observed"]
pub struct Guard {
    launch: Option<Launch>,
    process: Option<Process>,
    failure: Option<io::Error>,
    clock: WindowsClock,
    timeline: Clock,
    setup_deadline_ns: u64,
    exit_deadline_ns: u64,
    cooperative_stop_ns: u64,
    stop_observed_ns: Option<u64>,
    termination_deadline_ns: Option<u64>,
    facts: Facts,
    _metadata: Reservation,
}

impl Guard {
    /// Start exactly one manifest-bound worker or sampler launch. `first_entry`
    /// must be the original run-clock observation shared by the pilot launch
    /// pair; the profile's setup/exit phase boundaries are frozen from it.
    pub fn start(
        run: &BoundRun<'_>,
        declaration: Declaration,
        first_entry: Anchor,
        consent: &mut Record,
        clock: WindowsClock,
        budget: &Budget,
    ) -> Result<Self, StartFault> {
        let binding = declaration.binding();
        if WorkerBinding::new(run, clock.qpc_frequency(), binding.epoch())
            .map_err(|_| StartFault::Binding)?
            != binding
            || clock.clock_token() != binding.epoch().clock_token
            || clock.qpc_frequency() != binding.monitor().qpc_frequency
        {
            return Err(StartFault::Binding);
        }
        let mut timeline =
            Clock::new(binding.monitor(), binding.epoch()).map_err(|_| StartFault::ClockDomain)?;
        let entry_ns = timeline
            .observe(first_entry)
            .map_err(|_| StartFault::ClockDomain)?;
        let setup_deadline_ns = run
            .phase_deadline(Phase::Setup, entry_ns)
            .map_err(|_| StartFault::Deadline)?;
        let exit_deadline_ns = run
            .phase_deadline(Phase::Exit, entry_ns)
            .map_err(|_| StartFault::Deadline)?;
        let cooperative_stop_ns = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .cooperative_stop_ns;
        let initial = clock.capture().map_err(StartFault::Clock)?;
        if timeline
            .observe(initial)
            .map_err(|_| StartFault::ClockDomain)?
            >= setup_deadline_ns
        {
            return Err(StartFault::Deadline);
        }
        let metadata = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| StartFault::Capacity)?;
        let launch = Launch::start(declaration, budget.clone(), consent, &clock)
            .map_err(StartFault::Launch)?;
        Ok(Self {
            launch: Some(launch),
            process: None,
            failure: None,
            clock,
            timeline,
            setup_deadline_ns,
            exit_deadline_ns,
            cooperative_stop_ns,
            stop_observed_ns: None,
            termination_deadline_ns: None,
            facts: Facts::default(),
            _metadata: metadata,
        })
    }

    pub fn facts(&self) -> Facts {
        self.facts
    }

    pub fn failure(&self) -> Option<&io::Error> {
        self.failure.as_ref()
    }

    /// One clock/deadline step and one nonblocking result observation. A ready
    /// process is not returned as stable until its launcher thread has finished.
    pub fn poll(&mut self) -> Poll {
        self.facts.polls = self.facts.polls.saturating_add(1);
        if self.failure.is_none() && self.process.is_none() {
            self.observe_clock();
            if let Some(launch) = self.launch.as_mut() {
                if let Some(result) = launch.poll() {
                    match result {
                        Ok(process) => self.process = Some(process),
                        Err(error) => self.failure = Some(error),
                    }
                }
            }
        } else {
            self.observe_clock();
        }

        let launcher_finished = self.launch.as_ref().is_none_or(Launch::thread_finished);
        if !launcher_finished {
            return Poll::Pending;
        }
        if self.process.is_some() {
            Poll::ProcessObserved
        } else if self.failure.is_some() {
            Poll::Failed
        } else {
            Poll::Pending
        }
    }

    fn observe_clock(&mut self) {
        match self.clock.capture() {
            Ok(anchor) => match self.timeline.observe(anchor) {
                Ok(elapsed) => {
                    self.facts.elapsed_ns = Some(elapsed);
                    if elapsed >= self.setup_deadline_ns && self.stop_observed_ns.is_none() {
                        self.facts.deadline_exceeded = true;
                        self.stop_observed_ns = Some(elapsed);
                        self.termination_deadline_ns = Some(
                            elapsed
                                .checked_add(self.cooperative_stop_ns)
                                .unwrap_or(elapsed)
                                .min(self.exit_deadline_ns),
                        );
                        self.request_stop_once();
                    }
                    if self
                        .termination_deadline_ns
                        .is_some_and(|deadline| elapsed >= deadline)
                        || elapsed >= self.exit_deadline_ns
                    {
                        self.facts.deadline_exceeded = true;
                        self.request_termination_once();
                    }
                }
                Err(_) => {
                    self.facts.clock_domain_fault = true;
                    self.request_termination_once();
                }
            },
            Err(error) => {
                self.facts.clock_capture_error.get_or_insert(error);
                self.request_termination_once();
            }
        }
    }

    fn request_stop_once(&mut self) {
        if self.facts.stop_request.is_some() {
            return;
        }
        let result = self
            .launch
            .as_ref()
            .expect("launch owner retained until process transfer")
            .request_stop();
        self.facts.stop_request = Some(request_attempt(result));
        if self
            .facts
            .stop_request
            .is_some_and(|attempt| !attempt.succeeded)
        {
            self.request_termination_once();
        }
    }

    fn request_termination_once(&mut self) {
        if self.facts.termination_request.is_some() {
            return;
        }
        let result = self
            .launch
            .as_mut()
            .expect("launch owner retained until process transfer")
            .request_termination();
        self.facts.termination_request = Some(request_attempt(result));
    }

    /// Transfer a process only when the original setup window and clock remain
    /// valid. A failed check returns the complete guard for explicit cleanup.
    #[allow(clippy::result_large_err)] // Keep the live process and launcher owner inline.
    pub fn into_admission_process(mut self) -> Result<AdmissionProcess, Self> {
        if !self.can_transfer_to_admission() {
            return Err(self);
        }
        let process = self.process.take().expect("checked process owner");
        self.launch.take();
        Ok(AdmissionProcess {
            process,
            facts: self.facts,
        })
    }

    fn can_transfer_to_admission(&self) -> bool {
        self.facts.admission_eligible()
            && self.process.is_some()
            && self.launch.as_ref().is_none_or(Launch::thread_finished)
    }

    /// Take an observed process even after a timing fault, but retain its facts
    /// so the caller can route it only through failure cleanup.
    #[allow(clippy::result_large_err)] // Preserve the process/launcher owner on pending startup.
    pub fn into_observed_process(mut self) -> Result<(Process, Facts), Self> {
        if !self.launch.as_ref().is_none_or(Launch::thread_finished) {
            return Err(self);
        }
        let Some(process) = self.process.take() else {
            return Err(self);
        };
        self.launch.take();
        Ok((process, self.facts))
    }
}

fn request_attempt(result: io::Result<()>) -> RequestAttempt {
    match result {
        Ok(()) => RequestAttempt {
            succeeded: true,
            os_code: None,
            error_kind: None,
        },
        Err(error) => RequestAttempt {
            succeeded: false,
            os_code: error.raw_os_error(),
            error_kind: Some(error.kind()),
        },
    }
}
