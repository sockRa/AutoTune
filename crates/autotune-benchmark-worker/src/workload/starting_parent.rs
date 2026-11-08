//! Fixed parent-side starting process and response composition.
//!
//! The caller supplies the already-scoped declaration, captured consent and
//! wait/pump callback. This function retains every launch/handoff owner on
//! failure and returns only a strictly read starting receipt on completion.
use super::{
    consent::Record,
    host_environment::Reported as HostEnvironment,
    manifest::BoundRun,
    parent_admission::{self, PreflightInput, PreflightInputFault},
    starting::Scope,
    starting_bootstrap::Declaration,
    starting_launch::{self, AdmissionProcess, ClosedHandoff, LaunchFailure, LaunchPoll},
    starting_read::{self, Receipt},
    starting_session::Step,
};
use crate::monitor::Anchor;
use crate::{monitor::windows_clock::WindowsClock, sampler::budget::Budget};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpControl {
    Continue,
    Cancel,
}

#[allow(clippy::large_enum_variant)] // Preserve launch/process/handoff owners inline on every failure path.
pub enum Failure {
    LaunchStart(starting_launch::LaunchStartFault),
    LaunchCancelled(starting_launch::LaunchGuard),
    LaunchPending(starting_launch::LaunchGuard),
    LaunchFailed(LaunchFailure),
    LaunchIneligible(starting_launch::LaunchGuard),
    HandoffOpen {
        process: AdmissionProcess,
        fault: starting_launch::Fault,
        stop_request: starting_launch::RequestAttempt,
        termination_request: Option<starting_launch::RequestAttempt>,
    },
    BootstrapSend {
        process: AdmissionProcess,
        handoff: ClosedHandoff,
        fault: super::transport::SendFailure,
        stop_request: starting_launch::RequestAttempt,
        termination_request: Option<starting_launch::RequestAttempt>,
    },
    Cancelled {
        process: AdmissionProcess,
        handoff: ClosedHandoff,
        step: Step,
        stop_request: starting_launch::RequestAttempt,
        termination_request: Option<starting_launch::RequestAttempt>,
    },
    Receive {
        process: AdmissionProcess,
        handoff: ClosedHandoff,
        step: Step,
        stop_request: starting_launch::RequestAttempt,
        termination_request: Option<starting_launch::RequestAttempt>,
    },
    Read {
        process: AdmissionProcess,
        handoff: ClosedHandoff,
        fault: starting_read::Fault,
        stop_request: starting_launch::RequestAttempt,
        termination_request: Option<starting_launch::RequestAttempt>,
    },
}

/// The process, completed response session and strict receipt remain one
/// owner so preflight cannot accidentally mix evidence from different starts.
pub struct CompletedStarting {
    process: AdmissionProcess,
    handoff: ClosedHandoff,
    receipt: Receipt,
}

impl CompletedStarting {
    pub fn process(&self) -> &AdmissionProcess {
        &self.process
    }

    pub fn handoff(&self) -> &ClosedHandoff {
        &self.handoff
    }

    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }

    /// Build parent preflight from this exact process's verified response and
    /// completed delivery session, preserving all remaining caller inputs.
    #[allow(clippy::too_many_arguments)] // Keep run/consent/context references explicit at preflight binding.
    pub fn preflight_input<'a, 'r>(
        &'a self,
        run: &'a BoundRun<'r>,
        scope: &'a Scope,
        environment: &'a HostEnvironment,
        starting_decision: &'a Record,
        decisions: super::consent::Decisions,
        now: Anchor,
    ) -> Result<PreflightInput<'a, 'r>, PreflightInputFault> {
        parent_admission::PreflightInput::from_starting_response(
            run,
            scope,
            environment,
            &self.receipt,
            starting_decision,
            decisions,
            now,
            &self.handoff.session,
        )
    }

    pub fn into_parts(self) -> (AdmissionProcess, ClosedHandoff, Receipt) {
        (self.process, self.handoff, self.receipt)
    }
}

