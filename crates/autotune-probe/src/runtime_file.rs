//! Bounded file-byte observation, not mapped-image attestation or runtime consent.
//! The Windows owner pins a read handle; this core performs no OS/driver calls.
use sha2::{Digest, Sha256};
pub const CHUNK_BYTES: usize = 64 * 1024;
/// Implementation input bound, never additional workload time or output quota.
pub const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_READS: u32 = 4097;
pub const MAX_CHECKS: usize = MAX_READS as usize * 2 + 24;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    DirectoryBefore,
    DirectoryAfter,
    OpenBefore,
    OpenAfter,
    MetadataBefore,
    MetadataAfter,
    ReadBefore(u32),
    ReadAfter(u32),
    FinalMetadataBefore,
    FinalMetadataAfter,
    LibraryLoadBefore,
    LibraryLoadAfter,
    ModulePathBefore,
    ModulePathAfter,
    LoadedOpenBefore,
    LoadedOpenAfter,
    LoadedMetadataBefore,
    LoadedMetadataAfter,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub volume_serial: u32,
    pub index: u64,
    pub bytes: u64,
    pub last_write: u64,
    pub attributes: u32,
    pub file_type: u32,
    pub file_type_error: Option<u32>,
}
pub type Io<T> = Result<T, Option<i32>>;
pub trait Source {
    fn identity(&mut self) -> Io<Identity>;
    fn read(&mut self, buffer: &mut [u8]) -> Io<usize>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Guard,
    Metadata,
    FileKind,
    Size,
    Read,
    ReadCount,
    Protocol,
    Truncated,
    Extra,
    Changed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub before: Option<Io<Identity>>,
    pub after: Option<Io<Identity>>,
    pub reads: u32,
    pub last_read: Option<Io<usize>>,
    pub bytes: u64,
    pub sha256: Option<[u8; 32]>,
    pub fault: Option<Fault>,
}
impl Report {
    pub fn complete(&self) -> bool {
        self.fault.is_none() && self.sha256.is_some()
    }
    pub fn matches_identity(&self, identity: Identity) -> bool {
        self.complete() && self.before == Some(Ok(identity)) && self.after == Some(Ok(identity))
    }
    /// Caller prepays this inline report and fixed scratch before any read.
    /// Raw metadata/read return is recorded before every post-call guard.
    /// One-shot: even an aborted first attempt cannot be restarted on this report.
    pub fn measure(&mut self, source: &mut impl Source, mut guard: impl FnMut(Stage) -> bool) {
        if self.before.is_some() || self.fault.is_some() || self.sha256.is_some() {
            return;
        }
        let result = (|| -> Result<(), Fault> {
            if !guard(Stage::MetadataBefore) {
                return Err(Fault::Guard);
            }
            self.before = Some(source.identity());
            if !guard(Stage::MetadataAfter) {
                return Err(Fault::Guard);
            }
            let first = self
                .before
                .expect("recorded metadata")
                .map_err(|_| Fault::Metadata)?;
            if first.file_type != 1
                || first.file_type_error.is_some()
                || first.attributes & (0x10 | 0x400) != 0
            {
                return Err(Fault::FileKind);
            }
            if first.bytes == 0 || first.bytes > MAX_FILE_BYTES {
                return Err(Fault::Size);
            }
            let mut scratch = [0; CHUNK_BYTES];
            let mut hash = Sha256::new();
            loop {
                if self.reads == MAX_READS {
                    return Err(Fault::ReadCount);
                }
                let index = self.reads;
                if !guard(Stage::ReadBefore(index)) {
                    return Err(Fault::Guard);
                }
                let requested = (first.bytes - self.bytes).min(CHUNK_BYTES as u64).max(1) as usize;
                let raw = source.read(&mut scratch[..requested]);
                self.reads += 1;
                self.last_read = Some(raw);
                // Retain successful byte progress before post-call observation.
                if let Ok(n) = raw {
                    if n <= requested {
                        self.bytes = self.bytes.saturating_add(n as u64);
                    }
                }
                if !guard(Stage::ReadAfter(index)) {
                    return Err(Fault::Guard);
                }
                let n = raw.map_err(|_| Fault::Read)?;
                if n > requested {
                    return Err(Fault::Protocol);
                }
                if self.bytes > first.bytes {
                    return Err(Fault::Extra);
                }
                if n == 0 {
                    if self.bytes != first.bytes {
                        return Err(Fault::Truncated);
                    }
                    break;
                }
                hash.update(&scratch[..n]);
            }
            if !guard(Stage::FinalMetadataBefore) {
                return Err(Fault::Guard);
            }
            self.after = Some(source.identity());
            if !guard(Stage::FinalMetadataAfter) {
                return Err(Fault::Guard);
            }
            let after = self
                .after
                .expect("recorded metadata")
                .map_err(|_| Fault::Metadata)?;
            if first != after {
                return Err(Fault::Changed);
            }
            self.sha256 = Some(hash.finalize().into());
            Ok(())
        })();
        if let Err(f) = result {
            self.fault = Some(f);
        }
    }
}
#[cfg(test)]
mod tests;
#[cfg(windows)]
pub mod windows;
