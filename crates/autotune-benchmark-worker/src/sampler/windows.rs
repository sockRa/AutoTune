//! Fixed sampler process containment. Compile-only until exact execution consent.
//! Wait/exit/job facts are independent, and never native quiescence evidence.
pub use super::ingress::Received;
use super::{
    budget::Budget,
    supervisor::ProcessObservation,
    windows_io::Transport,
    wire::{Channel, Lane, Message, WireBinding},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, Read},
    mem::{size_of, size_of_val, zeroed},
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::FILE_SHARE_READ,
    System::{JobObjects::*, Pipes::CreatePipe, ProcessStatus::*, Threading::*},
};

fn ok(value: i32) -> io::Result<()> {
    if value == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
unsafe fn own(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(OwnedHandle::from_raw_handle(handle))
    }
}
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn fixed_path(fixture: bool) -> io::Result<PathBuf> {
    let current = std::env::current_exe()?;
    let parent = current
        .parent()
        .ok_or_else(|| io::Error::other("executable parent"))?;
    let parent = if parent.file_name() == Some(OsStr::new("deps")) {
        parent
            .parent()
            .ok_or_else(|| io::Error::other("target parent"))?
    } else {
        parent
    };
    parent
        .join(if fixture {
            "autotune-sampler-fixture.exe"
        } else {
            "autotune-nvml-sampler.exe"
        })
        .canonicalize()
}
fn measure(fixture: bool) -> io::Result<(PathBuf, File, [u8; 32])> {
    let path = fixed_path(fixture)?;
    let mut image = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 32768];
    loop {
        let n = image.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok((path, image, hash.finalize().into()))
}
fn pipe(child_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    unsafe {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let (mut read, mut write) = (null_mut(), null_mut());
        ok(CreatePipe(&mut read, &mut write, &attributes, 4096))?;
        let read = own(read)?;
        let write = own(write)?;
        let (parent, child) = if child_reads {
            (write, read)
        } else {
            (read, write)
        };
        ok(SetHandleInformation(
            parent.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            0,
        ))?;
        Ok((parent, child))
    }
}
fn job() -> io::Result<OwnedHandle> {
    unsafe {
        let job = own(CreateJobObjectW(null(), null()))?;
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let mut wanted: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        wanted.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_PROCESS_TIME;
        wanted.BasicLimitInformation.ActiveProcessLimit = p.processes_per_job;
        wanted.BasicLimitInformation.PerProcessUserTimeLimit =
            (p.sampler_cpu_ns_per_session / 100) as i64;
        wanted.ProcessMemoryLimit = p.sampler_commit_bytes as usize;
        wanted.JobMemoryLimit = p.sampler_commit_bytes as usize;
        ok(SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&wanted as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of_val(&wanted) as u32,
        ))?;
        let mut actual: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        ok(QueryInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&mut actual as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of_val(&actual) as u32,
            null_mut(),
        ))?;
        if wanted.BasicLimitInformation.LimitFlags != actual.BasicLimitInformation.LimitFlags
            || wanted.BasicLimitInformation.ActiveProcessLimit
                != actual.BasicLimitInformation.ActiveProcessLimit
            || wanted.BasicLimitInformation.PerProcessUserTimeLimit
                != actual.BasicLimitInformation.PerProcessUserTimeLimit
            || wanted.ProcessMemoryLimit != actual.ProcessMemoryLimit
            || wanted.JobMemoryLimit != actual.JobMemoryLimit
        {
            return Err(io::Error::other("sampler limits readback mismatch"));
        }
        Ok(job)
    }
}
fn spawn(
    path: &std::path::Path,
    command: &str,
    handles: &[HANDLE],
) -> io::Result<(OwnedHandle, OwnedHandle)> {
    unsafe {
        let mut bytes = 0;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        if bytes == 0 || bytes > 65536 {
            return Err(io::Error::other("attribute list size"));
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        ok(InitializeProcThreadAttributeList(list, 1, 0, &mut bytes))?;
        let result = (|| {
            ok(UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                null_mut(),
                null(),
            ))?;
            let mut startup: STARTUPINFOEXW = zeroed();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.lpAttributeList = list;
            let mut info: PROCESS_INFORMATION = zeroed();
            let environment = [0u16; 2];
            let mut cmd = wide(OsStr::new(command));
            ok(CreateProcessW(
                wide(path.as_os_str()).as_ptr(),
                cmd.as_mut_ptr(),
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
                &mut info,
            ))?;
            Ok((own(info.hProcess)?, own(info.hThread)?))
        })();
        DeleteProcThreadAttributeList(list);
        result
    }
}
pub struct SamplerProcess {
    _launch_metadata: super::budget::Reservation,
    process: OwnedHandle,
    _job: Arc<OwnedHandle>,
    stop: Arc<OwnedHandle>,
    _image: File,
    creation: u64,
    transport: Transport,
    tx: [Channel; 2],
    rx: [Channel; 2],
    termination: Option<Result<(), u32>>,
}
/// A created child whose launch did not complete. Its original process handle
/// survives the error and remains observable, including before job assignment.
pub struct FailedLaunch {
    process: OwnedHandle,
    job: Arc<OwnedHandle>,
    _metadata: super::budget::Reservation,
    stage: &'static str,
    error: u32,
    assigned: bool,
    creation: Option<u64>,
    termination: Option<Result<(), u32>>,
    io_shutdown: [super::runtime::IoObservation; 4],
}
impl std::fmt::Debug for FailedLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FailedLaunch")
            .field("stage", &self.stage)
            .field("error", &self.error)
            .field("assigned", &self.assigned)
            .field("creation", &self.creation)
            .field("termination", &self.termination)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for FailedLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sampler launch failed at {}: {}", self.stage, self.error)
    }
}
impl std::error::Error for FailedLaunch {}
impl FailedLaunch {
    pub fn from_error(error: io::Error) -> Result<Box<Self>, io::Error> {
        if error.get_ref().is_some_and(|e| e.is::<Self>()) {
            Ok(error
                .into_inner()
                .expect("checked inner error")
                .downcast()
                .expect("checked type"))
        } else {
            Err(error)
        }
    }
    pub fn request_termination(&mut self) -> io::Result<()> {
        let result = unsafe {
            if self.assigned {
                ok(TerminateJobObject(self.job.as_raw_handle(), 96))
            } else {
                ok(TerminateProcess(self.process.as_raw_handle(), 96))
            }
        };
        self.termination = Some(result.as_ref().map(|_| ()).map_err(raw_code));
        result
    }
    pub fn observe(&self) -> ProcessObservation {
        observe_process(
            &self.process,
            &self.job,
            self.creation.unwrap_or(0),
            self.termination,
        )
    }
    pub fn evidence(&self) -> super::supervisor::Input {
        super::supervisor::Input::LaunchFailure {
            stage: self.stage,
            error: self.error,
            assigned: self.assigned,
            creation: self.creation,
            process: self.observe(),
        }
    }
    pub fn io_shutdown(&self) -> [super::runtime::IoObservation; 4] {
        self.io_shutdown
    }
}
fn raw_code(error: &io::Error) -> u32 {
    error
        .raw_os_error()
        .map(|n| n as u32)
        .unwrap_or(ERROR_INVALID_DATA)
}
fn failed_launch(
    process: OwnedHandle,
    shared: &SharedLaunch,
    cleanup: (
        super::budget::Reservation,
        [super::runtime::IoObservation; 4],
    ),
    stage: &'static str,
    error: u32,
    assigned: bool,
    creation: Option<u64>,
) -> io::Error {
    let mut failure = FailedLaunch {
        process,
        job: shared.job.clone(),
        _metadata: cleanup.0,
        io_shutdown: cleanup.1,
        stage,
        error,
        assigned,
        creation,
        termination: None,
    };
    // A request is retained independently of the later process/job observation.
    let _ = failure.request_termination();
    io::Error::other(failure)
}
/// Decoded message allocations remain charged until supervision either retains
/// their raw payload under its own charge or stops on explicit capacity loss.
#[cfg(feature = "sampler-process-fixture")]
pub enum FixtureMode {
    Cooperative,
    Hang,
}
#[cfg(feature = "sampler-process-fixture")]
pub enum WireFixtureMode {
    Cooperative,
    BlockedGetter,
    WrongIdentity,
}
impl SamplerProcess {
    /// Fixed production binary only. Callers must validate the exact immutable
    /// capture/approval manifest before invoking this source-level launch API.
    pub fn launch(
        expected_digest: [u8; 32],
        binding: WireBinding,
        budget: Budget,
    ) -> io::Result<Self> {
        Self::launch_inner(expected_digest, binding, budget, None, SharedLaunch::new()?)
    }
    fn launch_inner(
        expected_digest: [u8; 32],
        binding: WireBinding,
        budget: Budget,
        fixture: Option<u32>,
        shared: SharedLaunch,
    ) -> io::Result<Self> {
        shared.check()?;
        // Two Windows maximum-length paths/command buffers plus the bounded
        // attribute list and hashing buffer, including simultaneous UTF-16
        // copies. Charge before measure/format/spawn and retain with the child.
        let launch_metadata = budget
            .reserve(1024 * 1024)
            .map_err(|_| io::Error::other("launch metadata capacity"))?;
        let (path, image, digest) = measure(fixture.is_some())?;
        if digest != expected_digest {
            return Err(io::Error::other("sampler artifact digest mismatch"));
        }
        let (parent_tx0, child_rx0) = pipe(true)?;
        let (parent_tx1, child_rx1) = pipe(true)?;
        let (parent_rx0, child_tx0) = pipe(false)?;
        let (parent_rx1, child_tx1) = pipe(false)?;
        let stop = shared.stop.clone();
        let handles = [
            child_rx0.as_raw_handle(),
            child_rx1.as_raw_handle(),
            child_tx0.as_raw_handle(),
            child_tx1.as_raw_handle(),
            stop.as_raw_handle(),
        ];
        let job = shared.job.clone();
        let b = binding.monitor;
        let command = format!(
            "\"{}\" --sampler-v1 {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            path.display(),
            handles[0] as usize,
            handles[1] as usize,
            handles[2] as usize,
            handles[3] as usize,
            handles[4] as usize,
            super::wire_model::hex(&b.session),
            super::wire_model::hex(&b.run),
            super::wire_model::hex(&binding.manifest_digest),
            super::wire_model::hex(&b.clock_token),
            b.qpc_frequency,
            super::wire_model::hex(&b.device.uuid),
            b.device.domain,
            b.device.bus,
            b.device.device,
            fixture.map_or(0, |n| n + 1)
        );
        // Prepare all fallible in-process components before creating a child.
        let tx = [
            Channel::new(binding, Lane::Ordinary),
            Channel::new(binding, Lane::Control),
        ]
        .map(|r| r.map_err(|_| io::Error::other("invalid channel binding")));
        let [a, b] = tx;
        let tx = [a?, b?];
        let rx = [
            Channel::new(binding, Lane::Ordinary),
            Channel::new(binding, Lane::Control),
        ]
        .map(|r| r.map_err(|_| io::Error::other("invalid channel binding")));
        let [a, b] = rx;
        let rx = [a?, b?];
        let transport = Transport::new(
            [parent_rx0, parent_rx1],
            [parent_tx0, parent_tx1],
            budget.clone(),
        )?;
        shared.check()?;
        let (process, thread) = spawn(&path, &command, &handles)?;
        let creation = unsafe {
            if shared.check().is_err() {
                return Err(failed_launch(
                    process,
                    &shared,
                    (launch_metadata, transport.shutdown()),
                    "cancel_before_assignment",
                    ERROR_OPERATION_ABORTED,
                    false,
                    None,
                ));
            }
            if AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) == 0 {
                let error = GetLastError();
                return Err(failed_launch(
                    process,
                    &shared,
                    (launch_metadata, transport.shutdown()),
                    "assign_job",
                    error,
                    false,
                    None,
                ));
            }
            let (mut created, mut exited, mut kernel, mut user) =
                (zeroed(), zeroed(), zeroed(), zeroed());
            if GetProcessTimes(
                process.as_raw_handle(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            ) == 0
            {
                let error = GetLastError();
                return Err(failed_launch(
                    process,
                    &shared,
                    (launch_metadata, transport.shutdown()),
                    "creation_time",
                    error,
                    true,
                    None,
                ));
            }
            let creation =
                (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
            if shared.check().is_err() {
                return Err(failed_launch(
                    process,
                    &shared,
                    (launch_metadata, transport.shutdown()),
                    "cancel_before_resume",
                    ERROR_OPERATION_ABORTED,
                    true,
                    Some(creation),
                ));
            }
            if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                let error = GetLastError();
                return Err(failed_launch(
                    process,
                    &shared,
                    (launch_metadata, transport.shutdown()),
                    "resume",
                    error,
                    true,
                    Some(creation),
                ));
            }
            creation
        };
        drop(thread);
        drop((child_rx0, child_rx1, child_tx0, child_tx1));
        Ok(Self {
            _launch_metadata: launch_metadata,
            process,
            _job: job,
            stop,
            _image: image,
            creation,
            transport,
            tx,
            rx,
            termination: None,
        })
    }
    pub fn creation_time(&self) -> u64 {
        self.creation
    }
    /// Observed user+kernel CPU is distinct from the job's user-mode-only hard
    /// limit. Observation is sampled, not an OS hard cap on total CPU time.
    pub fn resources(&self) -> super::runtime::ResourceObservation {
        unsafe {
            let mut counters: PROCESS_MEMORY_COUNTERS_EX = zeroed();
            counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let memory = K32GetProcessMemoryInfo(
                self.process.as_raw_handle(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                counters.cb,
            );
            let memory_error = (memory == 0).then(|| GetLastError());
            let (mut created, mut exited, mut kernel, mut user) =
                (zeroed(), zeroed(), zeroed(), zeroed());
            let time = GetProcessTimes(
                self.process.as_raw_handle(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            );
            let time_error = (time == 0).then(|| GetLastError());
            let ticks =
                |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
            super::runtime::ResourceObservation {
                commit_bytes: (memory != 0).then_some(counters.PrivateUsage as u64),
                user_100ns: (time != 0).then_some(ticks(user)),
                kernel_100ns: (time != 0).then_some(ticks(kernel)),
                memory_error,
                time_error,
            }
        }
    }
    pub fn request_stop(&self) -> io::Result<()> {
        unsafe { ok(SetEvent(self.stop.as_raw_handle())) }
    }
    pub fn request_termination(&mut self) -> io::Result<()> {
        let result = unsafe { ok(TerminateJobObject(self._job.as_raw_handle(), 93)) };
        self.termination = Some(result.as_ref().map(|_| ()).map_err(|e| {
            e.raw_os_error()
                .map(|n| n as u32)
                .unwrap_or(ERROR_INVALID_DATA)
        }));
        result
    }
    pub fn observe(&self) -> ProcessObservation {
        observe_process(&self.process, &self._job, self.creation, self.termination)
    }
    pub fn send(&mut self, message: &Message) -> io::Result<()> {
        let _temporary = self.transport.codec_reservation(message.lane())?;
        let index = if message.lane() == Lane::Ordinary {
            0
        } else {
            1
        };
        let bytes = self.tx[index]
            .encode(message)
            .map_err(|_| io::Error::other("wire encode rejection"))?;
        self.transport.send(message.lane(), bytes)
    }
    pub fn receive(&mut self) -> io::Result<Option<Received>> {
        match self.transport.receive()? {
            Some((lane, frame)) => {
                let temporary = self.transport.codec_reservation(lane).map_err(|_| ());
                Ok(Some(Received::decode(
                    lane,
                    frame.into_packet(),
                    &mut self.rx[if lane == Lane::Ordinary { 0 } else { 1 }],
                    temporary,
                )))
            }
            None => Ok(None),
        }
    }
    pub fn shutdown_transport(&self) -> [super::runtime::IoObservation; 4] {
        self.transport.shutdown()
    }
    /// A later observation of the same retained I/O worker handles. No new
    /// cancellation request or wait is performed.
    pub fn transport_completion(&self) -> [bool; 4] {
        self.transport.completion()
    }
    pub fn inputs_finished(&self) -> bool {
        self.transport.inputs_finished()
    }
    #[cfg(feature = "sampler-process-fixture")]
    pub fn fixture_digest() -> io::Result<[u8; 32]> {
        Ok(measure(true)?.2)
    }
    #[cfg(feature = "sampler-process-fixture")]
    pub fn launch_fixture(expected: [u8; 32], mode: FixtureMode) -> io::Result<Self> {
        let binding = WireBinding {
            monitor: crate::monitor::Binding {
                session: [1; 16],
                run: [2; 16],
                device: crate::worker::Identity {
                    uuid: [3; 16],
                    domain: 0,
                    bus: 1,
                    device: 0,
                },
                clock_token: [4; 16],
                qpc_frequency: 1_000_000_000,
            },
            manifest_digest: [5; 32],
        };
        Self::launch_inner(
            expected,
            binding,
            Budget::new(4 * 1024 * 1024).map_err(|_| io::Error::other("fixture budget"))?,
            Some(match mode {
                FixtureMode::Cooperative => 0,
                FixtureMode::Hang => 1,
            }),
            SharedLaunch::new()?,
        )
    }
}

impl super::drain::DrainSource for SamplerProcess {
    type Item = Received;
    type Error = io::Error;
    fn inputs_finished(&self) -> bool {
        self.inputs_finished()
    }
    fn receive(&mut self) -> io::Result<Option<Received>> {
        self.receive()
    }
}

#[derive(Clone)]
struct SharedLaunch {
    job: Arc<OwnedHandle>,
    stop: Arc<OwnedHandle>,
    cancelled: Arc<AtomicBool>,
}
impl SharedLaunch {
    fn new() -> io::Result<Self> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        Ok(Self {
            job: Arc::new(job()?),
            stop: Arc::new(unsafe { own(CreateEventW(&attributes, 1, 0, null()))? }),
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(io::Error::other("sampler launch cancelled"))
        } else {
            Ok(())
        }
    }
}
/// File measurement and process startup run off the deadline path. All Arc
/// owners share ONE noninherited job handle inside the controller process;
/// cloning an Arc never duplicates a kernel handle or transfers it to a child.
pub struct SamplerLaunch {
    shared: SharedLaunch,
    result: Receiver<io::Result<SamplerProcess>>,
    thread: JoinHandle<()>,
    transferred: bool,
    observed: bool,
}
impl SamplerLaunch {
    /// Feature-only fixed fake backend on the production wire/control engine.
    /// It never loads NVML, even when bound to a sampler-only capture manifest.
    #[cfg(feature = "sampler-process-fixture")]
    pub fn start_bound_fixture(
        expected: [u8; 32],
        binding: WireBinding,
        budget: Budget,
        mode: WireFixtureMode,
    ) -> io::Result<Self> {
        Self::start_inner(
            expected,
            binding,
            budget,
            Some(match mode {
                WireFixtureMode::Cooperative => 2,
                WireFixtureMode::BlockedGetter => 3,
                WireFixtureMode::WrongIdentity => 4,
            }),
        )
    }
    pub fn start(expected: [u8; 32], binding: WireBinding, budget: Budget) -> io::Result<Self> {
        Self::start_inner(expected, binding, budget, None)
    }
    fn start_inner(
        expected: [u8; 32],
        binding: WireBinding,
        budget: Budget,
        fixture: Option<u32>,
    ) -> io::Result<Self> {
        let shared = SharedLaunch::new()?;
        let background = shared.clone();
        let (send, result) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("sampler-launch".into())
            .spawn(move || {
                let outcome = SamplerProcess::launch_inner(
                    expected,
                    binding,
                    budget,
                    fixture,
                    background.clone(),
                );
                // If the observer vanished, a completed late launch is terminated
                // before dropping its retained handles, never handed to a new run.
                if let Err(mpsc::TrySendError::Disconnected(Ok(mut process))) =
                    send.try_send(outcome)
                {
                    let _ = process.request_termination();
                }
            })?;
        Ok(Self {
            shared,
            result,
            thread,
            transferred: false,
            observed: false,
        })
    }
    pub fn request_stop(&self) -> io::Result<()> {
        self.shared.cancelled.store(true, Ordering::Release);
        unsafe { ok(SetEvent(self.shared.stop.as_raw_handle())) }
    }
    pub fn request_termination(&self) -> io::Result<()> {
        self.shared.cancelled.store(true, Ordering::Release);
        unsafe { ok(TerminateJobObject(self.shared.job.as_raw_handle(), 95)) }
    }
    pub fn poll(&mut self) -> Option<io::Result<SamplerProcess>> {
        if self.observed {
            return None;
        }
        match self.result.try_recv() {
            Ok(outcome) => {
                self.observed = true;
                self.transferred = outcome.is_ok();
                Some(outcome)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.observed = true;
                Some(Err(io::Error::other("launch observation lost")))
            }
        }
    }
    pub fn thread_finished(&self) -> bool {
        self.thread.is_finished()
    }
    #[cfg(feature = "sampler-process-fixture")]
    pub fn start_fixture(expected: [u8; 32], mode: FixtureMode) -> io::Result<Self> {
        let binding = WireBinding {
            monitor: crate::monitor::Binding {
                session: [1; 16],
                run: [2; 16],
                device: crate::worker::Identity {
                    uuid: [3; 16],
                    domain: 0,
                    bus: 1,
                    device: 0,
                },
                clock_token: [4; 16],
                qpc_frequency: 1_000_000_000,
            },
            manifest_digest: [5; 32],
        };
        Self::start_inner(
            expected,
            binding,
            Budget::new(4 * 1024 * 1024).map_err(|_| io::Error::other("fixture budget"))?,
            Some(match mode {
                FixtureMode::Cooperative => 0,
                FixtureMode::Hang => 1,
            }),
        )
    }
}
impl Drop for SamplerLaunch {
    fn drop(&mut self) {
        if !self.transferred {
            let _ = self.request_termination();
        }
    }
}

fn observe_process(
    process: &OwnedHandle,
    job: &OwnedHandle,
    creation: u64,
    termination: Option<Result<(), u32>>,
) -> ProcessObservation {
    unsafe {
        let wait_status = WaitForSingleObject(process.as_raw_handle(), 0);
        let wait_error = (wait_status == WAIT_FAILED).then(|| GetLastError());
        let mut code = 0;
        let (exit_code, exit_error) = if wait_status == WAIT_OBJECT_0 {
            if GetExitCodeProcess(process.as_raw_handle(), &mut code) != 0 {
                (Some(code), None)
            } else {
                (None, Some(GetLastError()))
            }
        } else {
            (None, None)
        };
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = zeroed();
        let success = QueryInformationJobObject(
            job.as_raw_handle(),
            JobObjectBasicAccountingInformation,
            (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            size_of_val(&accounting) as u32,
            null_mut(),
        );
        ProcessObservation {
            creation,
            wait_status,
            wait_error,
            exit_code,
            exit_error,
            termination,
            active_processes: (success != 0).then_some(accounting.ActiveProcesses),
            accounting_error: (success == 0).then(|| GetLastError()),
        }
    }
}
