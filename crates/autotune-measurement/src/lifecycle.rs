//! Pure supplied synthetic lifecycle evidence, never execution or correctness authority.
use crate::{Identity, Provenance, TimeBasis};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Requested,
    Launched,
    Ready,
    Running,
    StopRequested,
    Terminated,
}

/// Positive absolute offsets from synthetic request time zero. No ordering is required.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Deadlines {
    requested_ns: u64,
    launched_ns: u64,
    ready_ns: u64,
    running_ns: u64,
    stop_requested_ns: u64,
}

impl Deadlines {
    pub fn new(
        requested_ns: u64,
        launched_ns: u64,
        ready_ns: u64,
        running_ns: u64,
        stop_requested_ns: u64,
    ) -> Result<Self, Error> {
        let deadlines = Self {
            requested_ns,
            launched_ns,
            ready_ns,
            running_ns,
            stop_requested_ns,
        };
        for phase in [
            Phase::Requested,
            Phase::Launched,
            Phase::Ready,
            Phase::Running,
            Phase::StopRequested,
        ] {
            if deadlines.for_phase(phase) == Some(0) {
                return Err(Error::ZeroDeadline { phase });
            }
        }
        Ok(deadlines)
    }

    pub fn for_phase(&self, phase: Phase) -> Option<u64> {
        match phase {
            Phase::Requested => Some(self.requested_ns),
            Phase::Launched => Some(self.launched_ns),
            Phase::Ready => Some(self.ready_ns),
            Phase::Running => Some(self.running_ns),
            Phase::StopRequested => Some(self.stop_requested_ns),
            Phase::Terminated => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Policy {
    event_capacity: usize,
    deadlines: Deadlines,
}

impl Policy {
    pub fn new(event_capacity: usize, deadlines: Deadlines) -> Result<Self, Error> {
        if event_capacity == 0 {
            return Err(Error::ZeroCapacity);
        }
        Ok(Self {
            event_capacity,
            deadlines,
        })
    }
}

/// Labels only: neither process authentication nor physical GPU mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Binding {
    identity: Identity,
    direct_worker: String,
}

impl Binding {
    pub fn new(identity: Identity, direct_worker: &str) -> Result<Self, Error> {
        if direct_worker.trim().is_empty()
            || direct_worker.len() > 256
            || direct_worker.chars().any(char::is_control)
        {
            return Err(Error::InvalidWorkerLabel);
        }
        Ok(Self {
            identity,
            direct_worker: direct_worker.into(),
        })
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn direct_worker(&self) -> &str {
        &self.direct_worker
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    Completed,
    Stopped,
    Crashed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    LaunchedObserved,
    ReadyObserved,
    RunningObserved,
    StopRequested,
    TerminatedObserved { reason: Termination },
    NeverLaunchedObserved,
    WorkerLost,
    ControllerLost,
    AdvanceTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    CompletedObserved,
    StopObserved,
    OutcomeUnknown,
    NeverLaunched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    Applied,
    NoOp,
    BoundExceeded { phase: Phase, deadline_ns: u64 },
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    ZeroCapacity,
    ZeroDeadline { phase: Phase },
    InvalidWorkerLabel,
    BindingMismatch,
    TimeRegression,
    CapacityReached,
    InvalidPhase { phase: Phase, event: Event },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Knowledge {
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Correctness {
    NotEvaluated,
}

/// Immutable accepted input and its decision, including retained terminal conflicts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Record {
    at_ns: u64,
    event: Event,
    decision: Decision,
    phase: Phase,
    outcome: Option<Outcome>,
}

impl Record {
    pub fn at_ns(&self) -> u64 {
        self.at_ns
    }

    pub fn event(&self) -> Event {
        self.event
    }

    pub fn decision(&self) -> Decision {
        self.decision
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }
}

/// Construction requests at time zero; only accepted supplied evidence advances time.
#[derive(Debug, Serialize)]
pub struct Lifecycle {
    schema_version: u32,
    provenance: Provenance,
    time_basis: TimeBasis,
    binding: Binding,
    policy: Policy,
    requested_at_ns: u64,
    last_at_ns: u64,
    phase: Phase,
    outcome: Option<Outcome>,
    descendants: Knowledge,
    orphans: Knowledge,
    resource_release: Knowledge,
    correctness: Correctness,
    had_rejections: bool,
    had_conflicts: bool,
    events: Vec<Record>,
    #[serde(skip)]
    launched: bool,
    #[serde(skip)]
    ran: bool,
}

impl Lifecycle {
    pub fn new(binding: Binding, policy: Policy) -> Self {
        Self {
            schema_version: 1,
            provenance: Provenance::Simulated,
            time_basis: TimeBasis::RunRelativeMonotonicNanoseconds,
            binding,
            policy,
            requested_at_ns: 0,
            last_at_ns: 0,
            phase: Phase::Requested,
            outcome: None,
            descendants: Knowledge::Unknown,
            orphans: Knowledge::Unknown,
            resource_release: Knowledge::Unknown,
            correctness: Correctness::NotEvaluated,
            had_rejections: false,
            had_conflicts: false,
            events: Vec::new(),
            launched: false,
            ran: false,
        }
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    pub fn events(&self) -> &[Record] {
        &self.events
    }

    pub fn had_rejections(&self) -> bool {
        self.had_rejections
    }

    pub fn had_conflicts(&self) -> bool {
        self.had_conflicts
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn append(
        &mut self,
        binding: &Binding,
        event: Event,
        at_ns: u64,
    ) -> Result<Decision, Error> {
        let result = self.try_append(binding, event, at_ns);
        if result.is_err() {
            self.had_rejections = true;
        }
        result
    }

    fn try_append(
        &mut self,
        binding: &Binding,
        event: Event,
        at_ns: u64,
    ) -> Result<Decision, Error> {
        if binding != &self.binding {
            return Err(Error::BindingMismatch);
        }
        if at_ns < self.last_at_ns {
            return Err(Error::TimeRegression);
        }
        if self.events.len() >= self.policy.event_capacity {
            return Err(Error::CapacityReached);
        }
        let decision = if self.outcome.is_some() {
            if event == Event::AdvanceTime {
                Decision::NoOp
            } else {
                self.had_conflicts = true;
                Decision::Conflict
            }
        } else if let Some(expired) = self.expiry(at_ns) {
            self.outcome = Some(Outcome::OutcomeUnknown);
            expired
        } else {
            let decision = self.apply(event)?;
            if let Some(expired) = self.expiry(at_ns) {
                self.outcome = Some(Outcome::OutcomeUnknown);
                expired
            } else {
                decision
            }
        };
        self.last_at_ns = at_ns;
        self.events.push(Record {
            at_ns,
            event,
            decision,
            phase: self.phase,
            outcome: self.outcome,
        });
        Ok(decision)
    }

    fn expiry(&self, at_ns: u64) -> Option<Decision> {
        if self.outcome.is_some() {
            return None;
        }
        self.policy
            .deadlines
            .for_phase(self.phase)
            .filter(|deadline| at_ns >= *deadline)
            .map(|deadline_ns| Decision::BoundExceeded {
                phase: self.phase,
                deadline_ns,
            })
    }

    fn apply(&mut self, event: Event) -> Result<Decision, Error> {
        match event {
            Event::LaunchedObserved if self.phase == Phase::Requested => {
                self.phase = Phase::Launched;
                self.launched = true;
            }
            Event::ReadyObserved if self.phase == Phase::Launched => self.phase = Phase::Ready,
            Event::RunningObserved if self.phase == Phase::Ready => {
                self.phase = Phase::Running;
                self.ran = true;
            }
            Event::StopRequested => {
                if self.phase == Phase::StopRequested {
                    return Ok(Decision::NoOp);
                }
                self.phase = Phase::StopRequested;
            }
            Event::TerminatedObserved { reason } => {
                let outcome = match reason {
                    Termination::Completed if self.ran => Outcome::CompletedObserved,
                    Termination::Stopped if self.phase == Phase::StopRequested && self.launched => {
                        Outcome::StopObserved
                    }
                    Termination::Crashed if self.launched => Outcome::OutcomeUnknown,
                    _ => {
                        return Err(Error::InvalidPhase {
                            phase: self.phase,
                            event,
                        });
                    }
                };
                self.phase = Phase::Terminated;
                self.outcome = Some(outcome);
            }
            Event::NeverLaunchedObserved
                if !self.launched
                    && matches!(self.phase, Phase::Requested | Phase::StopRequested) =>
            {
                self.outcome = Some(Outcome::NeverLaunched);
            }
            Event::WorkerLost if self.launched => self.outcome = Some(Outcome::OutcomeUnknown),
            Event::ControllerLost => self.outcome = Some(Outcome::OutcomeUnknown),
            Event::AdvanceTime => return Ok(Decision::NoOp),
            _ => {
                return Err(Error::InvalidPhase {
                    phase: self.phase,
                    event,
                });
            }
        }
        Ok(Decision::Applied)
    }
}
