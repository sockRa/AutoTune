//! In-memory crash/durability model. Never a claim about filesystem guarantees.
use crate::manifest::{Bounds, Identity, Manifest, Reading, Rejection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultPhase {
    Before,
    PartialWrite,
    Flush,
    Replace,
    Readback,
    After,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fault {
    pub publication: usize,
    pub phase: FaultPhase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub sequence: usize,
    pub mono_ms: u64,
    pub utc_ms: u64,
    pub event: String,
    pub manifest_digest: String,
    pub previous_checksum: String,
    pub checksum: String,
}

impl Record {
    fn checksum(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!(
                "{}|{}|{}|{}|{}|{}",
                self.sequence,
                self.mono_ms,
                self.utc_ms,
                self.event,
                self.manifest_digest,
                self.previous_checksum
            ))
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Journal {
    pub records: Vec<Record>,
    pub manifest: Option<Manifest>,
    pub consumed_nonces: BTreeSet<String>,
    pub unresolved: bool,
    pub invalid: bool,
    pub storage_failed: bool,
    pub fault: Option<Fault>,
    pub publications: usize,
}

impl Journal {
    pub fn validate(&self) -> bool {
        if self.invalid || self.storage_failed {
            return false;
        }
        if let Some(m) = &self.manifest {
            if !self.consumed_nonces.contains(&m.nonce)
                || self
                    .records
                    .first()
                    .is_none_or(|r| r.event != self.consent_event())
                || !self.unresolved
                    && self.records.last().is_none_or(|r| {
                        !matches!(
                            r.event.as_str(),
                            "restored_verified:durable_close" | "aborted_no_write"
                        )
                    })
            {
                return false;
            }
        } else if !self.records.is_empty() || !self.consumed_nonces.is_empty() {
            return false;
        }
        let mut previous = String::new();
        for (n, r) in self.records.iter().enumerate() {
            if r.sequence != n + 1
                || r.previous_checksum != previous
                || r.checksum != r.checksum()
                || self
                    .manifest
                    .as_ref()
                    .is_none_or(|m| r.manifest_digest != m.digest())
            {
                return false;
            }
            previous = r.checksum.clone();
        }
        self.manifest.as_ref().is_none_or(|m| m.validate().is_ok())
    }

    pub fn begin(&mut self, m: &Manifest, mono: u64, utc: u64) -> Result<(), Rejection> {
        if !self.validate() || self.unresolved {
            return Err(Rejection::JournalBlocked);
        }
        if self.consumed_nonces.contains(&m.nonce) {
            return Err(Rejection::ReplayedNonce);
        }
        // A single-active record. Callers retain closed reports separately;
        // consumed nonces survive replacement of a resolved active record.
        self.records.clear();
        self.manifest = Some(m.clone());
        self.unresolved = true;
        // Conservatively consume even if publication fails; no replay after ambiguous persistence.
        self.consumed_nonces.insert(m.nonce.clone());
        self.publish(&self.consent_event(), mono, utc)
    }

    fn consent_event(&self) -> String {
        format!(
            "consented:nonce_consumed:{}",
            serde_json::to_string(&self.consumed_nonces).expect("serializable nonce set")
        )
    }

    pub fn publish(&mut self, event: &str, mono: u64, utc: u64) -> Result<(), Rejection> {
        self.publications += 1;
        if self.storage_failed {
            return Err(Rejection::Storage);
        }
        let fault = self
            .fault
            .as_ref()
            .filter(|f| f.publication == self.publications);
        if fault.is_some_and(|f| f.phase != FaultPhase::After) {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        let mut r = Record {
            sequence: self.records.len() + 1,
            mono_ms: mono,
            utc_ms: utc,
            event: event.to_owned(),
            manifest_digest: self
                .manifest
                .as_ref()
                .ok_or(Rejection::JournalBlocked)?
                .digest(),
            previous_checksum: self
                .records
                .last()
                .map(|r| r.checksum.clone())
                .unwrap_or_default(),
            checksum: String::new(),
        };
        r.checksum = r.checksum();
        self.records.push(r);
        if fault.is_some() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        Ok(())
    }

    pub fn close(&mut self, mono: u64, utc: u64) -> Result<(), Rejection> {
        if !self.validate() {
            self.storage_failed = true;
            return Err(Rejection::JournalBlocked);
        }
        self.publish("restored_verified:durable_close", mono, utc)?;
        self.unresolved = false;
        Ok(())
    }

    /// Restart is deliberately read-only and cannot clear unresolved state or execute commands.
    pub fn reconcile(&self, observation: ReconcileObservation) -> Reconciliation {
        let identity_matches = self.manifest.as_ref().is_some_and(|m| {
            let mut matches = observation
                .identities
                .iter()
                .filter(|i| i.uuid == m.identity.uuid);
            matches.next() == Some(&m.identity) && matches.next().is_none()
        });
        let bounds_match = self
            .manifest
            .as_ref()
            .is_some_and(|m| observation.bounds == Reading::Available(m.bounds.clone()));
        let baseline_in_bounds =
            self.manifest
                .as_ref()
                .is_some_and(|m| match &observation.bounds {
                    Reading::Available(b) => {
                        b.min_mw < b.max_mw && (b.min_mw..=b.max_mw).contains(&m.baseline_mw)
                    }
                    _ => false,
                });
        Reconciliation {
            provenance: "simulated".into(),
            valid_journal: self.validate(),
            unresolved: self.unresolved || !self.validate(),
            observation,
            identity_matches,
            bounds_match,
            baseline_in_bounds,
            saved_baseline_mw: self.manifest.as_ref().map(|m| m.baseline_mw),
            setters_executed: 0,
            human_reconciliation_required: true,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ReconcileObservation {
    pub identities: Vec<Identity>,
    pub bounds: Reading<Bounds>,
    pub current: Reading<u32>,
}

#[derive(Debug, Serialize)]
pub struct Reconciliation {
    pub provenance: String,
    pub valid_journal: bool,
    pub unresolved: bool,
    pub observation: ReconcileObservation,
    pub identity_matches: bool,
    pub bounds_match: bool,
    pub baseline_in_bounds: bool,
    pub saved_baseline_mw: Option<u32>,
    pub setters_executed: u32,
    pub human_reconciliation_required: bool,
}
