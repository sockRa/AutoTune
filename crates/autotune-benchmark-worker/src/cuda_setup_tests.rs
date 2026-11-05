// Typed ABI fakes only. Never invokes open_observed, a DLL loader or GPU.
static SETUP_NATIVE: AtomicUsize = AtomicUsize::new(0);
static SETUP_FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
static SETUP_CLEANUP: AtomicUsize = AtomicUsize::new(0);
static SETUP_MODE: AtomicI32 = AtomicI32::new(1);
static SETUP_COUNT: AtomicI32 = AtomicI32::new(1);
static SETUP_NULL_CONTEXT: AtomicI32 = AtomicI32::new(0);
static SETUP_LOW_MEMORY: AtomicI32 = AtomicI32::new(0);
fn setup_status() -> i32 {
    let n = SETUP_NATIVE.fetch_add(1, Ordering::SeqCst);
    if n == SETUP_FAIL_AT.load(Ordering::SeqCst) {
        -71
    } else {
        0
    }
}
unsafe extern "system" fn setup_init(flags: u32) -> i32 {
    assert_eq!(flags, 0);
    setup_status()
}
unsafe extern "system" fn setup_version(out: *mut i32) -> i32 {
    *out = 12090;
    setup_status()
}
unsafe extern "system" fn setup_loading(out: *mut i32) -> i32 {
    *out = SETUP_MODE.load(Ordering::SeqCst);
    setup_status()
}
unsafe extern "system" fn setup_count(out: *mut i32) -> i32 {
    *out = SETUP_COUNT.load(Ordering::SeqCst);
    setup_status()
}
unsafe extern "system" fn setup_device(out: *mut i32, ordinal: i32) -> i32 {
    *out = ordinal;
    setup_status()
}
unsafe extern "system" fn setup_uuid(out: *mut Uuid, _: i32) -> i32 {
    *out = Uuid { bytes: [1; 16] };
    setup_status()
}
unsafe extern "system" fn setup_attribute(out: *mut i32, attribute: i32, _: i32) -> i32 {
    *out = match attribute {
        75 => 7,
        76 => 5,
        _ => 0,
    };
    setup_status()
}
unsafe extern "system" fn setup_context(out: *mut Handle, flags: u32, device: i32) -> i32 {
    assert_eq!((flags, device), (0, 0));
    *out = if SETUP_NULL_CONTEXT.load(Ordering::SeqCst) == 1 {
        null_mut()
    } else {
        std::ptr::dangling_mut()
    };
    setup_status()
}
unsafe extern "system" fn setup_current(out: *mut i32) -> i32 {
    *out = 0;
    setup_status()
}
unsafe extern "system" fn setup_module(out: *mut Handle, ptx: *const c_void) -> i32 {
    assert!(!ptx.is_null());
    *out = std::ptr::dangling_mut();
    setup_status()
}
unsafe extern "system" fn setup_function(out: *mut Handle, module: Handle, name: *const i8) -> i32 {
    assert!(!module.is_null());
    assert_eq!(std::ffi::CStr::from_ptr(name), c"autotune_fma");
    *out = std::ptr::dangling_mut();
    setup_status()
}
unsafe extern "system" fn setup_memory(free: *mut usize, total: *mut usize) -> i32 {
    *free = if SETUP_LOW_MEMORY.load(Ordering::SeqCst) == 1 {
        1
    } else {
        1 << 30
    };
    *total = 1 << 30;
    setup_status()
}
unsafe extern "system" fn setup_allocate(out: *mut u64, bytes: usize) -> i32 {
    assert_eq!(bytes, 8);
    *out = 64;
    setup_status()
}
unsafe extern "system" fn setup_stream(out: *mut Handle, flags: u32) -> i32 {
    assert_eq!(flags, 1);
    *out = std::ptr::dangling_mut();
    setup_status()
}
unsafe extern "system" fn setup_destroy(_: Handle) -> i32 {
    SETUP_CLEANUP.fetch_add(1, Ordering::SeqCst);
    0
}
unsafe extern "system" fn setup_free(_: u64) -> i32 {
    SETUP_CLEANUP.fetch_add(1, Ordering::SeqCst);
    0
}
fn setup_symbols(name: &str) -> Option<Box<dyn std::any::Any>> {
    Some(match name {
        "cuInit" => Box::new(setup_init as Init),
        "cuDriverGetVersion" => Box::new(setup_version as IntOut),
        "cuModuleGetLoadingMode" => Box::new(setup_loading as IntOut),
        "cuDeviceGetCount" => Box::new(setup_count as IntOut),
        "cuDeviceGet" => Box::new(setup_device as DeviceGet),
        "cuDeviceGetUuid_v2" => Box::new(setup_uuid as DeviceUuid),
        "cuDeviceGetAttribute" => Box::new(setup_attribute as Attribute),
        "cuCtxCreate_v2" => Box::new(setup_context as ContextCreate),
        "cuCtxGetDevice" => Box::new(setup_current as IntOut),
        "cuModuleLoadData" => Box::new(setup_module as ModuleLoad),
        "cuModuleGetFunction" => Box::new(setup_function as Function),
        "cuMemGetInfo_v2" => Box::new(setup_memory as MemInfo),
        "cuMemAlloc_v2" => Box::new(setup_allocate as Alloc),
        "cuStreamCreate" => Box::new(setup_stream as StreamCreate),
        "cuMemFree_v2" => Box::new(setup_free as Free),
        "cuCtxDestroy_v2" | "cuModuleUnload" | "cuStreamDestroy_v2" => {
            Box::new(setup_destroy as HandleCall)
        }
        _ => return symbols(name),
    })
}
fn setup_fixture() -> CudaBackend {
    SETUP_NATIVE.store(0, Ordering::SeqCst);
    SETUP_FAIL_AT.store(usize::MAX, Ordering::SeqCst);
    SETUP_CLEANUP.store(0, Ordering::SeqCst);
    SETUP_COUNT.store(1, Ordering::SeqCst);
    SETUP_MODE.store(1, Ordering::SeqCst);
    SETUP_NULL_CONTEXT.store(0, Ordering::SeqCst);
    SETUP_LOW_MEMORY.store(0, Ordering::SeqCst);
    let mut b = fixture();
    b.api = Api::fake(setup_symbols).unwrap();
    b.context = null_mut();
    b.module = null_mut();
    b.function = null_mut();
    b.stream = null_mut();
    b.buffers = [0; 4];
    b.driver_version = -1;
    b.state = State::Poisoned;
    b
}
#[derive(Default)]
struct SetupRecorded {
    before: Vec<SetupOperation>,
    raw: Vec<(SetupOperation, SetupReturn)>,
    deny_before: Option<SetupOperation>,
    deny_after: Option<SetupOperation>,
}
impl SetupObserver for SetupRecorded {
    fn before(&mut self, op: SetupOperation) -> Result<(), Fault> {
        self.before.push(op);
        if self.deny_before == Some(op) {
            Err(invalid("setup observer before"))
        } else {
            Ok(())
        }
    }
    fn returned(&mut self, op: SetupOperation, raw: SetupReturn) -> Result<(), Fault> {
        self.raw.push((op, raw));
        if self.deny_after == Some(op) {
            Err(invalid("setup observer after"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn setup_observes_all_native_outputs_in_order_before_ready_without_compute() {
    let _lock = serial();
    let mut b = setup_fixture();
    let mut o = SetupRecorded::default();
    b.setup_observed(&mut o).unwrap();
    assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 21);
    assert_eq!(o.raw.len(), 21);
    assert_eq!(o.before, o.raw.iter().map(|r| r.0).collect::<Vec<_>>());
    assert_eq!(o.raw[1].1.output, SetupOutput::Integer(12090));
    assert_eq!(o.raw[5].1.output, SetupOutput::Uuid([1; 16]));
    assert_eq!(
        o.raw[15].1.output,
        SetupOutput::Memory {
            free: 1 << 30,
            total: 1 << 30
        }
    );
    assert!(b.state == State::Ready);
    assert_eq!(
        b.setup_resources(),
        SetupResources {
            context: true,
            module: true,
            function: true,
            stream: true,
            buffers: [true; 4]
        }
    );
    assert_eq!(LAUNCHES.load(Ordering::SeqCst), 0);
    assert_eq!(UPLOADS.load(Ordering::SeqCst), 0);
    drop(b);
    assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
}
#[test]
fn every_setup_pre_or_post_observer_rejection_stops_before_successor() {
    let _lock = serial();
    let mut b = setup_fixture();
    let mut o = SetupRecorded::default();
    b.setup_observed(&mut o).unwrap();
    let operations = o.before.clone();
    drop(b);
    for (i, op) in operations.iter().copied().enumerate() {
        for after in [false, true] {
            let mut b = setup_fixture();
            let mut o = SetupRecorded::default();
            if after {
                o.deny_after = Some(op)
            } else {
                o.deny_before = Some(op)
            };
            assert!(b.setup_observed(&mut o).is_err());
            assert!(b.state == State::Poisoned);
            assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), i + usize::from(after));
            assert_eq!(o.raw.len(), i + usize::from(after));
            drop(b);
            assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
        }
    }
}
#[test]
fn every_native_setup_failure_keeps_signed_status_and_partial_owner_without_cleanup() {
    let _lock = serial();
    for i in 0..21 {
        let b = setup_fixture();
        SETUP_FAIL_AT.store(i, Ordering::SeqCst);
        let mut o = SetupRecorded::default();
        let input = SetupFailure {
            fault: invalid("unstarted fixture"),
            backend: Some(b),
            library: None,
            expected_path: None,
            loaded_path: None,
            runtime_file: None,
            load_error: None,
            child_options: Default::default(),
        };
        let owned = CudaBackend::finish_setup(input, &mut o).err().unwrap();
        assert_eq!(owned.fault.raw_status, -71);
        assert_eq!(o.raw.last().unwrap().1.status, -71);
        assert_eq!(o.raw.len(), i + 1);
        assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), i + 1);
        let resources = owned.resources().unwrap();
        assert_eq!(resources.context, i >= 11);
        assert_eq!(resources.buffers[0], i >= 16);
        drop(owned);
        assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
    }
}
#[test]
fn setup_rejects_identity_ambiguity_bad_count_null_context_and_headroom_before_successor() {
    let _lock = serial();
    let mut b = setup_fixture();
    b.identity.uuid = [2; 16];
    assert_eq!(
        b.setup_observed(&mut SetupRecorded::default())
            .unwrap_err()
            .operation,
        "UUID not found"
    );
    assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 6);
    assert!(!b.setup_resources().context);
    let mut b = setup_fixture();
    SETUP_COUNT.store(2, Ordering::SeqCst);
    assert_eq!(
        b.setup_observed(&mut SetupRecorded::default())
            .unwrap_err()
            .operation,
        "ambiguous UUID"
    );
    assert!(!b.setup_resources().context);
    let mut b = setup_fixture();
    SETUP_COUNT.store(65, Ordering::SeqCst);
    assert_eq!(
        b.setup_observed(&mut SetupRecorded::default())
            .unwrap_err()
            .operation,
        "device count"
    );
    assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 4);
    let mut b = setup_fixture();
    SETUP_NULL_CONTEXT.store(1, Ordering::SeqCst);
    assert_eq!(
        b.setup_observed(&mut SetupRecorded::default())
            .unwrap_err()
            .operation,
        "null context"
    );
    assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 12);
    let mut b = setup_fixture();
    SETUP_LOW_MEMORY.store(1, Ordering::SeqCst);
    assert_eq!(
        b.setup_observed(&mut SetupRecorded::default())
            .unwrap_err()
            .operation,
        "memory headroom"
    );
    assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 16);
    assert_eq!(b.buffers, [0; 4]);
    assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
}

