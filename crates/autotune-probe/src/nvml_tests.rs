// Test-only native ABI plumbing. No DLL is loaded, no setter function exists,
// and typed Any downcasts ensure each fake export has the requested getter ABI.
use std::{any::Any, cell::RefCell};

#[derive(Default)]
struct Script {
    calls: Vec<&'static str>,
    resolutions: Vec<String>,
    selectors: Vec<u32>,
    missing: Option<&'static str>,
    power_status: i32,
    init_status: i32,
    text_mode: u8,
    util: (u32, u32),
    offset_queries: Vec<(u32, u32, u32)>,
    offset_status: i32,
    offset_mode: u8,
    legacy_offset_status: i32,
    domain_offset_status: [i32; 2],
    modern_setter_present: bool,
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
}

fn reset() {
    SCRIPT.with(|s| {
        *s.borrow_mut() = Script {
            util: (17, 29),
            ..Script::default()
        }
    });
}
fn record(name: &'static str) {
    SCRIPT.with(|s| s.borrow_mut().calls.push(name));
}

// These ABI stubs are called only with writable production-owned output buffers.
unsafe extern "C" fn init() -> i32 {
    record("nvmlInit_v2");
    SCRIPT.with(|s| s.borrow().init_status)
}
unsafe extern "C" fn shutdown() -> i32 {
    record("nvmlShutdown");
    0
}
unsafe extern "C" fn count(out: *mut u32) -> i32 {
    record("nvmlDeviceGetCount_v2");
    *out = 1;
    0
}
unsafe extern "C" fn by_index(_: u32, out: *mut Handle) -> i32 {
    record("nvmlDeviceGetHandleByIndex_v2");
    *out = std::ptr::NonNull::<u8>::dangling().as_ptr().cast();
    0
}
unsafe fn write_text(out: *mut c_char, len: u32) -> i32 {
    if len < 8 {
        return 7;
    }
    std::ptr::copy_nonoverlapping(c"fixture".as_ptr(), out, 8);
    0
}
macro_rules! system_text_stub {
    ($function:ident, $name:literal) => {
        unsafe extern "C" fn $function(out: *mut c_char, len: u32) -> i32 {
            record($name);
            write_text(out, len)
        }
    };
}
system_text_stub!(driver, "nvmlSystemGetDriverVersion");
system_text_stub!(nvml, "nvmlSystemGetNVMLVersion");
unsafe extern "C" fn cuda(out: *mut i32) -> i32 {
    record("nvmlSystemGetCudaDriverVersion_v2");
    *out = 13040;
    0
}
unsafe extern "C" fn name(_: Handle, out: *mut c_char, len: u32) -> i32 {
    record("nvmlDeviceGetName");
    match SCRIPT.with(|s| s.borrow().text_mode) {
        1 => {
            std::ptr::write_bytes(out, b'x', len as usize);
            0
        }
        2 => {
            *out = -1;
            *out.add(1) = 0;
            0
        }
        3 => {
            *out = 0;
            0
        }
        4 => {
            *out = b'x' as c_char;
            4
        }
        _ => write_text(out, len),
    }
}
unsafe extern "C" fn uuid(_: Handle, out: *mut c_char, len: u32) -> i32 {
    record("nvmlDeviceGetUUID");
    write_text(out, len)
}
unsafe extern "C" fn pci(_: Handle, out: *mut Pci) -> i32 {
    record("nvmlDeviceGetPciInfo_v3");
    (&mut (*out).bus_id)[..13].copy_from_slice(b"0000:01:00.0\0");
    0
}
unsafe extern "C" fn temperature(_: Handle, selector: u32, out: *mut u32) -> i32 {
    record("nvmlDeviceGetTemperature");
    if selector != 0 {
        return 2;
    }
    *out = 42;
    0
}
unsafe extern "C" fn clock(_: Handle, selector: u32, out: *mut u32) -> i32 {
    record("nvmlDeviceGetClockInfo");
    SCRIPT.with(|s| s.borrow_mut().selectors.push(selector));
    *out = match selector {
        0 => 1500,
        2 => 7000,
        _ => return 2,
    };
    0
}
unsafe extern "C" fn power(_: Handle, out: *mut u32) -> i32 {
    record("nvmlDeviceGetPowerUsage");
    *out = 65000;
    SCRIPT.with(|s| s.borrow().power_status)
}
unsafe extern "C" fn utilization(_: Handle, out: *mut Util) -> i32 {
    record("nvmlDeviceGetUtilizationRates");
    let (gpu, memory) = SCRIPT.with(|s| s.borrow().util);
    *out = Util { gpu, memory };
    0
}
macro_rules! uint_stub {
    ($function:ident, $name:literal, $value:expr) => {
        unsafe extern "C" fn $function(_: Handle, out: *mut u32) -> i32 {
            record($name);
            *out = $value;
            0
        }
    };
}
uint_stub!(limit, "nvmlDeviceGetPowerManagementLimit", 215000);
uint_stub!(
    default_limit,
    "nvmlDeviceGetPowerManagementDefaultLimit",
    215000
);
unsafe extern "C" fn power_bounds(_: Handle, min: *mut u32, max: *mut u32) -> i32 {
    record("nvmlDeviceGetPowerManagementLimitConstraints");
    *min = 100000;
    *max = 250000;
    0
}
macro_rules! offset_stubs {
    ($current:ident, $bounds:ident, $get:literal, $range:literal) => {
        unsafe extern "C" fn $current(_: Handle, out: *mut i32) -> i32 {
            record($get);
            *out = 0;
            SCRIPT.with(|s| s.borrow().legacy_offset_status)
        }
        unsafe extern "C" fn $bounds(_: Handle, min: *mut i32, max: *mut i32) -> i32 {
            record($range);
            *min = -200;
            *max = 1000;
            SCRIPT.with(|s| s.borrow().legacy_offset_status)
        }
    };
}
offset_stubs!(
    graphics_offset,
    graphics_bounds,
    "nvmlDeviceGetGpcClkVfOffset",
    "nvmlDeviceGetGpcClkMinMaxVfOffset"
);
offset_stubs!(
    memory_offset,
    memory_bounds,
    "nvmlDeviceGetMemClkVfOffset",
    "nvmlDeviceGetMemClkMinMaxVfOffset"
);