/// Launch the fixed starting child, send its one-shot bootstrap, and poll the
/// bounded response to completion. `pump` runs between pending polls so the
/// host can dispatch UI/events and apply its polling cadence; returning
/// `Cancel` requests cooperative stop while returning every live owner. All
/// deadline checks still use the launch/session's original epoch on the next
/// poll.
#[allow(clippy::result_large_err)] // The failure value intentionally returns the complete live owner graph.
pub fn launch_and_receive(
    declaration: Declaration,
    scope: &Scope,
    consent: &mut Record,
    clock: WindowsClock,
    budget: &Budget,
    mut pump: impl FnMut() -> PumpControl,
) -> Result<CompletedStarting, Failure> {
    let mut guard =
        starting_launch::LaunchGuard::start(declaration, scope, consent, clock.clone(), budget)
            .map_err(Failure::LaunchStart)?;

    loop {
        match guard.poll() {
            LaunchPoll::Pending => {
                if pump() == PumpControl::Cancel {
                    guard.request_cancel();
                    return Err(Failure::LaunchCancelled(guard));
                }
            }
            LaunchPoll::ProcessReady => break,
            LaunchPoll::Failed => {
                return match guard.into_failure() {
                    Ok(failure) => Err(Failure::LaunchFailed(failure)),
                    Err(guard) => Err(Failure::LaunchPending(guard)),
                };
            }
        }
    }

    let mut process = guard
        .into_admission_process()
        .map_err(Failure::LaunchIneligible)?;
    let mut handoff = match process.open_handoff(scope, budget) {
        Ok(handoff) => handoff,
        Err(fault) => {
            let (stop_request, termination_request) = request_stop(&mut process);
            return Err(Failure::HandoffOpen {
                process,
                fault,
                stop_request,
                termination_request,
            });
        }
    };
    if let Err(fault) = handoff.send() {
        let handoff = handoff.close();
        let (stop_request, termination_request) = request_stop(&mut process);
        return Err(Failure::BootstrapSend {
            process,
            handoff,
            fault,
            stop_request,
            termination_request,
        });
    }

    loop {
        match handoff.poll(process.process(), || clock.capture().ok()) {
            Step::Idle | Step::Accepted { .. } => {
                if pump() == PumpControl::Cancel {
                    let step = handoff.session_mut().stop();
                    let handoff = handoff.close();
                    let (stop_request, termination_request) = request_stop(&mut process);
                    return Err(Failure::Cancelled {
                        process,
                        handoff,
                        step,
                        stop_request,
                        termination_request,
                    });
                }
            }
            Step::Complete { .. } => break,
            step => {
                let handoff = handoff.close();
                let (stop_request, termination_request) = request_stop(&mut process);
                return Err(Failure::Receive {
                    process,
                    handoff,
                    step,
                    stop_request,
                    termination_request,
                });
            }
        }
    }

    let handoff = handoff.close();
    let receipt = match handoff
        .session
        .read_report(scope, process.process().declaration(), budget)
    {
        Ok(receipt) => receipt,
        Err(fault) => {
            let (stop_request, termination_request) = request_stop(&mut process);
            return Err(Failure::Read {
                process,
                handoff,
                fault,
                stop_request,
                termination_request,
            });
        }
    };
    Ok(CompletedStarting {
        process,
        handoff,
        receipt,
    })
}

fn request_stop(
    process: &mut AdmissionProcess,
) -> (
    starting_launch::RequestAttempt,
    Option<starting_launch::RequestAttempt>,
) {
    let stop_request = request_attempt(process.request_stop());
    let termination_request = if stop_request.succeeded {
        None
    } else {
        Some(request_attempt(process.request_termination()))
    };
    (stop_request, termination_request)
}

fn request_attempt(result: io::Result<()>) -> starting_launch::RequestAttempt {
    match result {
        Ok(()) => starting_launch::RequestAttempt {
            succeeded: true,
            os_code: None,
            error_kind: None,
        },
        Err(error) => starting_launch::RequestAttempt {
            succeeded: false,
            os_code: error.raw_os_error(),
            error_kind: Some(error.kind()),
        },
    }
}
