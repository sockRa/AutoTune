//! Independent fake-worker policy, used by the actual fixed Windows worker role.
//! It trusts authenticated guardian publication receipts, never coordinator requests.
use autotune_recovery_contract::{process::*, storage::*, *};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPhase {
    Capture,
    Sealed,
    Armed,
    Recovering,
    Verified,
    Failed,
}
pub struct WorkerPolicy {
    profile: FakeProfile,
    session: Session,
    boot: Hash,
    custody: Hash,
    operator: String,
    preparation: Option<Preparation>,
    manifest: Option<Manifest>,
    arm: Option<ArmCommit>,
    recovery: Option<RecoveryWindow>,
    phase: WorkerPhase,
    generation: u64,
    last_id: u64,
    chain: usize,
    current: u32,
    candidate_reserved: bool,
    candidate_returned: bool,
    restores: u32,
    verifying: bool,
    reads: Vec<(u64, u32)>,
    ledger: Vec<StoredCall>,
    pending: Option<Permission>,
    call_entered: bool,
    receipt_pending: bool,
    storage_failed: bool,
}
pub struct Entry {
    permit: Permission,
    proof: DispatchProof,
}
const CHAIN: [Operation; 5] = [
    Operation::Enumerate,
    Operation::Identity,
    Operation::Runtime,
    Operation::Bounds,
    Operation::Current,
];
impl WorkerPolicy {
    pub fn new(
        profile: FakeProfile,
        session: Session,
        boot: Hash,
        custody: Hash,
        operator: String,
    ) -> Result<Self> {
        profile.validate()?;
        let current = profile.baseline_mw;
        Ok(Self {
            profile,
            session,
            boot,
            custody,
            operator,
            preparation: None,
            manifest: None,
            arm: None,
            recovery: None,
            phase: WorkerPhase::Capture,
            generation: 1,
            last_id: 0,
            chain: 0,
            current,
            candidate_reserved: false,
            candidate_returned: false,
            restores: 0,
            verifying: false,
            reads: vec![],
            ledger: vec![],
            pending: None,
            call_entered: false,
            receipt_pending: false,
            storage_failed: false,
        })
    }
    pub fn phase(&self) -> WorkerPhase {
        self.phase
    }
    pub fn candidate_entries(&self) -> u32 {
        self.ledger
            .iter()
            .filter(|c| {
                c.permit.operation == Operation::CandidateOnce
                    && matches!(
                        c.completion,
                        Some(StoredCompletion {
                            outcome: Completion::Returned { .. },
                            ..
                        })
                    )
            })
            .count() as u32
    }
    pub fn restore_entries(&self) -> u32 {
        self.ledger
            .iter()
            .filter(|c| {
                c.permit.operation == Operation::RestoreBaseline
                    && matches!(
                        c.completion,
                        Some(StoredCompletion {
                            outcome: Completion::Returned { .. },
                            ..
                        })
                    )
            })
            .count() as u32
    }
    pub fn prepare(&mut self, p: Preparation, publication: Hash) -> Result<Hash> {
        p.validate()?;
        if self.preparation.is_some()
            || publication == [0; 32]
            || p.session != self.session
            || p.boot != self.boot
            || p.device != self.profile.device
            || p.operator != self.operator
        {
            return Err(Rejection::Binding);
        }
        let digest = p.digest()?;
        self.preparation = Some(p);
        Ok(digest)
    }
    pub fn custody(&self) -> Hash {
        self.custody
    }
    pub fn seal(&mut self, m: Manifest) -> Result<Hash> {
        m.validate()?;
        if self.phase != WorkerPhase::Capture
            || self.ledger.len() != 6
            || self.pending.is_some()
            || self.receipt_pending
            || self.preparation.as_ref() != Some(&m.preparation)
            || m.runtime != self.profile.runtime
            || m.capture.minimum_mw != self.profile.minimum_mw
            || m.capture.maximum_mw != self.profile.maximum_mw
            || m.capture.readings_mw != [self.profile.baseline_mw; 2]
            || m.capture.default_mw != self.profile.default_mw
            || m.capture.trace != digest(b"stored-capture-v1", &canonical(&self.ledger)?)
        {
            return Err(Rejection::Binding);
        }
        let samples: Vec<_> = self
            .ledger
            .iter()
            .filter_map(|c| match &c.completion {
                Some(StoredCompletion {
                    outcome:
                        Completion::Returned {
                            return_ms,
                            result: RawResult::Value(value),
                            ..
                        },
                    ..
                }) if c.permit.operation == Operation::Current => Some((*return_ms, *value)),
                _ => None,
            })
            .collect();
        if samples
            != vec![
                (m.capture.sampled_ms[0], m.capture.readings_mw[0]),
                (m.capture.sampled_ms[1], m.capture.readings_mw[1]),
            ]
        {
            return Err(Rejection::Binding);
        }
        let digest = m.digest()?;
        self.manifest = Some(m);
        self.phase = WorkerPhase::Sealed;
        self.chain = 0;
        Ok(digest)
    }
    pub fn arm(&mut self, a: &ArmCapsule, now: u64, utc: u64, guardian_alive: bool) -> Result<()> {
        if !guardian_alive
            || self.phase != WorkerPhase::Sealed
            || a.nonce_publication == [0; 32]
            || a.commit.boot_ms > now
            || a.commit.utc_ms > utc
        {
            return Err(Rejection::Binding);
        }
        a.approval
            .validate(self.manifest.as_ref().ok_or(Rejection::Binding)?, now, utc)?;
        self.arm = Some(a.commit.clone());
        self.phase = WorkerPhase::Armed;
        Ok(())
    }
    pub fn revoke(&mut self, generation: u64, window: RecoveryWindow) -> Result<()> {
        if generation < self.generation
            || window.deadline_ms <= window.trigger_ms
            || window.deadline_ms
                > window
                    .trigger_ms
                    .checked_add(CLEANUP_MS)
                    .ok_or(Rejection::Expired)?
        {
            return Err(Rejection::Binding);
        }
        self.generation = generation;
        self.trigger(window.trigger_ms);
        if let Some(r) = &mut self.recovery {
            r.trigger_ms = r.trigger_ms.min(window.trigger_ms);
            r.deadline_ms = r.deadline_ms.min(window.deadline_ms);
        }
        Ok(())
    }
    pub fn storage_failed(&mut self, window: RecoveryWindow) -> Result<()> {
        if !self.candidate_reserved {
            return Err(Rejection::Binding);
        }
        self.revoke(self.generation, window)?;
        self.storage_failed = true;
        Ok(())
    }
    fn trigger(&mut self, now: u64) {
        if self.manifest.is_none() || self.arm.is_none() || !self.candidate_reserved {
            self.phase = WorkerPhase::Failed;
            return;
        }
        if self.recovery.is_none() {
            self.recovery = now
                .checked_add(CLEANUP_MS)
                .map(|deadline_ms| RecoveryWindow {
                    trigger_ms: now,
                    deadline_ms,
                });
            self.chain = 0;
        }
        if self.phase != WorkerPhase::Verified {
            self.phase = if self.recovery.is_some() {
                WorkerPhase::Recovering
            } else {
                WorkerPhase::Failed
            };
        }
    }
    fn expected(&self) -> Option<Operation> {
        match self.phase {
            WorkerPhase::Capture => {
                if self.ledger.len() < 5 {
                    Some(CHAIN[self.ledger.len()])
                } else if self.ledger.len() == 5 {
                    Some(Operation::Current)
                } else {
                    None
                }
            }
            WorkerPhase::Armed => {
                if self.chain < 5 {
                    Some(CHAIN[self.chain])
                } else if !self.candidate_reserved {
                    Some(Operation::CandidateOnce)
                } else if self.candidate_returned {
                    Some(Operation::Current)
                } else {
                    None
                }
            }
            WorkerPhase::Recovering => {
                if self.verifying {
                    Some(Operation::Current)
                } else if self.chain < 5 {
                    Some(CHAIN[self.chain])
                } else if self.restores < 2 {
                    Some(Operation::RestoreBaseline)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    pub fn admit(
        &mut self,
        proof: DispatchProof,
        now: u64,
        utc: u64,
        guardian_alive: bool,
    ) -> Result<std::result::Result<Entry, Completion>> {
        let p = &proof.permit;
        let prep = self.preparation.as_ref().ok_or(Rejection::Binding)?;
        let expected_digest = if let Some(m) = &self.manifest {
            m.digest()?
        } else {
            prep.digest()?
        };
        if self.pending.is_some()
            || self.receipt_pending
            || p.session != self.session
            || p.boot != self.boot
            || p.digest != expected_digest
            || p.generation != self.generation
            || p.operation_id != self.last_id.checked_add(1).ok_or(Rejection::Sequence)?
            || self.ledger.len() >= 64
            || (!self.storage_failed && proof.publication.is_none_or(|h| h == [0; 32]))
            || proof.intent_sequence == 0
        {
            return Err(Rejection::Binding);
        }
        if self
            .ledger
            .last()
            .and_then(|c| c.completion.as_ref())
            .is_some_and(|c| proof.intent_sequence <= c.sequence)
        {
            return Err(Rejection::Sequence);
        }
        if self.expected() != Some(p.operation) {
            return Err(Rejection::Invalid("worker finite operation chain"));
        }
        self.last_id = p.operation_id;
        if p.operation == Operation::CandidateOnce {
            self.candidate_reserved = true;
        }
        if p.operation == Operation::RestoreBaseline {
            self.restores += 1;
        }
        let mut rejection = p.validate_entry(now, &self.boot, guardian_alive).err();
        if self.phase == WorkerPhase::Capture && self.ledger.len() == 5 {
            let first = match &self.ledger[4].completion {
                Some(StoredCompletion {
                    outcome: Completion::Returned { return_ms, .. },
                    ..
                }) => *return_ms,
                _ => return Err(Rejection::Binding),
            };
            if now
                < first
                    .checked_add(CAPTURE_SEPARATION_MS)
                    .ok_or(Rejection::Expired)?
            {
                rejection = Some(Rejection::Expired);
            }
        }
        if self.phase == WorkerPhase::Armed {
            let m = self.manifest.as_ref().ok_or(Rejection::Binding)?;
            let arm = self.arm.as_ref().ok_or(Rejection::Binding)?;
            let ceiling = arm
                .boot_ms
                .checked_add(CANDIDATE_MS)
                .ok_or(Rejection::Expired)?
                .min(
                    m.capture.sampled_ms[1]
                        .checked_add(CAPTURE_FRESH_MS)
                        .ok_or(Rejection::Expired)?,
                );
            if p.expiry_ms > ceiling || now >= ceiling || utc >= m.approval_expiry_utc_ms {
                rejection = Some(Rejection::Expired);
            }
        }
        if self.phase == WorkerPhase::Recovering {
            let r = self.recovery.as_ref().ok_or(Rejection::Binding)?;
            if p.expiry_ms > r.deadline_ms
                || now >= r.deadline_ms
                || p.recovery_trigger_ms != Some(r.trigger_ms)
            {
                rejection = Some(Rejection::Expired);
            }
            if self.verifying
                && self
                    .reads
                    .first()
                    .is_some_and(|(time, _)| now < time.saturating_add(VERIFY_SEPARATION_MS))
            {
                rejection = Some(Rejection::Expired);
            }
        }
        if (p.operation == Operation::CandidateOnce
            && self.profile.behavior == FakeBehavior::MissingCandidateApi)
            || (p.operation == Operation::RestoreBaseline
                && self.profile.behavior == FakeBehavior::MissingRestoreApi)
        {
            rejection = Some(Rejection::Invalid("required fake API missing"));
        }
        self.ledger.push(StoredCall {
            permit: p.clone(),
            intent_sequence: proof.intent_sequence,
            intent_utc_ms: proof.intent_utc_ms,
            completion: None,
        });
        if let Some(reason) = rejection {
            let outcome = Completion::NotEntered {
                reason: format!(
                    "{reason:?} now={now} expiry={} trigger={:?} recovery={:?}",
                    p.expiry_ms, p.recovery_trigger_ms, self.recovery
                ),
            };
            self.record_outcome(outcome.clone());
            // The guardian's authenticated Revoke establishes the one recovery
            // epoch; a worker-local rejection time must not become a rival origin.
            return Ok(Err(outcome));
        }
        self.pending = Some(p.clone());
        self.call_entered = false;
        Ok(Ok(Entry {
            permit: p.clone(),
            proof,
        }))
    }
    /// Exactly one explicit fake API call, no validation getters hidden inside it.
    pub fn fake_call(&mut self, entry: &Entry) -> Result<RawResult> {
        if self.pending.as_ref() != Some(&entry.permit) || self.call_entered {
            return Err(Rejection::Binding);
        }
        self.call_entered = true;
        Ok(match entry.permit.operation {
            Operation::Enumerate | Operation::Identity => {
                RawResult::Identity(self.profile.device.clone())
            }
            Operation::Runtime => RawResult::Runtime(self.profile.runtime.clone()),
            Operation::Bounds => RawResult::Bounds {
                minimum_mw: self.profile.minimum_mw,
                maximum_mw: self.profile.maximum_mw,
            },
            Operation::Current => RawResult::Value(self.current),
            Operation::CandidateOnce => {
                self.current = self.manifest.as_ref().ok_or(Rejection::Binding)?.target_mw;
                RawResult::Api {
                    status: if self.profile.behavior == FakeBehavior::ErrorAfterChange {
                        -7
                    } else {
                        0
                    },
                }
            }
            Operation::RestoreBaseline => {
                self.current = self
                    .manifest
                    .as_ref()
                    .ok_or(Rejection::Binding)?
                    .capture
                    .baseline()?;
                RawResult::Api { status: 0 }
            }
        })
    }
    pub fn revalidate_entry(&self, entry: &Entry, now: u64, guardian_alive: bool) -> Result<()> {
        if self.pending.as_ref() != Some(&entry.permit) {
            return Err(Rejection::Binding);
        }
        entry.permit.validate_entry(now, &self.boot, guardian_alive)
    }
    pub fn not_entered(&mut self, entry: Entry, reason: String, _now: u64) -> Result<Completion> {
        if self.pending.as_ref() != Some(&entry.permit) || self.call_entered {
            return Err(Rejection::Binding);
        }
        let result = Completion::NotEntered { reason };
        self.record_outcome(result.clone());
        // No worker-local recovery epoch: an unused permit is retired and the
        // guardian decides whether any possible mutation requires Revoke.
        Ok(result)
    }
    fn record_outcome(&mut self, outcome: Completion) {
        self.pending = None;
        self.call_entered = false;
        self.receipt_pending = true;
        // Sequence/UTC are supplied ONLY by the later authenticated publication receipt.
        self.ledger.last_mut().unwrap().completion = Some(StoredCompletion {
            sequence: 0,
            received_utc_ms: 0,
            outcome,
        });
    }
    pub fn returned(
        &mut self,
        entry: Entry,
        entry_ms: u64,
        return_ms: u64,
        result: RawResult,
    ) -> Result<Completion> {
        if self.pending.as_ref() != Some(&entry.permit)
            || !self.call_entered
            || entry.proof.permit != entry.permit
            || return_ms < entry_ms
        {
            return Err(Rejection::Binding);
        }
        let operation = entry.permit.operation;
        let original_generation = entry.permit.generation;
        let original_recovery = entry.permit.recovery_trigger_ms;
        let outcome = Completion::Returned {
            entry_ms,
            return_ms,
            result: result.clone(),
        };
        self.record_outcome(outcome.clone());
        // A candidate may finish after cancellation or storage failure. Its
        // return retires that original permit, never an enumeration operation in
        // the newly started recovery validation chain.
        if operation == Operation::CandidateOnce {
            self.candidate_returned = true;
            // Recovery is started only by the guardian's authenticated Revoke.
            // A worker-local receipt must not choose a different trigger epoch.
            return Ok(outcome);
        }
        if original_generation != self.generation
            || (self.phase == WorkerPhase::Recovering
                && original_recovery != self.recovery.as_ref().map(|r| r.trigger_ms))
        {
            return Ok(outcome);
        }
        match self.phase {
            WorkerPhase::Capture => (),
            WorkerPhase::Armed => {
                if self.chain < 5 {
                    self.chain += 1;
                }
                // The post-candidate readback does not self-trigger; the guardian's
                // authenticated Revoke freezes the one recovery epoch.
            }
            WorkerPhase::Recovering => {
                if operation == Operation::RestoreBaseline {
                    self.verifying = true;
                    self.reads.clear();
                } else if self.verifying {
                    if result
                        == RawResult::Value(
                            self.manifest
                                .as_ref()
                                .ok_or(Rejection::Binding)?
                                .capture
                                .baseline()?,
                        )
                    {
                        self.reads.push((return_ms, self.current));
                        if self.reads.len() == 2 {
                            self.phase = WorkerPhase::Verified;
                        }
                    } else {
                        self.verifying = false;
                        self.chain = 0;
                        if self.restores >= 2 {
                            self.phase = WorkerPhase::Failed;
                        }
                    }
                } else {
                    self.chain += 1;
                }
            }
            _ => (),
        }
        Ok(outcome)
    }
    pub fn recorded(&mut self, r: &ResultReceipt) -> Result<()> {
        if !self.receipt_pending
            || self.pending.is_some()
            || r.operation_id != self.last_id
            || (!self.storage_failed && r.publication.is_none_or(|h| h == [0; 32]))
        {
            return Err(Rejection::Binding);
        }
        let call = self.ledger.last_mut().ok_or(Rejection::Binding)?;
        if r.sequence <= call.intent_sequence {
            return Err(Rejection::Sequence);
        }
        let c = call.completion.as_mut().ok_or(Rejection::Binding)?;
        c.sequence = r.sequence;
        c.received_utc_ms = r.received_utc_ms;
        self.receipt_pending = false;
        Ok(())
    }
}