unsafe extern "C" fn modern_offset(_: Handle, out: *mut ClockOffset) -> i32 {
    record("nvmlDeviceGetClockOffsets");
    let (status, mode) = SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        s.offset_queries
            .push(((*out).version, (*out).clock_type, (*out).pstate));
        let domain_status = s.domain_offset_status[usize::from((*out).clock_type == 2)];
        (
            if domain_status != 0 { domain_status } else { s.offset_status },
            s.offset_mode,
        )
    });
    if status != 0 {
        // Deliberately leave output untouched: failure must not expose sentinels.
        return status;
    }
    if (*out).version != 0x0100_0018
        || ![0, 2].contains(&(*out).clock_type)
        || (*out).pstate != 0
    {
        return 2;
    }
    if mode == 6 {
        return 0; // Driver returned success without filling any outputs.
    }
    (*out).current = if (*out).clock_type == 0 { 100 } else { 200 };
    (*out).minimum = -500;
    (*out).maximum = 1500;
    match mode {
        1 => (*out).current = i32::MAX,
        2 => (*out).minimum = i32::MAX,
        3 => (*out).minimum = 1600,
        4 => (*out).current = 1501,
        5 => (*out).pstate = 1,
        7 => (*out).maximum = i32::MAX,
        8 => (*out).version ^= 1 << 24,
        9 => (*out).clock_type ^= 2,
        _ => (),
    }
    0
}

