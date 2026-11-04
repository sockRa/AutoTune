// Included in the opt-in Windows CUDA adapter. No setup is executed by inclusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOperation {
    Initialize,
    DriverVersion,
    DeviceCount,
    Device(u32),
    Uuid(i32),
    Attribute { device: i32, attribute: i32 },
    ContextCreate,
    ContextDevice,
    ModuleLoad,
    Function,
    MemoryInfo,
    Allocate(u8),
    StreamCreate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOutput {
    None,
    Integer(i32),
    Uuid([u8; 16]),
    HandleIsNull(bool),
    Memory { free: usize, total: usize },
    AllocationIsZero(bool),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupReturn {
    pub status: i32,
    pub output: SetupOutput,
}
/// Reserve evidence before approving entry. `returned` receives actual status
/// and initialized output fields before post-clock validation. Observer methods
/// call no driver; any rejection aborts setup without a successor or cleanup.
pub trait SetupObserver {
    fn before(&mut self, operation: SetupOperation) -> Result<(), Fault>;
    fn returned(&mut self, operation: SetupOperation, raw: SetupReturn) -> Result<(), Fault>;
}
struct UnobservedSetup;
impl SetupObserver for UnobservedSetup {
    fn before(&mut self, _: SetupOperation) -> Result<(), Fault> {
        Ok(())
    }
    fn returned(&mut self, _: SetupOperation, _: SetupReturn) -> Result<(), Fault> {
        Ok(())
    }
}
/// Raw nonzero resource tokens retained in the original backend, not proof that
/// failed CUDA outputs designate valid resources or are safe to destroy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupResources {
    pub context: bool,
    pub module: bool,
    pub function: bool,
    pub stream: bool,
    pub buffers: [bool; 4],
}
pub struct SetupFailure {
    pub fault: Fault,
    backend: Option<CudaBackend>,
    library: Option<ManuallyDrop<Library>>,
    expected_path: Option<PathBuf>,
    loaded_path: Option<PathBuf>,
}
impl SetupFailure {
    pub fn resources(&self) -> Option<SetupResources> {
        self.backend.as_ref().map(CudaBackend::setup_resources)
    }
    pub fn library_retained(&self) -> bool {
        self.library.is_some() || self.backend.as_ref().is_some_and(|b| b._library.is_some())
    }
    pub fn expected_path(&self) -> Option<&std::path::Path> {
        self.expected_path.as_deref()
    }
    pub fn loaded_path(&self) -> Option<&std::path::Path> {
        self.loaded_path.as_deref()
    }
}
impl CudaBackend {
    fn setup_resources(&self) -> SetupResources {
        SetupResources {
            context: !self.context.is_null(),
            module: !self.module.is_null(),
            function: !self.function.is_null(),
            stream: !self.stream.is_null(),
            buffers: self.buffers.map(|p| p != 0),
        }
    }
    /// Load only the fixed trusted system driver and retain failed partial owners.
    /// No teardown is attempted on failure or Drop. The legacy open delegates to
    /// this same implementation with no observation, preserving its unsafe scope.
    /// # Safety
    /// This initializes the CUDA driver and creates GPU context/module/buffers.
    /// Caller must already have exact independent execution consent, process/
    /// clock/monitor supervision and required admission facts. An observer or
    /// manifest hash grants none of these. No workload NativeAdmission is issued.
    #[allow(clippy::result_large_err)] // Retain partial owners without a fallible allocation after CUDA entry.
    pub unsafe fn open_observed(
        identity: Identity,
        shape: Shape,
        observer: &mut impl SetupObserver,
    ) -> Result<Self, SetupFailure> {
        let mut failure = SetupFailure {
            fault: invalid("unstarted CUDA setup"),
            backend: None,
            library: None,
            expected_path: None,
            loaded_path: None,
        };
        let result = (|| -> Result<(), Fault> {
            if !identity.valid() {
                return Err(invalid("identity"));
            }
            let mut directory = [0u16; 32768];
            let length =
                GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) as usize;
            if length == 0 || length >= directory.len() {
                return Err(invalid("GetSystemDirectoryW"));
            }
            let path = PathBuf::from(OsString::from_wide(&directory[..length])).join("nvcuda.dll");
            failure.expected_path = Some(path.clone());
            if !path.is_absolute() {
                return Err(invalid("system driver path"));
            }
            let library = Library::load_with_flags(&path, 0x100 | 0x800)
                .map_err(|_| invalid("load system nvcuda"))?;
            let raw = library.into_raw();
            failure.library = Some(ManuallyDrop::new(Library::from_raw(raw)));
            let mut loaded = [0u16; 32768];
            let length =
                GetModuleFileNameW(raw as *mut c_void, loaded.as_mut_ptr(), loaded.len() as u32)
                    as usize;
            if length == 0 || length >= loaded.len() {
                return Err(invalid("loaded module path"));
            }
            let loaded = PathBuf::from(OsString::from_wide(&loaded[..length]));
            failure.loaded_path = Some(loaded.clone());
            if loaded
                .canonicalize()
                .map_err(|_| invalid("loaded path canonicalization"))?
                != path
                    .canonicalize()
                    .map_err(|_| invalid("trusted path canonicalization"))?
            {
                return Err(invalid("loaded path mismatch"));
            }
            let api = Api::load(failure.library.as_ref().expect("loaded library retained"))?;
            failure.backend = Some(Self {
                _library: failure.library.take(),
                api,
                identity,
                shape,
                context: null_mut(),
                module: null_mut(),
                function: null_mut(),
                stream: null_mut(),
                buffers: [0; 4],
                state: State::Poisoned,
                _thread_affinity: PhantomData,
                driver_path: loaded,
                driver_version: -1,
            });
            Ok(())
        })();
        match result {
            Ok(()) => Self::finish_setup(failure, observer),
            Err(fault) => {
                failure.fault = fault;
                Err(failure)
            }
        }
    }
    #[allow(clippy::result_large_err)] // The original partial owner must survive any post-call failure.
    fn finish_setup(
        mut failure: SetupFailure,
        observer: &mut impl SetupObserver,
    ) -> Result<Self, SetupFailure> {
        match failure
            .backend
            .as_mut()
            .expect("backend retained before init")
            .setup_observed(observer)
        {
            Ok(()) => Ok(failure.backend.take().expect("successful backend")),
            Err(fault) => {
                failure.fault = fault;
                Err(failure)
            }
        }
    }
    fn setup_observed(&mut self, observer: &mut impl SetupObserver) -> Result<(), Fault> {
        macro_rules! call {
            ($op:expr,$name:literal,$invoke:expr,$output:expr) => {{
                let op = $op;
                observer.before(op)?;
                let status = unsafe { $invoke };
                observer.returned(
                    op,
                    SetupReturn {
                        status,
                        output: $output,
                    },
                )?;
                checked($name, status)?;
            }};
        }
        call!(
            SetupOperation::Initialize,
            "cuInit",
            (self.api.init)(0),
            SetupOutput::None
        );
        call!(
            SetupOperation::DriverVersion,
            "cuDriverGetVersion",
            (self.api.version)(&mut self.driver_version),
            SetupOutput::Integer(self.driver_version)
        );
        if self.driver_version < 10010 {
            return Err(invalid("driver version"));
        }
        let mut count = -1;
        call!(
            SetupOperation::DeviceCount,
            "cuDeviceGetCount",
            (self.api.count)(&mut count),
            SetupOutput::Integer(count)
        );
        if !(1..=64).contains(&count) {
            return Err(invalid("device count"));
        }
        let mut selected = None;
        for ordinal in 0..count {
            let mut device = -1;
            call!(
                SetupOperation::Device(ordinal as u32),
                "cuDeviceGet",
                (self.api.device)(&mut device, ordinal),
                SetupOutput::Integer(device)
            );
            if device < 0 {
                return Err(invalid("device"));
            }
            let mut uuid = Uuid::default();
            call!(
                SetupOperation::Uuid(device),
                "cuDeviceGetUuid_v2",
                (self.api.uuid)(&mut uuid, device),
                SetupOutput::Uuid(uuid.bytes)
            );
            if uuid.bytes == self.identity.uuid {
                if selected.is_some() {
                    return Err(invalid("ambiguous UUID"));
                }
                for (attribute, expected) in [
                    (50, i32::from(self.identity.domain)),
                    (33, i32::from(self.identity.bus)),
                    (34, i32::from(self.identity.device)),
                    (75, 7),
                    (76, 5),
                ] {
                    let mut actual = -1;
                    call!(
                        SetupOperation::Attribute { device, attribute },
                        "cuDeviceGetAttribute",
                        (self.api.attr)(&mut actual, attribute, device),
                        SetupOutput::Integer(actual)
                    );
                    if actual != expected {
                        return Err(invalid("PCI/capability mismatch"));
                    }
                }
                selected = Some(device);
            }
        }
        let device = selected.ok_or_else(|| invalid("UUID not found"))?;
        call!(
            SetupOperation::ContextCreate,
            "cuCtxCreate_v2",
            (self.api.ctx_create)(&mut self.context, 0, device),
            SetupOutput::HandleIsNull(self.context.is_null())
        );
        if self.context.is_null() {
            return Err(invalid("null context"));
        }
        let mut current = -1;
        call!(
            SetupOperation::ContextDevice,
            "cuCtxGetDevice",
            (self.api.ctx_device)(&mut current),
            SetupOutput::Integer(current)
        );
        if current != device {
            return Err(invalid("context device mismatch"));
        }
        let ptx = std::ffi::CString::new(PTX).map_err(|_| invalid("PTX NUL"))?;
        call!(
            SetupOperation::ModuleLoad,
            "cuModuleLoadData",
            (self.api.module_load)(&mut self.module, ptx.as_ptr().cast()),
            SetupOutput::HandleIsNull(self.module.is_null())
        );
        if self.module.is_null() {
            return Err(invalid("null module"));
        }
        call!(
            SetupOperation::Function,
            "cuModuleGetFunction",
            (self.api.function)(&mut self.function, self.module, c"autotune_fma".as_ptr()),
            SetupOutput::HandleIsNull(self.function.is_null())
        );
        if self.function.is_null() {
            return Err(invalid("null function"));
        }
        let (mut free, mut total) = (0, 0);
        call!(
            SetupOperation::MemoryInfo,
            "cuMemGetInfo_v2",
            (self.api.mem_info)(&mut free, &mut total),
            SetupOutput::Memory { free, total }
        );
        let limits = Profile::approved_design().limits;
        let planned = self.shape.application_buffer_bytes();
        if free > total
            || (free as u64) < planned + limits.free_gpu_headroom_bytes
            || planned > limits.application_gpu_bytes
        {
            return Err(invalid("memory headroom"));
        }
        for i in 0..4 {
            call!(
                SetupOperation::Allocate(i as u8),
                "cuMemAlloc_v2",
                (self.api.alloc)(&mut self.buffers[i], self.shape.lanes() as usize * 4),
                SetupOutput::AllocationIsZero(self.buffers[i] == 0)
            );
            if self.buffers[i] == 0 {
                return Err(invalid("null allocation"));
            }
        }
        call!(
            SetupOperation::StreamCreate,
            "cuStreamCreate",
            (self.api.stream_create)(&mut self.stream, 1),
            SetupOutput::HandleIsNull(self.stream.is_null())
        );
        if self.stream.is_null() {
            return Err(invalid("null stream"));
        }
        self.state = State::Ready;
        Ok(())
    }
}
