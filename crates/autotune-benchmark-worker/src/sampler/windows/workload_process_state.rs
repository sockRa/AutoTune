//! Actual retained-handle reads. Compile/link evidence only until execution is
//! separately authorized; no PID reopening, process control, DLL or GPU calls.
use super::*;
use crate::{
    monitor::windows_clock::WindowsClock,
    monitor::Anchor,
    workload::process_state::{
        self as model, LimitRead, Limits, Operation, Output, Pid, Returned, Snapshot, SourceKind,
        Times, Window,
    },
};
fn limits(raw: &JOBOBJECT_EXTENDED_LIMIT_INFORMATION) -> Limits {
    Limits {
        flags: raw.BasicLimitInformation.LimitFlags,
        active_limit: raw.BasicLimitInformation.ActiveProcessLimit,
        user_100ns: raw.BasicLimitInformation.PerProcessUserTimeLimit,
        process_memory: raw.ProcessMemoryLimit as u64,
        job_memory: raw.JobMemoryLimit as u64,
    }
}
struct Native<'a, D: FixedDeclaration> {
    process: &'a Process<D>,
    clock: &'a WindowsClock,
}
impl<D: FixedDeclaration> model::Source for Native<'_, D> {
    fn capture(&mut self) -> Result<Anchor, model::ClockFault> {
        self.clock.capture().map_err(Into::into)
    }
    fn cancelled(&self) -> bool {
        self.process.cancelled()
    }
    fn clock_identity(&self) -> (u64, [u8; 16]) {
        (self.clock.qpc_frequency(), self.clock.clock_token())
    }
    fn read(&mut self, op: Operation) -> Output {
        let process = self.process.process.as_raw_handle();
        let job = self.process.shared.job.as_raw_handle();
        // SAFETY: Process owns both original handles throughout the immutable
        // borrow; output structs are initialized and sized before each API call.
        unsafe {
            match op {
                Operation::Pid => {
                    let value = GetProcessId(process);
                    Output::Pid(Pid {
                        value,
                        error: (value == 0).then(|| GetLastError()),
                    })
                }
                Operation::Times => {
                    let (mut created, mut exited, mut kernel, mut user) =
                        (zeroed(), zeroed(), zeroed(), zeroed());
                    let status =
                        GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
                    let error = (status == 0).then(|| GetLastError());
                    let ticks = |t: FILETIME| {
                        (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
                    };
                    Output::Times(Returned {
                        status,
                        error,
                        value: Times {
                            creation: ticks(created),
                            exit: ticks(exited),
                            kernel: ticks(kernel),
                            user: ticks(user),
                        },
                    })
                }
                Operation::JobFlags => {
                    let mut value = 0;
                    let status = GetHandleInformation(job, &mut value);
                    let error = (status == 0).then(|| GetLastError());
                    Output::JobFlags(Returned {
                        status,
                        value,
                        error,
                    })
                }
                Operation::Limits => {
                    let mut actual: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                    let mut returned_bytes = 0;
                    let status = QueryInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        (&mut actual as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                        size_of_val(&actual) as u32,
                        &mut returned_bytes,
                    );
                    let error = (status == 0).then(|| GetLastError());
                    Output::Limits(Returned {
                        status,
                        error,
                        value: LimitRead {
                            limits: limits(&actual),
                            returned_bytes,
                        },
                    })
                }
                Operation::Process => Output::Process(self.process.observe()),
                Operation::Membership => {
                    let mut value = 0;
                    let status = IsProcessInJob(process, job, &mut value);
                    let error = (status == 0).then(|| GetLastError());
                    Output::Membership(Returned {
                        status,
                        value,
                        error,
                    })
                }
            }
        }
    }
}
pub(super) fn capture<D: FixedDeclaration>(
    process: &Process<D>,
    window: Window,
    clock: &WindowsClock,
) -> Snapshot {
    let (commit, cpu) = process.declaration.launch_role().limits();
    let wanted = configured_job_limits(commit, cpu);
    model::collect(
        process.origin,
        window,
        limits(&wanted),
        size_of_val(&wanted) as u32,
        SourceKind::RetainedHandle,
        &mut Native { process, clock },
    )
}