fn resolve(name: &[u8]) -> Option<Box<dyn Any>> {
    let name = symbol_name(name);
    let missing = SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        s.resolutions.push(name.clone());
        s.missing == Some(name.as_str())
    });
    if missing {
        return None;
    }
    Some(match name.as_str() {
        "nvmlInit_v2" => Box::new(init as Init),
        "nvmlShutdown" => Box::new(shutdown as Init),
        "nvmlSystemGetDriverVersion" => Box::new(driver as Text),
        "nvmlSystemGetNVMLVersion" => Box::new(nvml as Text),
        "nvmlSystemGetCudaDriverVersion_v2" => {
            Box::new(cuda as unsafe extern "C" fn(*mut i32) -> i32)
        }
        "nvmlDeviceGetCount_v2" => Box::new(count as Count),
        "nvmlDeviceGetHandleByIndex_v2" => Box::new(by_index as ByIndex),
        "nvmlDeviceGetPciInfo_v3" => Box::new(pci as PciQuery),
        "nvmlDeviceGetName" => Box::new(self::name as DeviceText),
        "nvmlDeviceGetUUID" => Box::new(uuid as DeviceText),
        "nvmlDeviceGetTemperature" => Box::new(temperature as SelectedUint),
        "nvmlDeviceGetClockInfo" => Box::new(clock as SelectedUint),
        "nvmlDeviceGetPowerUsage" => Box::new(power as Uint),
        "nvmlDeviceGetUtilizationRates" => Box::new(utilization as UtilQuery),
        "nvmlDeviceGetPowerManagementLimit" => Box::new(limit as Uint),
        "nvmlDeviceGetPowerManagementDefaultLimit" => Box::new(default_limit as Uint),
        "nvmlDeviceGetPowerManagementLimitConstraints" => Box::new(power_bounds as UintBounds),
        "nvmlDeviceGetGpcClkVfOffset" => Box::new(graphics_offset as Int),
        "nvmlDeviceGetMemClkVfOffset" => Box::new(memory_offset as Int),
        "nvmlDeviceGetGpcClkMinMaxVfOffset" => Box::new(graphics_bounds as IntBounds),
        "nvmlDeviceGetMemClkMinMaxVfOffset" => Box::new(memory_bounds as IntBounds),
        "nvmlDeviceGetClockOffsets" => Box::new(modern_offset as ClockOffsetQuery),
        "nvmlDeviceSetClockOffsets" if SCRIPT.with(|s| s.borrow().modern_setter_present) => {
            // Opaque data only: no fake setter function or callable conversion exists.
            Box::new(std::ptr::NonNull::<u8>::dangling().as_ptr().cast::<c_void>() as *const c_void)
        }
        _ => return None, // Including every setter: never create callable bindings.
    })
}
fn session() -> Result<Session, Failure> {
    Session::initialize(Symbols::Fake(resolve), PathBuf::from("test-only-no-DLL"))
}
fn value(field: &Field) -> &serde_json::Value {
    match &field.reading {
        Reading::Available { value } => value,
        _ => panic!("expected available reading"),
    }
}
fn failure(field: &Field) -> &Failure {
    match &field.reading {
        Reading::Unavailable { failure } => failure,
        _ => panic!("expected unavailable reading"),
    }
}

#[test]
fn direct_calls_before_initialization_are_structured_failures() {
    let adapter = NvmlAdapter::default();
    for error in [
        adapter.device_count().unwrap_err(),
        adapter.device(0).unwrap_err(),
    ] {
        assert_eq!(error.reason, Reason::QueryFailed);
        assert_eq!(error.vendor_status, Some(1)); // NVML_ERROR_UNINITIALIZED
        assert!(error.detail.contains("before initialization"));
    }
}

