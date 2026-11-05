#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Environment(crate::workload::child_options::Variable),
    Initialize,
    DriverVersion,
    ModuleLoadingMode,
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
pub enum Output {
    /// Host Win32 return; Return.status is zero for this non-CUDA operation.
    Environment(crate::workload::child_options::Read),
    None,
    Integer(i32),
    Uuid([u8; 16]),
    HandleIsNull(bool),
    Memory {
        free: usize,
        total: usize,
    },
    AllocationIsZero(bool),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Return {
    pub status: i32,
    pub output: Output,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Binding,
    Capacity,
    Clock,
    Deadline,
    Stopped,
    Protocol,
    Native(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub operation: Operation,
    pub before: Option<crate::monitor::Anchor>,
    pub after: Option<crate::monitor::Anchor>,
    pub attempted: bool,
    pub raw: Option<Return>,
}
pub const CAP: usize = 149;
