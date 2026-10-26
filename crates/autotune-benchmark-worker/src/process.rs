//! Fixed fake-only Windows containment fixture. Not a native worker launcher,
//! production consent channel, security sandbox or GPU-completion observer.
use autotune_fma_reference::profile::Profile as FrozenProfile;
use std::{
    ffi::OsStr,
    io,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        JobObjects::*,
        Pipes::{CreatePipe, PeekNamedPipe},
        Threading::*,
    },
};

/// Frozen exit-observation cap in milliseconds for Win32 waits.
fn exit_wait_ms() -> u32 {
    (FrozenProfile::approved_design().limits.exit_observation_ns / 1_000_000) as u32
}

struct Owned(HANDLE);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Owned {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Profile {
    Normal,
    Cooperative,
    Hang,
    ChildAttempt,
    Inheritance,
    DoneThenHang,
    EofThenHang,
}
impl Profile {
    fn value(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Cooperative => 1,
            Self::Hang => 2,
            Self::ChildAttempt => 3,
            Self::Inheritance => 4,
            Self::DoneThenHang => 5,
            Self::EofThenHang => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub sequence: u32,
    pub kind: u32,
    pub value: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    pub wait_status: u32,
    pub exit_code: Option<u32>,
    pub active_processes: Option<u32>,
    pub wait_error: Option<u32>,
    pub accounting_error: Option<u32>,
}
impl Observation {
    pub fn exited_and_empty(self) -> bool {
        self.wait_status == WAIT_OBJECT_0
            && self.exit_code.is_some()
            && self.active_processes == Some(0)
    }
}
pub struct FakeProcess {
    process: Owned,
    job: Owned,
    stop: Owned,
    status: Owned,
    run: [u8; 16],
    sequence: u32,
    creation: u64,
}
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn binary() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let parent = exe
        .parent()
        .ok_or_else(|| io::Error::other("no executable parent"))?;
    let root = if parent.file_name() == Some(OsStr::new("deps")) {
        parent
            .parent()
            .ok_or_else(|| io::Error::other("no target parent"))?
    } else {
        parent
    };
    let fixed = root
        .join("autotune-benchmark-fake-worker.exe")
        .canonicalize()?;
    if fixed.file_name() != Some(OsStr::new("autotune-benchmark-fake-worker.exe")) {
        return Err(io::Error::other("fixed binary mismatch"));
    }
    Ok(fixed)
}
fn ok(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn attributes() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    }
}
fn event() -> io::Result<Owned> {
    let sa = attributes();
    unsafe { Owned::new(CreateEventW(&sa, 1, 0, null())) }
}