fn environment_return(
    variable: crate::workload::child_options::Variable,
) -> crate::workload::child_options::Read {
    let mut units = [0; crate::workload::child_options::UNITS];
    for (slot, unit) in units.iter_mut().zip(variable.expected().encode_utf16()) {
        *slot = unit;
    }
    crate::workload::child_options::Read {
        length: variable.expected().len() as u32,
        error: None,
        units,
    }
}
#[test]
fn child_environment_observer_rejection_keeps_raw_return_without_successor_read() {
    use crate::workload::child_options::{Facts, Variable};
    for variable in Variable::ALL {
        for after in [false, true] {
            let mut facts = Facts::default();
            let mut observer = SetupRecorded::default();
            if after {
                observer.deny_after = Some(SetupOperation::Environment(variable));
            } else {
                observer.deny_before = Some(SetupOperation::Environment(variable));
            }
            let mut reads = 0;
            assert!(capture_child_options(&mut facts, &mut observer, |v| {
                reads += 1;
                environment_return(v)
            })
            .is_err());
            assert_eq!(
                reads,
                usize::from(variable == Variable::ModuleLoading) + usize::from(after)
            );
            assert_eq!(
                facts.cache_disable.is_some(),
                variable == Variable::ModuleLoading || after
            );
            assert_eq!(
                facts.module_loading.is_some(),
                variable == Variable::ModuleLoading && after
            );
            for (_, raw) in observer.raw {
                assert!(matches!(raw.output, SetupOutput::Environment(_)));
            }
        }
    }
}
#[test]
fn child_environment_failure_and_unwind_retain_first_raw_facts() {
    use crate::workload::child_options::{Facts, Read};
    let mut facts = Facts::default();
    let mut reads = 0;
    let raw = Read {
        length: u32::MAX,
        error: Some(203),
        units: [0xd800; 16],
    };
    assert!(
        capture_child_options(&mut facts, &mut SetupRecorded::default(), |_| {
            reads += 1;
            raw
        })
        .is_err()
    );
    assert_eq!(reads, 1);
    assert_eq!(facts.cache_disable, Some(raw));
    assert_eq!(facts.module_loading, None);
    struct Panic;
    impl SetupObserver for Panic {
        fn before(&mut self, _: SetupOperation) -> Result<(), Fault> {
            Ok(())
        }
        fn returned(&mut self, _: SetupOperation, _: SetupReturn) -> Result<(), Fault> {
            panic!("post-clock fake")
        }
    }
    let mut facts = Facts::default();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| capture_child_options(
            &mut facts,
            &mut Panic,
            environment_return
        )))
        .is_err()
    );
    assert!(facts.cache_disable.is_some());
    assert_eq!(facts.module_loading, None);
}
#[test]
fn module_loading_query_rejects_lazy_unknown_and_failed_results_before_context_or_module() {
    let _lock = serial();
    for mode in [2, -1, i32::MAX] {
        let mut b = setup_fixture();
        SETUP_MODE.store(mode, Ordering::SeqCst);
        let mut o = SetupRecorded::default();
        assert_eq!(
            b.setup_observed(&mut o).unwrap_err().operation,
            "effective CUDA module loading"
        );
        assert_eq!(b.module_loading_mode(), mode);
        assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 3);
        assert!(!b.setup_resources().context);
        assert!(!b.setup_resources().module);
        assert_eq!(o.raw.last().unwrap().0, SetupOperation::ModuleLoadingMode);
        assert_eq!(o.raw.last().unwrap().1.output, SetupOutput::Integer(mode));
        assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
    }
    let mut b = setup_fixture();
    SETUP_FAIL_AT.store(2, Ordering::SeqCst);
    assert_eq!(
        b.setup_observed(&mut SetupRecorded::default())
            .unwrap_err()
            .raw_status,
        -71
    );
    assert!(!b.setup_resources().context);
}
