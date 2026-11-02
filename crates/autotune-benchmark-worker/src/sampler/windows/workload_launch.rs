//! Fixed pilot process startup using the sampler's existing containment core.
//! Source/compile evidence only; no launch or workload execution is authorized.
pub use super::FailedLaunch as RetainedFailure;
use super::*;
use crate::workload::{
    launch::{CancelQueue, Declaration, Role},
    transport::{Queue, SendFailure, Sender},
    windows_transport::{InheritedTransport, Receive},
    wire::Direction,
};
fn check(shared: &SharedLaunch) -> io::Result<()> {
    if shared.cancelled.load(Ordering::Acquire) {
        Err(io::Error::from_raw_os_error(ERROR_OPERATION_ABORTED as i32))
    } else {
        Ok(())
    }
}

pub struct Process {
    declaration: Declaration,
    process: OwnedHandle,
    shared: SharedLaunch,
    _image: File,
    image_path: PathBuf,
    image_digest: [u8; 32],
    creation: u64,
    transport: InheritedTransport,
    termination: Option<Result<(), u32>>,
    sender_taken: AtomicBool,
    _metadata: super::super::budget::Reservation,
}
pub struct ProcessQueue<'a> {
    process: &'a Process,
}
impl Queue for ProcessQueue<'_> {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        self.process.transport.queue_copy(lane, bytes)
    }
}
impl Process {
    pub fn reserved_control_budget(&self) -> Budget {
        self.transport.reserved_control_budget()
    }
    fn launch(declaration: Declaration, budget: Budget, shared: SharedLaunch) -> io::Result<Self> {
        check(&shared)?;
        // Same bounded hashing/UTF-16/attribute-list allowance as the existing
        // sampler launcher, acquired before file/path/pipe/thread construction.
        let metadata = budget
            .reserve(1024 * 1024)
            .map_err(|_| io::Error::other("pilot launch metadata capacity"))?;
        let (path, image, digest) = measure_path(sibling_path(declaration.role().image_name())?)?;
        if &digest != declaration.image_digest().as_bytes() {
            return Err(io::Error::other("pilot image digest mismatch"));
        }
        let (parent_tx0, child_rx0) = pipe(true)?;
        let (parent_tx1, child_rx1) = pipe(true)?;
        let (parent_rx0, child_tx0) = pipe(false)?;
        let (parent_rx1, child_tx1) = pipe(false)?;
        let handles = [
            child_rx0.as_raw_handle(),
            child_rx1.as_raw_handle(),
            child_tx0.as_raw_handle(),
            child_tx1.as_raw_handle(),
            shared.stop.as_raw_handle(),
        ];
        let arguments = declaration
            .arguments(handles.map(|h| h as usize))
            .map_err(|_| io::Error::other("pilot launch arguments"))?;
        let command = format!("\"{}\" {}", path.display(), arguments.join(" "));
        let environment = declaration.role().environment();
        let transport =
            InheritedTransport::new([parent_rx0, parent_rx1], [parent_tx0, parent_tx1], budget)?;
        check(&shared)?;
        let (process, thread) = spawn_environment(&path, &command, &handles, &environment)?;
        let mut creation = None;
        let mut assigned = false;
        let mut stage = "pilot_cancel_before_assignment";
        // Every post-CreateProcess failure transfers the original handle and
        // I/O shutdown observations into the existing retained failure owner.
        let preparation = (|| -> io::Result<()> {
            check(&shared)?;
            stage = "pilot_assign_job";
            unsafe {
                ok(AssignProcessToJobObject(
                    shared.job.as_raw_handle(),
                    process.as_raw_handle(),
                ))?;
            }
            assigned = true;
            stage = "pilot_creation_time";
            let (mut created, mut exited, mut kernel, mut user) =
                unsafe { (zeroed(), zeroed(), zeroed(), zeroed()) };
            unsafe {
                ok(GetProcessTimes(
                    process.as_raw_handle(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                ))?;
            }
            creation =
                Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime));
            stage = "pilot_cancel_before_resume";
            check(&shared)?;
            stage = "pilot_resume";
            unsafe {
                if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
            }
            stage = "pilot_cancel_after_resume";
            check(&shared)?;
            Ok(())
        })();
        if let Err(error) = preparation {
            let error = failed_launch(
                process,
                &shared,
                (metadata, transport.shutdown()),
                stage,
                raw_code(&error),
                assigned,
                creation,
            );
            let mut failure = *FailedLaunch::from_error(error).expect("created retained failure");
            failure._image = Some(image);
            failure.image = Some((path, digest));
            return Err(io::Error::other(failure));
        }
        drop(thread);
        drop((child_rx0, child_rx1, child_tx0, child_tx1));
        Ok(Self {
            declaration,
            process,
            shared,
            _image: image,
            image_path: path,
            image_digest: digest,
            creation: creation.expect("successful creation observation"),
            transport,
            termination: None,
            sender_taken: AtomicBool::new(false),
            _metadata: metadata,
        })
    }
    pub fn declaration(&self) -> &Declaration {
        &self.declaration
    }
    pub fn image(&self) -> (&std::path::Path, [u8; 32]) {
        (&self.image_path, self.image_digest)
    }
    pub fn creation_time(&self) -> u64 {
        self.creation
    }
    pub fn cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Acquire)
    }
    pub fn request_stop(&self) -> io::Result<()> {
        self.shared.cancelled.store(true, Ordering::Release);
        unsafe { ok(SetEvent(self.shared.stop.as_raw_handle())) }
    }
    pub fn request_termination(&mut self) -> io::Result<()> {
        self.shared.cancelled.store(true, Ordering::Release);
        let result = unsafe { ok(TerminateJobObject(self.shared.job.as_raw_handle(), 93)) };
        self.termination = Some(result.as_ref().map(|_| ()).map_err(raw_code));
        result
    }
    /// Parent outgoing frames bind this exact declaration; stop closes ordinary
    /// queue admission even for a sender created before the cancellation race.
    pub fn sender(
        &self,
        budget: &Budget,
    ) -> Result<Sender<CancelQueue<ProcessQueue<'_>>>, SendFailure> {
        if self.sender_taken.swap(true, Ordering::AcqRel) {
            return Err(SendFailure::Closed);
        }
        Sender::new(
            CancelQueue::new(
                ProcessQueue { process: self },
                self.shared.cancelled.clone(),
            ),
            self.declaration.binding(),
            Direction::ToWorker,
            budget,
        )
    }
    pub fn receive(&self, clock: impl FnMut() -> Option<crate::monitor::Anchor>) -> Receive {
        self.transport.receive(clock)
    }
    pub fn observe(&self) -> ProcessObservation {
        observe_process(
            &self.process,
            &self.shared.job,
            self.creation,
            self.termination,
        )
    }
    pub fn resources(&self) -> super::super::runtime::ResourceObservation {
        observe_resources(&self.process)
    }
    pub fn inputs_finished(&self) -> bool {
        self.transport.inputs_finished()
    }
    pub fn output_drained(&self) -> bool {
        self.transport.output_drained()
    }
    pub fn shutdown_transport(&self) -> [super::super::runtime::IoObservation; 4] {
        self.transport.shutdown()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        // The launch observer may still hold the shared job handle. Do not
        // depend on its eventual Drop to request containment after owner loss.
        if self.termination.is_none() {
            let _ = self.request_termination();
        }
    }
}

