//! Read-only ABI subset from NVIDIA NVML 13.4.92 (API 13).
//! See docs/third-party-notices.md for NVIDIA's disclaimer and government notice.
//! No setter is bound as a function: export inspection uses opaque data pointers.
/*
Copyright 1993-2026 NVIDIA Corporation. All rights reserved.

NOTICE TO USER:
This source code is subject to NVIDIA ownership rights under U.S. and
international Copyright laws. Users and possessors of this source code
are hereby granted a nonexclusive, royalty-free license to use this code
in individual and commercial software.

NVIDIA MAKES NO REPRESENTATION ABOUT THE SUITABILITY OF THIS SOURCE
CODE FOR ANY PURPOSE. IT IS PROVIDED "AS IS" WITHOUT EXPRESS OR
IMPLIED WARRANTY OF ANY KIND. NVIDIA DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOURCE CODE, INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY, NONINFRINGEMENT, AND FITNESS FOR A PARTICULAR PURPOSE.
IN NO EVENT SHALL NVIDIA BE LIABLE FOR ANY SPECIAL, INDIRECT, INCIDENTAL,
OR CONSEQUENTIAL DAMAGES, OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE
OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE
OR PERFORMANCE OF THIS SOURCE CODE.

U.S. Government End Users. This source code is a "commercial item" as
that term is defined at 48 C.F.R. 2.101 (OCT 1995), consisting of
"commercial computer software" and "commercial computer software
documentation" as such terms are used in 48 C.F.R. 12.212 (SEPT 1995)
and is provided to the U.S. Government only as a commercial end item.
Consistent with 48 C.F.R.12.212 and 48 C.F.R. 227.7202-1 through
227.7202-4 (JUNE 1995), all U.S. Government End Users acquire the
source code with only those rights set forth herein.

Any use of this source code in individual and commercial software must
include, in the user documentation and internal comments to the code,
the above Disclaimer and U.S. Government End Users Notice.
*/
use crate::{adapter::ReadOnlyAdapter, report::*};

/// One read-only monitor getter per sampling invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleMetric {
    Temperature,
    Power,
    GraphicsClock,
    MemoryClock,
    GpuUtilization,
    MemoryUtilization,
}

impl SampleMetric {
    fn unit(self) -> &'static str {
        match self {
            Self::Temperature => "degC",
            Self::Power => "mW",
            Self::GraphicsClock | Self::MemoryClock => "MHz",
            Self::GpuUtilization | Self::MemoryUtilization => "%",
        }
    }
}

/// Owns one trusted NVML session and its privately selected device handle.
///
/// Exact NVML UUID and PCI bus-ID matching is not physical workload mapping,
/// workload admission, or a claim about the provenance of a live recording.
pub struct NvmlSampler {
    #[cfg(windows)]
    selected: windows::SelectedSession,
}

/// Fresh runtime evidence and the exact identity retained at unique selection.
/// Describing does not re-enumerate, select another device or read controls.
#[derive(Clone, Debug)]
pub struct SamplerInfo {
    pub raw_uuid: String,
    pub raw_pci: String,
    pub runtime: Runtime,
}

