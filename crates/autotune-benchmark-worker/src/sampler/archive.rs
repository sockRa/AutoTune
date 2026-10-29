//! Fixed output leaves, aggregate quota and exact byte index. This storage layer
//! binds bytes, not consent or the truth of the producer's session assertions.
use super::wire_model::{hex, object};
use crate::evidence::{Digest as EvidenceDigest, Domain, Value};
use sha2::{Digest, Sha256};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Artifact {
    Manifest,
    Profile,
    Raw,
    Session,
}
impl Artifact {
    pub fn name(self) -> &'static str {
        match self {
            Self::Manifest => "manifest.json",
            Self::Profile => "profile.json",
            Self::Raw => "raw-events.ndjson",
            Self::Session => "session.json",
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Manifest => 0,
            Self::Profile => 1,
            Self::Raw => 2,
            Self::Session => 3,
        }
    }
}
pub trait Sink {
    fn write(&mut self, name: &'static str, bytes: &[u8]) -> io::Result<()>;
}
#[derive(Clone, Copy)]
pub struct IndexBinding {
    pub manifest: [u8; 32],
    pub profile: [u8; 32],
    pub source_tree: [u8; 32],
    pub controller: [u8; 32],
    pub sampler: [u8; 32],
    pub protocol: [u8; 32],
    pub session_digest: [u8; 32],
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
pub struct Archive<S: Sink> {
    sink: S,
    entries: Vec<Entry>,
    bytes: u64,
    failed: bool,
    finished: bool,
    raw_digest: Option<EvidenceDigest>,
}
impl<S: Sink> Archive<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            entries: Vec::with_capacity(4),
            bytes: 0,
            failed: false,
            finished: false,
            raw_digest: None,
        }
    }
    pub fn sink(&self) -> &S {
        &self.sink
    }
    pub fn write(&mut self, artifact: Artifact, bytes: &[u8]) -> io::Result<()> {
        if self.failed || self.finished {
            return Err(io::Error::other("capture terminal"));
        }
        let result = (|| {
            if artifact.index() != self.entries.len() {
                return Err(io::Error::other("artifact order/replay"));
            }
            let total = self
                .bytes
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| io::Error::other("output length overflow"))?;
            let p = autotune_fma_reference::profile::Profile::approved_design().limits;
            if total > p.serialized_bytes {
                return Err(io::Error::other("aggregate output quota"));
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
            return Err(io::Error::other("incomplete capture"));
        }
        let text = |s: String| Value::Text(s);
        let entries = self
            .entries
            .iter()
            .map(|e| {
                object([
                    ("name", text(e.artifact.name().into())),
                    ("bytes", Value::U64(e.bytes)),
                    ("sha256", text(hex(&e.sha256))),
                ])
            })
            .collect();
        let index = object([
            ("version", text("1".into())),
            ("scope", text("readonly_sampler".into())),
            ("status", text("complete".into())),
            ("manifest_digest", text(hex(&binding.manifest))),
            ("profile_digest", text(hex(&binding.profile))),
            ("source_tree", text(hex(&binding.source_tree))),
            ("controller_sha256", text(hex(&binding.controller))),
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
            ("artifacts", Value::Array(entries)),
        ])
        .to_canonical();
        let total = self
            .bytes
            .checked_add(index.len() as u64)
            .ok_or_else(|| io::Error::other("index length overflow"))?;
        if total
            > autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .serialized_bytes
        {
            self.failed = true;
            return Err(io::Error::other("aggregate output quota including index"));
        }
        if let Err(e) = self.sink.write("capture-index.json", index.as_bytes()) {
            self.failed = true;
            return Err(e);
        }
        self.finished = true;
        Ok(Receipt {
            primary_bytes: self.bytes,
            total_bytes: total,
            index_sha256: Sha256::digest(index.as_bytes()).into(),
        })
    }
}
