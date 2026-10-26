//! Originally authored minimal CUDA Driver ABI. Sources: CUDA 12.9.1 Driver
//! API; signatures/constants cross-checked with cudarc 0.17.3 commit
//! fdf313cc10cf5504953ac9fe6a5dc78dc6ec4152. No copied header/sample blocks.
//! No Toolkit, runtime, NVML, reset, tuning setters or user module/path inputs.
use crate::worker::{Backend, Fault, Identity, Provenance, Query};
use autotune_fma_reference::{input::Shape, profile::Profile};
use libloading::os::windows::Library;
use std::{
    ffi::{c_void, OsString},
    marker::PhantomData,
    mem::ManuallyDrop,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    ptr::null_mut,
    rc::Rc,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

type Handle = *mut c_void;
type Init = unsafe extern "system" fn(u32) -> i32;
type IntOut = unsafe extern "system" fn(*mut i32) -> i32;
type DeviceGet = unsafe extern "system" fn(*mut i32, i32) -> i32;
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Uuid {
    bytes: [u8; 16],
}
type DeviceUuid = unsafe extern "system" fn(*mut Uuid, i32) -> i32;
type Attribute = unsafe extern "system" fn(*mut i32, i32, i32) -> i32;
type ContextCreate = unsafe extern "system" fn(*mut Handle, u32, i32) -> i32;
type HandleCall = unsafe extern "system" fn(Handle) -> i32;
type ModuleLoad = unsafe extern "system" fn(*mut Handle, *const c_void) -> i32;
type Function = unsafe extern "system" fn(*mut Handle, Handle, *const i8) -> i32;
type MemInfo = unsafe extern "system" fn(*mut usize, *mut usize) -> i32;
type Alloc = unsafe extern "system" fn(*mut u64, usize) -> i32;
type Free = unsafe extern "system" fn(u64) -> i32;
type Upload = unsafe extern "system" fn(u64, *const c_void, usize) -> i32;
type Download = unsafe extern "system" fn(*mut c_void, u64, usize) -> i32;
type StreamCreate = unsafe extern "system" fn(*mut Handle, u32) -> i32;
type Launch = unsafe extern "system" fn(
    Handle,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    Handle,
    *mut *mut c_void,
    *mut *mut c_void,
) -> i32;

macro_rules! api {
    ($($field:ident : $ty:ty = $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy)]
        struct Api { $($field: $ty),+ }
        impl Api {
            unsafe fn load(library: &Library) -> Result<Self, Fault> {
                Ok(Self { $($field: *library.get::<$ty>(concat!($name,"\0").as_bytes()).map_err(|_| Fault {operation:$name,raw_status:-1})?),+ })
            }
            #[cfg(test)]
            fn fake(resolve: impl Fn(&str)->Option<Box<dyn std::any::Any>>)->Result<Self,Fault> {
                Ok(Self { $($field: *resolve($name).and_then(|v|v.downcast::<$ty>().ok()).ok_or(Fault{operation:$name,raw_status:-1})?),+ })
            }
        }
        pub const EXPORTS: &[&str] = &[$($name),+];
    }
}
api! {
    init: Init = "cuInit", version: IntOut = "cuDriverGetVersion",
    count: IntOut = "cuDeviceGetCount", device: DeviceGet = "cuDeviceGet",
    uuid: DeviceUuid = "cuDeviceGetUuid_v2", attr: Attribute = "cuDeviceGetAttribute",
    ctx_create: ContextCreate = "cuCtxCreate_v2", ctx_device: IntOut = "cuCtxGetDevice", ctx_destroy: HandleCall = "cuCtxDestroy_v2",
    module_load: ModuleLoad = "cuModuleLoadData", function: Function = "cuModuleGetFunction", module_unload: HandleCall = "cuModuleUnload",
    mem_info: MemInfo = "cuMemGetInfo_v2", alloc: Alloc = "cuMemAlloc_v2", free: Free = "cuMemFree_v2",
    upload: Upload = "cuMemcpyHtoD_v2", download: Download = "cuMemcpyDtoH_v2",
    stream_create: StreamCreate = "cuStreamCreate", query: HandleCall = "cuStreamQuery", stream_destroy: HandleCall = "cuStreamDestroy_v2",
    launch: Launch = "cuLaunchKernel",
}
pub const PTX: &str = include_str!("fma.ptx");
fn checked(operation: &'static str, raw_status: i32) -> Result<(), Fault> {
    if raw_status == 0 {
        Ok(())
    } else {
        Err(Fault {
            operation,
            raw_status,
        })
    }
}
/// Internal validation sentinel, never a CUDA status: `-1` marks a missing or
/// mistyped export (set in `Api::load`), `-2` a rejected pre-call validation.
fn invalid(operation: &'static str) -> Fault {
    Fault {
        operation,
        raw_status: -2,
    }
}