impl FakeProcess {
    /// Fixed owned quiet process, suspended until all job limits and assignment
    /// succeed. A containment failure terminates only this new suspended child.
    pub fn launch(profile: Profile, run: [u8; 16]) -> io::Result<Self> {
        if run == [0; 16] {
            return Err(io::Error::other("empty run"));
        }
        let stop = event()?;
        let decoy = event()?; // intentionally inheritable but NOT in handle list
                              // An explicitly inherited reference duplicate lets the child distinguish
                              // an actually leaked decoy handle from numeric handle-slot reuse during
                              // runtime startup. Handle validity alone is not object identity.
        let reference = unsafe {
            let mut reference = null_mut();
            ok(DuplicateHandle(
                GetCurrentProcess(),
                decoy.0,
                GetCurrentProcess(),
                &mut reference,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            ))?;
            Owned::new(reference)?
        };
        let (status, write) = unsafe {
            let (mut read, mut write) = (null_mut(), null_mut());
            ok(CreatePipe(&mut read, &mut write, &attributes(), 4096))?;
            let read = Owned::new(read)?;
            let write = Owned::new(write)?;
            ok(SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0))?;
            (read, write)
        };
        let job = unsafe { Owned::new(CreateJobObjectW(null(), null()))? };
        let frozen = FrozenProfile::approved_design().limits;
        unsafe {
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_PROCESS_TIME;
            limits.BasicLimitInformation.ActiveProcessLimit = frozen.processes_per_job;
            limits.BasicLimitInformation.PerProcessUserTimeLimit =
                (frozen.worker_cpu_ns_per_run / 100) as i64;
            limits.ProcessMemoryLimit = frozen.worker_commit_bytes as usize;
            limits.JobMemoryLimit = frozen.worker_commit_bytes as usize;
            ok(SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            ))?;
            let mut applied: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            ok(QueryInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&mut applied as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&applied) as u32,
                null_mut(),
            ))?;
            if applied.BasicLimitInformation.LimitFlags != limits.BasicLimitInformation.LimitFlags
                || applied.BasicLimitInformation.ActiveProcessLimit != frozen.processes_per_job
                || applied.ProcessMemoryLimit != limits.ProcessMemoryLimit
                || applied.JobMemoryLimit != limits.JobMemoryLimit
                || applied.BasicLimitInformation.PerProcessUserTimeLimit
                    != limits.BasicLimitInformation.PerProcessUserTimeLimit
            {
                return Err(io::Error::other("required job limits not applied"));
            }
        }
        let path = binary()?;
        let run_hex: String = run.iter().map(|b| format!("{b:02x}")).collect();
        let command = format!(
            "\"{}\" --fake-v1 {} {} {} {} {} {}",
            path.display(),
            profile.value(),
            run_hex,
            stop.0 as usize,
            write.0 as usize,
            decoy.0 as usize,
            reference.0 as usize
        );
        let (process, thread) = spawn_suspended(&path, &command, &[stop.0, write.0, reference.0])?;
        unsafe {
            if AssignProcessToJobObject(job.0, process.0) == 0 {
                let error = io::Error::last_os_error();
                TerminateProcess(process.0, 90);
                WaitForSingleObject(process.0, exit_wait_ms());
                return Err(error);
            }
            if ResumeThread(thread.0) == u32::MAX {
                let error = io::Error::last_os_error();
                TerminateJobObject(job.0, 91);
                WaitForSingleObject(process.0, exit_wait_ms());
                return Err(error);
            }
        }
        drop(thread);
        drop(write);
        let creation = creation_time(process.0)?;
        Ok(Self {
            process,
            job,
            stop,
            status,
            run,
            sequence: 0,
            creation,
        })
    }
    pub fn creation_time(&self) -> u64 {
        self.creation
    }
    pub fn accounting_diagnostic(&self) -> io::Result<Vec<(usize, String)>> {
        unsafe {
            let mut storage = [0usize; 66];
            ok(QueryInformationJobObject(
                self.job.0,
                JobObjectBasicProcessIdList,
                storage.as_mut_ptr().cast(),
                std::mem::size_of_val(&storage) as u32,
                null_mut(),
            ))?;
            let list = &*(storage.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST);
            let count = list.NumberOfProcessIdsInList as usize;
            if count > 64 {
                return Err(io::Error::other("job process-list capacity"));
            }
            let ids = std::slice::from_raw_parts(list.ProcessIdList.as_ptr(), count);
            ids.iter()
                .map(|id| {
                    let process = Owned::new(OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        *id as u32,
                    ))?;
                    let mut image = [0u16; 32768];
                    let mut length = image.len() as u32;
                    ok(QueryFullProcessImageNameW(
                        process.0,
                        0,
                        image.as_mut_ptr(),
                        &mut length,
                    ))?;
                    Ok((*id, String::from_utf16_lossy(&image[..length as usize])))
                })
                .collect()
        }
    }
    pub fn request_stop(&self) -> io::Result<()> {
        unsafe { ok(SetEvent(self.stop.0)) }
    }
    /// A request only, not exit evidence.
    pub fn request_termination(&self) -> io::Result<()> {
        unsafe { ok(TerminateJobObject(self.job.0, 92)) }
    }
    pub fn observe(&self, wait_ms: u32) -> Observation {
        unsafe {
            let wait_status = WaitForSingleObject(self.process.0, wait_ms.min(exit_wait_ms()));
            let wait_error = if wait_status == WAIT_FAILED {
                Some(GetLastError())
            } else {
                None
            };
            let mut exit = 0;
            let exit_code = if wait_status == WAIT_OBJECT_0
                && GetExitCodeProcess(self.process.0, &mut exit) != 0
            {
                Some(exit)
            } else {
                None
            };
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = zeroed();
            let success = QueryInformationJobObject(
                self.job.0,
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of_val(&accounting) as u32,
                null_mut(),
            );
            Observation {
                wait_status,
                exit_code,
                active_processes: if success != 0 {
                    Some(accounting.ActiveProcesses)
                } else {
                    None
                },
                wait_error,
                accounting_error: if success == 0 {
                    Some(GetLastError())
                } else {
                    None
                },
            }
        }
    }
    /// At most one fixed 40-byte frame; Peek prevents a blocking read. Protocol
    /// rejection is sticky (sequence set invalid), never reconnect/replay.
    pub fn receive(&mut self) -> io::Result<Option<Frame>> {
        if self.sequence == u32::MAX {
            return Err(io::Error::other("protocol terminal"));
        }
        let result = self.receive_inner();
        if result.is_err() {
            self.sequence = u32::MAX;
        }
        result
    }
    fn receive_inner(&mut self) -> io::Result<Option<Frame>> {
        let mut available = 0;
        unsafe {
            ok(PeekNamedPipe(
                self.status.0,
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            ))?;
        }
        if available == 0 {
            return Ok(None);
        }
        if !(40..=64 * 40).contains(&available) {
            return Err(io::Error::other("partial/oversize fixture frame"));
        }
        let mut bytes = [0u8; 40];
        let mut read = 0;
        unsafe {
            ok(ReadFile(
                self.status.0,
                bytes.as_mut_ptr(),
                40,
                &mut read,
                null_mut(),
            ))?;
        }
        let frame = decode(&bytes[..read as usize], self.run, self.sequence)?;
        self.sequence += 1;
        Ok(Some(frame))
    }
}
fn decode(bytes: &[u8], run: [u8; 16], sequence: u32) -> io::Result<Frame> {
    if bytes.len() != 40
        || bytes[..4] != 1u32.to_le_bytes()
        || bytes[4..20] != run
        || bytes[20..24] != sequence.to_le_bytes()
        || bytes[36..40] != [0; 4]
    {
        return Err(io::Error::other("fixture binding/sequence/length"));
    }
    let frame = Frame {
        sequence,
        kind: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        value: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
    };
    if !matches!(frame.kind, 1..=5) {
        return Err(io::Error::other("unknown fixture frame"));
    }
    Ok(frame)
}
fn creation_time(process: HANDLE) -> io::Result<u64> {
    unsafe {
        let (mut creation, mut exit, mut kernel, mut user) =
            (zeroed(), zeroed(), zeroed(), zeroed());
        ok(GetProcessTimes(
            process,
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        ))?;
        Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
    }
}
fn spawn_suspended(
    path: &std::path::Path,
    command: &str,
    handles: &[HANDLE],
) -> io::Result<(Owned, Owned)> {
    unsafe {
        let mut size = 0;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        ok(InitializeProcThreadAttributeList(list, 1, 0, &mut size))?;
        let result = (|| {
            ok(UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            ))?;
            let mut startup: STARTUPINFOEXW = zeroed();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.lpAttributeList = list;
            let mut process: PROCESS_INFORMATION = zeroed();
            // Explicit empty child environment, no parent GPU/loader overrides.
            let environment = [0u16; 2];
            let mut command = wide(OsStr::new(command));
            ok(CreateProcessW(
                wide(path.as_os_str()).as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                null(),
                &startup.StartupInfo,
                &mut process,
            ))?;
            Ok((Owned::new(process.hProcess)?, Owned::new(process.hThread)?))
        })();
        DeleteProcThreadAttributeList(list);
        result
    }
}
fn send(status: HANDLE, run: [u8; 16], sequence: u32, kind: u32, value: u64) -> io::Result<()> {
    let mut bytes = [0u8; 40];
    bytes[..4].copy_from_slice(&1u32.to_le_bytes());
    bytes[4..20].copy_from_slice(&run);
    bytes[20..24].copy_from_slice(&sequence.to_le_bytes());
    bytes[24..28].copy_from_slice(&kind.to_le_bytes());
    bytes[28..36].copy_from_slice(&value.to_le_bytes());
    let mut written = 0;
    unsafe {
        ok(WriteFile(
            status,
            bytes.as_ptr(),
            40,
            &mut written,
            null_mut(),
        ))?;
    }
    if written != 40 {
        return Err(io::Error::other("short status write"));
    }
    Ok(())
}

