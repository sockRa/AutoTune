//! Frozen storage v1: data only, including preparation before manifest sealing.
use crate::*;

pub const STORAGE_VERSION: u32 = 1;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub volume: u64,
    pub id: [u8; 16],
}
impl FileIdentity {
    pub fn digest(&self) -> Result<Hash> {
        Ok(digest(b"storage-file-identity-v1", &canonical(self)?))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredCompletion {
    pub sequence: u64,
    pub received_utc_ms: u64,
    pub outcome: Completion,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredCall {
    pub permit: Permission,
    pub intent_sequence: u64,
    pub intent_utc_ms: u64,
    pub completion: Option<StoredCompletion>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredState {
    Preparing,
    ManifestReady,
    Armed,
    MutationPossible,
    Recovering,
    RecoveryRequired,
    AbortedNoWrite,
    RestoredVerified,
}
impl StoredState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::AbortedNoWrite | Self::RestoredVerified)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmCommit {
    pub boot_ms: u64,
    pub utc_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredConsent {
    pub approval: SimulatedApproval,
    pub boot_ms: u64,
    pub utc_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryWindow {
    pub trigger_ms: u64,
    pub deadline_ms: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeCode {
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
    IngressOverflow,
    NormalRecoveryMissed,
    NotificationFailed,
    PublicationUncertain,
    ReconciliationRequired,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredNotice {
    pub sequence: u64,
    pub boot_ms: u64,
    pub utc_ms: u64,
    pub code: NoticeCode,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredSession {
    pub preparation: Preparation,
    pub manifest: Option<Manifest>,
    pub previous_terminal: Option<Hash>,
    pub nonce_consumed: bool,
    pub consent: Option<StoredConsent>,
    pub arm_commit: Option<ArmCommit>,
    pub recovery: Option<RecoveryWindow>,
    pub state: StoredState,
    pub notices: Vec<StoredNotice>,
    pub calls: Vec<StoredCall>,
}
impl StoredSession {
    pub fn capture_trace(&self) -> Result<Hash> {
        Ok(digest(b"stored-capture-v1", &canonical(&self.calls)?))
    }
    pub fn terminal_digest(&self) -> Result<Hash> {
        if !self.state.terminal() {
            return Err(Rejection::Invalid("not terminal"));
        }
        Ok(digest(b"stored-terminal-v1", &canonical(self)?))
    }
    fn validate(&self, sequence: u64) -> Result<()> {
        let mut last_notice = 0;
        for notice in &self.notices {
            if notice.sequence == 0 || notice.sequence < last_notice || notice.sequence > sequence {
                return Err(Rejection::Sequence);
            }
            last_notice = notice.sequence;
        }
        let prep = self.preparation.digest()?;
        let final_digest = self.manifest.as_ref().map(Manifest::digest).transpose()?;
        if self
            .manifest
            .as_ref()
            .is_some_and(|m| m.preparation != self.preparation)
            || (self.manifest.is_none()
                && !matches!(
                    self.state,
                    StoredState::Preparing
                        | StoredState::AbortedNoWrite
                        | StoredState::RecoveryRequired
                ))
            || (self.nonce_consumed && self.manifest.is_none())
            || self.nonce_consumed != self.consent.is_some()
            || (self.state == StoredState::Preparing && self.manifest.is_some())
            || (self.state == StoredState::ManifestReady && self.arm_commit.is_some())
            || (self.recovery.is_some()
                && !matches!(
                    self.state,
                    StoredState::Recovering
                        | StoredState::RecoveryRequired
                        | StoredState::RestoredVerified
                ))
            || (matches!(
                self.state,
                StoredState::Armed
                    | StoredState::MutationPossible
                    | StoredState::Recovering
                    | StoredState::RestoredVerified
            ) && !self.nonce_consumed)
            || (matches!(
                self.state,
                StoredState::Armed
                    | StoredState::MutationPossible
                    | StoredState::Recovering
                    | StoredState::RestoredVerified
            ) && self.arm_commit.is_none())
            || (matches!(
                self.state,
                StoredState::Recovering | StoredState::RestoredVerified
            ) && self.recovery.is_none())
        {
            return Err(Rejection::Binding);
        }
        if let Some(consent) = &self.consent {
            consent.approval.validate(
                self.manifest.as_ref().ok_or(Rejection::Binding)?,
                consent.boot_ms,
                consent.utc_ms,
            )?;
        }
        if let Some(arm) = &self.arm_commit {
            let m = self.manifest.as_ref().ok_or(Rejection::Binding)?;
            if !self.nonce_consumed
                || arm.boot_ms < m.capture.sampled_ms[1]
                || arm.boot_ms
                    >= m.capture.sampled_ms[1]
                        .checked_add(CAPTURE_FRESH_MS)
                        .ok_or(Rejection::Expired)?
                || arm.utc_ms >= m.approval_expiry_utc_ms
                || self
                    .consent
                    .as_ref()
                    .is_none_or(|c| arm.boot_ms < c.boot_ms || arm.utc_ms < c.utc_ms)
            {
                return Err(Rejection::Expired);
            }
        }
        if let Some(recovery) = &self.recovery {
            if recovery.deadline_ms <= recovery.trigger_ms
                || recovery.deadline_ms
                    > recovery
                        .trigger_ms
                        .checked_add(CLEANUP_MS)
                        .ok_or(Rejection::Expired)?
            {
                return Err(Rejection::Expired);
            }
        }
        let mut last = 0;
        let mut op = 0;
        let mut pending = false;
        let mut candidates = 0;
        let mut restores = 0;
        let mut entered_setter = false;
        let mut possible_setter = false;
        let mut baseline_reads = Vec::new();
        let mut restore_return = None;
        let mut previous_return = 0;
        for call in &self.calls {
            let p = &call.permit;
            if pending
                || call.intent_sequence <= last
                || call.intent_sequence > sequence
                || p.operation_id <= op
                || p.session != self.preparation.session
                || p.boot != self.preparation.boot
                || (p.digest != prep && Some(p.digest) != final_digest)
                || (p.operation.setter()
                    && (Some(p.digest) != final_digest || !self.nonce_consumed))
                || (p.digest == prep && p.operation.setter())
                || p.dispatched_ms.checked_add(WATCHDOG_MS) != Some(p.watchdog_ms)
                || p.expiry_ms > p.watchdog_ms
                || p.expiry_ms <= p.dispatched_ms
                || p.dispatched_ms < previous_return
            {
                return Err(Rejection::Invalid("stored intent"));
            }
            if p.operation == Operation::CandidateOnce {
                let arm = self.arm_commit.as_ref().ok_or(Rejection::Binding)?;
                let m = self.manifest.as_ref().ok_or(Rejection::Binding)?;
                if p.dispatched_ms < arm.boot_ms
                    || p.expiry_ms
                        > arm
                            .boot_ms
                            .checked_add(CANDIDATE_MS)
                            .ok_or(Rejection::Expired)?
                    || p.expiry_ms
                        > m.capture.sampled_ms[1]
                            .checked_add(CAPTURE_FRESH_MS)
                            .ok_or(Rejection::Expired)?
                    || matches!(
                        self.state,
                        StoredState::Preparing | StoredState::ManifestReady | StoredState::Armed
                    )
                {
                    return Err(Rejection::Binding);
                }
            }
            if p.operation == Operation::RestoreBaseline {
                let recovery = self.recovery.as_ref().ok_or(Rejection::Binding)?;
                let grant_trigger = p.recovery_trigger_ms.ok_or(Rejection::Binding)?;
                if grant_trigger < recovery.trigger_ms
                    || grant_trigger > p.dispatched_ms
                    || p.expiry_ms
                        > grant_trigger
                            .checked_add(CLEANUP_MS)
                            .ok_or(Rejection::Expired)?
                    || !matches!(
                        self.state,
                        StoredState::Recovering
                            | StoredState::RecoveryRequired
                            | StoredState::RestoredVerified
                    )
                {
                    return Err(Rejection::Binding);
                }
            }
            candidates += usize::from(p.operation == Operation::CandidateOnce);
            restores += usize::from(p.operation == Operation::RestoreBaseline);
            last = call.intent_sequence;
            op = p.operation_id;
            pending = call.completion.is_none();
            possible_setter |= p.operation.setter()
                && !matches!(
                    call.completion,
                    Some(StoredCompletion {
                        outcome: Completion::NotEntered { .. },
                        ..
                    })
                );
            if let Some(completion) = &call.completion {
                if completion.sequence <= last || completion.sequence > sequence {
                    return Err(Rejection::Sequence);
                }
                last = completion.sequence;
                if let Completion::Returned {
                    entry_ms,
                    return_ms,
                    result,
                } = &completion.outcome
                {
                    p.validate_entry(*entry_ms, &self.preparation.boot, true)?;
                    if return_ms < entry_ms {
                        return Err(Rejection::Invalid("stored return"));
                    }
                    previous_return = *return_ms;
                    entered_setter |= p.operation.setter();
                    if p.operation == Operation::RestoreBaseline {
                        if !matches!(result, RawResult::Api { .. }) {
                            return Err(Rejection::Invalid("restore result"));
                        }
                        restore_return = Some(*return_ms);
                        baseline_reads.clear();
                    } else if p.operation == Operation::Current && restore_return.is_some() {
                        if matches!(result, RawResult::Value(v) if Some(*v) == self.manifest.as_ref().map(|m| m.capture.readings_mw[0]))
                        {
                            baseline_reads.push((*entry_ms, *return_ms));
                        } else {
                            baseline_reads.clear();
                        }
                    }
                }
            }
        }
        if candidates > 1
            || restores > 2
            || (self.state.terminal() && pending)
            || (self.state == StoredState::AbortedNoWrite && (entered_setter || possible_setter))
            || (self.state == StoredState::RestoredVerified
                && (restore_return.is_none()
                    || baseline_reads.len() != 2
                    || baseline_reads[1]
                        .0
                        .checked_sub(baseline_reads[0].1)
                        .is_none_or(|d| d < VERIFY_SEPARATION_MS)
                    || self.recovery.as_ref().is_none_or(|r| {
                        baseline_reads
                            .last()
                            .is_none_or(|(_, time)| *time >= r.deadline_ms)
                    })))
        {
            return Err(Rejection::Invalid("stored closure"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSnapshot {
    pub version: u32,
    pub store: Hash,
    pub sequence: u64,
    pub previous: Hash,
    pub consumed_nonces: Vec<Hash>,
    pub sessions: Vec<StoredSession>,
}
impl StorageSnapshot {
    pub fn validate(&self) -> Result<()> {
        if self.version != STORAGE_VERSION {
            return Err(Rejection::Version);
        }
        if self.sequence == 0
            || self.sessions.is_empty()
            || self.sessions.len() > HISTORY_LIMIT
            || self.consumed_nonces.len() > HISTORY_LIMIT
            || canonical(self)?.len() > SNAPSHOT_LIMIT
        {
            return Err(Rejection::Invalid("storage limits"));
        }
        let mut terminal = None;
        let mut consumed = Vec::new();
        for (i, s) in self.sessions.iter().enumerate() {
            s.validate(self.sequence)?;
            if s.previous_terminal != terminal
                || (i > 0 && terminal.is_none())
                || self.sessions[..i].iter().any(|old| {
                    old.preparation.nonce == s.preparation.nonce
                        || old.preparation.session == s.preparation.session
                })
            {
                return Err(Rejection::Binding);
            }
            if s.nonce_consumed {
                consumed.push(s.preparation.nonce);
            }
            terminal = if s.state.terminal() {
                Some(s.terminal_digest()?)
            } else {
                None
            };
        }
        if consumed != self.consumed_nonces {
            return Err(Rejection::Binding);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum JournalPayload {
    Anchor { version: u32, store: Hash },
    Snapshot(StorageSnapshot),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEnvelope {
    pub payload: JournalPayload,
    pub checksum: Hash,
}
impl JournalEnvelope {
    pub fn new(payload: JournalPayload) -> Result<Self> {
        let envelope = Self {
            checksum: digest(b"journal-v1", &canonical(&payload)?),
            payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }
    pub fn validate(&self) -> Result<()> {
        if self.checksum != digest(b"journal-v1", &canonical(&self.payload)?) {
            return Err(Rejection::Binding);
        }
        match &self.payload {
            JournalPayload::Anchor { version, store } => {
                if *version != STORAGE_VERSION || *store == [0; 32] {
                    return Err(Rejection::Version);
                }
            }
            JournalPayload::Snapshot(s) => s.validate()?,
        }
        if canonical(self)?.len() > SNAPSHOT_LIMIT {
            return Err(Rejection::Oversize);
        }
        Ok(())
    }
    pub fn sequence(&self) -> u64 {
        match &self.payload {
            JournalPayload::Anchor { .. } => 0,
            JournalPayload::Snapshot(s) => s.sequence,
        }
    }
    pub fn store(&self) -> Hash {
        match &self.payload {
            JournalPayload::Anchor { store, .. } => *store,
            JournalPayload::Snapshot(s) => s.store,
        }
    }
    pub fn read_only_required(&self) -> bool {
        matches!(&self.payload, JournalPayload::Snapshot(s) if s.sessions.last().is_some_and(|s| !s.state.terminal()))
    }
    pub fn follows(&self, previous: &Self) -> Result<()> {
        self.validate()?;
        previous.validate()?;
        let JournalPayload::Snapshot(next) = &self.payload else {
            return Err(Rejection::Sequence);
        };
        if next.store != previous.store()
            || previous.sequence().checked_add(1) != Some(next.sequence)
            || next.previous != previous.checksum
        {
            return Err(Rejection::Binding);
        }
        if let JournalPayload::Snapshot(old) = &previous.payload {
            if next.sessions.len() < old.sessions.len()
                || next.sessions.len() > old.sessions.len() + 1
                || !next.consumed_nonces.starts_with(&old.consumed_nonces)
            {
                return Err(Rejection::Binding);
            }
            for (i, before) in old.sessions.iter().enumerate() {
                let after = &next.sessions[i];
                if before.state.terminal() {
                    if before != after {
                        return Err(Rejection::Binding);
                    }
                    continue;
                }
                if before.preparation != after.preparation
                    || before.previous_terminal != after.previous_terminal
                    || (before.manifest.is_some() && before.manifest != after.manifest)
                    || (before.nonce_consumed && !after.nonce_consumed)
                    || (before.arm_commit.is_some() && before.arm_commit != after.arm_commit)
                    || (before.consent.is_some() && before.consent != after.consent)
                    || !after.notices.starts_with(&before.notices)
                    || after.notices[before.notices.len().min(after.notices.len())..]
                        .iter()
                        .any(|n| n.sequence != next.sequence)
                    || (before.arm_commit.is_none()
                        && after.arm_commit.is_some()
                        && !before.nonce_consumed)
                    || before.recovery.as_ref().is_some_and(|old| {
                        after.recovery.as_ref().is_none_or(|new| {
                            new.trigger_ms > old.trigger_ms || new.deadline_ms > old.deadline_ms
                        })
                    })
                    || after.calls.len() < before.calls.len()
                    || after.calls.len() > before.calls.len() + 1
                {
                    return Err(Rejection::Binding);
                }
                if before.manifest.is_none() {
                    if let Some(m) = &after.manifest {
                        if m.capture.trace != before.capture_trace()?
                            || before.calls != after.calls
                            || before.calls.iter().any(|c| c.completion.is_none())
                            || before.calls.len() != 6
                            || !valid_chain(
                                &before.calls[..5],
                                m,
                                true,
                                before.preparation.digest()?,
                            )?
                        {
                            return Err(Rejection::Binding);
                        }
                        let values: Vec<_> = before
                            .calls
                            .iter()
                            .filter_map(|c| match (&c.permit.operation, &c.completion) {
                                (
                                    Operation::Current,
                                    Some(StoredCompletion {
                                        outcome:
                                            Completion::Returned {
                                                return_ms,
                                                result: RawResult::Value(value),
                                                ..
                                            },
                                        ..
                                    }),
                                ) => Some((*return_ms, *value)),
                                _ => None,
                            })
                            .collect();
                        if values
                            != vec![
                                (m.capture.sampled_ms[0], m.capture.readings_mw[0]),
                                (m.capture.sampled_ms[1], m.capture.readings_mw[1]),
                            ]
                        {
                            return Err(Rejection::Binding);
                        }
                    }
                }
                for (j, call) in before.calls.iter().enumerate() {
                    let new = &after.calls[j];
                    if call.permit != new.permit
                        || call.intent_sequence != new.intent_sequence
                        || call.intent_utc_ms != new.intent_utc_ms
                        || (call.completion.is_some() && call.completion != new.completion)
                        || (call.completion.is_none()
                            && new
                                .completion
                                .as_ref()
                                .is_some_and(|c| c.sequence != next.sequence))
                    {
                        return Err(Rejection::Binding);
                    }
                }
                if after.calls.len() > before.calls.len()
                    && after.calls.last().is_some_and(|c| {
                        c.intent_sequence != next.sequence || c.completion.is_some()
                    })
                {
                    return Err(Rejection::Sequence);
                }
                if after.calls.len() > before.calls.len() {
                    let call = after.calls.last().ok_or(Rejection::Binding)?;
                    if call.permit.operation.setter() {
                        let manifest = after.manifest.as_ref().ok_or(Rejection::Binding)?;
                        if !valid_chain(
                            &after.calls[..after.calls.len() - 1],
                            manifest,
                            call.permit.operation == Operation::CandidateOnce,
                            manifest.digest()?,
                        )? {
                            return Err(Rejection::Invalid(
                                "setter requires explicit fresh validation chain",
                            ));
                        }
                    }
                    if call.permit.operation == Operation::RestoreBaseline {
                        let recovery = after.recovery.as_ref().ok_or(Rejection::Binding)?;
                        if call.permit.expiry_ms > recovery.deadline_ms
                            || call.permit.recovery_trigger_ms != Some(recovery.trigger_ms)
                        {
                            return Err(Rejection::Binding);
                        }
                    }
                }
                if !allowed_transition(before.state, after.state) {
                    return Err(Rejection::Invalid("state regression"));
                }
            }
            if next.sessions.len() > old.sessions.len() {
                require_opening(next.sessions.last().ok_or(Rejection::Binding)?)?;
            }
        } else {
            if next.sessions.len() != 1 {
                return Err(Rejection::Binding);
            }
            require_opening(&next.sessions[0])?;
        }
        Ok(())
    }
}
fn require_opening(s: &StoredSession) -> Result<()> {
    if s.state != StoredState::Preparing
        || s.manifest.is_some()
        || s.nonce_consumed
        || s.consent.is_some()
        || s.arm_commit.is_some()
        || s.recovery.is_some()
        || !s.notices.is_empty()
        || !s.calls.is_empty()
    {
        return Err(Rejection::Invalid(
            "preparation publication must precede capture/manifest",
        ));
    }
    Ok(())
}
fn allowed_transition(a: StoredState, b: StoredState) -> bool {
    use StoredState::*;
    a == b
        || matches!(
            (a, b),
            (Preparing, ManifestReady | AbortedNoWrite | RecoveryRequired)
                | (ManifestReady, Armed | AbortedNoWrite | RecoveryRequired)
                | (Armed, MutationPossible | AbortedNoWrite | RecoveryRequired)
                | (MutationPossible, Recovering | RecoveryRequired)
                | (Recovering, RestoredVerified | RecoveryRequired)
        )
}
fn valid_chain(
    calls: &[StoredCall],
    m: &Manifest,
    candidate: bool,
    expected_digest: Hash,
) -> Result<bool> {
    if calls.len() < 5 {
        return Ok(false);
    }
    let chain = &calls[calls.len() - 5..];
    let expected = [
        Operation::Enumerate,
        Operation::Identity,
        Operation::Runtime,
        Operation::Bounds,
        Operation::Current,
    ];
    for (call, op) in chain.iter().zip(expected) {
        if call.permit.operation != op || call.permit.digest != expected_digest {
            return Ok(false);
        }
        let Some(StoredCompletion {
            outcome: Completion::Returned { result, .. },
            ..
        }) = &call.completion
        else {
            return Ok(false);
        };
        let valid = match (op, result) {
            (Operation::Enumerate | Operation::Identity, RawResult::Identity(device)) => {
                device == &m.preparation.device
            }
            (Operation::Runtime, RawResult::Runtime(runtime)) => runtime == &m.runtime,
            (
                Operation::Bounds,
                RawResult::Bounds {
                    minimum_mw,
                    maximum_mw,
                },
            ) => *minimum_mw == m.capture.minimum_mw && *maximum_mw == m.capture.maximum_mw,
            (Operation::Current, RawResult::Value(value)) => {
                if candidate {
                    *value == m.capture.baseline()?
                } else {
                    (m.capture.minimum_mw..=m.capture.maximum_mw).contains(value)
                }
            }
            _ => false,
        };
        if !valid {
            return Ok(false);
        }
    }
    Ok(true)
}
