use crate::journal::Journal;
use crate::manifest::{Bounds, Consent, Identity, Manifest, Reading, Rejection};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Candidate,
    CandidateRead,
    Restore,
    VerifyFirst,
    VerifySecond,
}

impl Operation {
    pub fn is_setter(self) -> bool {
        matches!(self, Self::Candidate | Self::Restore)
    }
}

#[derive(Clone, Debug)]
pub struct CallPlan {
    pub operation: Operation,
    /// None models an indefinitely blocked native call, not cancellation.
    pub duration_ms: Option<u64>,
    pub status: i32,
    pub change_to: Option<u32>,
    pub reading: Option<Reading<u32>>,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub devices: Vec<(Identity, String)>,
    pub bounds: Bounds,
    pub current: Reading<u32>,
    pub runtime: String,
    pub owned: bool,
    pub symbols_available: bool,
    pub guardian_ready: bool,
    pub guardian_alive: bool,
    pub plans: VecDeque<CallPlan>,
}

impl Device {
    fn resolve(&self, m: &Manifest) -> Result<String, Rejection> {
        let matches: Vec<_> = self
            .devices
            .iter()
            .filter(|(i, _)| i.uuid == m.identity.uuid)
            .collect();
        if matches.len() != 1 || matches[0].0 != m.identity {
            return Err(Rejection::Identity);
        }
        if !self.owned {
            return Err(Rejection::Ownership);
        }
        if self.bounds != m.bounds || self.runtime != m.driver_version {
            return Err(Rejection::Drift);
        }
        Ok(matches[0].1.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum State {
    Prepared,
    Armed,
    MutationPossible,
    CandidateObserved,
    RestorePending,
    RestoredVerified,
    AbortedNoWrite,
    RecoveryRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Failure {
    Rejected(Rejection),
    Api {
        operation: Operation,
        status: i32,
    },
    Readback {
        operation: Operation,
        reading: Reading<u32>,
    },
    Storage,
    Cancelled,
    ParentDeath,
    IpcLoss,
    HeartbeatLoss,
    LeaseExpired,
    ClockDiscontinuity,
    Watchdog(Operation),
    RecoveryDeadline,
    NormalRecoveryDeadline,
    GuardianLost,
    QuiescenceUnknown,
    InvalidCommand,
}

#[derive(Clone, Debug, Serialize)]
pub struct Trace {
    pub mono_ms: u64,
    pub utc_ms: u64,
    pub event: String,
    pub operation: Option<Operation>,
    pub handle: Option<String>,
    pub uuid: String,
    pub pci: String,
    pub value_mw: Option<u32>,
    pub status: Option<i32>,
    pub generation: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema: u32,
    pub provenance: String,
    pub scenario: String,
    pub manifest: Manifest,
    pub manifest_digest: String,
    pub state: State,
    pub candidate_api: Option<i32>,
    pub candidate_readback: Option<Reading<u32>>,
    pub restore_api: Vec<i32>,
    pub verification_reads: Vec<(u64, Reading<u32>)>,
    pub observed_restored: bool,
    pub evidence_durable: bool,
    pub overall: String,
    pub failures: Vec<Failure>,
    pub trace: Vec<Trace>,
    pub journal: Journal,
}

#[derive(Clone, Debug)]
struct Pending {
    operation: Operation,
    handle: String,
    value: Option<u32>,
    plan: CallPlan,
    entered: u64,
    due: Option<u64>,
    timed_out: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    Candidate {
        generation: u64,
        manifest_digest: String,
    },
    Heartbeat,
    Cancel,
    ParentDeath,
    IpcLoss,
    ClockDiscontinuity,
    GuardianDeath,
}

/// All transitions are serialized, with an injected clock, journal and device.
/// This models an independent guardian; it does NOT implement a Windows process.
pub struct Engine {
    manifest: Manifest,
    pub device: Device,
    pub journal: Journal,
    pub state: State,
    pub now: u64,
    utc_origin: u64,
    arm: u64,
    heartbeat: u64,
    generation: u64,
    admission: bool,
    possible_mutation: bool,
    candidate_entered: bool,
    restore_count: u32,
    recovery: Option<u64>,
    pending: Option<Pending>,
    verify_due: Option<u64>,
    first_verified: Option<u64>,
    normal_deadline_recorded: bool,
    report: Report,
}

impl Engine {
    pub fn arm(
        m: Manifest,
        consent: Option<Consent>,
        journal: &mut Journal,
        device: Device,
        now: u64,
        utc: u64,
    ) -> Result<Self, Rejection> {
        m.validate()?;
        m.fresh(now, utc)?;
        if utc.checked_sub(now).is_none()
            || now.checked_add(100_000).is_none()
            || utc.checked_add(100_000).is_none()
        {
            return Err(Rejection::ClockDiscontinuity);
        }
        consent.ok_or(Rejection::MissingConsent)?.validate(&m)?;
        if !device.guardian_ready || !device.guardian_alive {
            return Err(Rejection::GuardianUnavailable);
        }
        if !device.symbols_available {
            return Err(Rejection::Symbols);
        }
        device.resolve(&m)?;
        if device.current != Reading::Available(m.baseline_mw) {
            return Err(Rejection::Drift);
        }
        journal.begin(&m, now, utc)?;
        journal.publish("armed:guardian_ack", now, utc)?;
        let report = Report {
            schema: 1,
            provenance: "simulated".into(),
            scenario: String::new(),
            manifest: m.clone(),
            manifest_digest: m.digest(),
            state: State::Armed,
            candidate_api: None,
            candidate_readback: None,
            restore_api: vec![],
            verification_reads: vec![],
            observed_restored: false,
            evidence_durable: false,
            overall: "unresolved".into(),
            failures: vec![],
            trace: vec![],
            journal: journal.clone(),
        };
        let mut e = Self {
            manifest: m,
            device,
            journal: journal.clone(),
            state: State::Armed,
            now,
            utc_origin: utc - now,
            arm: now,
            heartbeat: now,
            generation: 1,
            admission: true,
            possible_mutation: false,
            candidate_entered: false,
            restore_count: 0,
            recovery: None,
            pending: None,
            verify_due: None,
            first_verified: None,
            normal_deadline_recorded: false,
            report,
        };
        e.trace("guardian_ack", None, None, None, None);
        Ok(e)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Modeled authenticated/manifest-bound ingress. This is not real IPC authentication.
    pub fn submit_candidate(&mut self, manifest_digest: &str, generation: u64) {
        if manifest_digest != self.manifest.digest() {
            self.fail(Failure::InvalidCommand);
            return;
        }
        self.event(Event::Candidate {
            generation,
            manifest_digest: manifest_digest.into(),
        });
    }
    pub fn next_completion(&self) -> Option<u64> {
        self.pending.as_ref().and_then(|p| p.due)
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, State::AbortedNoWrite | State::RestoredVerified)
            || self.state == State::RecoveryRequired
                && self
                    .recovery
                    .is_none_or(|r| self.now >= r + self.manifest.timers.recovery_budget_ms)
    }

    fn utc(&self) -> u64 {
        self.utc_origin + self.now
    }
    fn trace(
        &mut self,
        event: &str,
        op: Option<Operation>,
        handle: Option<String>,
        value: Option<u32>,
        status: Option<i32>,
    ) {
        self.report.trace.push(Trace {
            mono_ms: self.now,
            utc_ms: self.utc(),
            event: event.into(),
            operation: op,
            handle,
            uuid: self.manifest.identity.uuid.clone(),
            pci: self.manifest.identity.pci.clone(),
            value_mw: value,
            status,
            generation: self.generation,
        });
    }
    fn persist(&mut self, event: &str) -> bool {
        let utc = self.utc();
        match self.journal.publish(event, self.now, utc) {
            Ok(()) => {
                self.trace(&format!("durable:{event}"), None, None, None, None);
                true
            }
            Err(_) => {
                self.fail(Failure::Storage);
                false
            }
        }
    }
    fn fail(&mut self, f: Failure) {
        self.report.failures.push(f);
    }

    pub fn event(&mut self, event: Event) {
        match event {
            Event::Candidate {
                generation,
                manifest_digest,
            } => {
                if manifest_digest != self.manifest.digest()
                    || !self.admission
                    || self.candidate_entered
                    || generation != self.generation
                    || self.state != State::Armed
                {
                    self.fail(Failure::InvalidCommand);
                    return;
                }
                if self.now >= self.arm + self.manifest.timers.admission_ms {
                    self.trigger(Failure::Rejected(Rejection::AdmissionClosed));
                    return;
                }
                if !self.device.guardian_alive || !self.device.guardian_ready {
                    self.trigger(Failure::Rejected(Rejection::GuardianUnavailable));
                    return;
                }
                if !self.device.symbols_available {
                    self.trigger(Failure::Rejected(Rejection::Symbols));
                    return;
                }
                if !self.journal.validate() || !self.journal.unresolved {
                    self.trigger(Failure::Rejected(Rejection::JournalBlocked));
                    return;
                }
                if let Err(r) = self.manifest.fresh(self.now, self.utc()) {
                    self.trigger(Failure::Rejected(r));
                    return;
                }
                if let Err(r) = self.device.resolve(&self.manifest) {
                    self.trigger(Failure::Rejected(r));
                    return;
                }
                if self.device.current != Reading::Available(self.manifest.baseline_mw) {
                    self.trigger(Failure::Rejected(Rejection::Drift));
                    return;
                }
                if !self.persist("mutation_possible") {
                    self.abort();
                    return;
                }
                self.possible_mutation = true;
                self.state = State::MutationPossible;
                self.enter(Operation::Candidate);
            }
            Event::Heartbeat => {
                if self.admission {
                    self.heartbeat = self.now;
                    self.trace("heartbeat:lease_unchanged", None, None, None, None);
                }
            }
            Event::Cancel => self.trigger(Failure::Cancelled),
            Event::ParentDeath => self.trigger(Failure::ParentDeath),
            Event::IpcLoss => self.trigger(Failure::IpcLoss),
            Event::ClockDiscontinuity => self.trigger(Failure::ClockDiscontinuity),
            Event::GuardianDeath => {
                if matches!(self.state, State::RestoredVerified | State::AbortedNoWrite) {
                    return;
                }
                self.device.guardian_alive = false;
                self.close_admission();
                self.fail(Failure::GuardianLost);
                if self.possible_mutation {
                    self.state = State::RecoveryRequired;
                    self.recovery.get_or_insert(self.now);
                } else {
                    self.abort();
                }
            }
        }
    }

    fn close_admission(&mut self) {
        if self.admission {
            self.admission = false;
            self.generation += 1;
            self.trace(
                "admission_closed:queued_generation_fenced",
                None,
                None,
                None,
                None,
            );
        }
    }
    fn abort(&mut self) {
        self.close_admission();
        self.state = State::AbortedNoWrite;
        // Ambiguous storage state is deliberately retained, even with no native entry.
        if !self.journal.storage_failed && self.persist("aborted_no_write") {
            self.journal.unresolved = false;
        }
    }
    fn trigger(&mut self, failure: Failure) {
        if matches!(self.state, State::RestoredVerified | State::AbortedNoWrite) {
            return;
        }
        self.fail(failure);
        self.close_admission();
        if !self.possible_mutation {
            self.abort();
            return;
        }
        self.begin_recovery();
    }
    fn begin_recovery(&mut self) {
        self.close_admission();
        // Cancellation/expiry cannot restart cleanup, reset verification, or spend
        // another restore attempt while successful verification is in progress.
        if self.recovery.is_some() {
            return;
        }
        self.recovery = Some(self.now);
        self.trace("recovery_trigger:immutable_budget", None, None, None, None);
        self.state = State::RestorePending;
        if self.pending.is_some() {
            self.state = State::RecoveryRequired;
            self.fail(Failure::QuiescenceUnknown);
        } else {
            self.restore();
        }
    }
    fn in_budget(&self) -> bool {
        self.recovery
            .is_some_and(|r| self.now < r + self.manifest.timers.recovery_budget_ms)
    }
    fn restore(&mut self) {
        if !self.device.guardian_alive || !self.in_budget() || self.restore_count >= 2 {
            self.state = State::RecoveryRequired;
            return;
        }
        if self.pending.is_some() {
            self.state = State::RecoveryRequired;
            return;
        }
        if let Err(r) = self.device.resolve(&self.manifest) {
            self.fail(Failure::Rejected(r));
            self.state = State::RecoveryRequired;
            return;
        }
        self.first_verified = None;
        self.restore_count += 1;
        self.state = State::RestorePending;
        self.enter(Operation::Restore);
    }

    fn enter(&mut self, op: Operation) {
        if self.pending.is_some() {
            self.fail(Failure::InvalidCommand);
            self.state = State::RecoveryRequired;
            return;
        }
        if !self.device.symbols_available {
            self.fail(Failure::Rejected(Rejection::Symbols));
            self.close_admission();
            self.recovery.get_or_insert(self.now);
            self.state = State::RecoveryRequired;
            return;
        }
        let handle = match self.device.resolve(&self.manifest) {
            Ok(h) => h,
            Err(r) => {
                self.fail(Failure::Rejected(r));
                self.state = State::RecoveryRequired;
                return;
            }
        };
        // Candidate intent must be durable. Cleanup proceeds from retained guardian state
        // even when persistence fails after mutation became possible.
        let intent = format!("intent:{op:?}");
        if !self.persist(&intent) && op == Operation::Candidate {
            self.abort();
            return;
        }
        if op == Operation::Candidate {
            self.candidate_entered = true;
        }
        let value = match op {
            Operation::Candidate => Some(self.manifest.target_mw),
            Operation::Restore => Some(self.manifest.baseline_mw),
            _ => None,
        };
        let plan = if self.device.plans.front().is_some_and(|p| p.operation == op) {
            self.device.plans.pop_front().expect("front checked")
        } else {
            CallPlan {
                operation: op,
                duration_ms: Some(0),
                status: 0,
                change_to: value,
                reading: None,
            }
        };
        self.trace("native_entry", Some(op), Some(handle.clone()), value, None);
        self.pending = Some(Pending {
            operation: op,
            handle,
            value,
            due: plan.duration_ms.and_then(|d| self.now.checked_add(d)),
            entered: self.now,
            timed_out: false,
            plan,
        });
    }

    fn complete(&mut self) {
        let p = self
            .pending
            .take()
            .expect("completion requires pending call");
        if let Some(value) = p.plan.change_to {
            self.device.current = Reading::Available(value);
        }
        let reading = p
            .plan
            .reading
            .clone()
            .unwrap_or_else(|| self.device.current.clone());
        self.trace(
            "native_return",
            Some(p.operation),
            Some(p.handle),
            p.value.or(match &reading {
                Reading::Available(v) => Some(*v),
                _ => None,
            }),
            Some(p.plan.status),
        );
        if p.plan.status != 0 {
            self.fail(Failure::Api {
                operation: p.operation,
                status: p.plan.status,
            });
        }
        let result_persisted = self.persist(&format!(
            "result:{:?}:status={}:reading={reading:?}",
            p.operation, p.plan.status
        ));
        match p.operation {
            Operation::Candidate => self.report.candidate_api = Some(p.plan.status),
            Operation::Restore => self.report.restore_api.push(p.plan.status),
            _ => {}
        }
        if !self.device.guardian_alive {
            self.state = State::RecoveryRequired;
            return;
        }
        if self.recovery.is_some() && !self.in_budget() {
            self.state = State::RecoveryRequired;
            return;
        }
        if let Err(r) = self.device.resolve(&self.manifest) {
            self.fail(Failure::Rejected(r));
            self.close_admission();
            self.state = State::RecoveryRequired;
            if self.recovery.is_none() {
                self.recovery = Some(self.now);
            }
            return;
        }
        match p.operation {
            Operation::Candidate => {
                if self.recovery.is_some() {
                    self.restore();
                } else if p.plan.status != 0 || !result_persisted {
                    self.begin_recovery();
                } else {
                    self.enter(Operation::CandidateRead);
                }
            }
            Operation::CandidateRead => {
                self.report.candidate_readback = Some(if p.plan.status == 0 {
                    reading.clone()
                } else {
                    Reading::Unavailable(format!("native status {}", p.plan.status))
                });
                if p.plan.status != 0 || reading != Reading::Available(self.manifest.target_mw) {
                    self.fail(Failure::Readback {
                        operation: p.operation,
                        reading,
                    });
                }
                self.state = State::CandidateObserved;
                if self.recovery.is_some() {
                    self.restore();
                } else {
                    self.begin_recovery();
                }
            }
            Operation::Restore => self.enter(Operation::VerifyFirst),
            Operation::VerifyFirst | Operation::VerifySecond => {
                let reading = if p.plan.status == 0 {
                    reading
                } else {
                    Reading::Unavailable(format!("native status {}", p.plan.status))
                };
                self.report
                    .verification_reads
                    .push((self.now, reading.clone()));
                if reading != Reading::Available(self.manifest.baseline_mw) {
                    self.fail(Failure::Readback {
                        operation: p.operation,
                        reading,
                    });
                    self.restore();
                } else if p.operation == Operation::VerifyFirst {
                    self.first_verified = Some(self.now);
                    self.verify_due = Some(self.now + self.manifest.timers.verification_gap_ms);
                } else if self
                    .first_verified
                    .is_some_and(|t| self.now >= t + self.manifest.timers.verification_gap_ms)
                {
                    self.report.observed_restored = true;
                    let utc = self.utc();
                    if self.journal.close(self.now, utc).is_ok() {
                        self.trace(
                            "durable:restored_verified:durable_close",
                            None,
                            None,
                            None,
                            None,
                        );
                        self.report.evidence_durable = true;
                        self.state = State::RestoredVerified;
                    } else {
                        self.fail(Failure::Storage);
                        self.state = State::RecoveryRequired;
                    }
                }
            }
        }
    }

    /// Advance fake monotonic time. Timers win ties against completion/admission.
    /// Unknown calls remain pending even after watchdog/guardian death/budget expiry.
    pub fn advance(&mut self, to: u64) {
        if to < self.now
            || to.checked_add(100_000).is_none()
            || to.checked_add(self.utc_origin).is_none()
        {
            self.event(Event::ClockDiscontinuity);
            return;
        }
        loop {
            let mut next = to;
            if let Some(p) = &self.pending {
                if let Some(d) = p.due {
                    next = next.min(d.max(self.now));
                }
                if !p.timed_out {
                    next = next.min(p.entered + self.manifest.timers.watchdog_ms);
                }
            }
            if let Some(d) = self.verify_due {
                next = next.min(d);
            }
            if self.admission {
                next = next
                    .min(self.arm + self.manifest.timers.lease_ms)
                    .min(self.heartbeat + self.manifest.timers.heartbeat_loss_ms);
                if self.possible_mutation && !self.normal_deadline_recorded {
                    next = next.min(self.arm + self.manifest.timers.recovery_request_ms);
                }
            }
            if let Some(r) = self.recovery {
                let end = r + self.manifest.timers.recovery_budget_ms;
                if self.now < end {
                    next = next.min(end);
                }
            }
            self.now = next;
            if self.admission && self.now >= self.arm + self.manifest.timers.lease_ms {
                self.trigger(Failure::LeaseExpired);
            }
            if self.admission && self.now >= self.heartbeat + self.manifest.timers.heartbeat_loss_ms
            {
                self.trigger(Failure::HeartbeatLoss);
            }
            if self.admission
                && self.possible_mutation
                && !self.normal_deadline_recorded
                && self.now >= self.arm + self.manifest.timers.recovery_request_ms
            {
                self.normal_deadline_recorded = true;
                self.trigger(Failure::NormalRecoveryDeadline);
            }
            if self.pending.as_ref().is_some_and(|p| {
                !p.timed_out && self.now >= p.entered + self.manifest.timers.watchdog_ms
            }) {
                let p = self.pending.as_mut().expect("pending checked");
                p.timed_out = true;
                let op = p.operation;
                self.trigger(Failure::Watchdog(op));
            }
            if self
                .recovery
                .is_some_and(|r| self.now >= r + self.manifest.timers.recovery_budget_ms)
                && self.state != State::RestoredVerified
            {
                if !self.report.failures.contains(&Failure::RecoveryDeadline) {
                    self.fail(Failure::RecoveryDeadline);
                }
                self.state = State::RecoveryRequired;
                self.verify_due = None;
            }
            if self
                .pending
                .as_ref()
                .is_some_and(|p| p.due.is_some_and(|d| d <= self.now))
            {
                self.complete();
            }
            if self.verify_due.is_some_and(|d| d <= self.now) {
                self.verify_due = None;
                if self.in_budget() && self.device.guardian_alive && self.pending.is_none() {
                    self.enter(Operation::VerifySecond);
                }
            }
            // Drain zero-duration calls even at the requested time.
            let immediate = self
                .pending
                .as_ref()
                .is_some_and(|p| p.due == Some(self.now));
            if self.now == to && !immediate {
                break;
            }
        }
    }

    pub fn report(&self, scenario: &str) -> Report {
        let mut r = self.report.clone();
        r.scenario = scenario.into();
        r.state = self.state;
        r.journal = self.journal.clone();
        r.overall = if r.observed_restored
            && r.evidence_durable
            && r.candidate_api == Some(0)
            && r.candidate_readback == Some(Reading::Available(self.manifest.target_mw))
            && r.failures.is_empty()
        {
            "success"
        } else if self.state == State::AbortedNoWrite {
            "rejected"
        } else if self.state == State::RestoredVerified {
            "failed_restored"
        } else {
            "recovery_required"
        }
        .into();
        r
    }
}
