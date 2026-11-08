//! Fixed parent-side starting process and response composition.
//!
//! The caller supplies the already-scoped declaration, captured consent and
//! wait/pump callback. This function retains every launch/handoff owner on
//! failure and returns only a strictly read starting receipt on completion.
use super::{
    consent::Record,
    starting::Scope,
    starting_bootstrap::Declaration,
    starting_launch::{self, AdmissionProcess, ClosedHandoff, LaunchFailure, LaunchPoll},
    starting_read::{self, Receipt},
    starting_session::Step,
};
use crate::{monitor::windows_clock::WindowsClock, sampler::budget::Budget};
use std::io;

#[allow(clippy::large_enum_variant)] // Preserve launch/process/handoff owners inline on every failure path.
pub enum Failure {
    LaunchStart(starting_launch::LaunchStartFault),
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

/// Launch the fixed starting child, send its one-shot bootstrap, and poll the
/// bounded response to completion. `pump` runs between pending polls so the
/// host can dispatch UI/events and apply its polling cadence; all deadline
/// checks still use the launch/session's original epoch on the next poll.
#[allow(clippy::result_large_err)] // The failure value intentionally returns the complete live owner graph.
pub fn launch_and_receive(
    declaration: Declaration,
    scope: &Scope,
    consent: &mut Record,
    clock: WindowsClock,
    budget: &Budget,
    mut pump: impl FnMut(),
) -> Result<(AdmissionProcess, ClosedHandoff, Receipt), Failure> {
    let mut guard =
        starting_launch::LaunchGuard::start(declaration, scope, consent, clock.clone(), budget)
            .map_err(Failure::LaunchStart)?;

    loop {
        match guard.poll() {
            LaunchPoll::Pending => pump(),
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
            Step::Idle | Step::Accepted { .. } => pump(),
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
    Ok((process, handoff, receipt))
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
