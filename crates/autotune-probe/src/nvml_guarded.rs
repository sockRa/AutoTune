// Observed read-only startup/runtime path. Existing unobserved APIs keep their
// historical behavior. Every FFI entry uses the same existing ABI aliases.
pub(super) struct CallGuard<'a, O> {
    observer: &'a mut O,
    halted: bool,
}
fn observer_stopped() -> Failure {
    Failure::new(Reason::QueryFailed, "NVML observed call lane stopped")
}
impl<'a, O: SamplerObserver> CallGuard<'a, O> {
    pub(super) fn new(observer: &'a mut O) -> Self {
        Self {
            observer,
            halted: false,
        }
    }
    fn before(&mut self, call: SamplerCall) -> Result<(), Failure> {
        if self.halted || !self.observer.before(call) {
            self.halted = true;
            Err(observer_stopped())
        } else {
            Ok(())
        }
    }
    fn returned(&mut self, call: SamplerCall, status: i32, output: SamplerOutput) {
        // Deliver the actual status/output before any post-clock callback can
        // close the lane. A failed native call also prevents a successor.
        let keep = self
            .observer
            .returned(call, SamplerReturn { status, output });
        if !keep || status != 0 {
            self.halted = true;
        }
    }
}
impl Session {
    pub(super) fn open_observed<O: SamplerObserver>(
        guard: &mut CallGuard<'_, O>,
    ) -> Result<Self, Failure> {
        let (library, path) = Self::load()?;
        Self::initialize_observed(library, path, guard)
    }
    fn initialize_observed<O: SamplerObserver>(
        library: Symbols,
        path: PathBuf,
        guard: &mut CallGuard<'_, O>,
    ) -> Result<Self, Failure> {
        // SAFETY: exact existing init/shutdown ABI and retained trusted library.
        let (init, shutdown) = unsafe {
            (
                library.get::<Init>(b"nvmlInit_v2\0")?,
                library.get::<Init>(b"nvmlShutdown\0")?,
            )
        };
        guard.before(SamplerCall::Initialize)?;
        let status = unsafe { init() };
        if status == 0 {
            // Even observer unwind cannot silently call shutdown. Explicit
            // failure ownership or the selected owner is transferred below.
            let session = std::mem::ManuallyDrop::new(Self {
                library,
                path,
                shutdown,
                closed: false,
            });
            guard.returned(SamplerCall::Initialize, status, SamplerOutput::None);
            Ok(std::mem::ManuallyDrop::into_inner(session))
        } else {
            guard.returned(SamplerCall::Initialize, status, SamplerOutput::None);
            check(status, b"nvmlInit_v2")?;
            unreachable!()
        }
    }
    #[allow(clippy::result_large_err)]
    pub(super) fn select_observed<O: SamplerObserver>(
        self,
        uuid: &str,
        pci: &str,
        guard: &mut CallGuard<'_, O>,
    ) -> Result<SelectedSession, (Self, Failure)> {
        let session = std::mem::ManuallyDrop::new(self);
        let result = session.find_observed(uuid, pci, guard).and_then(|v| {
            if guard.halted {
                Err(observer_stopped())
            } else {
                Ok(v)
            }
        });
        let session = std::mem::ManuallyDrop::into_inner(session);
        match result {
            Ok((handle, raw_uuid, raw_pci)) => Ok(SelectedSession {
                session,
                handle,
                raw_uuid,
                raw_pci,
            }),
            Err(f) => Err((session, f)),
        }
    }
    fn find_observed<O: SamplerObserver>(
        &self,
        uuid: &str,
        pci: &str,
        guard: &mut CallGuard<'_, O>,
    ) -> Result<(Handle, String, String), Failure> {
        if uuid.is_empty() || pci.is_empty() {
            return Err(invalid("empty NVML selection identity"));
        }
        let mut count = 0;
        let query = unsafe { self.get::<Count>(b"nvmlDeviceGetCount_v2\0")? };
        guard.before(SamplerCall::Count)?;
        let status = unsafe { query(&mut count) };
        guard.returned(SamplerCall::Count, status, SamplerOutput::Count(count));
        check(status, b"nvmlDeviceGetCount_v2")?;
        if count > 1024 {
            return Err(invalid("device count exceeds safety bound (1024)"));
        }
        let mut selected = None;
        for index in 0..count {
            let mut handle = std::ptr::null_mut();
            let query = unsafe { self.get::<ByIndex>(b"nvmlDeviceGetHandleByIndex_v2\0")? };
            guard.before(SamplerCall::Handle(index))?;
            let status = unsafe { query(index, &mut handle) };
            guard.returned(
                SamplerCall::Handle(index),
                status,
                SamplerOutput::HandleIsNull(handle.is_null()),
            );
            check(status, b"nvmlDeviceGetHandleByIndex_v2")?;
            if handle.is_null() {
                return Err(invalid("NVML returned null device handle"));
            }
            let mut buffer = [0u8; 256];
            let query = unsafe { self.get::<DeviceText>(b"nvmlDeviceGetUUID\0")? };
            guard.before(SamplerCall::Uuid(index))?;
            let status = unsafe { query(handle, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
            guard.returned(
                SamplerCall::Uuid(index),
                status,
                SamplerOutput::Text(buffer),
            );
            check(status, b"nvmlDeviceGetUUID")?;
            let device_uuid = text(&buffer)?;
            let mut raw = Pci::default();
            let query = unsafe { self.get::<PciQuery>(b"nvmlDeviceGetPciInfo_v3\0")? };
            guard.before(SamplerCall::Pci(index))?;
            let status = unsafe { query(handle, &mut raw) };
            guard.returned(
                SamplerCall::Pci(index),
                status,
                SamplerOutput::Pci {
                    legacy: raw.legacy,
                    numbers: [
                        raw.domain,
                        raw.bus,
                        raw.device,
                        raw.device_id,
                        raw.subsystem_id,
                    ],
                    bus_id: raw.bus_id,
                },
            );
            check(status, b"nvmlDeviceGetPciInfo_v3")?;
            let device_pci = text(&raw.bus_id)?;
            if device_uuid == uuid
                && device_pci == pci
                && selected
                    .replace((handle, device_uuid, device_pci))
                    .is_some()
            {
                return Err(invalid("ambiguous NVML UUID and PCI identity"));
            }
        }
        selected.ok_or_else(|| invalid("NVML UUID and PCI identity not found"))
    }
    fn text_observed<O: SamplerObserver>(
        &self,
        name: &[u8],
        call: SamplerCall,
        guard: &mut CallGuard<'_, O>,
    ) -> Field {
        field(
            "text",
            (|| {
                let mut buffer = [0u8; 256];
                let query = unsafe { self.get::<Text>(name)? };
                guard.before(call)?;
                let status = unsafe { query(buffer.as_mut_ptr().cast(), buffer.len() as u32) };
                guard.returned(call, status, SamplerOutput::Text(buffer));
                check(status, name)?;
                text(&buffer).map(Into::into)
            })(),
        )
    }
    fn runtime_observed<O: SamplerObserver>(&self, guard: &mut CallGuard<'_, O>) -> Runtime {
        let cuda = (|| {
            let mut version = 0;
            let query = unsafe {
                self.get::<unsafe extern "C" fn(*mut i32) -> i32>(
                    b"nvmlSystemGetCudaDriverVersion_v2\0",
                )?
            };
            guard.before(SamplerCall::CudaVersion)?;
            let status = unsafe { query(&mut version) };
            guard.returned(
                SamplerCall::CudaVersion,
                status,
                SamplerOutput::Version(version),
            );
            check(status, b"nvmlSystemGetCudaDriverVersion_v2")?;
            if version <= 0 {
                Err(invalid("invalid CUDA driver version"))
            } else {
                Ok(version.into())
            }
        })();
        if cuda.is_err() {
            guard.halted = true;
        }
        let driver_version = self.text_observed(
            b"nvmlSystemGetDriverVersion\0",
            SamplerCall::DriverVersion,
            guard,
        );
        if matches!(driver_version.reading, Reading::Unavailable { .. }) {
            guard.halted = true;
        }
        let nvml_version = self.text_observed(
            b"nvmlSystemGetNVMLVersion\0",
            SamplerCall::NvmlVersion,
            guard,
        );
        Runtime {
            library_path: Some(self.path.display().to_string()),
            driver_version,
            nvml_version,
            cuda_driver_version: field("CUDA encoded integer", cuda),
            header_api_version: 13,
        }
    }
}
impl SelectedSession {
    pub(super) fn describe_observed(&self, observer: &mut impl SamplerObserver) -> SamplerInfo {
        let mut guard = CallGuard::new(observer);
        SamplerInfo {
            raw_uuid: self.raw_uuid.clone(),
            raw_pci: self.raw_pci.clone(),
            runtime: self.session.runtime_observed(&mut guard),
        }
    }
}
