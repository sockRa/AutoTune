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
    pub fn into_admission_process(mut self) -> Result<(Process, Facts), Self> {
        if !self.facts.admission_eligible()
            || self.process.is_none()
            || !self.launch.as_ref().is_none_or(Launch::thread_finished)
        {
            return Err(self);
        }
        let process = self.process.take().expect("checked process owner");
        self.launch.take();
        Ok((process, self.facts))
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
