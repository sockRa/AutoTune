//! Workload-session archive. Kept distinct from the readonly-sampler archive:
//! a worker-backed capture must never be serialized with readonly scope.
use super::archive::{Artifact, Sink};
use super::wire_model::{hex, object};
use crate::evidence::{Digest as EvidenceDigest, Domain, Value};
use sha2::{Digest as _, Sha256};
use std::io;

#[derive(Clone, Copy)]
pub struct IndexBinding {
    pub manifest: [u8; 32],
    pub profile: [u8; 32],
    pub source_tree: [u8; 32],
    pub controller: [u8; 32],
    pub worker: [u8; 32],
    pub sampler: [u8; 32],
    pub protocol: [u8; 32],
    pub session_digest: [u8; 32],
}
impl IndexBinding {
    /// Derive all index bindings from the frozen workload manifest rather than
    /// accepting independently supplied artifact identities at finalization.
    pub fn from_pilot(
        manifest: &crate::workload::manifest::PilotManifest,
        session_digest: EvidenceDigest,
    ) -> Self {
        let draft = manifest.manifest_draft();
        let artifacts = draft.artifacts;
        Self {
            manifest: *manifest.digest().as_bytes(),
            profile: *manifest.profile_digest().as_bytes(),
            source_tree: *artifacts.source_tree.as_bytes(),
            controller: *artifacts.controller.as_bytes(),
            worker: *artifacts.worker.as_bytes(),
            sampler: *artifacts.sampler.as_bytes(),
            protocol: *artifacts.protocol.as_bytes(),
            session_digest: *session_digest.as_bytes(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Receipt {
    pub primary_bytes: u64,
    pub total_bytes: u64,
    pub index_sha256: [u8; 32],
}

struct Entry {
    artifact: Artifact,
    bytes: u64,
    sha256: [u8; 32],
}

/// Fixed four-leaf workload archive with a separately scoped index.
/// Hashes bind bytes; they do not establish consent, truth, or qualification.
pub struct WorkloadArchive<S: Sink> {
    sink: S,
    entries: Vec<Entry>,
    bytes: u64,
    raw_digest: Option<EvidenceDigest>,
    failed: bool,
    finished: bool,
}

impl<S: Sink> WorkloadArchive<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            entries: Vec::with_capacity(4),
            bytes: 0,
            raw_digest: None,
            failed: false,
            finished: false,
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn write(&mut self, artifact: Artifact, bytes: &[u8]) -> io::Result<()> {
        if self.failed || self.finished {
            return Err(io::Error::other("workload archive terminal"));
        }
        let result = (|| {
            let expected = [
                Artifact::Manifest,
                Artifact::Profile,
                Artifact::Raw,
                Artifact::Session,
            ]
            .get(self.entries.len());
            if expected != Some(&artifact) {
                return Err(io::Error::other("workload artifact order/replay"));
            }
            let total = self
                .bytes
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| io::Error::other("workload output length overflow"))?;
            if total
                > autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .serialized_bytes
            {
                return Err(io::Error::other("workload aggregate output quota"));
            }
            self.sink.write(artifact.name(), bytes)?;
            self.entries.push(Entry {
                artifact,
                bytes: bytes.len() as u64,
                sha256: Sha256::digest(bytes).into(),
            });
            if artifact == Artifact::Raw {
                self.raw_digest = Some(EvidenceDigest::of(Domain::RawArtifact, bytes));
            }
            self.bytes = total;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    pub fn finish(&mut self, binding: IndexBinding) -> io::Result<Receipt> {
        if self.failed || self.finished || self.entries.len() != 4 {
            self.failed = true;
            return Err(io::Error::other("incomplete workload archive"));
        }
        let text = |s: String| Value::Text(s);
        let artifacts = self
            .entries
            .iter()
            .map(|entry| {
                object([
                    ("name", text(entry.artifact.name().into())),
                    ("bytes", Value::U64(entry.bytes)),
                    ("sha256", text(hex(&entry.sha256))),
                ])
            })
            .collect();
        let index = object([
            ("version", text("1".into())),
            ("scope", text("workload_session".into())),
            ("status", text("complete".into())),
            ("manifest_digest", text(hex(&binding.manifest))),
            ("profile_digest", text(hex(&binding.profile))),
            ("source_tree", text(hex(&binding.source_tree))),
            ("controller_sha256", text(hex(&binding.controller))),
            ("worker_sha256", text(hex(&binding.worker))),
            ("sampler_sha256", text(hex(&binding.sampler))),
            ("protocol_digest", text(hex(&binding.protocol))),
            (
                "raw_digest",
                text(
                    self.raw_digest
                        .expect("four artifacts include raw")
                        .to_hex(),
                ),
            ),
            ("session_digest", text(hex(&binding.session_digest))),
            (
                "index_scope",
                text("four_primary_artifacts_excludes_self".into()),
            ),
            ("primary_bytes", Value::U64(self.bytes)),
            ("artifacts", Value::Array(artifacts)),
        ])
        .to_canonical();
        let total = self
            .bytes
            .checked_add(index.len() as u64)
            .ok_or_else(|| io::Error::other("workload index length overflow"))?;
        if total
            > autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .serialized_bytes
        {
            self.failed = true;
            return Err(io::Error::other("workload output quota including index"));
        }
        if let Err(error) = self.sink.write("capture-index.json", index.as_bytes()) {
            self.failed = true;
            return Err(error);
        }
        self.finished = true;
        Ok(Receipt {
            primary_bytes: self.bytes,
            total_bytes: total,
            index_sha256: Sha256::digest(index.as_bytes()).into(),
        })
    }
}
