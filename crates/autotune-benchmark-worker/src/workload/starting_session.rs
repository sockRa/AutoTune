//! Parent-side composition of one starting reception under the original epoch.
//! The session drives the bounded receiver to one retained outcome; it creates
//! no process, issues no native admission and proves neither qualification nor
//! peer exit. The launched process stays owned by its caller.
use super::{
    starting::Scope,
    starting_bootstrap::Declaration,
    starting_delivery::Status,
    starting_receive::{Fault, Receiver},
};
use crate::{
    evidence::Digest,
    monitor::{clock::Clock, Anchor},
    sampler::budget::Budget,
};
/// Absolute original-epoch bound for the single starting response. The read is
/// due at the preflight boundary and the response follows the child's stop/
/// drain/control completion, so the conservative arrival bound adds the
/// cooperative-stop and exit-observation allowances. It never extends an
/// approved phase cap or grants session time.
pub fn arrival_bound_ns() -> u64 {
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    p.preflight_ns
        .saturating_add(p.cooperative_stop_ns)
        .saturating_add(p.exit_observation_ns)
        .min(p.pilot_session_ns)
}
/// One terminal or ongoing session state. A verified completion is retained and
/// cannot be replaced by a later transport or supervision observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Complete {
        received: u32,
        bytes: usize,
        digest: Digest,
    },
    /// A transport/OS receive failure closed reception before completion.
    Transport {
        os_code: Option<i32>,
    },
    Faulted(Fault),
}
/// Supervision facts observed by one driver poll, kept separate from the
/// receiver's own sticky chunk/clock/deadline fault.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Supervision {
    pub transport_fault: bool,
    pub os_code: Option<i32>,
    /// The peer closed its input side or lane/framing was rejected.
    pub closed: bool,
    /// One chunk was accepted by the receiver during this poll.
    pub accepted: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    Accepted { received: u32 },
    Complete { received: u32 },
    Transport { os_code: Option<i32> },
    Fault(Fault),
}
/// One bounded reception owning the original-epoch clock domain. The deadline
/// and every accepted chunk use elapsed nanoseconds from the parent's original
/// epoch, never a restarted or per-poll base.
pub struct Session {
    clock: Clock,
    receiver: Receiver,
    polls: u64,
    transport: Option<Option<i32>>,
    outcome: Option<Outcome>,
}
impl Session {
    /// Freeze the declaration/scope binding, the arrival bound and the reserved
    /// receiver capacity before any chunk is accepted.
    pub fn new(declaration: &Declaration, scope: &Scope, budget: &Budget) -> Result<Self, Fault> {
        if declaration.scope_digest() != scope.digest() {
            return Err(Fault::Assembly(super::starting_delivery::Fault::Scope));
        }
        let d = scope.draft();
        let clock =
            Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| Fault::Clock)?;
        let receiver = Receiver::new(declaration, scope, 0, arrival_bound_ns(), budget)?;
        Ok(Self {
            clock,
            receiver,
            polls: 0,
            transport: None,
            outcome: None,
        })
    }
    pub fn epoch_ns(&self) -> u64 {
        self.receiver.epoch_ns()
    }
    pub fn deadline_ns(&self) -> u64 {
        self.receiver.deadline_ns()
    }
    pub fn received(&self) -> u32 {
        self.receiver.received()
    }
    pub fn polls(&self) -> u64 {
        self.polls
    }
    /// Convert one parent clock observation into this session's original-epoch
    /// elapsed nanoseconds. A missing, regressed or mismatched observation
    /// yields `None` and never a restarted base; the driver must retain that as
    /// a clock fault instead of treating it as on time.
    pub fn observe(&mut self, anchor: Anchor) -> Option<u64> {
        self.clock.observe(anchor).ok()
    }
    /// First transport/OS cause observed while reception was still open.
    pub fn transport_fault(&self) -> Option<Option<i32>> {
        self.transport
    }
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }
    pub fn complete(&self) -> bool {
        matches!(self.outcome, Some(Outcome::Complete { .. }))
    }
    /// Only a fully verified payload bound to the parent's declaration/scope is
    /// exposed, and never after a transport supervision failure.
    pub fn canonical(&self) -> Option<&[u8]> {
        if self.complete() {
            self.receiver.canonical()
        } else {
            None
        }
    }
    /// Strictly read the one verified delivered response. Reception completion
    /// binds bytes only; reading still issues no native or qualification permit.
    pub fn read_report(
        &self,
        scope: &Scope,
        declaration: &Declaration,
        budget: &Budget,
    ) -> Result<super::starting_read::Receipt, super::starting_read::Fault> {
        let Some(Outcome::Complete { digest, .. }) = self.outcome else {
            return Err(super::starting_read::Fault::Incomplete);
        };
        let bytes = self
            .canonical()
            .ok_or(super::starting_read::Fault::Incomplete)?;
        super::starting_read::Receipt::read(
            bytes,
            scope,
            declaration,
            digest,
            autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .preflight_ns,
            budget,
        )
    }
    /// Explicit stop closes reception for every successor chunk. A stop after a
    /// verified completion is redundant and cannot withdraw that payload.
    pub fn stop(&mut self) -> Step {
        self.receiver.stop();
        self.settle(Supervision::default())
    }
    /// One bounded chunk with the parent's elapsed receive clock.
    pub fn accept(&mut self, now_ns: u64, bytes: &[u8]) -> Step {
        let accepted = self.receiver.accept(now_ns, bytes).is_ok();
        self.settle(Supervision {
            accepted,
            ..Supervision::default()
        })
    }
    /// Advance the receive clock without a chunk; idle polls still enforce the
    /// original arrival bound.
    pub fn tick(&mut self, now_ns: u64) -> Step {
        let _ = self.receiver.tick(now_ns);
        self.settle(Supervision::default())
    }
    /// A missing parent clock observation is retained as a clock fault and is
    /// never treated as on time.
    pub fn clock_lost(&mut self) -> Step {
        self.receiver.clock_lost();
        self.settle(Supervision::default())
    }
    /// Merge one poll's supervision facts into the sticky session outcome.
    /// Completion is checked first so a later fault cannot withdraw verified
    /// bytes; the first transport cause is retained for the caller.
    pub fn settle(&mut self, observed: Supervision) -> Step {
        self.polls += 1;
        if observed.transport_fault {
            self.transport.get_or_insert(observed.os_code);
        }
        if self.outcome.is_none() {
            if self.receiver.complete() {
                if let Status::Complete { bytes, digest } = self.receiver.status() {
                    self.outcome = Some(Outcome::Complete {
                        received: self.receiver.received(),
                        bytes,
                        digest,
                    });
                }
            } else if observed.transport_fault {
                self.outcome = Some(Outcome::Transport {
                    os_code: observed.os_code,
                });
            } else if observed.closed {
                self.receiver.stop();
            }
            if self.outcome.is_none() {
                if let Some(fault) = self.receiver.fault() {
                    self.outcome = Some(Outcome::Faulted(fault));
                }
            }
        }
        match self.outcome {
            Some(Outcome::Complete { received, .. }) => Step::Complete { received },
            Some(Outcome::Transport { os_code }) => Step::Transport { os_code },
            Some(Outcome::Faulted(fault)) => Step::Fault(fault),
            None if observed.accepted => Step::Accepted {
                received: self.receiver.received(),
            },
            None => Step::Idle,
        }
    }
}

#[cfg(windows)]
impl Session {
    /// Drive one nonblocking poll of the caller-owned launched starting process
    /// into the bounded receiver. The anchor is converted to original-epoch
    /// elapsed time before any deadline check, and a rejected packet remains
    /// caller-owned. No native admission or qualification follows.
    pub fn poll_process(
        &mut self,
        process: &super::windows_process::Process<Declaration>,
        mut clock: impl FnMut() -> Option<Anchor>,
    ) -> Step {
        let before = self.receiver.received();
        let Self {
            clock: session_clock,
            receiver,
            ..
        } = self;
        let poll = super::starting_receive::poll_native(
            receiver,
            process.inherited(),
            &mut clock,
            |anchor| session_clock.observe(anchor).ok(),
        );
        let accepted = receiver.received() > before;
        self.settle(Supervision {
            transport_fault: poll.transport_fault,
            os_code: poll.os_code,
            closed: poll.finished || poll.rejected.is_some(),
            accepted,
        })
    }
}
