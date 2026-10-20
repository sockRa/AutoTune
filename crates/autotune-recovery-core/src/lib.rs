//! Deterministic fake-only guardian policy. Platform observations are inputs, not OS proof.
#![forbid(unsafe_code)]

use autotune_recovery_contract::*;
pub mod journal;
pub mod worker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Transport,
    Capture,
    Manifest,
    Armed,
    Recovering,
    RestoredObserved,
    Aborted,
    RecoveryRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Cancel,
    ParentLost,
    Heartbeat,
    Lease,
    Clock,
    Storage,
    Watchdog,
    WorkerLost,
    Protocol,
    Budget,
    Overflow,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    PublishPreparation(Hash),
    PublishIntent(Permission),
    Dispatch(Permission),
    PublishResult(u64),
    PublishArm(Hash),
    PublishTerminal,
    Alert(Fault),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Publication {
    Preparation,
    Intent,
    Result,
    Arm,
    Terminal,
}
#[derive(Debug, Clone)]
struct Pending {
    permit: Permission,
    dispatched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Evidence {
    pub calls: Vec<CallRecord>,
    pub candidate_api: Option<i32>,
    pub candidate_readback: Option<u32>,
    pub restore_api: Vec<i32>,
    pub baseline_reads: Vec<(u64, u32)>,
    pub observed_restoration: bool,
    pub publication_verified: bool,
    pub normal_recovery_missed: bool,
    pub faults: Vec<Fault>,
}

/// One actor/lane. No method accepts a raw setter value or replacement worker.
pub struct Guardian {
    preparation: Preparation,
    manifest: Option<Manifest>,
    phase: Phase,
    custody: bool,
    publication: Option<(Publication, u64)>,
    storage_failed: bool,
    pending: Option<Pending>,
    result_waiting: bool,
    generation: u64,
    next_id: u64,
    candidate_reserved: bool,
    restore_count: usize,
    armed_ms: Option<u64>,
    heartbeat_ms: u64,
    trigger_ms: Option<u64>,
    cleanup_ms: Option<u64>,
    now_ms: u64,
    utc_ms: u64,
    chain: usize,
    samples: Vec<(u64, u32)>,
    capture_bounds: Option<(u32, u32)>,
    runtime: Option<Runtime>,
    candidate_read: bool,
    verifying: bool,
    evidence: Evidence,
    published_capture_trace: Option<Hash>,
    arm_commit_ms: Option<u64>,
}
const CHAIN: [Operation; 5] = [
    Operation::Enumerate,
    Operation::Identity,
    Operation::Runtime,
    Operation::Bounds,
    Operation::Current,
];

impl Guardian {
    pub fn new(preparation: Preparation, now_ms: u64, utc_ms: u64) -> Result<Self> {
        preparation.validate()?;
        if now_ms < preparation.created_ms {
            return Err(Rejection::Expired);
        }
        Ok(Self {
            preparation,
            manifest: None,
            phase: Phase::Transport,
            custody: false,
            publication: None,
            storage_failed: false,
            pending: None,
            result_waiting: false,
            generation: 1,
            next_id: 1,
            candidate_reserved: false,
            restore_count: 0,
            armed_ms: None,
            heartbeat_ms: now_ms,
            trigger_ms: None,
            cleanup_ms: None,
            now_ms,
            utc_ms,
            chain: 0,
            samples: Vec::new(),
            capture_bounds: None,
            runtime: None,
            candidate_read: false,
            verifying: false,
            evidence: Evidence::default(),
            published_capture_trace: None,
            arm_commit_ms: None,
        })
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }
    pub fn evidence(&self) -> &Evidence {
        &self.evidence
    }
    pub fn pending(&self) -> Option<&Permission> {
        self.pending.as_ref().map(|p| &p.permit)
    }
    pub fn cleanup_deadline(&self) -> Option<u64> {
        self.cleanup_ms
    }
    pub fn storage_failed(&self) -> bool {
        self.storage_failed
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn capture_trace_digest(&self) -> Result<Hash> {
        if self.phase != Phase::Capture || self.samples.len() != 2 || self.pending.is_some() {
            return Err(Rejection::Invalid("capture trace incomplete"));
        }
        match self.published_capture_trace {
            Some(trace) => Ok(trace),
            None => Ok(digest(b"capture-trace", &canonical(&self.evidence.calls)?)),
        }
    }

    /// Bind real journal sequence/UTC metadata to the exact already-returned
    /// capture operations. This is not an alternate source of call outcomes.
    pub fn bind_published_capture(&mut self, calls: &[storage::StoredCall]) -> Result<()> {
        if self.phase != Phase::Capture
            || self.samples.len() != 2
            || self.pending.is_some()
            || self.publication.is_some()
            || self.result_waiting
            || calls.len() != 6
            || calls.len() != self.evidence.calls.len()
        {
            return Err(Rejection::Binding);
        }
        let mut prior = 0;
        for (stored, observed) in calls.iter().zip(&self.evidence.calls) {
            let completion = stored.completion.as_ref().ok_or(Rejection::Binding)?;
            if stored.permit != observed.permit
                || Some(&completion.outcome) != observed.completion.as_ref()
                || stored.intent_sequence <= prior
                || completion.sequence <= stored.intent_sequence
            {
                return Err(Rejection::Binding);
            }
            prior = completion.sequence;
        }
        self.published_capture_trace = Some(digest(b"stored-capture-v1", &canonical(&calls)?));
        Ok(())
    }

    /// The journal freezes the commit before its publication; receiving a later
    /// acknowledgement must not move the lease origin forward.
    pub fn bind_arm_commit(&mut self, commit: &storage::ArmCommit) -> Result<()> {
        if self.phase != Phase::Manifest
            || !matches!(self.publication, Some((Publication::Arm, _)))
            || self.arm_commit_ms.is_some()
            || commit.boot_ms != self.now_ms
            || commit.utc_ms != self.utc_ms
        {
            return Err(Rejection::Binding);
        }
        self.arm_commit_ms = Some(commit.boot_ms);
        Ok(())
    }

    pub fn recovery_window(&self) -> Option<storage::RecoveryWindow> {
        Some(storage::RecoveryWindow {
            trigger_ms: self.trigger_ms?,
            deadline_ms: self.cleanup_ms?,
        })
    }

    pub fn storage_unavailable(&mut self, now: u64, utc: u64) -> Result<Vec<Effect>> {
        let mut effects = self.tick(now, utc)?;
        if !self.storage_failed {
            effects.extend(self.storage_fault());
        }
        Ok(effects)
    }

    /// Live-owner continuation only: the caller must pass the checksum of the
    /// already validated opening snapshot carrying this exact preparation. This
    /// is not cold-start authority and performs no second preparation publication.
    pub fn resume_prepared(&mut self, opening_checksum: Hash) -> Result<()> {
        if self.phase != Phase::Transport
            || self.publication.is_some()
            || self.pending.is_some()
            || !self.evidence.calls.is_empty()
            || opening_checksum == [0; 32]
        {
            return Err(Rejection::Binding);
        }
        self.custody = true;
        self.phase = Phase::Capture;
        Ok(())
    }

    /// Only a platform adapter may assert these observations; this is not an OS verifier.
    pub fn transport_ready(
        &mut self,
        no_console: bool,
        no_job: bool,
        authenticated: bool,
        same_lock_custody: bool,
    ) -> Result<Vec<Effect>> {
        if self.phase != Phase::Transport
            || self.publication.is_some()
            || !no_console
            || !no_job
            || !authenticated
            || !same_lock_custody
        {
            return Err(Rejection::Invalid("transport readiness"));
        }
        self.custody = true;
        self.start_publication(Publication::Preparation)?;
        Ok(vec![Effect::PublishPreparation(self.preparation.digest()?)])
    }
    fn start_publication(&mut self, kind: Publication) -> Result<()> {
        if self.publication.is_some() || self.storage_failed {
            return Err(Rejection::Invalid("storage lane"));
        }
        let deadline = self
            .now_ms
            .checked_add(PUBLICATION_MS)
            .ok_or(Rejection::Expired)?;
        self.publication = Some((kind, self.cleanup_ms.map_or(deadline, |d| d.min(deadline))));
        Ok(())
    }
    pub fn publication_ack(
        &mut self,
        now_ms: u64,
        utc_ms: u64,
        success: bool,
    ) -> Result<Vec<Effect>> {
        let mut effects = self.tick(now_ms, utc_ms)?;
        // A timed-out writer stays quarantined; late acknowledgement never changes authority.
        if self.storage_failed {
            return Ok(effects);
        }
        let (kind, _) = self
            .publication
            .take()
            .ok_or(Rejection::Invalid("unexpected publication"))?;
        if !success {
            effects.extend(self.storage_fault());
            return Ok(effects);
        }
        match kind {
            Publication::Preparation => {
                if self.phase == Phase::Transport {
                    self.phase = Phase::Capture;
                }
            }
            Publication::Arm => {
                if self.phase == Phase::Manifest {
                    self.phase = Phase::Armed;
                    self.armed_ms = Some(self.arm_commit_ms.unwrap_or(now_ms));
                    self.heartbeat_ms = now_ms;
                    self.chain = 0;
                }
            }
            Publication::Intent => effects.extend(self.dispatch_reserved()?),
            Publication::Result => self.result_waiting = false,
            Publication::Terminal => {
                if self.phase == Phase::RestoredObserved && self.pending.is_none() {
                    self.evidence.publication_verified = true;
                }
            }
        }
        Ok(effects)
    }
    pub fn seal(&mut self, manifest: Manifest, worker_digest_ack: Hash) -> Result<()> {
        manifest.validate()?;
        if self.phase != Phase::Capture
            || self.samples.len() != 2
            || self.chain != 5
            || self.pending.is_some()
            || self.publication.is_some()
            || self.result_waiting
            || manifest.preparation != self.preparation
            || manifest.digest()? != worker_digest_ack
            || manifest.capture.readings_mw != [self.samples[0].1, self.samples[1].1]
            || manifest.capture.sampled_ms != [self.samples[0].0, self.samples[1].0]
            || self.capture_bounds
                != Some((manifest.capture.minimum_mw, manifest.capture.maximum_mw))
            || self.runtime.as_ref() != Some(&manifest.runtime)
            || manifest.capture.trace != self.capture_trace_digest()?
        {
            return Err(Rejection::Binding);
        }
        self.manifest = Some(manifest);
        self.phase = Phase::Manifest;
        Ok(())
    }
    /// The journal port must persist complete nonce history; this check is model evidence only.
    pub fn approve(
        &mut self,
        approval: &SimulatedApproval,
        consumed: &[Hash],
    ) -> Result<Vec<Effect>> {
        if self.phase != Phase::Manifest
            || self.publication.is_some()
            || consumed.len() >= HISTORY_LIMIT
            || consumed.contains(&self.preparation.nonce)
        {
            return Err(Rejection::Invalid("approval state"));
        }
        let manifest = self.manifest.as_ref().ok_or(Rejection::Binding)?;
        approval.validate(manifest, self.now_ms, self.utc_ms)?;
        let digest = manifest.digest()?;
        self.start_publication(Publication::Arm)?;
        Ok(vec![Effect::PublishArm(digest)])
    }
    fn expected(&self) -> Option<Operation> {
        match self.phase {
            Phase::Capture => {
                if self.chain < 5 {
                    Some(CHAIN[self.chain])
                } else if self.samples.len() == 1 {
                    Some(Operation::Current)
                } else {
                    None
                }
            }
            Phase::Armed => {
                if self.chain < 5 {
                    Some(CHAIN[self.chain])
                } else if !self.candidate_reserved {
                    Some(Operation::CandidateOnce)
                } else if !self.candidate_read {
                    Some(Operation::Current)
                } else {
                    None
                }
            }
            Phase::Recovering => {
                if self.verifying {
                    Some(Operation::Current)
                } else if self.chain < 5 {
                    Some(CHAIN[self.chain])
                } else if self.restore_count < 2 {
                    Some(Operation::RestoreBaseline)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    pub fn next_operation(&self) -> Option<Operation> {
        if self.pending.is_some()
            || self.result_waiting
            || (!self.storage_failed && self.publication.is_some())
        {
            None
        } else {
            self.expected()
        }
    }
    pub fn reserve(
        &mut self,
        operation: Operation,
        now_ms: u64,
        utc_ms: u64,
    ) -> Result<Vec<Effect>> {
        let mut effects = self.tick(now_ms, utc_ms)?;
        if self.next_operation() != Some(operation) || !self.custody {
            return Err(Rejection::Invalid("operation order"));
        }
        if self.phase == Phase::Capture
            && self.chain == 5
            && now_ms
                < self.samples[0]
                    .0
                    .checked_add(CAPTURE_SEPARATION_MS)
                    .ok_or(Rejection::Expired)?
        {
            return Err(Rejection::Expired);
        }
        if self.verifying
            && !self.evidence.baseline_reads.is_empty()
            && now_ms
                < self.evidence.baseline_reads[0]
                    .0
                    .checked_add(VERIFY_SEPARATION_MS)
                    .ok_or(Rejection::Expired)?
        {
            return Err(Rejection::Expired);
        }
        let watchdog = now_ms.checked_add(WATCHDOG_MS).ok_or(Rejection::Expired)?;
        let mut expiry = watchdog;
        if let Some(cleanup) = self.cleanup_ms {
            expiry = expiry.min(cleanup);
        }
        if self.phase == Phase::Armed {
            let m = self.manifest.as_ref().ok_or(Rejection::Binding)?;
            expiry = expiry
                .min(
                    self.armed_ms
                        .ok_or(Rejection::Binding)?
                        .checked_add(CANDIDATE_MS)
                        .ok_or(Rejection::Expired)?,
                )
                .min(
                    m.capture.sampled_ms[1]
                        .checked_add(CAPTURE_FRESH_MS)
                        .ok_or(Rejection::Expired)?,
                );
        }
        if now_ms >= expiry {
            return Err(Rejection::Expired);
        }
        let permit = Permission {
            session: self.preparation.session,
            boot: self.preparation.boot,
            digest: match &self.manifest {
                Some(m) => m.digest()?,
                None => self.preparation.digest()?,
            },
            generation: self.generation,
            operation_id: self.next_id,
            operation,
            dispatched_ms: now_ms,
            watchdog_ms: watchdog,
            expiry_ms: expiry,
            recovery_trigger_ms: self.trigger_ms,
        };
        self.next_id = self.next_id.checked_add(1).ok_or(Rejection::Sequence)?;
        if operation == Operation::CandidateOnce {
            self.candidate_reserved = true;
        }
        if operation == Operation::RestoreBaseline {
            self.restore_count += 1;
        }
        self.pending = Some(Pending {
            permit: permit.clone(),
            dispatched: false,
        });
        self.evidence.calls.push(CallRecord {
            permit: permit.clone(),
            intent_sequence: permit.operation_id,
            completion: None,
        });
        if self.storage_failed {
            effects.extend(self.dispatch_reserved()?);
        } else {
            self.start_publication(Publication::Intent)?;
            effects.push(Effect::PublishIntent(permit));
        }
        Ok(effects)
    }
    fn dispatch_reserved(&mut self) -> Result<Vec<Effect>> {
        let Some(pending) = self.pending.as_mut() else {
            return Ok(Vec::new());
        };
        if pending.dispatched {
            return Err(Rejection::Invalid("duplicate dispatch"));
        }
        // A reserve is not proof of entry. Never dispatch a late candidate intent acknowledgement.
        if self.now_ms >= pending.permit.expiry_ms
            || pending.permit.generation != self.generation
            || matches!(
                self.phase,
                Phase::Aborted | Phase::RecoveryRequired | Phase::RestoredObserved
            )
            || (pending.permit.operation == Operation::CandidateOnce && self.phase != Phase::Armed)
        {
            if let Some(call) = self.evidence.calls.last_mut() {
                call.completion = Some(Completion::NotEntered {
                    reason: "intent retired before dispatch".into(),
                });
            }
            self.pending = None;
            return Ok(Vec::new());
        }
        pending.dispatched = true;
        Ok(vec![Effect::Dispatch(pending.permit.clone())])
    }
    /// Caller supplies only an already authenticated, sequence-checked atomic ReturnAndIdle.
    pub fn return_and_idle(
        &mut self,
        permit: &Permission,
        outcome: Completion,
        now_ms: u64,
        utc_ms: u64,
    ) -> Result<Vec<Effect>> {
        let mut effects = self.tick(now_ms, utc_ms)?;
        let pending = self.pending.as_ref().ok_or(Rejection::Binding)?;
        if !pending.dispatched || &pending.permit != permit {
            return Err(Rejection::Binding);
        }
        if let Completion::Returned {
            entry_ms,
            return_ms,
            ..
        } = &outcome
        {
            if *entry_ms < permit.dispatched_ms
                || *entry_ms >= permit.expiry_ms
                || return_ms < entry_ms
                || *return_ms > now_ms
            {
                return Err(Rejection::Invalid("completion times"));
            }
        }
        self.pending = None;
        self.evidence
            .calls
            .last_mut()
            .ok_or(Rejection::Binding)?
            .completion = Some(outcome.clone());
        let admissible = !matches!(self.phase, Phase::Aborted | Phase::RecoveryRequired);
        // Preserve every late API outcome, even after the cleanup deadline. Such evidence
        // must never advance validation or restore authority.
        if !admissible {
            if let Completion::Returned {
                result: RawResult::Api { status },
                ..
            } = &outcome
            {
                match permit.operation {
                    Operation::CandidateOnce => self.evidence.candidate_api = Some(*status),
                    Operation::RestoreBaseline => self.evidence.restore_api.push(*status),
                    _ => (),
                }
            }
        } else if permit.generation == self.generation || permit.operation.setter() {
            match outcome {
                Completion::NotEntered { .. } => effects.extend(self.fault(Fault::Protocol)),
                Completion::Returned {
                    entry_ms,
                    return_ms,
                    result,
                } => {
                    // Entry/return remain distinct; late returned results never renew authority.
                    if !self.consume(permit.operation, entry_ms, return_ms, result)? {
                        effects.extend(self.fault(Fault::Protocol));
                        if self.phase == Phase::Recovering && !permit.operation.setter() {
                            self.phase = Phase::RecoveryRequired;
                        }
                    }
                }
            }
        }
        self.result_waiting = true;
        if self.storage_failed {
            self.result_waiting = false;
        } else if self.publication.is_none() {
            self.start_publication(Publication::Result)?;
            effects.push(Effect::PublishResult(permit.operation_id));
        }
        Ok(effects)
    }
    fn consume(
        &mut self,
        op: Operation,
        entry: u64,
        returned: u64,
        result: RawResult,
    ) -> Result<bool> {
        // An old-generation operation may return after cancellation. Preserve its API result,
        // but do not let it satisfy the new recovery validation chain.
        if op == Operation::CandidateOnce {
            if let RawResult::Api { status } = result {
                self.evidence.candidate_api = Some(status);
            } else {
                return Ok(false);
            }
            return Ok(self.evidence.candidate_api == Some(0));
        }
        if op == Operation::RestoreBaseline {
            if let RawResult::Api { status } = result {
                self.evidence.restore_api.push(status);
            } else {
                return Ok(false);
            }
            self.verifying = true;
            self.evidence.baseline_reads.clear();
            return Ok(true); // API failure does not preclude observing B; failure remains recorded.
        }
        if self.phase == Phase::Recovering && self.verifying {
            let baseline = self
                .manifest
                .as_ref()
                .ok_or(Rejection::Binding)?
                .capture
                .baseline()?;
            if let RawResult::Value(value) = result {
                if value == baseline {
                    if let Some((first, _)) = self.evidence.baseline_reads.first() {
                        if entry
                            < first
                                .checked_add(VERIFY_SEPARATION_MS)
                                .ok_or(Rejection::Expired)?
                        {
                            return Ok(false);
                        }
                    }
                    self.evidence.baseline_reads.push((returned, value));
                    if self.evidence.baseline_reads.len() == 2 {
                        self.evidence.observed_restoration = true;
                        self.phase = Phase::RestoredObserved;
                    }
                    return Ok(true);
                }
            }
            self.verifying = false;
            self.chain = 0;
            if self.restore_count >= 2 {
                self.phase = Phase::RecoveryRequired;
            }
            return Ok(true);
        }
        if self.phase == Phase::Armed && self.chain == 5 && self.candidate_reserved {
            self.candidate_read = true;
            if let RawResult::Value(value) = result {
                self.evidence.candidate_readback = Some(value);
            }
            self.begin_recovery();
            return Ok(true);
        }
        // After a fault, results from the old validation chain cannot advance cleanup checks.
        if self.phase == Phase::Recovering && self.chain == 0 && op != Operation::Enumerate {
            return Ok(true);
        }
        if self.chain < 5 && CHAIN[self.chain] != op {
            return Ok(false);
        }
        let valid = match (op, result) {
            (Operation::Enumerate | Operation::Identity, RawResult::Identity(device)) => {
                device == self.preparation.device
            }
            (Operation::Runtime, RawResult::Runtime(runtime)) => {
                if let Some(m) = &self.manifest {
                    runtime == m.runtime
                } else {
                    self.runtime = Some(runtime);
                    true
                }
            }
            (
                Operation::Bounds,
                RawResult::Bounds {
                    minimum_mw,
                    maximum_mw,
                },
            ) => {
                let valid = minimum_mw > 0 && minimum_mw < maximum_mw && maximum_mw != u32::MAX;
                if let Some(m) = &self.manifest {
                    valid
                        && (minimum_mw, maximum_mw) == (m.capture.minimum_mw, m.capture.maximum_mw)
                } else {
                    self.capture_bounds = Some((minimum_mw, maximum_mw));
                    valid
                }
            }
            (Operation::Current, RawResult::Value(value)) => {
                if self.phase == Phase::Capture {
                    let valid = value > 0
                        && value != u32::MAX
                        && self
                            .capture_bounds
                            .is_some_and(|(min, max)| (min..=max).contains(&value))
                        && self.samples.first().is_none_or(|(time, b)| {
                            *b == value && entry >= time.saturating_add(CAPTURE_SEPARATION_MS)
                        });
                    if valid {
                        self.samples.push((returned, value));
                    }
                    valid
                } else if self.phase == Phase::Armed {
                    self.manifest
                        .as_ref()
                        .is_some_and(|m| m.capture.readings_mw[0] == value)
                } else {
                    value > 0 && value != u32::MAX
                }
            }
            _ => false,
        };
        if valid && self.chain < 5 {
            self.chain += 1;
        }
        Ok(valid)
    }
    fn begin_recovery(&mut self) {
        if self.trigger_ms.is_none() {
            self.trigger_ms = Some(self.now_ms);
            self.cleanup_ms = self.now_ms.checked_add(CLEANUP_MS);
            self.generation = self.generation.saturating_add(1);
        }
        self.chain = 0;
        self.phase = if self.cleanup_ms.is_some() {
            Phase::Recovering
        } else {
            Phase::RecoveryRequired
        };
    }
    pub fn fault(&mut self, fault: Fault) -> Vec<Effect> {
        if !self.evidence.faults.contains(&fault) {
            self.evidence.faults.push(fault);
        }
        self.evidence.publication_verified = false;
        if matches!(self.phase, Phase::Aborted | Phase::RecoveryRequired) {
            return vec![Effect::Alert(fault)];
        }
        if self.candidate_reserved {
            // Freeze the first trigger. Repeated faults never reset an active validation chain.
            if self.trigger_ms.is_none() {
                self.begin_recovery();
            }
            if matches!(fault, Fault::WorkerLost | Fault::Budget | Fault::Clock) {
                self.phase = Phase::RecoveryRequired;
            }
        } else {
            self.phase = Phase::Aborted;
        }
        vec![Effect::Alert(fault)]
    }
    fn storage_fault(&mut self) -> Vec<Effect> {
        self.storage_failed = true;
        // Retain the outstanding publication, quarantining late acknowledgements.
        self.result_waiting = false;
        let effects = self.fault(Fault::Storage);
        // An intent not yet dispatched may be retired locally; no possible entry occurred.
        if self.pending.as_ref().is_some_and(|p| !p.dispatched) {
            if let Some(call) = self.evidence.calls.last_mut() {
                call.completion = Some(Completion::NotEntered {
                    reason: "storage failed before dispatch".into(),
                });
            }
            self.pending = None;
        }
        effects
    }
    pub fn tick(&mut self, now_ms: u64, utc_ms: u64) -> Result<Vec<Effect>> {
        if now_ms < self.now_ms || utc_ms < self.utc_ms {
            return Ok(self.fault(Fault::Clock));
        }
        self.now_ms = now_ms;
        self.utc_ms = utc_ms;
        let mut effects = Vec::new();
        if self
            .publication
            .is_some_and(|(_, deadline)| now_ms >= deadline)
            && !self.storage_failed
        {
            effects.extend(self.storage_fault());
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.dispatched && now_ms >= p.permit.expiry_ms)
        {
            effects.extend(self.fault(Fault::Watchdog));
        }
        if let Some(armed) = self.armed_ms {
            if self.candidate_reserved
                && !self.candidate_read
                && now_ms >= armed.saturating_add(NORMAL_RECOVERY_MS)
            {
                self.evidence.normal_recovery_missed = true;
            }
            if now_ms >= armed.saturating_add(NORMAL_RECOVERY_MS) && self.trigger_ms.is_none() {
                self.evidence.normal_recovery_missed = true;
                effects.extend(self.fault(Fault::Lease));
            }
            if self.phase == Phase::Armed {
                if now_ms >= armed.saturating_add(CANDIDATE_MS)
                    || now_ms >= armed.saturating_add(LEASE_MS)
                    || self.manifest.as_ref().is_some_and(|m| {
                        now_ms >= m.capture.sampled_ms[1].saturating_add(CAPTURE_FRESH_MS)
                            || utc_ms >= m.approval_expiry_utc_ms
                    })
                {
                    effects.extend(self.fault(Fault::Lease));
                } else if now_ms >= self.heartbeat_ms.saturating_add(HEARTBEAT_LOSS_MS) {
                    effects.extend(self.fault(Fault::Heartbeat));
                }
            }
        }
        if self.cleanup_ms.is_some_and(|deadline| now_ms >= deadline)
            && self.phase == Phase::Recovering
        {
            effects.extend(self.fault(Fault::Budget));
        }
        Ok(effects)
    }
    pub fn heartbeat(&mut self, now_ms: u64, utc_ms: u64) -> Result<Vec<Effect>> {
        let effects = self.tick(now_ms, utc_ms)?;
        if self.phase == Phase::Armed {
            self.heartbeat_ms = now_ms;
        }
        Ok(effects)
    }
    pub fn close(&mut self) -> Result<Vec<Effect>> {
        if self.phase != Phase::RestoredObserved
            || self.pending.is_some()
            || self.publication.is_some()
            || self.result_waiting
            || self.storage_failed
        {
            return Err(Rejection::Invalid("terminal closure"));
        }
        self.start_publication(Publication::Terminal)?;
        Ok(vec![Effect::PublishTerminal])
    }
}

/// Original-anchor clock policy; never rebase a discontinuous clock.
#[derive(Debug, Clone, Copy)]
pub struct ClockSample {
    pub interrupt_ms: u64,
    pub unbiased_ms: u64,
    pub utc_ms: u64,
    pub uncertainty_ms: u64,
}
pub struct ClockGuard {
    anchor: ClockSample,
    previous: ClockSample,
}
impl ClockGuard {
    pub fn new(sample: ClockSample) -> Result<Self> {
        if sample.uncertainty_ms > 2 {
            return Err(Rejection::Invalid("clock bracket"));
        }
        Ok(Self {
            anchor: sample,
            previous: sample,
        })
    }
    pub fn observe(&mut self, s: ClockSample) -> Result<()> {
        if s.uncertainty_ms > 2
            || s.interrupt_ms < self.previous.interrupt_ms
            || s.unbiased_ms < self.previous.unbiased_ms
            || s.utc_ms < self.previous.utc_ms
        {
            return Err(Rejection::Invalid("clock regression"));
        }
        let elapsed = s
            .interrupt_ms
            .checked_sub(self.anchor.interrupt_ms)
            .ok_or(Rejection::Expired)?;
        let unbiased = s
            .unbiased_ms
            .checked_sub(self.anchor.unbiased_ms)
            .ok_or(Rejection::Expired)?;
        let utc = s
            .utc_ms
            .checked_sub(self.anchor.utc_ms)
            .ok_or(Rejection::Expired)?;
        if elapsed.abs_diff(unbiased) > 2 || elapsed.abs_diff(utc) > 20 {
            return Err(Rejection::Invalid("clock discontinuity"));
        }
        self.previous = s;
        Ok(())
    }
}