#[test]
fn native_construction_selectors_absent_setters_and_executed_allowlist() {
    reset();
    {
        let session = session().unwrap();
        assert_eq!(
            value(&session.runtime().cuda_driver_version),
            &serde_json::json!(13040)
        );
        assert_eq!(session.count().unwrap(), 1);
        let device = session.device(0).unwrap();
        assert!(!device.incomplete());
        assert_eq!(
            value(&device.telemetry["graphics_clock"]),
            &serde_json::json!(1500)
        );
        assert_eq!(
            value(&device.telemetry["memory_clock"]),
            &serde_json::json!(7000)
        );
        for control in device.controls.values() {
            assert_eq!(control.setter_symbol_presence, SymbolPresence::Absent);
            assert_eq!(control.write_verification, WriteVerification::NotTested);
            for getter in control.getters.values() {
                let _ = value(getter);
            }
        }
    }
    SCRIPT.with(|s| {
        let s = s.borrow();
        assert_eq!(s.selectors, [0, 2]);
        assert_eq!(
            s.calls,
            [
                "nvmlInit_v2",
                "nvmlSystemGetCudaDriverVersion_v2",
                "nvmlSystemGetDriverVersion",
                "nvmlSystemGetNVMLVersion",
                "nvmlDeviceGetCount_v2",
                "nvmlDeviceGetHandleByIndex_v2",
                "nvmlDeviceGetPciInfo_v3",
                "nvmlDeviceGetName",
                "nvmlDeviceGetUUID",
                "nvmlDeviceGetTemperature",
                "nvmlDeviceGetClockInfo",
                "nvmlDeviceGetClockInfo",
                "nvmlDeviceGetPowerUsage",
                "nvmlDeviceGetUtilizationRates",
                "nvmlDeviceGetPowerManagementLimitConstraints",
                "nvmlDeviceGetPowerManagementLimit",
                "nvmlDeviceGetPowerManagementDefaultLimit",
                "nvmlDeviceGetGpcClkMinMaxVfOffset",
                "nvmlDeviceGetGpcClkVfOffset",
                "nvmlDeviceGetMemClkMinMaxVfOffset",
                "nvmlDeviceGetMemClkVfOffset",
                "nvmlDeviceGetClockOffsets",
                "nvmlDeviceGetClockOffsets",
                "nvmlShutdown",
            ]
        );
        let setters: Vec<_> = s
            .resolutions
            .iter()
            .filter(|s| s.starts_with("nvmlDeviceSet"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            setters,
            [
                "nvmlDeviceSetPowerManagementLimit",
                "nvmlDeviceSetGpcClkVfOffset",
                "nvmlDeviceSetMemClkVfOffset",
                "nvmlDeviceSetClockOffsets",
                "nvmlDeviceSetClockOffsets"
            ]
        );
    });
}

#[test]
fn native_outputs_keep_valid_utilization_siblings() {
    for (gpu, memory, invalid_key, valid_key, valid) in [
        (101, 29, "gpu_utilization", "memory_utilization", 29),
        (17, u32::MAX, "memory_utilization", "gpu_utilization", 17),
    ] {
        reset();
        SCRIPT.with(|s| s.borrow_mut().util = (gpu, memory));
        let device = session().unwrap().device(0).unwrap();
        assert!(device.incomplete());
        assert_eq!(
            failure(&device.telemetry[invalid_key]).reason,
            Reason::InvalidValue
        );
        assert_eq!(
            value(&device.telemetry[valid_key]),
            &serde_json::json!(valid)
        );
    }
}

#[test]
fn native_errors_missing_symbols_and_buffers_are_preserved() {
    for mode in 1..=4 {
        reset();
        SCRIPT.with(|s| s.borrow_mut().text_mode = mode);
        let device = session().unwrap().device(0).unwrap();
        let error = failure(&device.identity["name"]);
        assert_eq!(
            error.reason,
            if mode == 4 {
                Reason::PermissionDenied
            } else {
                Reason::InvalidValue
            }
        );
        assert_eq!(error.vendor_status, if mode == 4 { Some(4) } else { None });
        assert_eq!(
            value(&device.telemetry["graphics_clock"]),
            &serde_json::json!(1500)
        );
    }
    for status in [3, 4, 15, 999] {
        reset();
        SCRIPT.with(|s| s.borrow_mut().power_status = status);
        let device = session().unwrap().device(0).unwrap();
        assert_eq!(
            failure(&device.telemetry["power"]).vendor_status,
            Some(status)
        );
        assert_eq!(
            value(&device.telemetry["temperature"]),
            &serde_json::json!(42)
        );
    }
    reset();
    SCRIPT.with(|s| s.borrow_mut().missing = Some("nvmlDeviceGetClockInfo"));
    let device = session().unwrap().device(0).unwrap();
    for key in ["graphics_clock", "memory_clock"] {
        assert_eq!(
            device.telemetry[key].symbol_presence,
            SymbolPresence::Absent
        );
        assert_eq!(
            failure(&device.telemetry[key]).reason,
            Reason::SymbolMissing
        );
    }
    SCRIPT.with(|s| assert!(!s.borrow().calls.contains(&"nvmlDeviceGetClockInfo")));
}

#[test]
fn native_failed_initialization_does_not_shutdown() {
    reset();
    SCRIPT.with(|s| s.borrow_mut().init_status = 9);
    assert_eq!(session().err().unwrap().vendor_status, Some(9));
    SCRIPT.with(|s| assert_eq!(s.borrow().calls, ["nvmlInit_v2"]));
}

#[test]
fn modern_offset_abi_inputs_and_independent_domain_values() {
    assert_eq!(std::mem::size_of::<ClockOffset>(), 24);
    assert_eq!(std::mem::align_of::<ClockOffset>(), 4);
    assert_eq!(std::mem::offset_of!(ClockOffset, current), 12);
    assert_eq!(std::mem::offset_of!(ClockOffset, minimum), 16);
    assert_eq!(std::mem::offset_of!(ClockOffset, maximum), 20);
    assert_eq!(CLOCK_OFFSET_VERSION, 0x0100_0018);
    reset();
    let device = session().unwrap().device(0).unwrap();
    for (key, current) in [
        ("graphics_clock_offset_p0", 100),
        ("memory_clock_offset_p0", 200),
    ] {
        let control = &device.controls[key];
        assert_eq!(value(&control.getters["current"]), &serde_json::json!(current));
        assert_eq!(value(&control.getters["minimum"]), &serde_json::json!(-500));
        assert_eq!(value(&control.getters["maximum"]), &serde_json::json!(1500));
        assert_eq!(control.write_verification, WriteVerification::NotTested);
        assert_eq!(control.setter_symbol_presence, SymbolPresence::Absent);
    }
    SCRIPT.with(|s| {
        assert_eq!(
            s.borrow().offset_queries,
            [(0x0100_0018, 0, 0), (0x0100_0018, 2, 0)]
        );
    });
}

#[test]
fn modern_offset_failures_preserve_status_without_zero_fallback() {
    for status in [3, 4, 15, 25, 999] {
        reset();
        SCRIPT.with(|s| s.borrow_mut().offset_status = status);
        let device = session().unwrap().device(0).unwrap();
        for key in ["graphics_clock_offset_p0", "memory_clock_offset_p0"] {
            for getter in device.controls[key].getters.values() {
                assert_eq!(failure(getter).vendor_status, Some(status));
                assert_eq!(getter.symbol_presence, SymbolPresence::Present);
                assert_eq!(getter.unit, "MHz");
            }
        }
        assert_eq!(value(&device.controls["graphics_clock_offset"].getters["current"]), &serde_json::json!(0));
        assert_eq!(value(&device.telemetry["temperature"]), &serde_json::json!(42));
    }
    reset();
    SCRIPT.with(|s| s.borrow_mut().missing = Some("nvmlDeviceGetClockOffsets"));
    let device = session().unwrap().device(0).unwrap();
    for key in ["graphics_clock_offset_p0", "memory_clock_offset_p0"] {
        for getter in device.controls[key].getters.values() {
            assert_eq!(failure(getter).reason, Reason::SymbolMissing);
            assert_eq!(getter.symbol_presence, SymbolPresence::Absent);
        }
    }
    SCRIPT.with(|s| assert!(s.borrow().offset_queries.is_empty()));
}

#[test]
fn modern_offset_invalid_outputs_preserve_independent_valid_fields() {
    for mode in 1..=9 {
        reset();
        SCRIPT.with(|s| s.borrow_mut().offset_mode = mode);
        let device = session().unwrap().device(0).unwrap();
        assert!(device.incomplete());
        let getters = &device.controls["graphics_clock_offset_p0"].getters;
        match mode {
            1 | 4 => {
                assert_eq!(failure(&getters["current"]).reason, Reason::InvalidValue);
                assert_eq!(value(&getters["minimum"]), &serde_json::json!(-500));
                assert_eq!(value(&getters["maximum"]), &serde_json::json!(1500));
            }
            2 | 3 | 7 => {
                assert_eq!(value(&getters["current"]), &serde_json::json!(100));
                assert_eq!(failure(&getters["minimum"]).reason, Reason::InvalidValue);
                assert_eq!(failure(&getters["maximum"]).reason, Reason::InvalidValue);
            }
            _ => {
                for getter in getters.values() {
                    assert_eq!(failure(getter).reason, Reason::InvalidValue);
                }
            }
        }
    }
}

#[test]
fn modern_reads_succeed_independently_of_unsupported_legacy_getters() {
    reset();
    SCRIPT.with(|s| s.borrow_mut().legacy_offset_status = 3);
    let device = session().unwrap().device(0).unwrap();
    for (legacy, modern, expected) in [
        ("graphics_clock_offset", "graphics_clock_offset_p0", 100),
        ("memory_clock_offset", "memory_clock_offset_p0", 200),
    ] {
        for getter in device.controls[legacy].getters.values() {
            assert_eq!(failure(getter).reason, Reason::Unsupported);
            assert_eq!(failure(getter).vendor_status, Some(3));
        }
        assert_eq!(value(&device.controls[modern].getters["current"]), &serde_json::json!(expected));
    }
    assert!(!device.incomplete()); // Unsupported optional legacy APIs are not collection failures.
}

#[test]
fn modern_domain_failures_do_not_hide_the_other_domain() {
    for (statuses, failed, successful, expected) in [
        ([4, 0], "graphics_clock_offset_p0", "memory_clock_offset_p0", 200),
        ([0, 4], "memory_clock_offset_p0", "graphics_clock_offset_p0", 100),
    ] {
        reset();
        SCRIPT.with(|s| s.borrow_mut().domain_offset_status = statuses);
        let device = session().unwrap().device(0).unwrap();
        for getter in device.controls[failed].getters.values() {
            assert_eq!(failure(getter).reason, Reason::PermissionDenied);
        }
        let control = &device.controls[successful];
        assert_eq!(value(&control.getters["current"]), &serde_json::json!(expected));
        assert_eq!(value(&control.getters["minimum"]), &serde_json::json!(-500));
        assert_eq!(value(&control.getters["maximum"]), &serde_json::json!(1500));
    }
}

#[test]
fn modern_setter_presence_is_opaque_and_does_not_verify_writes() {
    reset();
    SCRIPT.with(|s| s.borrow_mut().modern_setter_present = true);
    let device = session().unwrap().device(0).unwrap();
    for key in ["graphics_clock_offset_p0", "memory_clock_offset_p0"] {
        assert_eq!(device.controls[key].setter_symbol_presence, SymbolPresence::Present);
        assert_eq!(device.controls[key].write_verification, WriteVerification::NotTested);
    }
    SCRIPT.with(|s| {
        let s = s.borrow();
        assert!(!s.calls.iter().any(|name| name.starts_with("nvmlDeviceSet")));
        assert_eq!(s.resolutions.iter().filter(|name| name.as_str() == "nvmlDeviceSetClockOffsets").count(), 2);
    });
}