/// Bounded background startup; file hashing and process creation never run on
/// the controller's deadline path. Callers must already have exact scoped
/// process/native approval; declaration hashes are not that approval.
pub struct Launch {
    role: Role,
    binding: crate::workload::wire::WorkerBinding,
    shared: SharedLaunch,
    result: Receiver<io::Result<Process>>,
    thread: JoinHandle<()>,
    transferred: bool,
    observed: bool,
    _metadata: super::super::budget::Reservation,
}
impl Launch {
    pub fn start(declaration: Declaration, budget: Budget) -> io::Result<Self> {
        let role = declaration.role();
        let binding = declaration.binding();
        let metadata = budget
            .reserve(std::mem::size_of::<Self>() + 4096)
            .map_err(|_| io::Error::other("pilot launch capacity"))?;
        let (commit, cpu_ns) = declaration.role().limits();
        let shared = SharedLaunch::with_job(job_limits(commit, cpu_ns)?)?;
        let background = shared.clone();
        let (send, result) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("pilot-launch".into())
            .spawn(move || {
                let outcome = Process::launch(declaration, budget, background);
                if let Err(mpsc::TrySendError::Disconnected(Ok(mut process))) =
                    send.try_send(outcome)
                {
                    let _ = process.request_termination();
                }
            })?;
        Ok(Self {
            role,
            binding,
            shared,
            result,
            thread,
            transferred: false,
            observed: false,
            _metadata: metadata,
        })
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn binding(&self) -> crate::workload::wire::WorkerBinding {
        self.binding
    }
    pub fn request_stop(&self) -> io::Result<()> {
        self.shared.cancelled.store(true, Ordering::Release);
        unsafe { ok(SetEvent(self.shared.stop.as_raw_handle())) }
    }
    pub fn request_termination(&self) -> io::Result<()> {
        self.shared.cancelled.store(true, Ordering::Release);
        unsafe { ok(TerminateJobObject(self.shared.job.as_raw_handle(), 95)) }
    }
    /// A late cancelled result is still returned as its original observable
    /// process owner. It stays cancelled, requests termination and cannot send
    /// ordinary work; never discard a live handle to turn it into a plain error.
    pub fn poll(&mut self) -> Option<io::Result<Process>> {
        if self.observed {
            return None;
        }
        match self.result.try_recv() {
            Ok(mut outcome) => {
                self.observed = true;
                if self.shared.cancelled.load(Ordering::Acquire) {
                    if let Ok(process) = &mut outcome {
                        let _ = process.request_termination();
                    }
                }
                self.transferred = outcome.is_ok();
                Some(outcome)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.observed = true;
                Some(Err(io::Error::other("pilot launch observation lost")))
            }
        }
    }
    pub fn thread_finished(&self) -> bool {
        self.thread.is_finished()
    }
}
impl Drop for Launch {
    fn drop(&mut self) {
        if !self.transferred {
            let _ = self.request_termination();
        }
    }
}
