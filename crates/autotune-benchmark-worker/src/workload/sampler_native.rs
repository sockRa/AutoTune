//! Shared retained native owner; cleanup is explicit and separately supervised.
use autotune_probe::nvml::{NvmlSampler, SamplerOpenFailure};
use std::mem::ManuallyDrop;
/// Suppress implicit native cleanup during unwind/drop. Explicit supervised
/// cleanup releases closed owners; otherwise process containment remains needed.
pub(super) enum NativeOwner {
    Selected(ManuallyDrop<NvmlSampler>),
    Failed(ManuallyDrop<SamplerOpenFailure>),
    Released,
}
impl NativeOwner {
    pub(super) fn initialized(&self) -> bool {
        match self {
            Self::Selected(_) => true,
            Self::Failed(f) => f.initialized(),
            Self::Released => false,
        }
    }
    pub(super) fn selected(&self) -> Option<&NvmlSampler> {
        if let Self::Selected(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub(super) fn selected_mut(&mut self) -> Option<&mut NvmlSampler> {
        if let Self::Selected(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub(super) fn shutdown(&mut self) -> Option<i32> {
        match self {
            Self::Selected(s) => s.shutdown(),
            Self::Failed(f) => f.shutdown(),
            Self::Released => None,
        }
    }
    /// Caller proved no initialized owner, or a shutdown call actually returned.
    pub(super) fn release(&mut self) {
        match self {
            Self::Selected(s) => unsafe { ManuallyDrop::drop(s) },
            Self::Failed(f) => unsafe { ManuallyDrop::drop(f) },
            Self::Released => (),
        };
        *self = Self::Released;
    }
}