impl NvmlSampler {
    #[cfg(windows)]
    pub fn describe(&self) -> SamplerInfo {
        self.selected.describe()
    }
    /// Enumerate and select exactly one device; incomplete identity fails closed.
    /// `pci_bus_id` is the exact NVML v3 bus-ID text, without normalization.
    pub fn open(uuid: &str, pci_bus_id: &str) -> Result<Self, Failure> {
        #[cfg(windows)]
        {
            Ok(Self {
                selected: windows::Session::open()?.select(uuid, pci_bus_id)?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (uuid, pci_bus_id);
            Err(Failure::new(
                Reason::UnsupportedPlatform,
                "live NVML sampling requires Windows",
            ))
        }
    }

    /// Perform only the requested getter and return only its requested component.
    pub fn sample(&self, metric: SampleMetric) -> Field {
        #[cfg(windows)]
        {
            self.selected.sample(metric)
        }
        #[cfg(not(windows))]
        {
            Field::unavailable(
                metric.unit(),
                Failure::new(
                    Reason::UnsupportedPlatform,
                    "live NVML sampling requires Windows",
                ),
            )
        }
    }
}

#[derive(Default)]
pub struct NvmlAdapter {
    #[cfg(windows)]
    session: Option<windows::Session>,
}

impl ReadOnlyAdapter for NvmlAdapter {
    fn provenance(&self) -> Provenance {
        Provenance::Live
    }
    fn initialize(&mut self) -> Result<Runtime, Failure> {
        #[cfg(windows)]
        {
            let session = windows::Session::open()?;
            let runtime = session.runtime();
            self.session = Some(session);
            Ok(runtime)
        }
        #[cfg(not(windows))]
        {
            Err(Failure::new(
                Reason::UnsupportedPlatform,
                "live NVML probe requires Windows",
            ))
        }
    }
    fn device_count(&self) -> Result<u32, Failure> {
        #[cfg(windows)]
        {
            self.session
                .as_ref()
                .ok_or_else(|| Failure::vendor(1, "device_count called before initialization"))?
                .count()
        }
        #[cfg(not(windows))]
        {
            Err(Failure::new(
                Reason::UnsupportedPlatform,
                "live NVML probe requires Windows",
            ))
        }
    }
    fn device(&self, index: u32) -> Result<Device, Failure> {
        #[cfg(windows)]
        {
            self.session
                .as_ref()
                .ok_or_else(|| Failure::vendor(1, "device called before initialization"))?
                .device(index)
        }
        #[cfg(not(windows))]
        {
            let _ = index;
            Err(Failure::new(
                Reason::UnsupportedPlatform,
                "live NVML probe requires Windows",
            ))
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use libloading::os::windows::Library;
    use std::{
        collections::BTreeMap,
        ffi::{c_char, c_void, OsString},
        os::windows::ffi::OsStringExt,
        path::PathBuf,
    };

    // NVML uses C calling convention, 32-bit enums/ints and a single pointer device
    // handle. All buffers below follow the documented C layouts, not Rust enums.
    type Handle = *mut c_void;
    type Init = unsafe extern "C" fn() -> i32;
    type Count = unsafe extern "C" fn(*mut u32) -> i32;
    type ByIndex = unsafe extern "C" fn(u32, *mut Handle) -> i32;
    type Text = unsafe extern "C" fn(*mut c_char, u32) -> i32;
    type DeviceText = unsafe extern "C" fn(Handle, *mut c_char, u32) -> i32;
    type Uint = unsafe extern "C" fn(Handle, *mut u32) -> i32;
    type SelectedUint = unsafe extern "C" fn(Handle, u32, *mut u32) -> i32;
    type Int = unsafe extern "C" fn(Handle, *mut i32) -> i32;
    type UintBounds = unsafe extern "C" fn(Handle, *mut u32, *mut u32) -> i32;
    type IntBounds = unsafe extern "C" fn(Handle, *mut i32, *mut i32) -> i32;
    type PciQuery = unsafe extern "C" fn(Handle, *mut Pci) -> i32;
    type UtilQuery = unsafe extern "C" fn(Handle, *mut Util) -> i32;
    type ClockOffsetQuery = unsafe extern "C" fn(Handle, *mut ClockOffset) -> i32;

    // nvmlClockOffset_v1_t and NVML_STRUCT_VERSION(ClockOffset, 1).
    // Clock and P-state enums have the same 32-bit representation as C int.
    #[repr(C)]
    struct ClockOffset {
        version: u32,
        clock_type: u32,
        pstate: u32,
        current: i32,
        minimum: i32,
        maximum: i32,
    }
    const CLOCK_OFFSET_VERSION: u32 = 24 | (1 << 24);

    #[repr(C)]
    #[derive(Default)]
    struct Pci {
        legacy: [u8; 16],
        domain: u32,
        bus: u32,
        device: u32,
        device_id: u32,
        subsystem_id: u32,
        bus_id: [u8; 32],
    }
    #[repr(C)]
    #[derive(Default)]
    struct Util {
        gpu: u32,
        memory: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }

    // Fake symbol sources exist only in unit-test builds; production always owns
    // the trusted library loaded by Session::open.
    enum Symbols {
        Native(Library),
        #[cfg(test)]
        Fake(fn(&[u8]) -> Option<Box<dyn std::any::Any>>),
    }

    impl Symbols {
        // SAFETY: callers supply the documented ABI for the named getter.
        unsafe fn get<T: Copy + 'static>(&self, name: &[u8]) -> Result<T, Failure> {
            let missing = || Failure::new(Reason::SymbolMissing, symbol_name(name));
            match self {
                Self::Native(library) => library.get::<T>(name).map(|s| *s).map_err(|_| missing()),
                #[cfg(test)]
                Self::Fake(resolve) => resolve(name)
                    .and_then(|s| s.downcast::<T>().ok())
                    .map(|s| *s)
                    .ok_or_else(missing),
            }
        }
    }

    pub(super) struct Session {
        library: Symbols,
        path: PathBuf,
        shutdown: Init,
    }

    pub(super) struct SelectedSession {
        session: Session,
        handle: Handle,
        raw_uuid: String,
        raw_pci: String,
    }

    impl SelectedSession {
        pub(super) fn describe(&self) -> SamplerInfo {
            SamplerInfo {
                raw_uuid: self.raw_uuid.clone(),
                raw_pci: self.raw_pci.clone(),
                runtime: self.session.runtime(),
            }
        }
        pub(super) fn sample(&self, metric: SampleMetric) -> Field {
            let session = &self.session;
            let handle = self.handle;
            match metric {
                SampleMetric::Temperature => session.uint(
                    handle,
                    b"nvmlDeviceGetTemperature\0",
                    metric.unit(),
                    Some(0),
                    200,
                    false,
                ),
                SampleMetric::Power => session.uint(
                    handle,
                    b"nvmlDeviceGetPowerUsage\0",
                    metric.unit(),
                    None,
                    10_000_000,
                    false,
                ),
                SampleMetric::GraphicsClock | SampleMetric::MemoryClock => session.uint(
                    handle,
                    b"nvmlDeviceGetClockInfo\0",
                    metric.unit(),
                    Some(if metric == SampleMetric::GraphicsClock {
                        0
                    } else {
                        2
                    }),
                    100_000,
                    true,
                ),
                SampleMetric::GpuUtilization | SampleMetric::MemoryUtilization => field(
                    metric.unit(),
                    (|| {
                        let mut util = Util::default();
                        // SAFETY: documented getter ABI, writable repr(C) output;
                        // the selected handle's owning Session remains alive.
                        unsafe {
                            check(
                                session.get::<UtilQuery>(b"nvmlDeviceGetUtilizationRates\0")?(
                                    handle, &mut util,
                                ),
                                b"nvmlDeviceGetUtilizationRates",
                            )?;
                        }
                        unsigned(
                            if metric == SampleMetric::GpuUtilization {
                                util.gpu
                            } else {
                                util.memory
                            },
                            100,
                            false,
                        )
                    })(),
                ),
            }
        }
    }

    fn symbol_name(name: &[u8]) -> String {
        String::from_utf8_lossy(name).trim_end_matches('\0').into()
    }
    fn check(status: i32, name: &[u8]) -> Result<(), Failure> {
        if status == 0 {
            Ok(())
        } else {
            Err(Failure::vendor(status, &symbol_name(name)))
        }
    }
    fn invalid(detail: &str) -> Failure {
        Failure::new(Reason::InvalidValue, detail)
    }
    fn text(buffer: &[u8]) -> Result<String, Failure> {
        let end = buffer
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| invalid("unterminated NVML text"))?;
        if end == 0 {
            return Err(invalid("empty NVML text"));
        }
        std::str::from_utf8(&buffer[..end])
            .map(str::to_owned)
            .map_err(|_| invalid("non-UTF8 NVML text"))
    }
    fn field(unit: &str, value: Result<serde_json::Value, Failure>) -> Field {
        match value {
            Ok(value) => Field::available(unit, value),
            Err(failure) => Field::unavailable(unit, failure),
        }
    }
    fn unsigned(value: u32, max: u32, positive: bool) -> Result<serde_json::Value, Failure> {
        if value > max || (positive && value == 0) {
            Err(invalid("out-of-range NVML numeric reading"))
        } else {
            Ok(value.into())
        }
    }

    impl Session {
        pub fn open() -> Result<Self, Failure> {
            // OS API, not environment variables or PATH, establishes the trusted root.
            let mut directory = [0u16; 32768];
            // SAFETY: buffer size is accurate and the kernel function writes UTF-16.
            let length =
                unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) }
                    as usize;
            if length == 0 || length >= directory.len() {
                return Err(Failure::new(
                    Reason::RuntimeLoadFailed,
                    "GetSystemDirectoryW failed",
                ));
            }
            let path = PathBuf::from(OsString::from_wide(&directory[..length])).join("nvml.dll");
            if !path.is_absolute() {
                return Err(Failure::new(
                    Reason::RuntimeLoadFailed,
                    "system directory is not absolute",
                ));
            }
            // Restrict both the primary module and dependency resolution. Never use
            // Library::new, PATH, CWD or a user-supplied/environment DLL path.
            // SAFETY: only the driver-installed DLL in the OS system directory loads.
            let library = unsafe { Library::load_with_flags(&path, 0x00000100 | 0x00000800) }
                .map_err(|error| {
                    Failure::new(
                        if !path.exists() {
                            Reason::RuntimeMissing
                        } else {
                            Reason::RuntimeLoadFailed
                        },
                        format!("{}: {error}", path.display()),
                    )
                })?;
            Self::initialize(Symbols::Native(library), path)
        }

        fn initialize(library: Symbols, path: PathBuf) -> Result<Self, Failure> {
            // Resolve shutdown before initialization so every initialized session
            // has guaranteed cleanup even if later enumeration fails.
            // SAFETY: names and signatures are from the published NVML header.
            let (init, shutdown) = unsafe {
                let init = library.get::<Init>(b"nvmlInit_v2\0")?;
                let shutdown = library.get::<Init>(b"nvmlShutdown\0")?;
                (init, shutdown)
            };
            // SAFETY: correct no-argument signature; library remains alive.
            check(unsafe { init() }, b"nvmlInit_v2")?;
            Ok(Self {
                library,
                path,
                shutdown,
            })
        }

        // SAFETY: callers use only the ABI aliases above and documented getter
        // symbols; returned pointers never outlive the owning Session/library.
        unsafe fn get<T: Copy + 'static>(&self, name: &[u8]) -> Result<T, Failure> {
            self.library.get::<T>(name)
        }
        fn setter_presence(&self, name: &[u8]) -> SymbolPresence {
            // SAFETY: inspect only an opaque address. It is never transmuted or called.
            if unsafe { self.library.get::<*const c_void>(name) }.is_ok() {
                SymbolPresence::Present
            } else {
                SymbolPresence::Absent
            }
        }
        fn system_text(&self, name: &[u8]) -> Field {
            field(
                "text",
                (|| {
                    let mut buffer = [0u8; 256];
                    // SAFETY: published Text ABI, writable bounded buffer, live library.
                    unsafe {
                        check(
                            self.get::<Text>(name)?(
                                buffer.as_mut_ptr().cast(),
                                buffer.len() as u32,
                            ),
                            name,
                        )?;
                    }
                    text(&buffer).map(Into::into)
                })(),
            )
        }
        pub fn runtime(&self) -> Runtime {
            let cuda = (|| {
                let mut version = 0;
                // SAFETY: published getter takes a writable C int.
                unsafe {
                    check(
                        self.get::<unsafe extern "C" fn(*mut i32) -> i32>(
                            b"nvmlSystemGetCudaDriverVersion_v2\0",
                        )?(&mut version),
                        b"nvmlSystemGetCudaDriverVersion_v2",
                    )?;
                }
                if version <= 0 {
                    Err(invalid("invalid CUDA driver version"))
                } else {
                    Ok(version.into())
                }
            })();
            Runtime {
                library_path: Some(self.path.display().to_string()),
                driver_version: self.system_text(b"nvmlSystemGetDriverVersion\0"),
                nvml_version: self.system_text(b"nvmlSystemGetNVMLVersion\0"),
                cuda_driver_version: field("CUDA encoded integer", cuda),
                header_api_version: 13,
            }
        }
        pub fn count(&self) -> Result<u32, Failure> {
            let mut count = 0;
            // SAFETY: published count getter; pointer is writable.
            unsafe {
                check(
                    self.get::<Count>(b"nvmlDeviceGetCount_v2\0")?(&mut count),
                    b"nvmlDeviceGetCount_v2",
                )?;
            }
            Ok(count)
        }
        pub(super) fn select(
            self,
            uuid: &str,
            pci_bus_id: &str,
        ) -> Result<SelectedSession, Failure> {
            if uuid.is_empty() || pci_bus_id.is_empty() {
                return Err(invalid("empty NVML selection identity"));
            }
            let count = self.count()?;
            if count > 1024 {
                return Err(invalid("device count exceeds safety bound (1024)"));
            }
            let mut selected = None;
            for index in 0..count {
                let handle = self.handle(index)?;
                let device_uuid = match self.device_text(handle, b"nvmlDeviceGetUUID\0").reading {
                    Reading::Available { value } => value,
                    Reading::Unavailable { failure } => return Err(failure),
                };
                let pci = self.pci_bus_id(handle)?;
                if device_uuid.as_str() == Some(uuid)
                    && pci == pci_bus_id
                    && selected
                        .replace((
                            handle,
                            device_uuid.as_str().expect("exact UUID match").to_owned(),
                            pci,
                        ))
                        .is_some()
                {
                    return Err(invalid("ambiguous NVML UUID and PCI identity"));
                }
            }
            let (handle, raw_uuid, raw_pci) =
                selected.ok_or_else(|| invalid("NVML UUID and PCI identity not found"))?;
            Ok(SelectedSession {
                session: self,
                handle,
                raw_uuid,
                raw_pci,
            })
        }
        fn device_text(&self, handle: Handle, name: &[u8]) -> Field {
            field(
                "text",
                (|| {
                    let mut buffer = [0u8; 256];
                    // SAFETY: handle obtained from this session, sized writable buffer.
                    unsafe {
                        check(
                            self.get::<DeviceText>(name)?(
                                handle,
                                buffer.as_mut_ptr().cast(),
                                buffer.len() as u32,
                            ),
                            name,
                        )?;
                    }
                    text(&buffer).map(Into::into)
                })(),
            )
        }
        fn uint(
            &self,
            handle: Handle,
            name: &[u8],
            unit: &str,
            selector: Option<u32>,
            max: u32,
            positive: bool,
        ) -> Field {
            field(
                unit,
                (|| {
                    let mut value = 0;
                    // SAFETY: selection determines the exact documented getter ABI.
                    unsafe {
                        let status = if let Some(selector) = selector {
                            self.get::<SelectedUint>(name)?(handle, selector, &mut value)
                        } else {
                            self.get::<Uint>(name)?(handle, &mut value)
                        };
                        check(status, name)?;
                    }
                    unsigned(value, max, positive)
                })(),
            )
        }
        fn offset(&self, handle: Handle, name: &[u8]) -> Field {
            field(
                "MHz",
                (|| {
                    let mut value = 0;
                    // SAFETY: documented signed offset getter ABI.
                    unsafe {
                        check(self.get::<Int>(name)?(handle, &mut value), name)?;
                    }
                    if !(-100_000..=100_000).contains(&value) {
                        Err(invalid("invalid clock offset"))
                    } else {
                        Ok(value.into())
                    }
                })(),
            )
        }
        fn bounds(&self, handle: Handle, name: &[u8], signed: bool) -> (Field, Field) {
            let result: Result<(i64, i64), Failure> = (|| {
                let (min, max) = if signed {
                    let (mut min, mut max) = (0i32, 0i32);
                    // SAFETY: documented signed offset bounds ABI.
                    unsafe {
                        check(
                            self.get::<IntBounds>(name)?(handle, &mut min, &mut max),
                            name,
                        )?;
                    }
                    if !(-100_000..=100_000).contains(&min) || !(-100_000..=100_000).contains(&max)
                    {
                        return Err(invalid("invalid clock offset bounds"));
                    }
                    (i64::from(min), i64::from(max))
                } else {
                    let (mut min, mut max) = (0u32, 0u32);
                    // SAFETY: documented unsigned power bounds ABI.
                    unsafe {
                        check(
                            self.get::<UintBounds>(name)?(handle, &mut min, &mut max),
                            name,
                        )?;
                    }
                    unsigned(min, 10_000_000, true)?;
                    unsigned(max, 10_000_000, true)?;
                    (i64::from(min), i64::from(max))
                };
                if min > max {
                    Err(invalid("inverted bounds"))
                } else {
                    Ok((min, max))
                }
            })();
            let unit = if signed { "MHz" } else { "mW" };
            match result {
                Ok((min, max)) => (Field::available(unit, min), Field::available(unit, max)),
                Err(failure) => (
                    Field::unavailable(unit, failure.clone()),
                    Field::unavailable(unit, failure),
                ),
            }
        }
        fn modern_offsets(&self, handle: Handle, clock_type: u32) -> BTreeMap<String, Field> {
            let result = (|| {
                let mut info = ClockOffset {
                    version: CLOCK_OFFSET_VERSION,
                    clock_type,
                    pstate: 0,
                    // Detect success without populated output rather than fabricating zero.
                    current: i32::MAX,
                    minimum: i32::MAX,
                    maximum: i32::MAX,
                };
                // SAFETY: published getter ABI, writable 24-byte versioned struct,
                // graphics/memory domain and P0 inputs, owning session remains alive.
                unsafe {
                    check(
                        self.get::<ClockOffsetQuery>(b"nvmlDeviceGetClockOffsets\0")?(
                            handle, &mut info,
                        ),
                        b"nvmlDeviceGetClockOffsets",
                    )?;
                }
                if info.version != CLOCK_OFFSET_VERSION
                    || info.clock_type != clock_type
                    || info.pstate != 0
                {
                    return Err(invalid("clock offset response changed query identity"));
                }
                Ok(info)
            })();
            let checked = |value: i32| {
                if (-100_000..=100_000).contains(&value) {
                    Ok(serde_json::Value::from(value))
                } else {
                    Err(invalid("invalid modern clock offset"))
                }
            };
            match result {
                Ok(info) => {
                    let bounds_valid = checked(info.minimum).is_ok()
                        && checked(info.maximum).is_ok()
                        && info.minimum <= info.maximum;
                    let bound = |value| {
                        field(
                            "MHz",
                            if bounds_valid {
                                checked(value)
                            } else {
                                Err(invalid("invalid modern clock offset bounds"))
                            },
                        )
                    };
                    let current =
                        if bounds_valid && !(info.minimum..=info.maximum).contains(&info.current) {
                            Err(invalid("modern clock offset is outside returned bounds"))
                        } else {
                            checked(info.current)
                        };
                    [
                        ("current".into(), field("MHz", current)),
                        ("minimum".into(), bound(info.minimum)),
                        ("maximum".into(), bound(info.maximum)),
                    ]
                    .into()
                }
                Err(failure) => ["current", "minimum", "maximum"]
                    .into_iter()
                    .map(|key| (key.into(), Field::unavailable("MHz", failure.clone())))
                    .collect(),
            }
        }
        fn handle(&self, index: u32) -> Result<Handle, Failure> {
            let mut handle = std::ptr::null_mut();
            // SAFETY: documented index getter; handle belongs to this session.
            unsafe {
                check(
                    self.get::<ByIndex>(b"nvmlDeviceGetHandleByIndex_v2\0")?(index, &mut handle),
                    b"nvmlDeviceGetHandleByIndex_v2",
                )?;
            }
            if handle.is_null() {
                return Err(invalid("NVML returned null device handle"));
            }
            Ok(handle)
        }
        fn pci_bus_id(&self, handle: Handle) -> Result<String, Failure> {
            let mut pci = Pci::default();
            // SAFETY: repr(C) Pci is the 68-byte nvmlPciInfo_t v3 layout.
            unsafe {
                check(
                    self.get::<PciQuery>(b"nvmlDeviceGetPciInfo_v3\0")?(handle, &mut pci),
                    b"nvmlDeviceGetPciInfo_v3",
                )?;
            }
            text(&pci.bus_id)
        }
        pub fn device(&self, index: u32) -> Result<Device, Failure> {
            let handle = self.handle(index)?;
            let pci = self.pci_bus_id(handle).map(Into::into);
            let identity = [
                (
                    "name".into(),
                    self.device_text(handle, b"nvmlDeviceGetName\0"),
                ),
                (
                    "uuid".into(),
                    self.device_text(handle, b"nvmlDeviceGetUUID\0"),
                ),
                ("pci_bus_id".into(), field("text", pci)),
            ]
            .into();
            let mut telemetry: BTreeMap<String, Field> = [
                (
                    "temperature".into(),
                    self.uint(
                        handle,
                        b"nvmlDeviceGetTemperature\0",
                        "degC",
                        Some(0),
                        200,
                        false,
                    ),
                ),
                (
                    "graphics_clock".into(),
                    self.uint(
                        handle,
                        b"nvmlDeviceGetClockInfo\0",
                        "MHz",
                        Some(0),
                        100_000,
                        true,
                    ),
                ),
                (
                    "memory_clock".into(),
                    self.uint(
                        handle,
                        b"nvmlDeviceGetClockInfo\0",
                        "MHz",
                        Some(2),
                        100_000,
                        true,
                    ),
                ),
                (
                    "power".into(),
                    self.uint(
                        handle,
                        b"nvmlDeviceGetPowerUsage\0",
                        "mW",
                        None,
                        10_000_000,
                        false,
                    ),
                ),
            ]
            .into();
            let utilization: Result<Util, Failure> = (|| {
                let mut util = Util::default();
                // SAFETY: Util matches nvmlUtilization_t, two unsigned ints.
                unsafe {
                    check(
                        self.get::<UtilQuery>(b"nvmlDeviceGetUtilizationRates\0")?(
                            handle, &mut util,
                        ),
                        b"nvmlDeviceGetUtilizationRates",
                    )?;
                }
                Ok(util)
            })();
            let (gpu, memory) = match utilization {
                Ok(util) => (
                    field("%", unsigned(util.gpu, 100, false)),
                    field("%", unsigned(util.memory, 100, false)),
                ),
                Err(failure) => (
                    Field::unavailable("%", failure.clone()),
                    Field::unavailable("%", failure),
                ),
            };
            telemetry.insert("gpu_utilization".into(), gpu);
            telemetry.insert("memory_utilization".into(), memory);
            let mut controls = BTreeMap::new();
            let (min, max) = self.bounds(
                handle,
                b"nvmlDeviceGetPowerManagementLimitConstraints\0",
                false,
            );
            controls.insert(
                "power_limit".into(),
                Control {
                    setter_symbol_presence: self
                        .setter_presence(b"nvmlDeviceSetPowerManagementLimit\0"),
                    write_verification: WriteVerification::NotTested,
                    getters: [
                        (
                            "current".into(),
                            self.uint(
                                handle,
                                b"nvmlDeviceGetPowerManagementLimit\0",
                                "mW",
                                None,
                                10_000_000,
                                true,
                            ),
                        ),
                        (
                            "default".into(),
                            self.uint(
                                handle,
                                b"nvmlDeviceGetPowerManagementDefaultLimit\0",
                                "mW",
                                None,
                                10_000_000,
                                true,
                            ),
                        ),
                        ("minimum".into(), min),
                        ("maximum".into(), max),
                    ]
                    .into(),
                },
            );
            for (name, getter, bounds, setter) in [
                (
                    "graphics_clock_offset",
                    b"nvmlDeviceGetGpcClkVfOffset\0".as_slice(),
                    b"nvmlDeviceGetGpcClkMinMaxVfOffset\0".as_slice(),
                    b"nvmlDeviceSetGpcClkVfOffset\0".as_slice(),
                ),
                (
                    "memory_clock_offset",
                    b"nvmlDeviceGetMemClkVfOffset\0".as_slice(),
                    b"nvmlDeviceGetMemClkMinMaxVfOffset\0".as_slice(),
                    b"nvmlDeviceSetMemClkVfOffset\0".as_slice(),
                ),
            ] {
                let (min, max) = self.bounds(handle, bounds, true);
                controls.insert(
                    name.into(),
                    Control {
                        setter_symbol_presence: self.setter_presence(setter),
                        write_verification: WriteVerification::NotTested,
                        getters: [
                            ("current".into(), self.offset(handle, getter)),
                            ("minimum".into(), min),
                            ("maximum".into(), max),
                        ]
                        .into(),
                    },
                );
            }
            for (name, clock_type) in [
                ("graphics_clock_offset_p0", 0),
                ("memory_clock_offset_p0", 2),
            ] {
                controls.insert(
                    name.into(),
                    Control {
                        setter_symbol_presence: self
                            .setter_presence(b"nvmlDeviceSetClockOffsets\0"),
                        write_verification: WriteVerification::NotTested,
                        getters: self.modern_offsets(handle, clock_type),
                    },
                );
            }
            Ok(Device {
                index,
                failure: None,
                identity,
                telemetry,
                controls,
            })
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: successful init is paired once, before unloading the library.
            // Shutdown releases NVML client resources; it does not reset the driver.
            let status = unsafe { (self.shutdown)() };
            if status != 0 {
                eprintln!(
                    "{}",
                    serde_json::json!({"reason":"nvml_shutdown_failed", "vendor_status":status})
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        include!("nvml_tests.rs");
        #[test]
        fn abi_layouts_and_checked_values() {
            assert_eq!(std::mem::size_of::<Pci>(), 68);
            assert_eq!(std::mem::size_of::<Util>(), 8);
            assert!(unsigned(u32::MAX, 100, false).is_err());
            assert!(unsigned(0, 100, true).is_err());
            assert!(unsigned(0, 100, false).is_ok());
            assert!(text(&[b'x'; 32]).is_err());
            assert!(text(&[0]).is_err());
        }
    }
}
