//! Pure adapter from the existing guardian reducer to the frozen disk contract.
//! A publication effect is NOT an acknowledgement. Only an exact timely receipt
//! advances the published view; after failure cleanup is RAM-only forever.
use crate::{Effect, Fault, Guardian, Phase};
use autotune_recovery_contract::{process::DispatchProof, storage::*, *};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalEffect {
    Publish(Box<JournalEnvelope>),
    Dispatch(Box<DispatchProof>),
    Alert(Fault),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationKind {
    Reducer,
    Seal,
    Consent,
    Metadata,
}
struct PendingPublication {
    kind: PublicationKind,
    checksum: Hash,
    deadline: u64,
}

pub struct JournaledGuardian {
    guardian: Guardian,
    working: JournalEnvelope,
    published: JournalEnvelope,
    publication: Option<PendingPublication>,
    consent: Option<StoredConsent>,
    now: u64,
    utc: u64,
    publication_failed: bool,
}

impl JournaledGuardian {
    pub fn new(
        preparation: Preparation,
        anchor: JournalEnvelope,
        now: u64,
        utc: u64,
    ) -> Result<Self> {
        anchor.validate()?;
        if !matches!(anchor.payload, JournalPayload::Anchor { .. }) {
            return Err(Rejection::Invalid(
                "startup journals are reconciliation data, never authority",
            ));
        }
        Ok(Self {
            guardian: Guardian::new(preparation, now, utc)?,
            working: anchor.clone(),
            published: anchor,
            publication: None,
            consent: None,
            now,
            utc,
            publication_failed: false,
        })
    }
    pub fn guardian(&self) -> &Guardian {
        &self.guardian
    }
    /// Live-owner resume from an opening snapshot this owner just published.
    /// Cold/foreign snapshots are rejected: no anchor lineage verification exists
    /// here, so this constructor is deliberately not a startup authority path.
    pub fn resume(
        preparation: Preparation,
        opening: JournalEnvelope,
        now: u64,
        utc: u64,
    ) -> Result<Self> {
        opening.validate()?;
        let JournalPayload::Snapshot(snapshot) = &opening.payload else {
            return Err(Rejection::Binding);
        };
        if snapshot.sequence != 1
            || snapshot.sessions.len() != 1
            || !snapshot.consumed_nonces.is_empty()
        {
            return Err(Rejection::Binding);
        }
        let session = &snapshot.sessions[0];
        if session.preparation != preparation
            || session.manifest.is_some()
            || session.state != StoredState::Preparing
            || session.nonce_consumed
            || session.consent.is_some()
            || session.arm_commit.is_some()
            || session.recovery.is_some()
            || !session.calls.is_empty()
            || !session.notices.is_empty()
        {
            return Err(Rejection::Binding);
        }
        let mut guardian = Guardian::new(preparation, now, utc)?;
        guardian.resume_prepared(opening.checksum)?;
        Ok(Self {
            guardian,
            working: opening.clone(),
            published: opening,
            publication: None,
            consent: None,
            now,
            utc,
            publication_failed: false,
        })
    }
    pub fn working(&self) -> &JournalEnvelope {
        &self.working
    }
    pub fn published(&self) -> &JournalEnvelope {
        &self.published
    }
    pub fn publication_failed(&self) -> bool {
        self.publication_failed
    }

    pub fn start(
        &mut self,
        preparation: Preparation,
        no_console: bool,
        no_job: bool,
        authenticated: bool,
        custody: bool,
    ) -> Result<Vec<JournalEffect>> {
        if self.publication.is_some() || self.working.sequence() != 0 {
            return Err(Rejection::Binding);
        }
        let effects = self
            .guardian
            .transport_ready(no_console, no_job, authenticated, custody)?;
        let expected = preparation.digest()?;
        if !matches!(effects.as_slice(), [Effect::PublishPreparation(d)] if *d == expected) {
            return Err(Rejection::Binding);
        }
        self.working = JournalEnvelope::new(JournalPayload::Snapshot(StorageSnapshot {
            version: STORAGE_VERSION,
            store: self.working.store(),
            sequence: 1,
            previous: self.working.checksum,
            consumed_nonces: vec![],
            sessions: vec![StoredSession {
                preparation,
                manifest: None,
                previous_terminal: None,
                nonce_consumed: false,
                consent: None,
                arm_commit: None,
                recovery: None,
                state: StoredState::Preparing,
                notices: vec![],
                calls: vec![],
            }],
        }))?;
        self.request(PublicationKind::Reducer)
    }

    fn edit(&mut self, change: impl FnOnce(&mut StorageSnapshot) -> Result<()>) -> Result<()> {
        let JournalPayload::Snapshot(mut snapshot) = self.working.payload.clone() else {
            return Err(Rejection::Binding);
        };
        snapshot.sequence = snapshot
            .sequence
            .checked_add(1)
            .ok_or(Rejection::Sequence)?;
        snapshot.previous = self.working.checksum;
        change(&mut snapshot)?;
        let next = JournalEnvelope::new(JournalPayload::Snapshot(snapshot))?;
        next.follows(&self.working)?;
        self.working = next;
        Ok(())
    }
    fn request(&mut self, kind: PublicationKind) -> Result<Vec<JournalEffect>> {
        if self.publication_failed {
            return Ok(vec![]);
        }
        if self.publication.is_some() {
            return Err(Rejection::Invalid("one publisher only"));
        }
        let deadline = self
            .now
            .checked_add(PUBLICATION_MS)
            .ok_or(Rejection::Expired)?;
        let deadline = self
            .guardian
            .cleanup_deadline()
            .map_or(deadline, |d| d.min(deadline));
        self.publication = Some(PendingPublication {
            kind,
            checksum: self.working.checksum,
            deadline,
        });
        Ok(vec![JournalEffect::Publish(Box::new(self.working.clone()))])
    }
    fn times(&mut self, now: u64, utc: u64) -> Result<()> {
        if now < self.now || utc < self.utc {
            self.guardian.fault(Fault::Clock);
            return Err(Rejection::Invalid("clock regression"));
        }
        self.now = now;
        self.utc = utc;
        Ok(())
    }
    pub fn acknowledge(
        &mut self,
        checksum: Hash,
        now: u64,
        utc: u64,
    ) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        if self.publication_failed {
            return Err(Rejection::Invalid("late storage receipt is evidence only"));
        }
        let pending = self.publication.as_ref().ok_or(Rejection::Binding)?;
        if checksum != pending.checksum {
            return Err(Rejection::Binding);
        }
        if now >= pending.deadline {
            return self.storage_failure(now, utc);
        }
        let kind = self.publication.take().unwrap().kind;
        self.published = self.working.clone();
        match kind {
            PublicationKind::Reducer => {
                let effects = self.guardian.publication_ack(now, utc, true)?;
                self.translate(effects)
            }
            PublicationKind::Consent => {
                let consent = self.consent.as_ref().ok_or(Rejection::Binding)?;
                let prior = self.session()?.preparation.nonce;
                let consumed: Vec<Hash> = match &self.published.payload {
                    JournalPayload::Snapshot(s) => s
                        .consumed_nonces
                        .iter()
                        .copied()
                        .filter(|n| *n != prior)
                        .collect(),
                    _ => return Err(Rejection::Binding),
                };
                self.guardian.tick(now, utc)?;
                let effects = self.guardian.approve(&consent.approval, &consumed)?;
                let commit = ArmCommit {
                    boot_ms: now,
                    utc_ms: utc,
                };
                self.guardian.bind_arm_commit(&commit)?;
                self.edit(|s| {
                    s.sessions[0].arm_commit = Some(commit);
                    s.sessions[0].state = StoredState::Armed;
                    Ok(())
                })?;
                if !matches!(effects.as_slice(), [Effect::PublishArm(_)]) {
                    return Err(Rejection::Binding);
                }
                self.request(PublicationKind::Reducer)
            }
            PublicationKind::Seal | PublicationKind::Metadata => Ok(vec![]),
        }
    }

    fn session(&self) -> Result<&StoredSession> {
        match &self.working.payload {
            JournalPayload::Snapshot(s) if s.sessions.len() == 1 => Ok(&s.sessions[0]),
            _ => Err(Rejection::Binding),
        }
    }
    pub fn capture_trace(&mut self) -> Result<Hash> {
        if self.publication.is_some() || self.publication_failed {
            return Err(Rejection::Binding);
        }
        let calls = self.session()?.calls.clone();
        self.guardian.bind_published_capture(&calls)?;
        self.guardian.capture_trace_digest()
    }
    pub fn seal(&mut self, manifest: Manifest, worker_digest: Hash) -> Result<Vec<JournalEffect>> {
        self.capture_trace()?;
        self.guardian.seal(manifest.clone(), worker_digest)?;
        self.edit(|s| {
            s.sessions[0].manifest = Some(manifest);
            s.sessions[0].state = StoredState::ManifestReady;
            Ok(())
        })?;
        self.request(PublicationKind::Seal)
    }
    pub fn approve(
        &mut self,
        approval: SimulatedApproval,
        now: u64,
        utc: u64,
    ) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        if self.publication.is_some()
            || self.publication_failed
            || self.consent.is_some()
            || self.guardian.phase() != Phase::Manifest
        {
            return Err(Rejection::Binding);
        }
        approval.validate(
            self.guardian.manifest().ok_or(Rejection::Binding)?,
            now,
            utc,
        )?;
        let consent = StoredConsent {
            approval,
            boot_ms: now,
            utc_ms: utc,
        };
        self.consent = Some(consent.clone());
        self.edit(|s| {
            if s.consumed_nonces.contains(&s.sessions[0].preparation.nonce) {
                return Err(Rejection::Binding);
            }
            s.consumed_nonces.push(s.sessions[0].preparation.nonce);
            s.sessions[0].nonce_consumed = true;
            s.sessions[0].consent = Some(consent);
            Ok(())
        })?;
        self.request(PublicationKind::Consent)
    }
    pub fn reserve(&mut self, op: Operation, now: u64, utc: u64) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        if self.publication.is_some() && !self.publication_failed {
            return Err(Rejection::Invalid("await publication"));
        }
        let effects = self.guardian.reserve(op, now, utc)?;
        self.translate(effects)
    }
    pub fn completed(
        &mut self,
        permit: &Permission,
        outcome: Completion,
        now: u64,
        utc: u64,
    ) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        let effects = self.guardian.return_and_idle(permit, outcome, now, utc)?;
        self.translate(effects)
    }
    pub fn tick(&mut self, now: u64, utc: u64) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        if !self.publication_failed && self.publication.as_ref().is_some_and(|p| now >= p.deadline)
        {
            return self.storage_failure(now, utc);
        }
        let effects = self.guardian.tick(now, utc)?;
        self.translate(effects)
    }
    pub fn heartbeat(&mut self, now: u64, utc: u64) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        let effects = self.guardian.heartbeat(now, utc)?;
        self.translate(effects)
    }
    pub fn fault(&mut self, fault: Fault, now: u64, utc: u64) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        let mut effects = self.guardian.tick(now, utc)?;
        effects.extend(self.guardian.fault(fault));
        self.translate(effects)
    }
    pub fn storage_failure(&mut self, now: u64, utc: u64) -> Result<Vec<JournalEffect>> {
        self.times(now, utc)?;
        self.publication_failed = true; // Never remove or reuse the uncertain publisher.
        let effects = self.guardian.storage_unavailable(now, utc)?;
        self.translate(effects)
    }
    pub fn close(&mut self) -> Result<Vec<JournalEffect>> {
        let effects = self.guardian.close()?;
        self.translate(effects)
    }

    fn translate(&mut self, effects: Vec<Effect>) -> Result<Vec<JournalEffect>> {
        let mut outputs = vec![];
        for effect in effects {
            match effect {
                Effect::PublishPreparation(_) | Effect::PublishArm(_) => {
                    return Err(Rejection::Invalid("unexpected lifecycle publication"))
                }
                Effect::PublishIntent(permit) => {
                    self.add_intent(permit)?;
                    outputs.extend(self.request(PublicationKind::Reducer)?);
                }
                Effect::Dispatch(permit) => {
                    if self.publication_failed
                        && self
                            .session()?
                            .calls
                            .last()
                            .is_none_or(|c| c.permit != permit)
                    {
                        self.add_intent(permit.clone())?;
                    }
                    let call = self.session()?.calls.last().ok_or(Rejection::Binding)?;
                    if call.permit != permit || call.completion.is_some() {
                        return Err(Rejection::Binding);
                    }
                    outputs.push(JournalEffect::Dispatch(Box::new(DispatchProof {
                        permit,
                        intent_sequence: call.intent_sequence,
                        intent_utc_ms: call.intent_utc_ms,
                        publication: (!self.publication_failed).then_some(self.published.checksum),
                    })));
                }
                Effect::PublishResult(id) => {
                    self.add_result(id)?;
                    outputs.extend(self.request(PublicationKind::Reducer)?);
                }
                Effect::PublishTerminal => {
                    self.edit(|s| {
                        s.sessions[0].state = StoredState::RestoredVerified;
                        Ok(())
                    })?;
                    outputs.extend(self.request(PublicationKind::Reducer)?);
                }
                Effect::Alert(fault) => outputs.push(JournalEffect::Alert(fault)),
            }
        }
        // After failed storage the reducer deliberately emits no PublishResult.
        // Retain the exact completion in RAM, never an invented durable receipt.
        if self.publication_failed {
            if let Some(last) = self.guardian.evidence().calls.last() {
                if last.completion.is_some()
                    && self
                        .session()?
                        .calls
                        .last()
                        .is_some_and(|c| c.permit == last.permit && c.completion.is_none())
                {
                    self.add_result(last.permit.operation_id)?;
                }
            }
            if !self.session()?.state.terminal() {
                self.synchronize_state()?;
            }
        }
        if !self.publication_failed && self.publication.is_none() {
            if let Some(last) = self.guardian.evidence().calls.last() {
                if matches!(last.completion, Some(Completion::NotEntered { .. }))
                    && self.guardian.pending().is_none()
                    && self
                        .session()?
                        .calls
                        .last()
                        .is_some_and(|c| c.permit == last.permit && c.completion.is_none())
                {
                    self.add_result(last.permit.operation_id)?;
                    outputs.extend(self.request(PublicationKind::Metadata)?);
                }
            }
        }
        if !outputs
            .iter()
            .any(|e| matches!(e, JournalEffect::Publish(_)))
            && self.publication.is_none()
            && outputs.iter().any(|e| matches!(e, JournalEffect::Alert(_)))
        {
            self.synchronize_state()?;
            outputs.extend(self.request(PublicationKind::Metadata)?);
        }
        Ok(outputs)
    }
    fn add_intent(&mut self, permit: Permission) -> Result<()> {
        let utc = self.utc;
        self.edit(|s| {
            if permit.operation == Operation::CandidateOnce {
                s.sessions[0].state = StoredState::MutationPossible;
            }
            s.sessions[0].calls.push(StoredCall {
                permit,
                intent_sequence: s.sequence,
                intent_utc_ms: utc,
                completion: None,
            });
            Ok(())
        })
    }
    fn add_result(&mut self, id: u64) -> Result<()> {
        let observed = self
            .guardian
            .evidence()
            .calls
            .iter()
            .find(|c| c.permit.operation_id == id)
            .ok_or(Rejection::Binding)?;
        let outcome = observed.completion.clone().ok_or(Rejection::Binding)?;
        let utc = self.utc;
        let window = self.guardian.recovery_window();
        let phase = self.guardian.phase();
        let faults = self.guardian.evidence().faults.clone();
        let missed = self.guardian.evidence().normal_recovery_missed;
        let now = self.now;
        self.edit(|s| {
            let session = &mut s.sessions[0];
            let call = session.calls.last_mut().ok_or(Rejection::Binding)?;
            if call.permit.operation_id != id || call.completion.is_some() {
                return Err(Rejection::Binding);
            }
            call.completion = Some(StoredCompletion {
                sequence: s.sequence,
                received_utc_ms: utc,
                outcome,
            });
            sync_state(session, phase, window);
            append_notices(session, &faults, missed, s.sequence, now, utc);
            Ok(())
        })
    }
    fn synchronize_state(&mut self) -> Result<()> {
        let phase = self.guardian.phase();
        let window = self.guardian.recovery_window();
        let now = self.now;
        let utc = self.utc;
        let faults = self.guardian.evidence().faults.clone();
        let missed = self.guardian.evidence().normal_recovery_missed;
        self.edit(|s| {
            let session = &mut s.sessions[0];
            sync_state(session, phase, window);
            append_notices(session, &faults, missed, s.sequence, now, utc);
            Ok(())
        })
    }
}
fn append_notices(
    session: &mut StoredSession,
    faults: &[Fault],
    missed: bool,
    sequence: u64,
    now: u64,
    utc: u64,
) {
    let mut codes: Vec<_> = faults.iter().copied().map(notice).collect();
    if missed {
        codes.push(NoticeCode::NormalRecoveryMissed);
    }
    for code in codes {
        if !session.notices.iter().any(|n| n.code == code) {
            session.notices.push(StoredNotice {
                sequence,
                boot_ms: now,
                utc_ms: utc,
                code,
            });
        }
    }
}
fn sync_state(session: &mut StoredSession, phase: Phase, window: Option<RecoveryWindow>) {
    if window.is_some() {
        session.recovery = window;
    }
    match phase {
        Phase::Recovering | Phase::RestoredObserved => session.state = StoredState::Recovering,
        Phase::RecoveryRequired => session.state = StoredState::RecoveryRequired,
        Phase::Aborted if session.calls.iter().all(|c| c.completion.is_some()) => {
            session.state = StoredState::AbortedNoWrite
        }
        _ => (),
    }
}
fn notice(fault: Fault) -> NoticeCode {
    match fault {
        Fault::Cancel => NoticeCode::Cancel,
        Fault::ParentLost => NoticeCode::ParentLost,
        Fault::Heartbeat => NoticeCode::Heartbeat,
        Fault::Lease => NoticeCode::Lease,
        Fault::Clock => NoticeCode::Clock,
        Fault::Storage => NoticeCode::Storage,
        Fault::Watchdog => NoticeCode::Watchdog,
        Fault::WorkerLost => NoticeCode::WorkerLost,
        Fault::Protocol => NoticeCode::Protocol,
        Fault::Budget => NoticeCode::Budget,
        Fault::Overflow => NoticeCode::IngressOverflow,
    }
}
