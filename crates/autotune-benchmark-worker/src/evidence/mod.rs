//! Canonical live-evidence primitives: bytes, decimal integers/ratios, digests and
//! strict decoding. Values are injected; a passing codec never produces live
//! evidence or authorizes native work.
pub mod canonical;
pub mod digest;
pub mod parse;

pub use canonical::{
    encode, AccessError, FieldError, Object, ObjectError, Ratio, RatioError, Value,
};
pub use digest::{Digest, DigestError, Domain};
pub use parse::{decode, ParseError, MAX_DEPTH};
