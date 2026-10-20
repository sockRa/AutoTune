//! Fake-only recovery contracts. No hardware authority is represented here.
#![forbid(unsafe_code)]

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub mod process;
pub mod storage;

pub const VERSION: u32 = 1;
pub const FRAME_LIMIT: usize = 64 * 1024;
pub const SNAPSHOT_LIMIT: usize = 4 * 1024 * 1024;
pub const ARTIFACT_LIMIT: usize = 1024;
pub const HISTORY_LIMIT: usize = 256;
pub const EVENT_LIMIT: usize = 64;
pub const EVENTS_PER_TURN: usize = 8;
pub const PUBLICATION_MS: u64 = 100;
pub const WATCHDOG_MS: u64 = 1000;
pub const CAPTURE_SEPARATION_MS: u64 = 500;
pub const CAPTURE_FRESH_MS: u64 = 60_000;
pub const LEASE_MS: u64 = 10_000;
pub const CANDIDATE_MS: u64 = 2000;
pub const NORMAL_RECOVERY_MS: u64 = 3000;
pub const HEARTBEAT_MS: u64 = 250;
pub const HEARTBEAT_LOSS_MS: u64 = 1000;
pub const CLEANUP_MS: u64 = 5000;
pub const VERIFY_SEPARATION_MS: u64 = 250;
pub type Hash = [u8; 32];
pub type Session = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Encoding,
    NonCanonical,
    Oversize,
    Version,
    Invalid(&'static str),
    Authentication,
    Sequence,
    Expired,
    Binding,
}
pub type Result<T> = std::result::Result<T, Rejection>;

pub fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| Rejection::Encoding)
}

/// Fixed-field structs only: callers must not use maps or floats as wire types.
pub fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8], limit: usize) -> Result<T> {
    if bytes.len() > limit {
        return Err(Rejection::Oversize);
    }
    let value: T = serde_json::from_slice(bytes).map_err(|_| Rejection::Encoding)?;
    if canonical(&value)? != bytes {
        return Err(Rejection::NonCanonical);
    }
    Ok(value)
}

pub fn digest(domain: &[u8], bytes: &[u8]) -> Hash {
    let mut hash = Sha256::new();
    hash.update(digest_prefix(domain));
    hash.update(bytes);
    hash.finalize().into()
}
pub fn digest_prefix(domain: &[u8]) -> Vec<u8> {
    let mut bytes = b"AutoTune/recovery/v1\0".to_vec();
    bytes.extend((domain.len() as u64).to_le_bytes());
    bytes.extend(domain);
    bytes
}

/// RFC 2104 HMAC-SHA256 for portable protocol vectors. Windows keys come from CNG.
pub fn mac(key: &Hash, transcript: &[u8]) -> Hash {
    let mut inner = [0x36; 64];
    let mut outer = [0x5c; 64];
    for i in 0..32 {
        inner[i] ^= key[i];
        outer[i] ^= key[i];
    }
    let mut h = Sha256::new();
    h.update(inner);
    h.update(transcript);
    let mut o = Sha256::new();
    o.update(outer);
    o.update(h.finalize());
    o.finalize().into()
}

pub fn constant_time_equal(a: &Hash, b: &Hash) -> bool {
    a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_100ns: u64,
    pub binary: Hash,
    pub logon_sid: String,
    pub integrity: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub uuid: String,
    pub pci: String,
    pub vendor: u32,
    pub device: u32,
    pub subsystem: u32,
    pub model: String,
}

