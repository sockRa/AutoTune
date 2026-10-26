//! Domain-separated SHA-256 digest convention.
//!
//! The digest is SHA-256 over the fixed domain prefix, one `0x00` byte and the
//! exact canonical payload without its own digest field. A digest binds bytes,
//! not truth, consent or authority.

use sha2::{Digest as _, Sha256};

use super::canonical::{encode, Value};

/// Evidence class bound into every digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Domain {
    Manifest,
    Profile,
    Session,
    RawArtifact,
}

impl Domain {
    pub const ALL: [Self; 4] = [
        Self::Manifest,
        Self::Profile,
        Self::Session,
        Self::RawArtifact,
    ];
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Manifest => "autotune.live-evidence.manifest.v1",
            Self::Profile => "autotune.live-evidence.profile.v1",
            Self::Session => "autotune.live-evidence.session.v1",
            Self::RawArtifact => "autotune.live-evidence.raw-artifact.v1",
        }
    }
}

/// Error returned when parsing a lowercase 64-hex digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestError {
    Length,
    Digit,
}

/// SHA-256 digest with strict lowercase hexadecimal form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Hash arbitrary payload bytes. Callers must supply canonical payload bytes;
    /// prefer [`Digest::of_value`] when a [`Value`] is available.
    pub fn of(domain: Domain, payload: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(domain.prefix().as_bytes());
        hasher.update([0u8]);
        hasher.update(payload);
        Self(hasher.finalize().into())
    }
    /// Canonical entry point: hash the compact canonical encoding of `value`.
    pub fn of_value(domain: Domain, value: &Value) -> Self {
        Self::of(domain, encode(value).as_bytes())
    }
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        out
    }
    pub fn parse(text: &str) -> Result<Self, DigestError> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return Err(DigestError::Length);
        }
        let mut out = [0u8; 32];
        let (pairs, _) = bytes.as_chunks::<2>();
        for (slot, pair) in out.iter_mut().zip(pairs) {
            *slot = (hex(pair[0])? << 4) | hex(pair[1])?;
        }
        Ok(Self(out))
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

fn hex(byte: u8) -> Result<u8, DigestError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(DigestError::Digit),
    }
}
