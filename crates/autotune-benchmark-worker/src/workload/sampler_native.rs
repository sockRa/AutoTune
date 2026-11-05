//! Shared retained native owner; cleanup is explicit and separately supervised.
use autotune_probe::nvml::{NvmlSampler, SamplerOpenFailure};
use std::mem::ManuallyDrop;
/// Suppress implicit native cleanup during unwind/drop. Explicit supervised
/// cleanup releases closed owners; otherwise process containment remains needed.
#[allow(clippy::large_enum_variant)] // Retain original failure/file owners without fallible boxing.
pub(super) enum NativeOwner {
    Selected(ManuallyDrop<NvmlSampler>),
    Failed(ManuallyDrop<SamplerOpenFailure>),
    Released(autotune_probe::nvml::ReleaseFacts),
}
impl NativeOwner {
    pub(super) fn runtime_file(&self) -> Option<&autotune_probe::runtime_file::windows::Image> {
        match self {
            Self::Selected(s) => s.runtime_file(),
            Self::Failed(f) => f.runtime_file(),
            Self::Released(f) => f.file.as_ref(),
        }
    }
    pub(super) fn load_error(&self) -> Option<&autotune_probe::nvml::RuntimeLoadError> {
        match self {
            Self::Selected(_) => None,
            Self::Failed(f) => f.load_error(),
            Self::Released(f) => f.load_error.as_ref(),
        }
    }
    pub(super) fn initialization(&self) -> Option<i32> {
        match self {
            Self::Selected(_) => Some(0),
            Self::Failed(f) => f.initialization_status(),
            Self::Released(f) => f.initialization,
        }
    }
    pub(super) fn initialized(&self) -> bool {
        match self {
            Self::Selected(_) => true,
            Self::Failed(f) => f.initialized(),
            Self::Released(_) => false,
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
            Self::Released(_) => None,
        }
    }
    /// Caller proved no initialized owner, or a shutdown call actually returned.
    pub(super) fn release(&mut self) {
        let facts = match self {
            Self::Selected(s) => {
                let facts = s.take_release_facts();
                unsafe { ManuallyDrop::drop(s) };
                facts
            }
            Self::Failed(f) => {
                let facts = f.take_release_facts();
                unsafe { ManuallyDrop::drop(f) };
                facts
            }
            Self::Released(_) => return,
        };
        *self = Self::Released(facts);
    }
}
