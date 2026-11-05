// Observed loader only. Legacy Session::open retains its historical behavior.
// A ManuallyDrop local retains library/file owners across observer unwind; the
// returned owner is handed to the caller's explicitly supervised failure path.
pub(super) struct Startup {
    symbols: Option<Symbols>,
    image: Option<crate::runtime_file::windows::Image>,
    path: PathBuf,
    load_error: Option<libloading::Error>,
    initialization: Option<i32>,
}
pub(super) struct OpenFailure {
    failure: Failure,
    startup: Startup,
}
impl std::fmt::Debug for OpenFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NVML observed startup failure")
            .field("failure", &self.failure)
            .field("initialization", &self.startup.initialization)
            .finish()
    }
}
impl OpenFailure {
    pub(super) fn into_public(self) -> SamplerOpenFailure {
        SamplerOpenFailure {
            failure: self.failure,
            session: None,
            startup: Some(self.startup),
        }
    }
}
impl Startup {
    pub(super) fn runtime_file(&self) -> Option<&crate::runtime_file::windows::Image> {
        self.symbols
            .as_ref()
            .and_then(Symbols::runtime_file)
            .or(self.image.as_ref())
    }
    pub(super) fn take_runtime_file(&mut self) -> Option<crate::runtime_file::windows::Image> {
        self.symbols
            .as_mut()
            .and_then(Symbols::take_runtime_file)
            .or_else(|| self.image.take())
    }
    pub(super) fn load_error(&self) -> Option<&libloading::Error> {
        self.load_error.as_ref()
    }
    pub(super) fn take_load_error(&mut self) -> Option<libloading::Error> {
        self.load_error.take()
    }
    pub(super) fn initialization(&self) -> Option<i32> {
        self.initialization
    }
    pub(super) fn library_retained(&self) -> bool {
        matches!(self.symbols, Some(Symbols::Native { .. }))
    }
    #[cfg(test)]
    fn from_symbols(symbols: Symbols, path: PathBuf) -> Self {
        Self {
            symbols: Some(symbols),
            image: None,
            path,
            load_error: None,
            initialization: None,
        }
    }
    fn failed(owner: std::mem::ManuallyDrop<Self>, failure: Failure) -> OpenFailure {
        OpenFailure {
            failure,
            startup: std::mem::ManuallyDrop::into_inner(owner),
        }
    }
    #[allow(clippy::result_large_err)] // Do not allocate after a loader/native failure.
    fn load<O: SamplerObserver>(guard: &mut CallGuard<'_, O>) -> Result<Self, OpenFailure> {
        use crate::runtime_file::{
            windows::{Image, Kind},
            Stage,
        };
        let mut owner = std::mem::ManuallyDrop::new(Self {
            symbols: None,
            image: None,
            path: PathBuf::new(),
            load_error: None,
            initialization: None,
        });
        let result = (|| -> Result<(), Failure> {
            owner.image = Some(Image::prepare(Kind::Nvml, |stage| guard.file_check(stage)));
            let image = owner.image.as_ref().expect("retained file observation");
            if !image.measured() {
                return Err(Failure::new(
                    if matches!(image.open, Some(Err(Some(2 | 3)))) {
                        Reason::RuntimeMissing
                    } else {
                        Reason::RuntimeLoadFailed
                    },
                    "NVML system file observation failed",
                ));
            }
            owner.path = image
                .requested()
                .expect("measured fixed path")
                .to_path_buf();
            if !guard.file_check(Stage::LibraryLoadBefore) {
                return Err(observer_stopped());
            }
            let loaded = unsafe { Library::load_with_flags(&owner.path, 0x100 | 0x800) };
            let raw = match loaded {
                Ok(library) => {
                    let raw = library.into_raw();
                    let image = owner.image.take();
                    owner.symbols = Some(Symbols::Native {
                        library: unsafe { Library::from_raw(raw) },
                        image,
                    });
                    Some(raw)
                }
                Err(error) => {
                    owner.load_error = Some(error);
                    None
                }
            };
            if !guard.file_check(Stage::LibraryLoadAfter) {
                return Err(observer_stopped());
            }
            let raw =
                raw.ok_or_else(|| Failure::new(Reason::RuntimeLoadFailed, "load system nvml"))?;
            let image = match owner.symbols.as_mut() {
                Some(Symbols::Native {
                    image: Some(image), ..
                }) => image,
                _ => unreachable!("retained native file"),
            };
            unsafe {
                image.bind_module(raw as *mut c_void, |stage| guard.file_check(stage));
            }
            if !image.bound() {
                return Err(Failure::new(
                    Reason::RuntimeLoadFailed,
                    "loaded NVML file binding failed",
                ));
            }
            let loaded = image.loaded().expect("bound module path");
            if loaded
                .canonicalize()
                .map_err(|_| invalid("NVML loaded path canonicalization"))?
                != owner
                    .path
                    .canonicalize()
                    .map_err(|_| invalid("NVML trusted path canonicalization"))?
            {
                return Err(invalid("NVML loaded path mismatch"));
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(std::mem::ManuallyDrop::into_inner(owner)),
            Err(f) => Err(Self::failed(owner, f)),
        }
    }
}