impl Device {
    pub fn validate(&self) -> Result<()> {
        let pci = self.pci.as_bytes();
        if !self.uuid.starts_with("SYNTHETIC-") || self.uuid.len() > 128 {
            return Err(Rejection::Invalid("synthetic UUID"));
        }
        if pci.len() != 12 || pci[4] != b':' || pci[7] != b':' || pci[10] != b'.' {
            return Err(Rejection::Invalid("normalized PCI"));
        }
        for (i, c) in pci.iter().enumerate() {
            if ![4, 7, 10].contains(&i) && !(c.is_ascii_digit() || (b'a'..=b'f').contains(c)) {
                return Err(Rejection::Invalid("normalized PCI"));
            }
        }
        if pci[11] > b'7'
            || u8::from_str_radix(&self.pci[8..10], 16).unwrap_or(255) > 31
            || self.model.is_empty()
        {
            return Err(Rejection::Invalid("PCI or model"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    pub version: u32,
    pub session: Session,
    pub nonce: Hash,
    pub device: Device,
    pub protocol: Hash,
    pub executables: [Hash; 3],
    pub operator: String,
    pub boot: Hash,
    pub created_ms: u64,
    pub synthetic: bool,
}
impl Preparation {
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            return Err(Rejection::Version);
        }
        if !self.synthetic || self.operator.is_empty() || self.nonce == [0; 32] {
            return Err(Rejection::Invalid("preparation"));
        }
        self.device.validate()
    }
    pub fn digest(&self) -> Result<Hash> {
        self.validate()?;
        Ok(digest(b"preparation", &canonical(self)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub readings_mw: [u32; 2],
    pub sampled_ms: [u64; 2],
    pub trace: Hash,
    pub minimum_mw: u32,
    pub maximum_mw: u32,
    pub default_mw: Option<u32>,
}
impl Capture {
    pub fn baseline(&self) -> Result<u32> {
        let b = self.readings_mw[0];
        if b == 0
            || b == u32::MAX
            || self.readings_mw[1] != b
            || self.minimum_mw == 0
            || self.minimum_mw >= self.maximum_mw
            || self.maximum_mw == u32::MAX
            || !(self.minimum_mw..=self.maximum_mw).contains(&b)
            || self.sampled_ms[1]
                .checked_sub(self.sampled_ms[0])
                .is_none_or(|gap| gap < CAPTURE_SEPARATION_MS)
        {
            return Err(Rejection::Invalid("capture"));
        }
        Ok(b)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub windows_build: String,
    pub driver: String,
    pub api_version: String,
    pub header: String,
    pub module_path: String,
    pub privilege: String,
    pub setter: String,
    pub getter: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Supervision {
    pub guardian: ProcessIdentity,
    pub coordinator: ProcessIdentity,
    pub worker: ProcessIdentity,
    pub root: String,
    pub acl_expectation: Hash,
    pub lock_identity: Hash,
    pub other_tools_stopped_simulated: bool,
    pub escalation: String,
    pub risks: String,
}

/// All policy timers are fixed by protocol v1; changing them requires a new version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub preparation: Preparation,
    pub preparation_digest: Hash,
    pub capture: Capture,
    pub target_mw: u32,
    pub delta_mw: u32,
    pub runtime: Runtime,
    pub supervision: Supervision,
    pub approval_expiry_utc_ms: u64,
    pub approval_challenge: Hash,
    pub policy_version: u32,
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.preparation.digest()? != self.preparation_digest {
            return Err(Rejection::Binding);
        }
        let b = self.capture.baseline()?;
        if !(1000..=5000).contains(&self.delta_mw)
            || b.checked_sub(self.delta_mw) != Some(self.target_mw)
            || self.target_mw < self.capture.minimum_mw
            || self.policy_version != VERSION
            || self.capture.sampled_ms[0] < self.preparation.created_ms
            || !self.supervision.other_tools_stopped_simulated
            || self.runtime.setter != "synthetic-set-configured-limit"
            || self.runtime.getter != "synthetic-get-configured-limit"
            || self.runtime.privilege != "unelevated-synthetic"
            || !self.runtime.module_path.starts_with("synthetic:")
            || self.supervision.risks.is_empty()
            || self.supervision.escalation.is_empty()
            || self.approval_challenge == [0; 32]
        {
            return Err(Rejection::Invalid("manifest"));
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<Hash> {
        self.validate()?;
        Ok(digest(b"manifest", &canonical(self)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulatedApproval {
    pub manifest: Hash,
    pub nonce: Hash,
    pub challenge: Hash,
    pub operator: String,
    pub full_and_summary_presented: bool,
}
impl SimulatedApproval {
    pub fn validate(&self, manifest: &Manifest, now_ms: u64, utc_ms: u64) -> Result<()> {
        if self.manifest != manifest.digest()?
            || self.nonce != manifest.preparation.nonce
            || self.challenge != manifest.approval_challenge
            || self.operator != manifest.preparation.operator
            || !self.full_and_summary_presented
        {
            return Err(Rejection::Binding);
        }
        if utc_ms >= manifest.approval_expiry_utc_ms
            || now_ms < manifest.capture.sampled_ms[1]
            || now_ms
                >= manifest.capture.sampled_ms[1]
                    .checked_add(CAPTURE_FRESH_MS)
                    .ok_or(Rejection::Expired)?
        {
            return Err(Rejection::Expired);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Launcher,
    Verifier,
    Guardian,
    Coordinator,
    Worker,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Enumerate,
    Identity,
    Runtime,
    Bounds,
    Current,
    CandidateOnce,
    RestoreBaseline,
}
impl Operation {
    pub fn setter(self) -> bool {
        matches!(self, Self::CandidateOnce | Self::RestoreBaseline)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub version: u32,
    pub role: Role,
    pub session: Session,
    pub pipe: String,
    pub guardian: ProcessIdentity,
    pub child: ProcessIdentity,
    pub guardian_handle: u64,
    pub lock_handle: Option<u64>,
    pub lock_identity: Option<Hash>,
    pub key: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permission {
    pub session: Session,
    pub boot: Hash,
    pub digest: Hash,
    pub generation: u64,
    pub operation_id: u64,
    pub operation: Operation,
    pub dispatched_ms: u64,
    pub watchdog_ms: u64,
    pub expiry_ms: u64,
    pub recovery_trigger_ms: Option<u64>,
}
impl Permission {
    pub fn validate_entry(&self, now_ms: u64, boot: &Hash, guardian_alive: bool) -> Result<()> {
        if &self.boot != boot
            || !guardian_alive
            || self.operation_id == 0
            || self.dispatched_ms.checked_add(WATCHDOG_MS) != Some(self.watchdog_ms)
            || self.expiry_ms > self.watchdog_ms
            || now_ms < self.dispatched_ms
        {
            return Err(Rejection::Binding);
        }
        if now_ms >= self.expiry_ms {
            return Err(Rejection::Expired);
        }
        if self.operation == Operation::RestoreBaseline {
            let trigger = self.recovery_trigger_ms.ok_or(Rejection::Binding)?;
            if self.expiry_ms > trigger.checked_add(CLEANUP_MS).ok_or(Rejection::Expired)? {
                return Err(Rejection::Binding);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RawResult {
    Value(u32),
    Identity(Device),
    Bounds { minimum_mw: u32, maximum_mw: u32 },
    Runtime(Runtime),
    Api { status: i32 },
    Unavailable { reason: String, status: Option<i32> },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Completion {
    NotEntered {
        reason: String,
    },
    Returned {
        entry_ms: u64,
        return_ms: u64,
        result: RawResult,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Message {
    Prepare {
        capsule: Box<Preparation>,
        publication: Hash,
    },
    Prepared {
        digest: Hash,
        custody: Hash,
    },
    Seal(Box<Manifest>),
    Sealed {
        digest: Hash,
    },
    Arm(Box<process::ArmCapsule>),
    Dispatch(Box<process::DispatchProof>),
    Recorded(process::ResultReceipt),
    StorageFailed(storage::RecoveryWindow),
    Armed {
        commit_ms: u64,
    },
    Revoke {
        generation: u64,
        trigger_ms: u64,
        deadline_ms: u64,
    },
    Status(Box<process::RoleStatus>),
    Control(process::Control),
    Rejected {
        reason: String,
    },
    Challenge {
        local: Hash,
        remote: Hash,
        peer: ProcessIdentity,
    },
    Permit(Permission),
    ReturnAndIdle {
        permit: Permission,
        outcome: Box<Completion>,
    },
    Heartbeat,
    Cancel,
    /// Fake-harness completion report only; carries no execution authority.
    RoundTrip(Box<process::RoundTripReport>),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub version: u32,
    pub from: Role,
    pub to: Role,
    pub session: Session,
    pub digest: Hash,
    pub generation: u64,
    pub sequence: u64,
    pub operation_id: u64,
    pub message: Message,
}

pub fn encode_frame(frame: &Frame, key: &Hash) -> Result<Vec<u8>> {
    let payload = canonical(frame)?;
    let body_len = payload.len().checked_add(32).ok_or(Rejection::Oversize)?;
    if body_len + 4 > FRAME_LIMIT {
        return Err(Rejection::Oversize);
    }
    let mut bytes = (body_len as u32).to_le_bytes().to_vec();
    bytes.extend(payload);
    bytes.extend(mac(key, &bytes));
    Ok(bytes)
}

/// Per-direction sequence state. Failures consume no new authority.
pub struct Receiver {
    pub from: Role,
    pub to: Role,
    pub session: Session,
    pub digest: Hash,
    next: u64,
}
impl Receiver {
    /// Explicit authenticated preparation/seal handoff only; sequence never resets.
    pub fn bind_digest(&mut self, digest: Hash) {
        self.digest = digest;
    }
    pub fn new(from: Role, to: Role, session: Session, digest: Hash) -> Self {
        Self {
            from,
            to,
            session,
            digest,
            next: 1,
        }
    }
    pub fn receive(&mut self, bytes: &[u8], key: &Hash) -> Result<Frame> {
        if bytes.len() > FRAME_LIMIT || bytes.len() < 36 {
            return Err(Rejection::Oversize);
        }
        let len =
            u32::from_le_bytes(bytes[..4].try_into().map_err(|_| Rejection::Encoding)?) as usize;
        if len != bytes.len() - 4 {
            return Err(Rejection::Encoding);
        }
        let end = bytes.len() - 32;
        let tag: Hash = bytes[end..].try_into().map_err(|_| Rejection::Encoding)?;
        if !constant_time_equal(&tag, &mac(key, &bytes[..end])) {
            return Err(Rejection::Authentication);
        }
        let frame: Frame = decode(&bytes[4..end], FRAME_LIMIT)?;
        if frame.version != VERSION {
            return Err(Rejection::Version);
        }
        if frame.from != self.from
            || frame.to != self.to
            || frame.session != self.session
            || frame.digest != self.digest
        {
            return Err(Rejection::Binding);
        }
        if frame.sequence != self.next {
            return Err(Rejection::Sequence);
        }
        self.next = self.next.checked_add(1).ok_or(Rejection::Sequence)?;
        // Generation/permit matching belongs to the reducer, AFTER sequence acceptance.
        Ok(frame)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallRecord {
    pub permit: Permission,
    pub intent_sequence: u64,
    pub completion: Option<Completion>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    pub manifest: Manifest,
    pub previous_terminal: Option<Hash>,
    pub calls: Vec<CallRecord>,
    pub terminal: Option<Hash>,
    pub recovery_required: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub sequence: u64,
    pub previous: Option<Hash>,
    pub consumed_nonces: Vec<Hash>,
    pub sessions: Vec<SessionRecord>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            return Err(Rejection::Version);
        }
        if self.sequence == 0
            || self.sessions.is_empty()
            || self.sessions.len() > HISTORY_LIMIT
            || self.consumed_nonces.len() != self.sessions.len()
            || canonical(self)?.len() > SNAPSHOT_LIMIT
        {
            return Err(Rejection::Invalid("journal limits"));
        }
        let mut previous = None;
        for (i, session) in self.sessions.iter().enumerate() {
            session.manifest.validate()?;
            let nonce = session.manifest.preparation.nonce;
            if self.consumed_nonces[i] != nonce
                || self.consumed_nonces[..i].contains(&nonce)
                || session.previous_terminal != previous
                || (i > 0 && previous.is_none())
            {
                return Err(Rejection::Binding);
            }
            let mut last_op = 0;
            let mut pending = false;
            let mut candidate = 0;
            let mut restores = 0;
            let mut restored_at = None;
            let mut baseline_reads = Vec::new();
            let mut last_intent = 0;
            for call in &session.calls {
                if pending
                    || call.permit.operation_id <= last_op
                    || call.intent_sequence > self.sequence
                    || call.intent_sequence <= last_intent
                    || call.permit.session != session.manifest.preparation.session
                    || call.permit.boot != session.manifest.preparation.boot
                    || (call.permit.digest != session.manifest.digest()?
                        && (call.permit.operation.setter()
                            || call.permit.digest != session.manifest.preparation.digest()?))
                {
                    return Err(Rejection::Binding);
                }
                if let Some(Completion::Returned {
                    entry_ms,
                    return_ms,
                    result,
                }) = &call.completion
                {
                    call.permit.validate_entry(
                        *entry_ms,
                        &session.manifest.preparation.boot,
                        true,
                    )?;
                    if return_ms < entry_ms {
                        return Err(Rejection::Invalid("journal return time"));
                    }
                    if call.permit.operation == Operation::RestoreBaseline {
                        restored_at = Some(*return_ms);
                        baseline_reads.clear();
                    } else if call.permit.operation == Operation::Current && restored_at.is_some() {
                        if *result == RawResult::Value(session.manifest.capture.baseline()?) {
                            baseline_reads.push((*entry_ms, *return_ms));
                        } else {
                            baseline_reads.clear();
                        }
                    }
                }
                candidate += usize::from(call.permit.operation == Operation::CandidateOnce);
                restores += usize::from(call.permit.operation == Operation::RestoreBaseline);
                last_op = call.permit.operation_id;
                last_intent = call.intent_sequence;
                pending = call.completion.is_none();
            }
            if candidate > 1
                || restores > 2
                || (session.terminal.is_some() && (pending || session.recovery_required))
            {
                return Err(Rejection::Invalid("journal calls"));
            }
            if session.terminal.is_some() {
                if baseline_reads.len() != 2
                    || restored_at.is_none()
                    || baseline_reads[1]
                        .0
                        .checked_sub(baseline_reads[0].1)
                        .is_none_or(|gap| gap < VERIFY_SEPARATION_MS)
                {
                    return Err(Rejection::Invalid("terminal restoration evidence"));
                }
                let mut unsealed = session.clone();
                unsealed.terminal = None;
                if session.terminal != Some(digest(b"terminal-session", &canonical(&unsealed)?)) {
                    return Err(Rejection::Binding);
                }
            }
            previous = session.terminal;
        }
        Ok(())
    }
}
