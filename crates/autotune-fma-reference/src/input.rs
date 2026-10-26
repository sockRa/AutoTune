//! Versioned, indexed input generation; no dispatch or GPU computation.
use sha2::{Digest, Sha256};

pub const GENERATOR_ID: &str = "sha256-counter-fma-v1";
pub const MAX_LANES: u32 = 262_144;
pub const DISPATCH_CAPACITY: u32 = 4096;
const DOMAIN: &[u8] = b"AutoTune/FMA/input/v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLanes,
    InvalidSteps,
    InvalidDispatch,
    LaneOutOfRange,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Steps(u32);

impl Steps {
    pub fn new(value: u32) -> Result<Self, Error> {
        if value == 0 || value > 65_536 || !value.is_multiple_of(4096) {
            return Err(Error::InvalidSteps);
        }
        Ok(Self(value))
    }
    pub fn value(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    lanes: u32,
    steps: Steps,
}

impl Shape {
    pub fn new(lanes: u32, steps: u32) -> Result<Self, Error> {
        if lanes == 0 || lanes > MAX_LANES {
            return Err(Error::InvalidLanes);
        }
        Ok(Self {
            lanes,
            steps: Steps::new(steps)?,
        })
    }
    pub fn lanes(self) -> u32 {
        self.lanes
    }
    pub fn steps(self) -> Steps {
        self.steps
    }
    /// Three FP32 inputs and one FP32 output; excludes driver/context overhead.
    pub fn application_buffer_bytes(self) -> u64 {
        u64::from(self.lanes) * 16
    }
    pub fn fma_steps_per_dispatch(self) -> u64 {
        u64::from(self.lanes) * u64::from(self.steps.value())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchIndex(u32);

impl DispatchIndex {
    pub fn new(value: u32) -> Result<Self, Error> {
        if value >= DISPATCH_CAPACITY {
            return Err(Error::InvalidDispatch);
        }
        Ok(Self(value))
    }
    pub fn value(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane {
    a: i8,
    b: i8,
    base: i8,
}

impl Lane {
    pub fn from_word(word: u32) -> Self {
        fn operand(nibble: u32) -> i8 {
            let magnitude = 1 + (nibble & 7) as i8;
            if nibble & 8 == 0 {
                magnitude
            } else {
                -magnitude
            }
        }
        Self {
            a: operand(word & 15),
            b: operand((word >> 4) & 15),
            base: ((word >> 8) as u8) as i8,
        }
    }
    pub fn a(self) -> i8 {
        self.a
    }
    pub fn b(self) -> i8 {
        self.b
    }
    pub fn base(self) -> i8 {
        self.base
    }
    /// Exact FP32 integers, not a result of executing the future FMA workload.
    pub fn inputs(self, dispatch: DispatchIndex) -> [f32; 3] {
        let x0 = 4096 * i32::from(self.base) + dispatch.value() as i32;
        [f32::from(self.a), f32::from(self.b), x0 as f32]
    }
}

/// Original counter block bytes, with explicit domain and little-endian fields.
pub fn block_digest(seed: u64, block: u64) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    hash.update(seed.to_le_bytes());
    hash.update(block.to_le_bytes());
    hash.finalize().into()
}

fn word_at(digest: &[u8; 32], word: usize) -> u32 {
    let start = word * 4;
    u32::from_le_bytes([
        digest[start],
        digest[start + 1],
        digest[start + 2],
        digest[start + 3],
    ])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recipe {
    seed: u64,
    shape: Shape,
}

impl Recipe {
    pub fn new(seed: u64, shape: Shape) -> Self {
        Self { seed, shape }
    }
    pub fn seed(self) -> u64 {
        self.seed
    }
    pub fn shape(self) -> Shape {
        self.shape
    }
    pub fn lane(self, index: u32) -> Result<Lane, Error> {
        if index >= self.shape.lanes() {
            return Err(Error::LaneOutOfRange);
        }
        let digest = block_digest(self.seed, u64::from(index / 8));
        Ok(Lane::from_word(word_at(&digest, (index % 8) as usize)))
    }
    /// Constant-space iteration; each eight-lane block is hashed only once.
    pub fn lanes(self) -> impl ExactSizeIterator<Item = Lane> {
        let mut digest = [0; 32];
        (0..self.shape.lanes()).map(move |index| {
            if index % 8 == 0 {
                digest = block_digest(self.seed, u64::from(index / 8));
            }
            Lane::from_word(word_at(&digest, (index % 8) as usize))
        })
    }
}
