use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reading<T> {
    Available(T),
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub uuid: String,
    pub pci: String,
    pub vendor: u32,
    pub device: u32,
    pub subsystem: u32,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    pub min_mw: u32,
    pub max_mw: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timers {
    pub freshness_ms: u64,
    pub lease_ms: u64,
    pub admission_ms: u64,
    pub watchdog_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub heartbeat_loss_ms: u64,
    pub recovery_request_ms: u64,
    pub verification_gap_ms: u64,
    pub recovery_budget_ms: u64,
}

impl Default for Timers {
    fn default() -> Self {
        Self {
            freshness_ms: 60_000,
            lease_ms: 10_000,
            admission_ms: 2_000,
            watchdog_ms: 1_000,
            heartbeat_interval_ms: 250,
            heartbeat_loss_ms: 1_000,
            recovery_request_ms: 3_000,
            verification_gap_ms: 250,
            recovery_budget_ms: 5_000,
        }
    }
}

/// Every field is included in canonical serialization and the consent digest.
/// Engine holds a private clone, so callers cannot edit an armed contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub experiment: String,
    pub nonce: String,
    pub protocol_digest: String,
    pub executable_digest: String,
    pub guardian_digest: String,
    pub worker_digest: String,
    pub approval_contract_digest: String,
    pub approval_expiry_utc_ms: u64,
    pub operator: String,
    pub identity: Identity,
    pub windows_build: String,
    pub driver_version: String,
    pub nvml_version: String,
    pub header_source: String,
    pub trusted_dll_path: String,
    pub privilege: String,
    pub baseline_readings_mw: [Reading<u32>; 2],
    pub capture_mono_ms: [u64; 2],
    pub capture_utc_ms: [u64; 2],
    pub baseline_mw: u32,
    pub bounds: Bounds,
    pub default_mw: Reading<u32>,
    pub target_mw: u32,
    pub delta_mw: u32,
    pub setter: String,
    pub getter: String,
    pub candidate_attempts: u32,
    pub restore_attempts: u32,
    pub timers: Timers,
    pub guardian_identity: String,
    pub parent_identity: String,
    pub worker_identity: String,
    pub boot_identity: String,
    pub journal_location: String,
    pub acl_expectations: String,
    pub ownership_inspection: String,
    pub other_tools_stopped: bool,
    pub risk_acknowledgement: String,
    pub supervised_escalation: String,
    pub provenance: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    InvalidManifest,
    InvalidBounds,
    InvalidCapture,
    InvalidReduction,
    MissingConsent,
    ConsentMismatch,
    WrongOperator,
    ExpiredConsent,
    ClockDiscontinuity,
    ReplayedNonce,
    JournalBlocked,
    Storage,
    GuardianUnavailable,
    Identity,
    Ownership,
    Drift,
    Symbols,
    AdmissionClosed,
    InvalidCommand,
}

impl Manifest {
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("manifest contains only serializable values");
        format!("{:x}", Sha256::digest(bytes))
    }

    pub fn validate(&self) -> Result<(), Rejection> {
        let required = [
            &self.experiment,
            &self.nonce,
            &self.protocol_digest,
            &self.executable_digest,
            &self.guardian_digest,
            &self.worker_digest,
            &self.approval_contract_digest,
            &self.operator,
            &self.windows_build,
            &self.driver_version,
            &self.nvml_version,
            &self.header_source,
            &self.trusted_dll_path,
            &self.privilege,
            &self.guardian_identity,
            &self.parent_identity,
            &self.worker_identity,
            &self.boot_identity,
            &self.journal_location,
            &self.acl_expectations,
            &self.ownership_inspection,
            &self.risk_acknowledgement,
            &self.supervised_escalation,
        ];
        if self.schema != 1
            || self.provenance != "simulated"
            || required.iter().any(|s| s.is_empty())
            || !valid_identity(&self.identity)
            || !self.other_tools_stopped
            || self.timers != Timers::default()
            || self.candidate_attempts != 1
            || self.restore_attempts != 2
            || self.setter != "nvmlDeviceSetPowerManagementLimit"
            || self.getter != "nvmlDeviceGetPowerManagementLimit"
        {
            return Err(Rejection::InvalidManifest);
        }
        if self.bounds.min_mw == 0
            || self.bounds.max_mw == u32::MAX
            || self.bounds.min_mw >= self.bounds.max_mw
            || !(self.bounds.min_mw..=self.bounds.max_mw).contains(&self.baseline_mw)
        {
            return Err(Rejection::InvalidBounds);
        }
        if self.baseline_readings_mw
            != [
                Reading::Available(self.baseline_mw),
                Reading::Available(self.baseline_mw),
            ]
            || self.capture_mono_ms[0].checked_add(500) != Some(self.capture_mono_ms[1])
            || self.capture_utc_ms[0].checked_add(500) != Some(self.capture_utc_ms[1])
        {
            return Err(Rejection::InvalidCapture);
        }
        if !(1000..=5000).contains(&self.delta_mw)
            || self.baseline_mw.checked_sub(self.delta_mw) != Some(self.target_mw)
            || self.target_mw < self.bounds.min_mw
        {
            return Err(Rejection::InvalidReduction);
        }
        Ok(())
    }

    pub fn fresh(&self, mono: u64, utc: u64) -> Result<(), Rejection> {
        let elapsed = mono
            .checked_sub(self.capture_mono_ms[1])
            .ok_or(Rejection::ClockDiscontinuity)?;
        if self.capture_utc_ms[1].checked_add(elapsed) != Some(utc) {
            return Err(Rejection::ClockDiscontinuity);
        }
        if elapsed >= self.timers.freshness_ms || utc >= self.approval_expiry_utc_ms {
            return Err(Rejection::ExpiredConsent);
        }
        Ok(())
    }
}

pub fn valid_identity(i: &Identity) -> bool {
    let p = i.pci.as_bytes();
    !i.uuid.is_empty()
        && !i.model.is_empty()
        && i.vendor != 0
        && i.device != 0
        && i.vendor < 0xffff
        && i.device < 0xffff
        && i.subsystem != 0
        && i.subsystem != u32::MAX
        && p.len() == 12
        && p[4] == b':'
        && p[7] == b':'
        && p[10] == b'.'
        && p.iter().enumerate().all(|(n, c)| {
            [4, 7, 10].contains(&n) || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()
        })
        && p[11] <= b'7'
        && u8::from_str_radix(&i.pci[8..10], 16).is_ok_and(|d| d <= 31)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Consent {
    pub manifest_digest: String,
    pub nonce: String,
    pub operator: String,
    pub simulated_exact_interaction: bool,
}

impl Consent {
    pub fn simulated(m: &Manifest) -> Self {
        Self {
            manifest_digest: m.digest(),
            nonce: m.nonce.clone(),
            operator: m.operator.clone(),
            simulated_exact_interaction: true,
        }
    }
    pub fn validate(&self, m: &Manifest) -> Result<(), Rejection> {
        if !self.simulated_exact_interaction
            || self.manifest_digest != m.digest()
            || self.nonce != m.nonce
        {
            return Err(Rejection::ConsentMismatch);
        }
        if self.operator != m.operator {
            return Err(Rejection::WrongOperator);
        }
        Ok(())
    }
}
