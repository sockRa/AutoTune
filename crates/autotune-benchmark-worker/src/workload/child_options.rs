//! Fixed child-only CUDA options. Readback is process evidence, never proof of
//! absent driver filesystem activity or execution consent.
pub const ENVIRONMENT: &str = "CUDA_CACHE_DISABLE=1\0CUDA_MODULE_LOADING=EAGER\0\0";
pub const UNITS: usize = 16;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variable {
    CacheDisable,
    ModuleLoading,
}
impl Variable {
    pub const ALL: [Self; 2] = [Self::CacheDisable, Self::ModuleLoading];
    pub fn name(self) -> &'static str {
        match self {
            Self::CacheDisable => "CUDA_CACHE_DISABLE",
            Self::ModuleLoading => "CUDA_MODULE_LOADING",
        }
    }
    pub fn expected(self) -> &'static str {
        match self {
            Self::CacheDisable => "1",
            Self::ModuleLoading => "EAGER",
        }
    }
}
/// Raw initialized scratch is diagnostic only when the Win32 call fails or
/// returns an oversized required length; no value is inferred from that buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read {
    pub length: u32,
    pub error: Option<u32>,
    pub units: [u16; UNITS],
}
impl Read {
    pub fn matches(self, variable: Variable) -> bool {
        self.error.is_none()
            && self.length as usize == variable.expected().len()
            && (self.length as usize) < UNITS
            && self.units[self.length as usize] == 0
            && self.units[..self.length as usize]
                .iter()
                .copied()
                .eq(variable.expected().encode_utf16())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub cache_disable: Option<Read>,
    pub module_loading: Option<Read>,
}
impl Facts {
    /// Store the raw return before any post-clock callback can reject/unwind.
    /// This helper does not call the OS; the caller supplies its fixed reader.
    pub fn observe<E>(
        &mut self,
        mut call: impl FnMut(Variable, &mut Option<Read>) -> Result<(), E>,
    ) -> Result<bool, E> {
        for (variable, slot) in [
            (Variable::CacheDisable, &mut self.cache_disable),
            (Variable::ModuleLoading, &mut self.module_loading),
        ] {
            call(variable, slot)?;
            if !slot.is_some_and(|read| read.matches(variable)) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
#[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
pub(crate) fn read(variable: Variable) -> Read {
    use windows_sys::Win32::{
        Foundation::{GetLastError, SetLastError},
        System::Environment::GetEnvironmentVariableW,
    };
    let mut name = [0u16; 32];
    for (slot, unit) in name.iter_mut().zip(variable.name().encode_utf16()) {
        *slot = unit;
    }
    let mut units = [0u16; UNITS];
    // SAFETY: fixed NUL-terminated key and initialized bounded output, no retry,
    // allocation or mutation of any environment variable.
    let (length, error) = unsafe {
        SetLastError(0);
        let length = GetEnvironmentVariableW(name.as_ptr(), units.as_mut_ptr(), UNITS as u32);
        (length, (length == 0).then(|| GetLastError()))
    };
    Read {
        length,
        error,
        units,
    }
}
