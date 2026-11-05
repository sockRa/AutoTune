/// Fixed read-only startup/runtime calls. The selected index is an NVML ordinal,
/// not a physical workload mapping or an authorization token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerCall {
    Initialize,
    Count,
    Handle(u32),
    Uuid(u32),
    Pci(u32),
    CudaVersion,
    DriverVersion,
    NvmlVersion,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerOutput {
    None,
    Count(u32),
    HandleIsNull(bool),
    Text([u8; 256]),
    Pci {
        legacy: [u8; 16],
        numbers: [u32; 5],
        bus_id: [u8; 32],
    },
    Version(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerReturn {
    pub status: i32,
    pub output: SamplerOutput,
}
/// Storage/clock/gate hooks, not native execution permission. Reserve evidence
/// before returning true from before(). returned() receives complete initialized
/// output buffers and signed status, including on error, before the caller's
/// post-call clock. False from either hook permanently stops this invocation;
/// no successor native call is issued. No callback is made for an uncalled ABI.
pub trait SamplerObserver {
    /// Host file/loader checkpoint; not an NVML ABI call or execution permit.
    fn file_check(&mut self, _: crate::runtime_file::Stage) -> bool {
        true
    }
    fn before(&mut self, call: SamplerCall) -> bool;
    fn returned(&mut self, call: SamplerCall, raw: SamplerReturn) -> bool;
}