/// Thread-affine owned computation resources. No native calls occur in Drop.
/// A failed/abandoned session intentionally retains the DLL/resources until
/// process exit. This is not a claim of resource release or GPU quiescence.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    InFlight,
    Complete,
    Poisoned,
    Closed,
}
pub struct CudaBackend {
    _library: Option<ManuallyDrop<Library>>,
    api: Api,
    identity: Identity,
    shape: Shape,
    context: Handle,
    module: Handle,
    function: Handle,
    stream: Handle,
    buffers: [u64; 4],
    state: State,
    _thread_affinity: PhantomData<Rc<()>>,
    driver_path: PathBuf,
    driver_version: i32,
}
impl CudaBackend {
    /// Loads the system driver and creates an owned context/module/buffers.
    ///
    /// # Safety
    /// This performs native driver calls. Caller must have separately approved
    /// isolation, monitor, clock and exact-manifest execution authority. The
    /// shipped fake binary never calls this; this change grants no such authority.
    pub unsafe fn open(identity: Identity, shape: Shape) -> Result<Self, Fault> {
        if !identity.valid() {
            return Err(invalid("identity"));
        }
        let mut directory = [0u16; 32768];
        let length = GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) as usize;
        if length == 0 || length >= directory.len() {
            return Err(invalid("GetSystemDirectoryW"));
        }
        let path = PathBuf::from(OsString::from_wide(&directory[..length])).join("nvcuda.dll");
        if !path.is_absolute() {
            return Err(invalid("system driver path"));
        }
        let library = Library::load_with_flags(&path, 0x100 | 0x800)
            .map_err(|_| invalid("load system nvcuda"))?;
        let raw = library.into_raw();
        // Retain the loaded module from the first successful load: every later
        // early error leaks it instead of unloading after `cuInit`.
        let library = ManuallyDrop::new(Library::from_raw(raw));
        let mut loaded = [0u16; 32768];
        let length =
            GetModuleFileNameW(raw as *mut c_void, loaded.as_mut_ptr(), loaded.len() as u32)
                as usize;
        if length == 0 || length >= loaded.len() {
            return Err(invalid("loaded module path"));
        }
        let loaded = PathBuf::from(OsString::from_wide(&loaded[..length]));
        if loaded
            .canonicalize()
            .map_err(|_| invalid("loaded path canonicalization"))?
            != path
                .canonicalize()
                .map_err(|_| invalid("trusted path canonicalization"))?
        {
            return Err(invalid("loaded path mismatch"));
        }
        let api = Api::load(&library)?; // all exports before any CUDA call
        checked("cuInit", (api.init)(0))?;
        let mut version = -1;
        checked("cuDriverGetVersion", (api.version)(&mut version))?;
        if version < 10010 {
            return Err(invalid("driver version"));
        }
        let mut count = -1;
        checked("cuDeviceGetCount", (api.count)(&mut count))?;
        if !(1..=64).contains(&count) {
            return Err(invalid("device count"));
        }
        let mut selected = None;
        for ordinal in 0..count {
            let mut device = -1;
            checked("cuDeviceGet", (api.device)(&mut device, ordinal))?;
            if device < 0 {
                return Err(invalid("device"));
            }
            let mut uuid = Uuid::default();
            checked("cuDeviceGetUuid_v2", (api.uuid)(&mut uuid, device))?;
            if uuid.bytes == identity.uuid {
                if selected.is_some() {
                    return Err(invalid("ambiguous UUID"));
                }
                for (attribute, expected) in [
                    (50, i32::from(identity.domain)),
                    (33, i32::from(identity.bus)),
                    (34, i32::from(identity.device)),
                    (75, 7),
                    (76, 5),
                ] {
                    let mut actual = -1;
                    checked(
                        "cuDeviceGetAttribute",
                        (api.attr)(&mut actual, attribute, device),
                    )?;
                    if actual != expected {
                        return Err(invalid("PCI/capability mismatch"));
                    }
                }
                selected = Some(device);
            }
        }
        let device = selected.ok_or_else(|| invalid("UUID not found"))?;
        let mut session = Self {
            _library: Some(library),
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
            driver_version: version,
        };
        // From this point, an error leaks rather than invoking speculative cleanup.
        checked(
            "cuCtxCreate_v2",
            (api.ctx_create)(&mut session.context, 0, device),
        )?;
        if session.context.is_null() {
            return Err(invalid("null context"));
        }
        let mut current = -1;
        checked("cuCtxGetDevice", (api.ctx_device)(&mut current))?;
        if current != device {
            return Err(invalid("context device mismatch"));
        }
        let ptx = std::ffi::CString::new(PTX).map_err(|_| invalid("PTX NUL"))?;
        checked(
            "cuModuleLoadData",
            (api.module_load)(&mut session.module, ptx.as_ptr().cast()),
        )?;
        if session.module.is_null() {
            return Err(invalid("null module"));
        }
        checked(
            "cuModuleGetFunction",
            (api.function)(
                &mut session.function,
                session.module,
                c"autotune_fma".as_ptr(),
            ),
        )?;
        if session.function.is_null() {
            return Err(invalid("null function"));
        }
        let (mut free, mut total) = (0, 0);
        checked("cuMemGetInfo_v2", (api.mem_info)(&mut free, &mut total))?;
        let limits = Profile::approved_design().limits;
        let (free, total, planned) = (free as u64, total as u64, shape.application_buffer_bytes());
        if free > total
            || free < planned + limits.free_gpu_headroom_bytes
            || planned > limits.application_gpu_bytes
        {
            return Err(invalid("memory headroom"));
        }
        for buffer in &mut session.buffers {
            checked(
                "cuMemAlloc_v2",
                (api.alloc)(buffer, shape.lanes() as usize * 4),
            )?;
            if *buffer == 0 {
                return Err(invalid("null allocation"));
            }
        }
        checked(
            "cuStreamCreate",
            (api.stream_create)(&mut session.stream, 1),
        )?;
        if session.stream.is_null() {
            return Err(invalid("null stream"));
        }
        session.state = State::Ready;
        Ok(session)
    }
    pub fn driver_path(&self) -> &std::path::Path {
        &self.driver_path
    }
    pub fn driver_version(&self) -> i32 {
        self.driver_version
    }
    /// Explicit teardown only after observed stream idle; raw cleanup failures
    /// leave remaining resources retained, never retried or called from Drop.
    pub fn close(&mut self) -> Result<(), Fault> {
        if !matches!(self.state, State::Ready | State::Complete) {
            return Err(invalid("cleanup without idle"));
        }
        self.state = State::Poisoned;
        unsafe {
            checked("cuStreamDestroy_v2", (self.api.stream_destroy)(self.stream))?;
            for buffer in self.buffers {
                checked("cuMemFree_v2", (self.api.free)(buffer))?;
            }
            checked("cuModuleUnload", (self.api.module_unload)(self.module))?;
            checked("cuCtxDestroy_v2", (self.api.ctx_destroy)(self.context))?;
            self.state = State::Closed;
            if let Some(library) = &mut self._library {
                ManuallyDrop::drop(library);
            }
            self._library = None;
        }
        Ok(())
    }
}
impl Backend for CudaBackend {
    fn provenance(&self) -> Provenance {
        Provenance::Native
    }
    fn identity(&mut self) -> Result<Identity, Fault> {
        if matches!(self.state, State::Closed | State::Poisoned) {
            Err(invalid("closed"))
        } else {
            Ok(self.identity)
        }
    }
    fn submit(
        &mut self,
        a: &[f32],
        b: &[f32],
        x: &[f32],
        steps: u32,
        sentinel: &[f32],
    ) -> Result<(), Fault> {
        if self.state != State::Ready
            || steps != self.shape.steps().value()
            || [a, b, x, sentinel]
                .iter()
                .any(|v| v.len() != self.shape.lanes() as usize)
            || !sentinel.iter().all(|v| v.is_nan())
        {
            return Err(invalid("submit state/shape"));
        }
        self.state = State::Poisoned;
        unsafe {
            for (device, host) in self.buffers.iter().zip([a, b, x, sentinel]) {
                checked(
                    "cuMemcpyHtoD_v2",
                    (self.api.upload)(*device, host.as_ptr().cast(), host.len() * 4),
                )?;
            }
            // Pageable synchronous uploads may return after host staging but
            // before device DMA finishes. The nonblocking compute stream must
            // not race that default-stream work. Fail closed unless its idle
            // completion is observed; NOT_READY is uncertainty, not permission
            // to launch or retry this submit.
            checked(
                "cuStreamQuery(upload completion)",
                (self.api.query)(null_mut()),
            )?;
            let mut n = self.shape.lanes();
            let mut k = steps;
            let mut parameters = [
                (&mut self.buffers[0] as *mut u64).cast(),
                (&mut self.buffers[1] as *mut u64).cast(),
                (&mut self.buffers[2] as *mut u64).cast(),
                (&mut self.buffers[3] as *mut u64).cast(),
                (&mut n as *mut u32).cast(),
                (&mut k as *mut u32).cast(),
            ];
            checked(
                "cuLaunchKernel",
                (self.api.launch)(
                    self.function,
                    n.div_ceil(256),
                    1,
                    1,
                    256,
                    1,
                    1,
                    0,
                    self.stream,
                    parameters.as_mut_ptr(),
                    null_mut(),
                ),
            )?;
            self.state = State::InFlight;
            Ok(())
        }
    }
    fn query(&mut self) -> Result<Query, Fault> {
        if self.state != State::InFlight {
            return Err(invalid("query without in-flight launch"));
        }
        match unsafe { (self.api.query)(self.stream) } {
            0 => {
                self.state = State::Complete;
                Ok(Query::Complete)
            }
            600 => Ok(Query::Pending),
            status => {
                self.state = State::Poisoned;
                Err(Fault {
                    operation: "cuStreamQuery",
                    raw_status: status,
                })
            }
        }
    }
    fn readback(&mut self, output: &mut [f32]) -> Result<usize, Fault> {
        if self.state != State::Complete || output.len() != self.shape.lanes() as usize {
            return Err(invalid("readback without idle/shape"));
        }
        self.state = State::Poisoned;
        unsafe {
            checked(
                "cuMemcpyDtoH_v2",
                (self.api.download)(
                    output.as_mut_ptr().cast(),
                    self.buffers[3],
                    output.len() * 4,
                ),
            )?;
        }
        self.state = State::Ready;
        Ok(output.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
    use std::sync::Mutex;
    static SERIAL: Mutex<()> = Mutex::new(());
    static STATUS: AtomicI32 = AtomicI32::new(600);
    static UPLOAD_STATUS: AtomicI32 = AtomicI32::new(0);
    static UPLOADS: AtomicUsize = AtomicUsize::new(0);
    static LAUNCHES: AtomicUsize = AtomicUsize::new(0);
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        let guard = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        STATUS.store(600, Ordering::SeqCst);
        UPLOAD_STATUS.store(0, Ordering::SeqCst);
        UPLOADS.store(0, Ordering::SeqCst);
        LAUNCHES.store(0, Ordering::SeqCst);
        guard
    }
    unsafe extern "system" fn init(_: u32) -> i32 {
        0
    }
    unsafe extern "system" fn int_out(out: *mut i32) -> i32 {
        *out = 1;
        0
    }
    unsafe extern "system" fn device(out: *mut i32, _: i32) -> i32 {
        *out = 0;
        0
    }
    unsafe extern "system" fn uuid(out: *mut Uuid, _: i32) -> i32 {
        *out = Uuid { bytes: [1; 16] };
        0
    }
    unsafe extern "system" fn attr(out: *mut i32, _: i32, _: i32) -> i32 {
        *out = 0;
        0
    }
    unsafe extern "system" fn context(out: *mut Handle, _: u32, _: i32) -> i32 {
        *out = std::ptr::dangling_mut();
        0
    }
    unsafe extern "system" fn handle(_: Handle) -> i32 {
        0
    }
    unsafe extern "system" fn query(_: Handle) -> i32 {
        STATUS.load(Ordering::SeqCst)
    }
    unsafe extern "system" fn module(out: *mut Handle, _: *const c_void) -> i32 {
        *out = std::ptr::dangling_mut();
        0
    }
    unsafe extern "system" fn function(out: *mut Handle, _: Handle, _: *const i8) -> i32 {
        *out = std::ptr::dangling_mut();
        0
    }
    unsafe extern "system" fn mem_info(free: *mut usize, total: *mut usize) -> i32 {
        *free = 1 << 30;
        *total = 1 << 30;
        0
    }
    unsafe extern "system" fn alloc(out: *mut u64, _: usize) -> i32 {
        *out = 64;
        0
    }
    unsafe extern "system" fn free(_: u64) -> i32 {
        0
    }
    unsafe extern "system" fn upload(_: u64, src: *const c_void, len: usize) -> i32 {
        assert!(!src.is_null());
        assert_eq!(len, 8);
        UPLOADS.fetch_add(1, Ordering::SeqCst);
        UPLOAD_STATUS.load(Ordering::SeqCst)
    }
    unsafe extern "system" fn download(out: *mut c_void, _: u64, len: usize) -> i32 {
        assert_eq!(len, 8);
        std::ptr::write_bytes(out, 0, len);
        0
    }
    unsafe extern "system" fn stream(out: *mut Handle, flags: u32) -> i32 {
        assert_eq!(flags, 1);
        *out = std::ptr::dangling_mut();
        0
    }
    unsafe extern "system" fn launch(
        _: Handle,
        gx: u32,
        gy: u32,
        gz: u32,
        bx: u32,
        by: u32,
        bz: u32,
        shared: u32,
        _: Handle,
        params: *mut *mut c_void,
        extra: *mut *mut c_void,
    ) -> i32 {
        assert_eq!([gx, gy, gz, bx, by, bz, shared], [1, 1, 1, 256, 1, 1, 0]);
        assert!(extra.is_null());
        assert_eq!(*(*params.add(4)).cast::<u32>(), 2);
        assert_eq!(*(*params.add(5)).cast::<u32>(), 4096);
        LAUNCHES.fetch_add(1, Ordering::SeqCst);
        0
    }
    fn symbols(name: &str) -> Option<Box<dyn std::any::Any>> {
        Some(match name {
            "cuInit" => Box::new(init as Init),
            "cuDriverGetVersion" | "cuDeviceGetCount" | "cuCtxGetDevice" => {
                Box::new(int_out as IntOut)
            }
            "cuDeviceGet" => Box::new(device as DeviceGet),
            "cuDeviceGetUuid_v2" => Box::new(uuid as DeviceUuid),
            "cuDeviceGetAttribute" => Box::new(attr as Attribute),
            "cuCtxCreate_v2" => Box::new(context as ContextCreate),
            "cuStreamQuery" => Box::new(query as HandleCall),
            "cuCtxDestroy_v2" | "cuModuleUnload" | "cuStreamDestroy_v2" => {
                Box::new(handle as HandleCall)
            }
            "cuModuleLoadData" => Box::new(module as ModuleLoad),
            "cuModuleGetFunction" => Box::new(function as Function),
            "cuMemGetInfo_v2" => Box::new(mem_info as MemInfo),
            "cuMemAlloc_v2" => Box::new(alloc as Alloc),
            "cuMemFree_v2" => Box::new(free as Free),
            "cuMemcpyHtoD_v2" => Box::new(upload as Upload),
            "cuMemcpyDtoH_v2" => Box::new(download as Download),
            "cuStreamCreate" => Box::new(stream as StreamCreate),
            "cuLaunchKernel" => Box::new(launch as Launch),
            _ => return None,
        })
    }
    fn fixture() -> CudaBackend {
        CudaBackend {
            _library: None,
            api: Api::fake(symbols).unwrap(),
            identity: Identity {
                uuid: [1; 16],
                domain: 0,
                bus: 0,
                device: 0,
            },
            shape: Shape::new(2, 4096).unwrap(),
            context: std::ptr::dangling_mut(),
            module: std::ptr::dangling_mut(),
            function: std::ptr::dangling_mut(),
            stream: std::ptr::dangling_mut(),
            buffers: [64; 4],
            state: State::Ready,
            _thread_affinity: PhantomData,
            driver_path: PathBuf::from("typed-fake-not-a-DLL"),
            driver_version: 12090,
        }
    }
    #[test]
    fn typed_fake_exports_launch_readback_and_sticky_uncertainty() {
        let _guard = serial();
        assert!(Api::fake(|_| None).is_err());
        for missing in EXPORTS {
            let result = Api::fake(|name| {
                if name == *missing {
                    None
                } else {
                    symbols(name)
                }
            });
            assert_eq!(result.err().unwrap().operation, *missing);
        }
        assert!(Api::fake(|name| if name == "cuLaunchKernel" {
            Some(Box::new(0u32))
        } else {
            symbols(name)
        })
        .is_err());
        let mut backend = fixture();
        let values = [1.0; 2];
        let sentinel = [f32::NAN; 2];
        let mut output = [f32::NAN; 2];
        assert!(backend.readback(&mut output).is_err());
        STATUS.store(0, Ordering::SeqCst);
        backend
            .submit(&values, &values, &values, 4096, &sentinel)
            .unwrap();
        assert_eq!(UPLOADS.load(Ordering::SeqCst), 4);
        STATUS.store(600, Ordering::SeqCst);
        assert!(backend
            .submit(&values, &values, &values, 4096, &sentinel)
            .is_err());
        assert_eq!(backend.query().unwrap(), Query::Pending);
        assert!(backend.close().is_err());
        let mut idle = fixture();
        idle.close().unwrap();
        assert!(idle.close().is_err());
        assert!(idle.query().is_err());
        assert!(idle.identity().is_err());
        STATUS.store(0, Ordering::SeqCst);
        assert_eq!(backend.query().unwrap(), Query::Complete);
        assert!(backend
            .submit(&values, &values, &values, 4096, &sentinel)
            .is_err());
        backend.readback(&mut output).unwrap();
        assert_eq!(output, [0.0; 2]);
        backend
            .submit(&values, &values, &values, 4096, &sentinel)
            .unwrap();
        STATUS.store(12345, Ordering::SeqCst);
        assert_eq!(
            backend.query(),
            Err(Fault {
                operation: "cuStreamQuery",
                raw_status: 12345
            })
        );
        STATUS.store(0, Ordering::SeqCst);
        assert!(backend.query().is_err());
        assert!(backend.close().is_err());
    }
    #[test]
    fn upload_completion_and_upload_failure_fail_closed_before_launch() {
        let _guard = serial();
        let values = [1.0; 2];
        let sentinel = [f32::NAN; 2];
        // Uploads succeed, but the default-stream completion query is still
        // pending, so the submit must fail without launching the kernel.
        let mut backend = fixture();
        assert_eq!(
            backend.query().err().unwrap().operation,
            "query without in-flight launch"
        );
        assert!(backend
            .submit(&values, &values, &values, 4096, &sentinel)
            .is_err());
        assert_eq!(UPLOADS.load(Ordering::SeqCst), 4);
        assert_eq!(LAUNCHES.load(Ordering::SeqCst), 0);
        // A nonzero upload status fails before the completion query or launch.
        let mut backend = fixture();
        UPLOAD_STATUS.store(700, Ordering::SeqCst);
        assert_eq!(
            backend.submit(&values, &values, &values, 4096, &sentinel),
            Err(Fault {
                operation: "cuMemcpyHtoD_v2",
                raw_status: 700
            })
        );
        assert_eq!(UPLOADS.load(Ordering::SeqCst), 5);
        assert_eq!(LAUNCHES.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn abi_ptx_and_status_static_contract_without_loading() {
        assert_eq!(std::mem::size_of::<Uuid>(), 16);
        assert_eq!(std::mem::size_of::<Handle>(), 8);
        assert_eq!(std::mem::size_of::<usize>(), 8);
        assert!(PTX.contains("fma.rn.f32 %f3, %f1, %f2, %f3"));
        assert!(PTX.contains("st.global.f32"));
        assert!(PTX.contains(".target sm_75"));
        assert_eq!(EXPORTS.len(), 21);
        assert!(!EXPORTS
            .iter()
            .any(|s| s.contains("Reset") || s.contains("Set")));
        assert_eq!(
            checked("fake", 123456),
            Err(Fault {
                operation: "fake",
                raw_status: 123456
            })
        );
    }
}
