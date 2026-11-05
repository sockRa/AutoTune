//! Canonical retained worker setup observations. A receipt binds reported bytes
//! to the parent's run/deadline; it grants no native admission or qualification.
use super::{
    child_options,
    nvml_trace::FileCheck,
    runtime_evidence, settings,
    setup_model::{self, Entry, Fault as TraceFault, Operation, Output, Return},
    wire::WorkerBinding,
};
use crate::{
    evidence::{decode, Digest, Domain, Object, Value},
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        wire_model::{hex, object, unhex},
    },
};
use autotune_probe::runtime_file::{FileRecord, MAX_CHECKS, MAX_PATH_UNITS};
pub const CAP: usize = 2 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Shape,
    Binding,
    Digest,
    Deadline,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preparation {
    Ready,
    CudaFailed,
    CpuFailed,
}
pub enum File<'a> {
    Recorded(&'a FileRecord),
    #[cfg(windows)]
    Native(&'a autotune_probe::runtime_file::windows::Image),
}
pub struct Facts<'a> {
    pub first: Anchor,
    pub deadline_raw: u64,
    pub preparation: Preparation,
    pub cpu_fault: Option<&'a str>,
    pub native_fault: Option<&'a crate::worker::Fault>,
    pub entries: &'a [Entry],
    pub pending: bool,
    pub fault: Option<TraceFault>,
    pub overflow: Option<(Operation, Option<Anchor>)>,
    pub rejected: Option<(Operation, Return, Option<Anchor>)>,
    pub file_checks: &'a [FileCheck],
    pub file_overflow: Option<FileCheck>,
    pub file: Option<File<'a>>,
    pub load_error: Option<runtime_evidence::LoadError>,
    pub environment: child_options::Facts,
    pub driver_version: Option<i32>,
    pub module_loading_mode: Option<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    pub simulated: bool,
    pub first: Anchor,
    pub deadline_raw: u64,
    pub preparation: Preparation,
    pub cpu_fault: Option<String>,
    pub native_fault: Option<(String, i32)>,
    pub entries: Vec<Entry>,
    pub pending: bool,
    pub fault: Option<TraceFault>,
    pub overflow: Option<(Operation, Option<Anchor>)>,
    pub rejected: Option<(Operation, Return, Option<Anchor>)>,
    pub file_checks: Vec<FileCheck>,
    pub file_overflow: Option<FileCheck>,
    pub file: Option<FileRecord>,
    pub load_error: Option<runtime_evidence::LoadError>,
    pub environment: child_options::Facts,
    pub driver_version: Option<i32>,
    pub module_loading_mode: Option<i32>,
}
pub struct Artifact {
    binding: WorkerBinding,
    deadline_raw: u64,
    canonical: String,
    digest: Digest,
    _allocation: Reservation,
}
impl Artifact {
    pub fn binding(&self) -> WorkerBinding {
        self.binding
    }
    pub fn deadline_raw(&self) -> u64 {
        self.deadline_raw
    }
    pub fn recorded(
        binding: WorkerBinding,
        facts: Facts<'_>,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        Self::build(binding, facts, true, budget)
    }
    #[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
    pub(crate) fn native(
        binding: WorkerBinding,
        facts: Facts<'_>,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        Self::build(binding, facts, false, budget)
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    fn build(
        binding: WorkerBinding,
        f: Facts<'_>,
        simulated: bool,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        if f.entries.len() > setup_model::CAP || f.file_checks.len() > MAX_CHECKS {
            return Err(Fault::Capacity);
        }
        if f.cpu_fault.is_some_and(|s| !CPU_FAULTS.contains(&s))
            || f.native_fault.is_some_and(|f| f.operation.len() > 128)
            || f.load_error.is_some_and(|e| !e.known && e.code.is_some())
        {
            return Err(Fault::Shape);
        }
        let _codec = budget.reserve(CAP * 8).map_err(|_| Fault::Capacity)?;
        let allocation = budget
            .reserve(CAP * 2 + std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let file = match f.file {
            Some(File::Recorded(v)) => {
                check_file(v)?;
                Some(v.clone())
            }
            #[cfg(windows)]
            Some(File::Native(v)) => Some(v.record()),
            None => None,
        };
        if let Some(v) = &file {
            check_file(v)?;
        }
        let value = object([
            ("version", text("1")),
            ("scope", text("pilot_worker_setup")),
            ("binding", binding.value()),
            (
                "source",
                text(if simulated {
                    "simulated_fixture"
                } else {
                    "native_setup_path"
                }),
            ),
            ("first", settings::raw_anchor(f.first)),
            ("deadline_raw", Value::U64(f.deadline_raw)),
            ("preparation", text(preparation_name(f.preparation))),
            ("cpu_fault", f.cpu_fault.map(text).unwrap_or(Value::Null)),
            (
                "native_fault",
                f.native_fault
                    .map(|f| {
                        object([
                            ("operation", text(f.operation)),
                            ("status", Value::I64(f.raw_status.into())),
                        ])
                    })
                    .unwrap_or(Value::Null),
            ),
            (
                "entries",
                Value::Array(f.entries.iter().map(entry_value).collect()),
            ),
            ("pending", Value::Bool(f.pending)),
            ("fault", f.fault.map(fault_value).unwrap_or(Value::Null)),
            (
                "overflow",
                f.overflow
                    .map(|(op, a)| {
                        object([("operation", operation_value(op)), ("anchor", anchor(a))])
                    })
                    .unwrap_or(Value::Null),
            ),
            (
                "rejected",
                f.rejected
                    .map(|(op, r, a)| {
                        object([
                            ("operation", operation_value(op)),
                            ("raw", return_value(r)),
                            ("anchor", anchor(a)),
                        ])
                    })
                    .unwrap_or(Value::Null),
            ),
            (
                "file_checks",
                Value::Array(
                    f.file_checks
                        .iter()
                        .map(runtime_evidence::check_value)
                        .collect(),
                ),
            ),
            (
                "file_overflow",
                f.file_overflow
                    .as_ref()
                    .map(runtime_evidence::check_value)
                    .unwrap_or(Value::Null),
            ),
            (
                "file",
                file.as_ref()
                    .map(runtime_evidence::file_value)
                    .unwrap_or(Value::Null),
            ),
            (
                "load_error",
                f.load_error
                    .map(|e| object([("known", Value::Bool(e.known)), ("code", int(e.code))]))
                    .unwrap_or(Value::Null),
            ),
            (
                "environment",
                object([
                    (
                        "cache_disable",
                        f.environment
                            .cache_disable
                            .map(environment_value)
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "module_loading",
                        f.environment
                            .module_loading
                            .map(environment_value)
                            .unwrap_or(Value::Null),
                    ),
                ]),
            ),
            ("driver_version", int(f.driver_version)),
            ("module_loading_mode", int(f.module_loading_mode)),
        ]);
        let canonical = value.to_canonical();
        if canonical.len() > CAP {
            return Err(Fault::Capacity);
        }
        let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
        Ok(Self {
            binding,
            deadline_raw: f.deadline_raw,
            canonical,
            digest,
            _allocation: allocation,
        })
    }
}
pub struct Receipt {
    canonical: String,
    digest: Digest,
    reported: Reported,
    _allocation: Reservation,
}
impl Receipt {
    pub fn read(
        bytes: &[u8],
        binding: WorkerBinding,
        deadline_raw: u64,
        expected: Digest,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        if bytes.len() > CAP {
            return Err(Fault::Capacity);
        }
        let _codec = budget.reserve(CAP * 8).map_err(|_| Fault::Capacity)?;
        let allocation = budget
            .reserve(CAP * 2 + std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        crate::sampler::wire::validate_complexity_limit_wide(bytes, 262144)
            .map_err(|_| Fault::Capacity)?;
        let v = decode(bytes, CAP).map_err(|_| Fault::Shape)?;
        if Digest::of(Domain::RawArtifact, bytes) != expected {
            return Err(Fault::Digest);
        }
        let o = obj(
            &v,
            &[
                "version",
                "scope",
                "binding",
                "source",
                "first",
                "deadline_raw",
                "preparation",
                "cpu_fault",
                "native_fault",
                "entries",
                "pending",
                "fault",
                "overflow",
                "rejected",
                "file_checks",
                "file_overflow",
                "file",
                "load_error",
                "environment",
                "driver_version",
                "module_loading_mode",
            ],
        )?;
        if txt(get(o, "version")?)? != "1"
            || txt(get(o, "scope")?)? != "pilot_worker_setup"
            || get(o, "binding")?.to_canonical() != binding.value().to_canonical()
        {
            return Err(Fault::Binding);
        }
        if deadline_raw == 0 || num::<u64>(get(o, "deadline_raw")?)? != deadline_raw {
            return Err(Fault::Deadline);
        }
        let simulated = match txt(get(o, "source")?)? {
            "simulated_fixture" => true,
            "native_setup_path" => false,
            _ => return Err(Fault::Shape),
        };
        let first = settings::parse_anchor(get(o, "first")?)
            .map_err(|_| Fault::Shape)?
            .ok_or(Fault::Shape)?;
        // Raw bad/missing per-call clocks remain evidence. The parent's original
        // deadline and run are compared above, never reconstructed from a child.
        let entries = array(get(o, "entries")?, setup_model::CAP)?
            .iter()
            .map(read_entry)
            .collect::<Result<_, _>>()?;
        let file_checks = array(get(o, "file_checks")?, MAX_CHECKS)?
            .iter()
            .map(|v| runtime_evidence::read_check(v).map_err(map_runtime))
            .collect::<Result<_, _>>()?;
        let load_error = optional(get(o, "load_error")?, |v| {
            let o = obj(v, &["known", "code"])?;
            let e = runtime_evidence::LoadError {
                known: boolean(get(o, "known")?)?,
                code: read_int(get(o, "code")?)?,
            };
            if !e.known && e.code.is_some() {
                return Err(Fault::Shape);
            }
            Ok(e)
        })?;
        let environment = obj(get(o, "environment")?, &["cache_disable", "module_loading"])?;
        let cpu_fault = optional(get(o, "cpu_fault")?, |v| {
            let s = txt(v)?;
            if !CPU_FAULTS.contains(&s) {
                return Err(Fault::Shape);
            }
            Ok(s.to_owned())
        })?;
        let native_fault = optional(get(o, "native_fault")?, |v| {
            let o = obj(v, &["operation", "status"])?;
            let s = txt(get(o, "operation")?)?;
            if s.len() > 128 {
                return Err(Fault::Capacity);
            }
            Ok((s.to_owned(), signed(get(o, "status")?)?))
        })?;
        let reported = Reported {
            simulated,
            first,
            deadline_raw,
            preparation: read_preparation(get(o, "preparation")?)?,
            cpu_fault,
            native_fault,
            entries,
            pending: boolean(get(o, "pending")?)?,
            fault: optional(get(o, "fault")?, read_fault)?,
            overflow: optional(get(o, "overflow")?, |v| {
                let o = obj(v, &["operation", "anchor"])?;
                Ok((
                    read_operation(get(o, "operation")?)?,
                    read_anchor(get(o, "anchor")?)?,
                ))
            })?,
            rejected: optional(get(o, "rejected")?, |v| {
                let o = obj(v, &["operation", "raw", "anchor"])?;
                Ok((
                    read_operation(get(o, "operation")?)?,
                    read_return(get(o, "raw")?)?,
                    read_anchor(get(o, "anchor")?)?,
                ))
            })?,
            file_checks,
            file_overflow: optional(get(o, "file_overflow")?, |v| {
                runtime_evidence::read_check(v).map_err(map_runtime)
            })?,
            file: optional(get(o, "file")?, |v| {
                runtime_evidence::read_file(v).map_err(map_runtime)
            })?,
            load_error,
            environment: child_options::Facts {
                cache_disable: optional(get(environment, "cache_disable")?, read_environment)?,
                module_loading: optional(get(environment, "module_loading")?, read_environment)?,
            },
            driver_version: read_int(get(o, "driver_version")?)?,
            module_loading_mode: read_int(get(o, "module_loading_mode")?)?,
        };
        Ok(Self {
            canonical: std::str::from_utf8(bytes)
                .map_err(|_| Fault::Shape)?
                .to_owned(),
            digest: expected,
            reported,
            _allocation: allocation,
        })
    }
    pub fn reported(&self) -> &Reported {
        &self.reported
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
}
const CPU_FAULTS: [&str; 10] = [
    "admission",
    "binding",
    "capacity",
    "clock",
    "deadline",
    "protocol",
    "native",
    "incorrect",
    "length",
    "transport",
];
fn check_file(f: &FileRecord) -> Result<(), Fault> {
    if [&f.requested, &f.loaded]
        .into_iter()
        .flatten()
        .any(|p| p.len() > MAX_PATH_UNITS)
    {
        Err(Fault::Capacity)
    } else {
        Ok(())
    }
}
fn map_runtime(f: runtime_evidence::Fault) -> Fault {
    if f == runtime_evidence::Fault::Capacity {
        Fault::Capacity
    } else {
        Fault::Shape
    }
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn int(v: Option<i32>) -> Value {
    v.map(|v| Value::I64(v.into())).unwrap_or(Value::Null)
}
fn anchor(v: Option<Anchor>) -> Value {
    v.map(settings::raw_anchor).unwrap_or(Value::Null)
}
fn preparation_name(p: Preparation) -> &'static str {
    match p {
        Preparation::Ready => "ready",
        Preparation::CudaFailed => "cuda_failed",
        Preparation::CpuFailed => "cpu_failed",
    }
}
fn read_preparation(v: &Value) -> Result<Preparation, Fault> {
    match txt(v)? {
        "ready" => Ok(Preparation::Ready),
        "cuda_failed" => Ok(Preparation::CudaFailed),
        "cpu_failed" => Ok(Preparation::CpuFailed),
        _ => Err(Fault::Shape),
    }
}
fn environment_value(r: child_options::Read) -> Value {
    object([
        ("length", Value::U64(r.length.into())),
        (
            "error",
            r.error.map(|e| Value::U64(e.into())).unwrap_or(Value::Null),
        ),
        (
            "utf16le",
            text(&hex(&r
                .units
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>())),
        ),
    ])
}
fn read_environment(v: &Value) -> Result<child_options::Read, Fault> {
    let o = obj(v, &["length", "error", "utf16le"])?;
    let bytes: [u8; 32] = unhex(get(o, "utf16le")?).map_err(|_| Fault::Shape)?;
    let mut units = [0; 16];
    for (u, b) in units.iter_mut().zip(bytes.as_chunks::<2>().0) {
        *u = u16::from_le_bytes(*b);
    }
    Ok(child_options::Read {
        length: num(get(o, "length")?)?,
        error: optional(get(o, "error")?, num::<u32>)?,
        units,
    })
}
fn operation_value(op: Operation) -> Value {
    let (name, a, b) = match op {
        Operation::Environment(v) => (
            match v {
                child_options::Variable::CacheDisable => "environment_cache",
                child_options::Variable::ModuleLoading => "environment_loading",
            },
            None,
            None,
        ),
        Operation::Initialize => ("initialize", None, None),
        Operation::DriverVersion => ("driver_version", None, None),
        Operation::ModuleLoadingMode => ("module_loading_mode", None, None),
        Operation::DeviceCount => ("device_count", None, None),
        Operation::Device(v) => ("device", Some(Value::U64(v.into())), None),
        Operation::Uuid(v) => ("uuid", Some(Value::I64(v.into())), None),
        Operation::Attribute { device, attribute } => (
            "attribute",
            Some(Value::I64(device.into())),
            Some(Value::I64(attribute.into())),
        ),
        Operation::ContextCreate => ("context_create", None, None),
        Operation::ContextDevice => ("context_device", None, None),
        Operation::ModuleLoad => ("module_load", None, None),
        Operation::Function => ("function", None, None),
        Operation::MemoryInfo => ("memory_info", None, None),
        Operation::Allocate(v) => ("allocate", Some(Value::U64(v.into())), None),
        Operation::StreamCreate => ("stream_create", None, None),
    };
    object([
        ("name", text(name)),
        ("a", a.unwrap_or(Value::Null)),
        ("b", b.unwrap_or(Value::Null)),
    ])
}
fn read_operation(v: &Value) -> Result<Operation, Fault> {
    let o = obj(v, &["name", "a", "b"])?;
    let a = get(o, "a")?;
    let b = get(o, "b")?;
    let op = match txt(get(o, "name")?)? {
        "device" => Operation::Device(num(a)?),
        "uuid" => Operation::Uuid(signed(a)?),
        "attribute" => Operation::Attribute {
            device: signed(a)?,
            attribute: signed(b)?,
        },
        "allocate" => Operation::Allocate(num(a)?),
        name => {
            if a != &Value::Null || b != &Value::Null {
                return Err(Fault::Shape);
            }
            match name {
                "environment_cache" => {
                    Operation::Environment(child_options::Variable::CacheDisable)
                }
                "environment_loading" => {
                    Operation::Environment(child_options::Variable::ModuleLoading)
                }
                "initialize" => Operation::Initialize,
                "driver_version" => Operation::DriverVersion,
                "module_loading_mode" => Operation::ModuleLoadingMode,
                "device_count" => Operation::DeviceCount,
                "context_create" => Operation::ContextCreate,
                "context_device" => Operation::ContextDevice,
                "module_load" => Operation::ModuleLoad,
                "function" => Operation::Function,
                "memory_info" => Operation::MemoryInfo,
                "stream_create" => Operation::StreamCreate,
                _ => return Err(Fault::Shape),
            }
        }
    };
    if !matches!(op, Operation::Attribute { .. }) && b != &Value::Null {
        return Err(Fault::Shape);
    }
    Ok(op)
}
fn output_value(v: Output) -> Value {
    let (kind, value) = match v {
        Output::None => ("none", Value::Null),
        Output::Integer(n) => ("integer", Value::I64(n.into())),
        Output::Uuid(v) => ("uuid", text(&hex(&v))),
        Output::HandleIsNull(v) => ("handle_null", Value::Bool(v)),
        Output::Memory { free, total } => (
            "memory",
            object([
                ("free", Value::U64(free as u64)),
                ("total", Value::U64(total as u64)),
            ]),
        ),
        Output::AllocationIsZero(v) => ("allocation_zero", Value::Bool(v)),
        Output::Environment(r) => ("environment", environment_value(r)),
    };
    object([("kind", text(kind)), ("value", value)])
}
fn read_output(v: &Value) -> Result<Output, Fault> {
    let o = obj(v, &["kind", "value"])?;
    let v = get(o, "value")?;
    Ok(match txt(get(o, "kind")?)? {
        "none" => {
            if v != &Value::Null {
                return Err(Fault::Shape);
            }
            Output::None
        }
        "integer" => Output::Integer(signed(v)?),
        "uuid" => Output::Uuid(unhex(v).map_err(|_| Fault::Shape)?),
        "handle_null" => Output::HandleIsNull(boolean(v)?),
        "allocation_zero" => Output::AllocationIsZero(boolean(v)?),
        "environment" => Output::Environment(read_environment(v)?),
        "memory" => {
            let o = obj(v, &["free", "total"])?;
            Output::Memory {
                free: num(get(o, "free")?)?,
                total: num(get(o, "total")?)?,
            }
        }
        _ => return Err(Fault::Shape),
    })
}
fn return_value(v: Return) -> Value {
    object([
        ("status", Value::I64(v.status.into())),
        ("output", output_value(v.output)),
    ])
}
fn read_return(v: &Value) -> Result<Return, Fault> {
    let o = obj(v, &["status", "output"])?;
    Ok(Return {
        status: signed(get(o, "status")?)?,
        output: read_output(get(o, "output")?)?,
    })
}
fn entry_value(e: &Entry) -> Value {
    object([
        ("operation", operation_value(e.operation)),
        ("before", anchor(e.before)),
        ("after", anchor(e.after)),
        ("attempted", Value::Bool(e.attempted)),
        ("raw", e.raw.map(return_value).unwrap_or(Value::Null)),
    ])
}
fn read_entry(v: &Value) -> Result<Entry, Fault> {
    let o = obj(v, &["operation", "before", "after", "attempted", "raw"])?;
    Ok(Entry {
        operation: read_operation(get(o, "operation")?)?,
        before: read_anchor(get(o, "before")?)?,
        after: read_anchor(get(o, "after")?)?,
        attempted: boolean(get(o, "attempted")?)?,
        raw: optional(get(o, "raw")?, read_return)?,
    })
}
fn fault_value(f: TraceFault) -> Value {
    let (kind, status) = match f {
        TraceFault::Binding => ("binding", None),
        TraceFault::Capacity => ("capacity", None),
        TraceFault::Clock => ("clock", None),
        TraceFault::Deadline => ("deadline", None),
        TraceFault::Stopped => ("stopped", None),
        TraceFault::Protocol => ("protocol", None),
        TraceFault::Native(n) => ("native", Some(n)),
    };
    object([("kind", text(kind)), ("status", int(status))])
}
fn read_fault(v: &Value) -> Result<TraceFault, Fault> {
    let o = obj(v, &["kind", "status"])?;
    let status = get(o, "status")?;
    let kind = txt(get(o, "kind")?)?;
    if kind == "native" {
        return Ok(TraceFault::Native(signed(status)?));
    }
    if status != &Value::Null {
        return Err(Fault::Shape);
    }
    Ok(match kind {
        "binding" => TraceFault::Binding,
        "capacity" => TraceFault::Capacity,
        "clock" => TraceFault::Clock,
        "deadline" => TraceFault::Deadline,
        "stopped" => TraceFault::Stopped,
        "protocol" => TraceFault::Protocol,
        _ => return Err(Fault::Shape),
    })
}
fn obj<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Object, Fault> {
    let o = v.as_object().ok_or(Fault::Shape)?;
    o.require_exact(keys).map_err(|_| Fault::Shape)?;
    Ok(o)
}
fn get<'a>(o: &'a Object, key: &str) -> Result<&'a Value, Fault> {
    o.get(key).ok_or(Fault::Shape)
}
fn txt(v: &Value) -> Result<&str, Fault> {
    v.as_text().ok_or(Fault::Shape)
}
fn num<T: TryFrom<u64>>(v: &Value) -> Result<T, Fault> {
    T::try_from(v.as_u64().map_err(|_| Fault::Shape)?).map_err(|_| Fault::Shape)
}
fn signed(v: &Value) -> Result<i32, Fault> {
    i32::try_from(v.as_i64().map_err(|_| Fault::Shape)?).map_err(|_| Fault::Shape)
}
fn read_int(v: &Value) -> Result<Option<i32>, Fault> {
    optional(v, signed)
}
fn boolean(v: &Value) -> Result<bool, Fault> {
    if let Value::Bool(v) = v {
        Ok(*v)
    } else {
        Err(Fault::Shape)
    }
}
fn optional<T>(v: &Value, f: impl FnOnce(&Value) -> Result<T, Fault>) -> Result<Option<T>, Fault> {
    if v == &Value::Null {
        Ok(None)
    } else {
        f(v).map(Some)
    }
}
fn array(v: &Value, max: usize) -> Result<&[Value], Fault> {
    let a = v.as_array().ok_or(Fault::Shape)?;
    if a.len() > max {
        return Err(Fault::Capacity);
    }
    Ok(a)
}
fn read_anchor(v: &Value) -> Result<Option<Anchor>, Fault> {
    settings::parse_anchor(v).map_err(|_| Fault::Shape)
}