/// Called only by the fixed fake binary; no native module is referenced here.
pub fn fake_main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--fake-owner-v1") {
        return owner_main(&args);
    }
    if args.len() != 8
        || args[1] != "--fake-v1"
        || args[3].len() != 32
        || !args[3].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(io::Error::other("fixed fake arguments required"));
    }
    let profile: u32 = args[2].parse().map_err(io::Error::other)?;
    if profile > 6 {
        return Err(io::Error::other("unknown fixed fake profile"));
    }
    let mut run = [0u8; 16];
    for (i, byte) in run.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&args[3][i * 2..i * 2 + 2], 16).map_err(io::Error::other)?;
    }
    if run == [0; 16] {
        return Err(io::Error::other("empty run"));
    }
    let handle = |arg: usize| -> io::Result<HANDLE> {
        Ok(args[arg].parse::<usize>().map_err(io::Error::other)? as HANDLE)
    };
    let stop = handle(4)?;
    let status = handle(5)?;
    let decoy = handle(6)?;
    let reference = handle(7)?;
    let mut flags = 0;
    unsafe {
        ok(GetHandleInformation(stop, &mut flags))?;
        ok(GetHandleInformation(status, &mut flags))?;
        ok(GetHandleInformation(reference, &mut flags))?;
    }
    send(status, run, 0, 1, 0)?; // ready, not exit
    match profile {
        0 => send(status, run, 1, 2, 0)?,
        1 => {
            if unsafe { WaitForSingleObject(stop, 3000) } != WAIT_OBJECT_0 {
                return Err(io::Error::other("stop not observed"));
            }
            send(status, run, 1, 3, 0)?;
        }
        2 | 5 | 6 => {
            if profile == 5 {
                send(status, run, 1, 2, 0)?;
            }
            if profile == 6 {
                unsafe {
                    ok(CloseHandle(status))?;
                }
            }
            std::thread::sleep(Duration::from_secs(10));
        }
        3 => {
            let path = binary()?;
            // The denied child, if Windows creates a suspended handle anyway,
            // is immediately terminated and never resumed.
            let attempt = spawn_suspended(
                &path,
                &format!("\"{}\" --denied-child", path.display()),
                &[stop, status],
            );
            match attempt {
                Ok((child, _)) => unsafe {
                    TerminateProcess(child.0, 93);
                    WaitForSingleObject(child.0, 1000);
                    return Err(io::Error::other(
                        "active-process limit did not reject child",
                    ));
                },
                Err(error) => send(
                    status,
                    run,
                    1,
                    4,
                    u64::from(error.raw_os_error().unwrap_or(-1) as u32),
                )?,
            }
        }
        4 => {
            let inherited = unsafe { CompareObjectHandles(decoy, reference) } != 0;
            send(status, run, 1, 5, u64::from(inherited))?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn owner_main(args: &[String]) -> io::Result<()> {
    if args.len() != 6 {
        return Err(io::Error::other("owner fixture arguments"));
    }
    let parent = args[2].parse::<usize>().map_err(io::Error::other)? as HANDLE;
    let status = args[3].parse::<usize>().map_err(io::Error::other)? as HANDLE;
    let stop = args[4].parse::<usize>().map_err(io::Error::other)? as HANDLE;
    if args[5] != "owned-parent-loss-fixture" {
        return Err(io::Error::other("owner fixture binding"));
    }
    let child = FakeProcess::launch(Profile::Hang, [7; 16])?;
    let mut transferred = null_mut();
    unsafe {
        ok(DuplicateHandle(
            GetCurrentProcess(),
            child.process.0,
            parent,
            &mut transferred,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        ))?;
    }
    send(status, [7; 16], 0, 1, transferred as usize as u64)?;
    // Keep the sole job handle alive until this newly owned controller fixture
    // is killed. The external observer retains only the child's process handle.
    unsafe {
        WaitForSingleObject(stop, 10_000);
    }
    drop(child);
    Ok(())
}

#[derive(Debug)]
pub struct OwnerLossEvidence {
    pub owner_wait_status: u32,
    pub child_wait_status: u32,
    pub child_exit_code: Option<u32>,
    /// Intentionally unavailable: duplicating the sole job handle would defeat
    /// the owner-loss condition. No surviving observer can query this job.
    pub empty_job_observed: Option<bool>,
}
/// Owned fake controller-loss experiment, never a production controller path.
pub fn owner_loss_fixture() -> io::Result<OwnerLossEvidence> {
    let stop = event()?;
    let (read, write) = unsafe {
        let (mut read, mut write) = (null_mut(), null_mut());
        ok(CreatePipe(&mut read, &mut write, &attributes(), 4096))?;
        let read = Owned::new(read)?;
        let write = Owned::new(write)?;
        ok(SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0))?;
        (read, write)
    };
    let parent = unsafe {
        let mut parent = null_mut();
        ok(DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentProcess(),
            GetCurrentProcess(),
            &mut parent,
            PROCESS_DUP_HANDLE,
            1,
            0,
        ))?;
        Owned::new(parent)?
    };
    let path = binary()?;
    let command = format!(
        "\"{}\" --fake-owner-v1 {} {} {} owned-parent-loss-fixture",
        path.display(),
        parent.0 as usize,
        write.0 as usize,
        stop.0 as usize
    );
    let (owner, thread) = spawn_suspended(&path, &command, &[parent.0, write.0, stop.0])?;
    // This process is the fixture controller, deliberately OUTSIDE its child's
    // max-one-process job; the test harness retains its direct process handle.
    struct Controller(Owned);
    impl Drop for Controller {
        fn drop(&mut self) {
            unsafe {
                TerminateProcess(self.0 .0, 94);
                WaitForSingleObject(self.0 .0, exit_wait_ms());
            }
        }
    }
    let owner = Controller(owner);
    unsafe {
        if ResumeThread(thread.0) == u32::MAX {
            return Err(io::Error::last_os_error());
        }
    }
    drop(thread);
    drop(write);
    drop(parent);
    let start = Instant::now();
    let mut available = 0;
    loop {
        unsafe {
            ok(PeekNamedPipe(
                read.0,
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            ))?;
        }
        if available >= 40 {
            break;
        }
        if start.elapsed() >= Duration::from_millis(u64::from(exit_wait_ms())) {
            return Err(io::Error::other("owner readiness timeout"));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut bytes = [0u8; 40];
    let mut count = 0;
    unsafe {
        ok(ReadFile(
            read.0,
            bytes.as_mut_ptr(),
            40,
            &mut count,
            null_mut(),
        ))?;
    }
    if count != 40
        || bytes[..4] != 1u32.to_le_bytes()
        || bytes[4..20] != [7; 16]
        || bytes[20..24] != [0; 4]
        || bytes[24..28] != 1u32.to_le_bytes()
        || bytes[36..40] != [0; 4]
    {
        return Err(io::Error::other("owner frame binding"));
    }
    let child =
        Owned::new(u64::from_le_bytes(bytes[28..36].try_into().unwrap()) as usize as HANDLE)?;
    // Retained duplicate of the ORIGINAL child process, not a PID reopen.
    creation_time(child.0)?;
    unsafe {
        ok(TerminateProcess(owner.0 .0, 94))?;
        let owner_wait_status = WaitForSingleObject(owner.0 .0, exit_wait_ms());
        let child_wait_status = WaitForSingleObject(child.0, exit_wait_ms());
        let mut exit = 0;
        let child_exit_code =
            if child_wait_status == WAIT_OBJECT_0 && GetExitCodeProcess(child.0, &mut exit) != 0 {
                Some(exit)
            } else {
                None
            };
        Ok(OwnerLossEvidence {
            owner_wait_status,
            child_wait_status,
            child_exit_code,
            empty_job_observed: None,
        })
    }
}

/// Bounded observation helper, never waits on the job object itself.
pub fn observe_until_exit(process: &FakeProcess, timeout: Duration) -> Observation {
    let start = Instant::now();
    loop {
        let observation = process.observe(10);
        if observation.exited_and_empty()
            || start.elapsed() >= timeout.min(Duration::from_millis(u64::from(exit_wait_ms())))
            || observation.wait_error.is_some()
            || observation.accounting_error.is_some()
        {
            return observation;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_protocol_rejects_version_run_sequence_kind_and_truncation() {
        let mut bytes = [0u8; 40];
        bytes[..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[4..20].copy_from_slice(&[1; 16]);
        bytes[24..28].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(&bytes, [1; 16], 0).is_ok());
        assert!(decode(&bytes, [2; 16], 0).is_err());
        assert!(decode(&bytes, [1; 16], 1).is_err());
        for length in 0..40 {
            assert!(decode(&bytes[..length], [1; 16], 0).is_err());
        }
        for index in [0, 4, 20, 24, 36] {
            let mut bad = bytes;
            bad[index] = 255;
            assert!(decode(&bad, [1; 16], 0).is_err());
        }
    }
}
