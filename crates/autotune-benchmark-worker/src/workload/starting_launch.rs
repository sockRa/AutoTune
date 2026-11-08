//! Parent-side handoff for one already-launched starting process: one-shot
//! bootstrap frame, then the bounded session driver. It creates no process and
//! issues no native admission; the caller keeps the process and its consent.
use super::{
    launch::CancelQueue,
    starting::Scope,
    starting_bootstrap::{self, Frame},
    starting_receive::Fault as SessionFault,
    starting_session::{Session, Step},
    transport::SendFailure,
    windows_process::{Process, ProcessQueue},
};
use crate::{
    monitor::{
        clock::Clock,
        windows_clock::{CaptureError, WindowsClock},
        Anchor,
    },
    sampler::budget::Budget,
};
use std::io;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Session(SessionFault),
    Frame(starting_bootstrap::Error),
    Sender(SendFailure),
}
/// One bootstrap attempt and one bounded reception over a caller-owned starting
/// process. A failed send keeps the exact unsent frame and the first cause; no
/// successor frame is attempted.
pub struct Handoff<'a> {
    sender:
        starting_bootstrap::Sender<CancelQueue<ProcessQueue<'a, starting_bootstrap::Declaration>>>,
    session: Session,
    frame: Option<Frame>,
    send_fault: Option<SendFailure>,
}
/// Owned terminal parent-side starting evidence after its process borrow ends.
pub struct ClosedHandoff {
    pub session: Session,
    /// Exact bootstrap frame remains owned if its one-shot send failed.
    pub unsent_frame: Option<Frame>,
    pub send_fault: Option<SendFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestAttempt {
    pub succeeded: bool,
    pub os_code: Option<i32>,
    pub error_kind: Option<io::ErrorKind>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaunchFacts {
    pub polls: u64,
    pub elapsed_ns: Option<u64>,
    pub deadline_exceeded: bool,
    pub clock_capture_error: Option<CaptureError>,
    pub clock_domain_fault: bool,
    pub stop_request: Option<RequestAttempt>,
    pub termination_request: Option<RequestAttempt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchPoll {
    Pending,
    ProcessReady,
    Failed,
}

#[derive(Debug)]
pub enum LaunchStartFault {
    Scope,
    Clock(CaptureError),
    ClockDomain,
    Deadline,
    Capacity,
    Launch(io::Error),
}

pub struct LaunchFailure {
    /// Retains the launcher/job owner while the original failure is reviewed.
    pub launch: super::windows_process::Launch<starting_bootstrap::Declaration>,
    pub error: io::Error,
    pub facts: LaunchFacts,
}

/// Parent-side starting launch guard. It enforces the approved preflight and
/// response-arrival bounds from the scope's original epoch while process
/// hashing/startup runs on the launch thread. It never reruns a launch.
#[must_use = "poll the starting launch until its process or retained failure is transferred"]
pub struct LaunchGuard {
    launch: Option<super::windows_process::Launch<starting_bootstrap::Declaration>>,
    process: Option<Process<starting_bootstrap::Declaration>>,
    failure: Option<io::Error>,
    windows_clock: WindowsClock,
    epoch_clock: Clock,
    preflight_ns: u64,
    arrival_ns: u64,
    facts: LaunchFacts,
    _metadata: crate::sampler::budget::Reservation,
}

impl LaunchGuard {
    /// Bind one exact pre-manifest declaration and previously captured console
    /// decision. `declaration` must be the same declaration used to construct
    /// `consent`; the launch API enforces that exact target before startup.
    pub fn start(
        declaration: starting_bootstrap::Declaration,
        scope: &super::starting::Scope,
        consent: &mut super::consent::Record,
        windows_clock: WindowsClock,
        budget: &Budget,
    ) -> Result<Self, LaunchStartFault> {
        if declaration.scope_digest() != scope.digest() {
            return Err(LaunchStartFault::Scope);
        }
        let draft = scope.draft();
        let anchor = windows_clock.capture().map_err(LaunchStartFault::Clock)?;
        if anchor.qpc_before < declaration.entry().qpc_after {
            return Err(LaunchStartFault::ClockDomain);
        }
        let mut epoch_clock = Clock::temporal(draft.frequency, draft.clock_token, draft.epoch)
            .map_err(|_| LaunchStartFault::ClockDomain)?;
        let elapsed = epoch_clock
            .observe(anchor)
            .map_err(|_| LaunchStartFault::ClockDomain)?;
        let preflight_ns = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .preflight_ns;
        if elapsed >= preflight_ns {
            return Err(LaunchStartFault::Deadline);
        }
        let metadata = budget
            .reserve(std::mem::size_of::<Self>() + 4096)
            .map_err(|_| LaunchStartFault::Capacity)?;
        let launch = super::windows_process::Launch::start(
            declaration,
            budget.clone(),
            consent,
            &windows_clock,
        )
        .map_err(LaunchStartFault::Launch)?;
        Ok(Self {
            launch: Some(launch),
            process: None,
            failure: None,
            windows_clock,
            epoch_clock,
            preflight_ns,
            arrival_ns: super::starting_session::arrival_bound_ns(),
            facts: LaunchFacts {
                polls: 0,
                elapsed_ns: Some(elapsed),
                deadline_exceeded: false,
                clock_capture_error: None,
                clock_domain_fault: false,
                stop_request: None,
                termination_request: None,
            },
            _metadata: metadata,
        })
    }

    pub fn facts(&self) -> LaunchFacts {
        self.facts
    }

    pub fn failure(&self) -> Option<&io::Error> {
        self.failure.as_ref()
    }

    /// One caller-driven clock/deadline step plus one nonblocking observation
    /// of the asynchronous process launch.
    pub fn poll(&mut self) -> LaunchPoll {
        self.facts.polls = self.facts.polls.saturating_add(1);
        if self.process.is_some() {
            return LaunchPoll::ProcessReady;
        }
        if self.failure.is_some() {
            return LaunchPoll::Failed;
        }

        match self.windows_clock.capture() {
            Ok(anchor) => match self.epoch_clock.observe(anchor) {
                Ok(elapsed) => {
                    self.facts.elapsed_ns = Some(elapsed);
                    if elapsed >= self.preflight_ns {
                        self.facts.deadline_exceeded = true;
                        self.request_stop_once();
                    }
                    if elapsed >= self.arrival_ns {
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

        let launch = self.launch.as_mut().expect("pending launch owner");
        match launch.poll() {
            None => LaunchPoll::Pending,
            Some(Ok(process)) => {
                self.process = Some(process);
                LaunchPoll::ProcessReady
            }
            Some(Err(error)) => {
                self.failure = Some(error);
                LaunchPoll::Failed
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
            .expect("pending launch owner")
            .request_stop();
        self.facts.stop_request = Some(match result {
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
        });
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
            .expect("pending launch owner")
            .request_termination();
        self.facts.termination_request = Some(match result {
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
        });
    }

    /// Transfer the original process and launch timing facts after observing
    /// the process. Late/failed clock and stop facts remain in `LaunchFacts`;
    /// the caller must not treat them as successful admission.
    #[allow(clippy::result_large_err)] // Return the original process/launch owner graph without an unbudgeted box.
    pub fn into_process(
        mut self,
    ) -> Result<(Process<starting_bootstrap::Declaration>, LaunchFacts), Self> {
        match self.process.take() {
            Some(process) => {
                self.launch.take();
                Ok((process, self.facts))
            }
            None => Err(self),
        }
    }

    /// Transfer a completed asynchronous launch error without dropping its
    /// job/event owner or the first OS error payload.
    #[allow(clippy::result_large_err)] // Preserve the launch/job owner inline until the caller handles the error.
    pub fn into_failure(mut self) -> Result<LaunchFailure, Self> {
        match self.failure.take() {
            Some(error) => match self.launch.take() {
                Some(launch) => Ok(LaunchFailure {
                    launch,
                    error,
                    facts: self.facts,
                }),
                None => unreachable!("failed launch owner is retained"),
            },
            None => Err(self),
        }
    }
}
impl<'a> Handoff<'a> {
    /// Bind the session and freeze the one-shot frame before any bytes move.
    pub fn open(
        process: &'a Process<starting_bootstrap::Declaration>,
        scope: &Scope,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let session = Session::new(process.declaration(), scope, budget).map_err(Fault::Session)?;
        let frame = Frame::encode(process.declaration(), scope, budget).map_err(Fault::Frame)?;
        let sender = process.starting_sender(budget).map_err(Fault::Sender)?;
        Ok(Self {
            sender,
            session,
            frame: Some(frame),
            send_fault: None,
        })
    }
    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }
    pub fn send_fault(&self) -> Option<SendFailure> {
        self.send_fault
    }
    /// Send the single bootstrap frame. A second attempt is rejected and the
    /// exact frame of a failed attempt remains owned for the caller's evidence.
    pub fn send(&mut self) -> Result<(), SendFailure> {
        let Some(frame) = self.frame.take() else {
            let fault = SendFailure::Closed;
            self.send_fault.get_or_insert(fault);
            return Err(fault);
        };
        match self.sender.send(frame) {
            Ok(()) => Ok(()),
            Err(frame) => {
                let fault = self.sender.fault().unwrap_or(SendFailure::Closed);
                self.frame = Some(frame);
                self.send_fault = Some(fault);
                Err(fault)
            }
        }
    }
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }
    /// Close the process-borrowing sender and retain the complete reception
    /// state plus any unsent frame/failure for later session finalization.
    pub fn close(self) -> ClosedHandoff {
        let Self {
            sender,
            session,
            frame,
            send_fault,
        } = self;
        drop(sender);
        ClosedHandoff {
            session,
            unsent_frame: frame,
            send_fault,
        }
    }
    /// Drive one nonblocking poll of the caller-owned process into the session.
    pub fn poll(
        &mut self,
        process: &Process<starting_bootstrap::Declaration>,
        clock: impl FnMut() -> Option<Anchor>,
    ) -> Step {
        self.session.poll_process(process, clock)
    }
}
