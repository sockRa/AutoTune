//! Process/bootstrap v1. Private anonymous inherited section, never argv/env/files.
use crate::*;
use storage::{ArmCommit, FileIdentity, RecoveryWindow, StoredState};

pub const PROCESS_VERSION: u32 = 1;
pub const BOOTSTRAP_BYTES: usize = 16 * 1024;
pub const BOOTSTRAP_LIMIT: usize = 8 * 1024;
pub const BOOTSTRAP_ACCESS: u32 = 5; // SECTION_QUERY | SECTION_MAP_READ
pub const SUPERVISOR_ACCESS: u32 = 0x101000; // QUERY_LIMITED_INFORMATION | SYNCHRONIZE
pub const CUSTODY_ACCESS: u32 = 0x120080; // READ_CONTROL | READ_ATTRIBUTES | SYNCHRONIZE
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredProcess {
    pub process: ProcessIdentity,
    pub user_sid: String,
    pub image: FileIdentity,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelContext {
    pub version: u32,
    pub session: Session,
    pub supervisor_role: Role,
    pub supervisor: MeasuredProcess,
    pub child_role: Role,
    pub child: MeasuredProcess,
}
impl ChannelContext {
    pub fn validate(&self) -> Result<()> {
        if self.version != PROCESS_VERSION {
            return Err(Rejection::Version);
        }
        if !matches!(
            (self.supervisor_role, self.child_role),
            (Role::Launcher, Role::Guardian | Role::Verifier)
                | (Role::Guardian, Role::Worker | Role::Coordinator)
        ) || self.session == [0; 32]
            || self.supervisor.process.pid == self.child.process.pid
            || self.supervisor.process.binary != self.child.process.binary
            || self.supervisor.user_sid != self.child.user_sid
            || self.supervisor.process.logon_sid != self.child.process.logon_sid
            || self.supervisor.process.integrity != self.child.process.integrity
        {
            return Err(Rejection::Binding);
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<Hash> {
        self.validate()?;
        Ok(digest(b"process-channel-v1", &canonical(self)?))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustodyHandle {
    pub handle: u64,
    pub identity: FileIdentity,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InheritedHandles {
    pub bootstrap_map: u64,
    pub supervisor_query: u64,
    pub custody: Option<CustodyHandle>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessBootstrap {
    pub context: ChannelContext,
    pub handles: InheritedHandles,
    pub pipe: String,
    pub key: Hash,
    pub fixture_root: Option<String>,
    /// Explicit opt-in for an owned console-closure diagnostic, never normal work.
    pub console_test: bool,
    pub profile: FakeProfile,
}
impl std::fmt::Debug for ProcessBootstrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessBootstrap")
            .field("role", &self.context.child_role)
            .field("key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
impl ProcessBootstrap {
    pub fn validate(&self) -> Result<()> {
        self.context.validate()?;
        self.profile.validate()?;
        if self.key == [0; 32]
            || self.handles.bootstrap_map == 0
            || self.handles.supervisor_query == 0
            || self.handles.bootstrap_map == self.handles.supervisor_query
            || !self.pipe.starts_with(r"\\.\pipe\AutoTune-fake-v1-")
            || self
                .pipe
                .strip_prefix(r"\\.\pipe\AutoTune-fake-v1-")
                .is_none_or(|s| {
                    s.len() != 64
                        || !s
                            .bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
            || self.context.child_role == Role::Worker && self.handles.custody.is_none()
            || self.context.child_role != Role::Worker && self.handles.custody.is_some()
            || self.context.child_role == Role::Guardian && self.fixture_root.is_none()
            || self.context.child_role != Role::Guardian && self.fixture_root.is_some()
            || self.console_test && self.context.child_role == Role::Worker
        {
            return Err(Rejection::Binding);
        }
        if let Some(c) = &self.handles.custody {
            if c.handle == 0
                || c.handle == self.handles.bootstrap_map
                || c.handle == self.handles.supervisor_query
            {
                return Err(Rejection::Binding);
            }
        }
        if canonical(self)?.len() > BOOTSTRAP_LIMIT {
            return Err(Rejection::Oversize);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FakeBehavior {
    Normal,
    MissingCandidateApi,
    MissingRestoreApi,
    ErrorAfterChange,
    CancelAfterCandidate,
    StorageFailureAfterCandidate,
    WorkerKilledDuringCandidate,
    BlockedCandidateReturns,
    StorageFailureDuringBlockedCandidate,
    HeartbeatLossDuringBlockedCandidate,
    StorageStallDuringBlockedCandidate,
}
/// Test oracle invocation, emitted only after worker admission and entry checks.
/// It is not an execution permission or a ReturnAndIdle completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeInvocation {
    pub permit: Permission,
    pub device: Device,
    pub entry_ms: u64,
    pub write_mw: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessCall {
    pub invocation: FakeInvocation,
    pub effect_ms: Option<u64>,
    pub result: Option<RawResult>,
    pub worker_lost_before_effect: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeProfile {
    pub device: Device,
    pub runtime: Runtime,
    pub baseline_mw: u32,
    pub minimum_mw: u32,
    pub maximum_mw: u32,
    pub default_mw: Option<u32>,
    pub behavior: FakeBehavior,
}
impl FakeProfile {
    pub fn validate(&self) -> Result<()> {
        self.device.validate()?;
        if self.minimum_mw == 0
            || self.minimum_mw >= self.maximum_mw
            || self.maximum_mw == u32::MAX
            || !(self.minimum_mw..=self.maximum_mw).contains(&self.baseline_mw)
            || self.runtime.setter != "synthetic-set-configured-limit"
            || self.runtime.getter != "synthetic-get-configured-limit"
            || !self.runtime.module_path.starts_with("synthetic:")
            || self.runtime.privilege != "unelevated-synthetic"
        {
            return Err(Rejection::Invalid("fake profile"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmCapsule {
    pub approval: SimulatedApproval,
    pub commit: ArmCommit,
    pub nonce_publication: Hash,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchProof {
    pub permit: Permission,
    pub intent_sequence: u64,
    pub intent_utc_ms: u64,
    pub publication: Option<Hash>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultReceipt {
    pub operation_id: u64,
    pub sequence: u64,
    pub received_utc_ms: u64,
    pub publication: Option<Hash>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    Observe,
    Stop,
    TerminateCoordinator,
    CloseCoordinatorJob,
    CloseCoordinatorConsole,
    ExpiredGetter,
    DeniedCandidate,
    FullFakeRoundTrip,
    ProbeIsolation,
    Fault(WireFault),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFault {
    BadMac,
    Replay,
    SequenceGap,
    WrongRole,
    WrongDigest,
    Oversize,
    Truncated,
    UnknownField,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessCase {
    RoundTrip,
    PipeSquatter,
    WrongPeer,
    WrongMac,
    Wire(WireFault),
    JobClose,
    ConsoleClose,
    IsolationGate,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessCheckReport {
    pub case: ProcessCase,
    pub passed: bool,
    pub blocker: Option<String>,
    pub rejection: Option<String>,
    pub host_in_job: bool,
    pub host_job_flags: Option<u32>,
    pub child: Option<MeasuredProcess>,
    pub sibling: Option<MeasuredProcess>,
    pub zero_device_permissions: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundTripReport {
    pub in_call_controls_acknowledged: u64,
    pub guardian: MeasuredProcess,
    pub worker: MeasuredProcess,
    pub coordinator: MeasuredProcess,
    pub baseline_mw: u32,
    pub target_mw: u32,
    pub delta_mw: u32,
    pub candidate_entries: u32,
    pub restore_entries: u32,
    pub candidate_api: Option<i32>,
    pub candidate_readback: Option<u32>,
    pub restore_api: Vec<i32>,
    pub baseline_reads: Vec<(u64, u32)>,
    pub observed_restored: bool,
    pub publication_verified: bool,
    pub final_state: StoredState,
    pub journal_sequence: u64,
    pub cold_read_only: bool,
    pub cold_state: StoredState,
    pub fixture_root: String,
    pub pending: Option<Permission>,
    pub recovery: Option<RecoveryWindow>,
    pub witness_calls: Vec<WitnessCall>,
    pub witness_current_mw: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleStatus {
    pub role: Role,
    pub process: MeasuredProcess,
    pub no_job: bool,
    pub no_console: bool,
    pub console_window: u64,
    pub console_processes: Vec<u32>,
    pub worker: Option<MeasuredProcess>,
    pub coordinator: Option<MeasuredProcess>,
    pub candidate_entries: u32,
    pub restore_entries: u32,
    pub pending: Option<Permission>,
    pub recovery: Option<RecoveryWindow>,
}

/// Golden handshake frames: fresh parent challenge, child proof, parent proof.
/// MAC covers the normal complete shared frame; context digest binds both measured peers.
pub fn challenge(
    context: &ChannelContext,
    from: Role,
    sequence: u64,
    local: Hash,
    remote: Hash,
) -> Result<Frame> {
    let (to, peer) = if from == context.supervisor_role {
        (context.child_role, context.supervisor.process.clone())
    } else if from == context.child_role {
        (context.supervisor_role, context.child.process.clone())
    } else {
        return Err(Rejection::Binding);
    };
    Ok(Frame {
        version: VERSION,
        from,
        to,
        session: context.session,
        digest: context.digest()?,
        generation: 0,
        sequence,
        operation_id: 0,
        message: Message::Challenge {
            local,
            remote,
            peer,
        },
    })
}
pub fn verify_challenge(
    frame: &Frame,
    context: &ChannelContext,
    from: Role,
    local: Hash,
    remote: Hash,
) -> Result<()> {
    let expected = challenge(context, from, frame.sequence, local, remote)?;
    if frame != &expected || local == [0; 32] {
        return Err(Rejection::Binding);
    }
    Ok(())
}
