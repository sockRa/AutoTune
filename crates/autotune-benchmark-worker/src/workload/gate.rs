//! Child handshake intent/state only. Parent safety/consent/process supervision
//! remain mandatory; this gate calls no backend and confers no native authority.
use super::wire::{Message, WorkerBinding};
use crate::monitor::{clock::Clock, Anchor};
use autotune_fma_reference::profile::{Phase, RunPlan};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Protocol,
    Clock,
    Deadline,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Prepare,
    Start {
        plan: RunPlan,
        issued_ns: u64,
        receipt_ns: u64,
        warmup_deadline_ns: u64,
    },
    Stop,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Requested,
    Bootstrapped,
    Ready,
    Started,
    Stopped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopWindow {
    pub trigger_ns: u64,
    pub cooperative_ns: u64,
    pub exit_ns: u64,
}
/// Single-use observation from the validated handshake, not native authority.
pub struct StartObservation {
    pub(super) binding: WorkerBinding,
    pub(super) issued: Anchor,
    pub(super) receipt: Anchor,
}
pub struct Gate {
    binding: WorkerBinding,
    clock: Clock,
    state: State,
    ready_anchor: Option<Anchor>,
    setup_deadline_ns: Option<u64>,
    fault: Option<Fault>,
    stop_window: Option<StopWindow>,
    start: Option<StartObservation>,
}
impl Gate {
    pub fn new(binding: WorkerBinding) -> Result<Self, Fault> {
        Ok(Self {
            binding,
            clock: Clock::new(binding.monitor, binding.epoch).map_err(|_| Fault::Clock)?,
            state: State::Requested,
            ready_anchor: None,
            setup_deadline_ns: None,
            fault: None,
            stop_window: None,
            start: None,
        })
    }
    pub fn stopped(&self) -> bool {
        self.state == State::Stopped
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn stop_window(&self) -> Option<StopWindow> {
        self.stop_window
    }
    pub fn take_start(&mut self) -> Option<StartObservation> {
        if self.state != State::Started {
            return None;
        }
        self.start.take()
    }
    fn fail<T>(&mut self, f: Fault) -> Result<T, Fault> {
        self.state = State::Stopped;
        self.fault.get_or_insert(f);
        Err(f)
    }
    pub fn ready(&mut self, message: &Message) -> Result<(), Fault> {
        if self.stopped() {
            return Err(Fault::Closed);
        }
        if self.state != State::Bootstrapped {
            return self.fail(Fault::Protocol);
        }
        let Message::Ready { anchor, .. } = message else {
            return self.fail(Fault::Protocol);
        };
        if message.validate(self.binding).is_err() {
            return self.fail(Fault::Protocol);
        }
        let ns = match self.clock.observe(*anchor) {
            Ok(n) => n,
            Err(_) => return self.fail(Fault::Clock),
        };
        if self.setup_deadline_ns.is_none_or(|deadline| ns >= deadline) {
            return self.fail(Fault::Deadline);
        }
        self.ready_anchor = Some(*anchor);
        self.state = State::Ready;
        Ok(())
    }
    pub fn accept(&mut self, message: &Message, receipt: Option<Anchor>) -> Result<Action, Fault> {
        if self.stopped() {
            return if matches!(message, Message::Stop { .. }) {
                Ok(Action::Stop)
            } else {
                Err(Fault::Closed)
            };
        }
        if message.validate(self.binding).is_err() {
            return self.fail(Fault::Protocol);
        }
        let Some(receipt) = receipt else {
            return self.fail(Fault::Clock);
        };
        let mut issued_ns = None;
        if let Message::Start { issued } = message {
            if self.state != State::Ready {
                return self.fail(Fault::Protocol);
            }
            if self
                .ready_anchor
                .is_none_or(|a| a.qpc_after > issued.qpc_before)
                || issued.qpc_after > receipt.qpc_before
            {
                return self.fail(Fault::Clock);
            }
            issued_ns = Some(match self.clock.observe(*issued) {
                Ok(n) => n,
                Err(_) => return self.fail(Fault::Clock),
            });
        }
        let now = match self.clock.observe(receipt) {
            Ok(n) => n,
            Err(_) => return self.fail(Fault::Clock),
        };
        match message {
            Message::Stop {
                trigger_ns,
                cooperative_ns,
                exit_ns,
                ..
            } => {
                if *trigger_ns > now {
                    return self.fail(Fault::Clock);
                }
                self.stop_window = Some(StopWindow {
                    trigger_ns: *trigger_ns,
                    cooperative_ns: *cooperative_ns,
                    exit_ns: *exit_ns,
                });
                self.state = State::Stopped;
                Ok(Action::Stop)
            }
            Message::Bootstrap { .. } => {
                if self.state != State::Requested {
                    return self.fail(Fault::Protocol);
                }
                if now < self.binding.origin_ns || now >= self.binding.setup_boundary() {
                    return self.fail(Fault::Deadline);
                }
                self.setup_deadline_ns = Some(
                    match self.binding.plan.phase_deadline_ns(
                        Phase::Setup,
                        self.binding.origin_ns,
                        now,
                    ) {
                        Ok(deadline) => deadline,
                        Err(_) => return self.fail(Fault::Deadline),
                    },
                );
                self.state = State::Bootstrapped;
                Ok(Action::Prepare)
            }
            Message::Start { .. } => {
                if self
                    .setup_deadline_ns
                    .is_none_or(|deadline| now >= deadline)
                {
                    return self.fail(Fault::Deadline);
                }
                let issued_ns = issued_ns.expect("validated Start");
                let deadline = match self.binding.plan.phase_deadline_ns(
                    Phase::Warmup,
                    self.binding.origin_ns,
                    issued_ns,
                ) {
                    Ok(n) => n,
                    Err(_) => return self.fail(Fault::Deadline),
                };
                if now >= deadline {
                    return self.fail(Fault::Deadline);
                }
                self.state = State::Started;
                let Message::Start { issued } = message else {
                    unreachable!("Start arm")
                };
                self.start = Some(StartObservation {
                    binding: self.binding,
                    issued: *issued,
                    receipt,
                });
                Ok(Action::Start {
                    plan: self.binding.plan,
                    issued_ns,
                    receipt_ns: now,
                    warmup_deadline_ns: deadline,
                })
            }
            _ => self.fail(Fault::Protocol),
        }
    }
}
