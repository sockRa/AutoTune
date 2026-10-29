//! Dedicated bounded artifact writer. All calls made by the control path are
//! try_send/try_recv; teardown never joins an unfinished storage thread.
use super::{
    archive::{Archive, Artifact, IndexBinding, Receipt, Sink},
    budget::{Budget, Reservation},
    producer::BoundText,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
};
enum Command {
    Artifact(Artifact, BoundText),
    Finish(IndexBinding),
}
#[derive(Clone, Copy, Debug)]
pub struct Failure {
    pub operation: &'static str,
    pub os_code: Option<i32>,
    pub kind: io::ErrorKind,
}
/// Returned failure evidence stays available even when the artifact writer
/// cannot persist another byte. Diagnostic delivery is not a durability claim.
pub struct IncompleteCapture {
    pub manifest: crate::evidence::Digest,
    pub failure: Option<Failure>,
    pub cancellation: Result<(), u32>,
    pub writer_finished: bool,
    pub raw: Option<BoundText>,
    pub(super) _metadata: Reservation,
}
impl IncompleteCapture {
    pub fn retain(
        manifest: crate::evidence::Digest,
        failure: Option<Failure>,
        cancellation: Result<(), u32>,
        writer_finished: bool,
        raw: impl FnOnce() -> Option<BoundText>,
        budget: &Budget,
    ) -> io::Result<Self> {
        let metadata = budget
            .reserve(4096)
            .map_err(|_| io::Error::other("failure diagnostic metadata unavailable"))?;
        let raw = raw();
        Ok(Self {
            manifest,
            failure,
            cancellation,
            writer_finished,
            raw,
            _metadata: metadata,
        })
    }
}
impl std::fmt::Debug for IncompleteCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncompleteCapture")
            .field("manifest", &self.manifest)
            .field("failure", &self.failure)
            .field("cancellation", &self.cancellation)
            .field("writer_finished", &self.writer_finished)
            .field("raw_bytes", &self.raw.as_ref().map(|raw| raw.len()))
            .finish()
    }
}
impl std::fmt::Display for IncompleteCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "incomplete_capture {:?}", self)?;
        match &self.raw {
            Some(raw) => f.write_str(raw),
            None => f.write_str("raw_failure_evidence_unavailable\n"),
        }
    }
}
impl std::error::Error for IncompleteCapture {}
impl Failure {
    pub(super) fn new(operation: &'static str, e: io::Error) -> Self {
        Self {
            operation,
            os_code: e.raw_os_error(),
            kind: e.kind(),
        }
    }
}
pub struct Writer {
    sender: Option<SyncSender<Command>>,
    completion: Receiver<Result<Receipt, Failure>>,
    thread: JoinHandle<()>,
    budget: Budget,
    next: usize,
    finished: bool,
    _metadata: Arc<Reservation>,
    ready: Arc<AtomicBool>,
    written: Arc<AtomicUsize>,
}
impl Writer {
    pub fn start<S: Sink + Send + 'static>(sink: S, budget: Budget) -> io::Result<Self> {
        Self::start_factory(move || Ok(sink), budget)
    }
    pub fn start_factory<
        S: Sink + Send + 'static,
        F: FnOnce() -> io::Result<S> + Send + 'static,
    >(
        factory: F,
        budget: Budget,
    ) -> io::Result<Self> {
        // Four fixed primary artifacts and one final index request. Queue slot
        // storage remains charged until the last worker/queue owner drops.
        let metadata = Arc::new(
            budget
                .reserve(std::mem::size_of::<Command>() * 5 * 2 + 4096)
                .map_err(|_| io::Error::other("writer metadata capacity"))?,
        );
        let storage = metadata.clone();
        let (sender, receiver) = mpsc::sync_channel(5);
        let (done, completion) = mpsc::sync_channel(1);
        let ready = Arc::new(AtomicBool::new(false));
        let background_ready = ready.clone();
        let written = Arc::new(AtomicUsize::new(0));
        let background_written = written.clone();
        let thread = thread::Builder::new()
            .name("sampler-artifact-writer".into())
            .spawn(move || {
                let _storage = storage;
                let sink = match factory() {
                    Ok(sink) => sink,
                    Err(e) => {
                        let _ = done.try_send(Err(Failure::new("output_root", e)));
                        return;
                    }
                };
                let mut archive = Archive::new(sink);
                background_ready.store(true, Ordering::Release);
                while let Ok(command) = receiver.recv() {
                    let result = match command {
                        Command::Artifact(kind, bytes) => archive
                            .write(kind, bytes.as_bytes())
                            .map(|_| None)
                            .map_err(|e| Failure::new(kind.name(), e)),
                        Command::Finish(binding) => archive
                            .finish(binding)
                            .map(Some)
                            .map_err(|e| Failure::new("capture-index.json", e)),
                    };
                    match result {
                        Ok(None) => {
                            background_written.fetch_add(1, Ordering::AcqRel);
                        }
                        Ok(Some(receipt)) => {
                            let _ = done.try_send(Ok(receipt));
                            break;
                        }
                        Err(e) => {
                            // Close admission before publishing the failure or
                            // dropping a sink whose destructor can still block.
                            // A peer observing teardown must not enqueue more
                            // mandatory artifacts into a terminal writer.
                            drop(receiver);
                            let _ = done.try_send(Err(e));
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            completion,
            thread,
            budget,
            next: 0,
            finished: false,
            _metadata: metadata,
            ready,
            written,
        })
    }
    pub fn enqueue(&mut self, artifact: Artifact, mut bytes: BoundText) -> io::Result<()> {
        let expected = [
            Artifact::Manifest,
            Artifact::Profile,
            Artifact::Raw,
            Artifact::Session,
        ]
        .get(self.next);
        if self.finished || expected != Some(&artifact) {
            self.close();
            return Err(io::Error::other("writer order/terminal"));
        }
        bytes
            .recharge(&self.budget)
            .map_err(|_| io::Error::other("writer bytes exhausted"))?;
        let result = self
            .sender
            .as_ref()
            .ok_or_else(|| io::Error::other("writer closed"))?
            .try_send(Command::Artifact(artifact, bytes))
            .map_err(|_| io::Error::other("writer queue full/closed"));
        if result.is_ok() {
            self.next += 1;
        } else {
            self.close();
        }
        result
    }
    pub fn finish(&mut self, binding: IndexBinding) -> io::Result<()> {
        if self.finished || self.next != 4 {
            self.close();
            return Err(io::Error::other("incomplete writer"));
        }
        let result = self
            .sender
            .as_ref()
            .ok_or_else(|| io::Error::other("writer closed"))?
            .try_send(Command::Finish(binding))
            .map_err(|_| io::Error::other("writer queue full/closed"));
        self.finished = true;
        self.sender.take();
        result
    }
    pub fn poll(&self) -> Option<Result<Receipt, Failure>> {
        match self.completion.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(Failure {
                operation: "writer_completion",
                os_code: None,
                kind: io::ErrorKind::BrokenPipe,
            })),
        }
    }
    pub fn thread_finished(&self) -> bool {
        self.thread.is_finished()
    }
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn written_artifacts(&self) -> usize {
        self.written.load(Ordering::Acquire)
    }
    pub fn close(&mut self) {
        self.finished = true;
        self.sender.take();
    }
    #[cfg(windows)]
    pub fn request_cancel(&mut self) -> Result<(), u32> {
        use std::os::windows::io::AsRawHandle;
        self.close();
        // This requests cancellation only. The retained thread handle and
        // buffers remain alive; a false finished observation stays unresolved.
        if unsafe {
            windows_sys::Win32::System::IO::CancelSynchronousIo(self.thread.as_raw_handle())
        } != 0
        {
            Ok(())
        } else {
            Err(unsafe { windows_sys::Win32::Foundation::GetLastError() })
        }
    }
}

/// Fixed run-owned leaves, create_new only. Existing output is never overwritten.
/// Completion is an observed write/flush result, not power-loss durability.
pub struct DirectorySink {
    root: PathBuf,
    _directory: File,
}
impl DirectorySink {
    pub fn open(root: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(root)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "output root must be an existing ordinary directory",
            ));
        }
        if fs::read_dir(root)?.next().is_some() {
            return Err(io::Error::other("output root is not empty"));
        }
        let canonical = fs::canonicalize(root)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            options
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        let directory = options.open(&canonical)?;
        Ok(Self {
            root: canonical,
            _directory: directory,
        })
    }
}
impl Sink for DirectorySink {
    fn write(&mut self, name: &'static str, bytes: &[u8]) -> io::Result<()> {
        if ![
            "manifest.json",
            "profile.json",
            "raw-events.ndjson",
            "session.json",
            "capture-index.json",
        ]
        .contains(&name)
        {
            return Err(io::Error::other("unknown output leaf"));
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join(name))?;
        output.write_all(bytes)?;
        output.flush()
    }
}
