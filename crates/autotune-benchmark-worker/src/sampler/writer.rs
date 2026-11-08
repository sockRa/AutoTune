//! Dedicated bounded artifact writer. All calls made by the control path are
//! try_send/try_recv; teardown never joins an unfinished storage thread.
use super::{
    archive::{Archive, Artifact, IndexBinding, Receipt, Sink},
    budget::{Budget, Reservation},
    producer::BoundText,
    workload_archive::{IndexBinding as WorkloadIndexBinding, WorkloadArchive},
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
    Finish(FinishBinding),
}
enum FinishBinding {
    ReadOnly(IndexBinding),
    Workload(WorkloadIndexBinding),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArchiveMode {
    ReadOnly,
    Workload,
}
enum ActiveArchive<S: Sink> {
    ReadOnly(Archive<S>),
    Workload(WorkloadArchive<S>),
}
impl<S: Sink> ActiveArchive<S> {
    fn new(sink: S, mode: ArchiveMode) -> Self {
        match mode {
            ArchiveMode::ReadOnly => Self::ReadOnly(Archive::new(sink)),
            ArchiveMode::Workload => Self::Workload(WorkloadArchive::new(sink)),
        }
    }
    fn write(&mut self, artifact: Artifact, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::ReadOnly(archive) => archive.write(artifact, bytes),
            Self::Workload(archive) => archive.write(artifact, bytes),
        }
    }
    fn finish(&mut self, binding: FinishBinding) -> io::Result<Receipt> {
        match (self, binding) {
            (Self::ReadOnly(archive), FinishBinding::ReadOnly(binding)) => archive.finish(binding),
            (Self::Workload(archive), FinishBinding::Workload(binding)) => {
                archive.finish(binding).map(|receipt| Receipt {
                    primary_bytes: receipt.primary_bytes,
                    total_bytes: receipt.total_bytes,
                    index_sha256: receipt.index_sha256,
                })
            }
            _ => Err(io::Error::other("writer/archive scope mismatch")),
        }
    }
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
    mode: ArchiveMode,
    _metadata: Arc<Reservation>,
    ready: Arc<AtomicBool>,
    written: Arc<AtomicUsize>,
}
impl Writer {
    pub fn start<S: Sink + Send + 'static>(sink: S, budget: Budget) -> io::Result<Self> {
        Self::start_factory_mode(move || Ok(sink), budget, ArchiveMode::ReadOnly)
    }
    /// Start a separate workload-scoped writer. It writes the same four fixed
    /// leaves but finalizes with a `workload_session` index and worker binding.
    pub fn start_workload<S: Sink + Send + 'static>(sink: S, budget: Budget) -> io::Result<Self> {
        Self::start_factory_mode(move || Ok(sink), budget, ArchiveMode::Workload)
    }
    /// Start a pilot-bound workload writer and enqueue the exact frozen
    /// manifest/profile bytes as its first two mandatory leaves.
    pub fn start_pilot_workload<S: Sink + Send + 'static>(
        sink: S,
        budget: Budget,
        manifest: &crate::workload::manifest::PilotManifest,
    ) -> io::Result<Self> {
        let (frozen_manifest, frozen_profile) = pilot_header(manifest, &budget)?;
        let mut writer = Self::start_workload(sink, budget.clone())?;
        writer.enqueue(Artifact::Manifest, frozen_manifest)?;
        writer.enqueue(Artifact::Profile, frozen_profile)?;
        Ok(writer)
    }
    pub fn start_factory<
        S: Sink + Send + 'static,
        F: FnOnce() -> io::Result<S> + Send + 'static,
    >(
        factory: F,
        budget: Budget,
    ) -> io::Result<Self> {
        Self::start_factory_mode(factory, budget, ArchiveMode::ReadOnly)
    }
    pub fn start_workload_factory<
        S: Sink + Send + 'static,
        F: FnOnce() -> io::Result<S> + Send + 'static,
    >(
        factory: F,
        budget: Budget,
    ) -> io::Result<Self> {
        Self::start_factory_mode(factory, budget, ArchiveMode::Workload)
    }
    /// Start a pilot-bound workload writer whose sink is created on its
    /// dedicated writer thread. This keeps filesystem setup off the parent
    /// deadline path while preserving the manifest/profile-first archive.
    pub fn start_pilot_workload_factory<
        S: Sink + Send + 'static,
        F: FnOnce() -> io::Result<S> + Send + 'static,
    >(
        factory: F,
        budget: Budget,
        manifest: &crate::workload::manifest::PilotManifest,
    ) -> io::Result<Self> {
        let (frozen_manifest, frozen_profile) = pilot_header(manifest, &budget)?;
        let mut writer = Self::start_workload_factory(factory, budget)?;
        writer.enqueue(Artifact::Manifest, frozen_manifest)?;
        writer.enqueue(Artifact::Profile, frozen_profile)?;
        Ok(writer)
    }
    fn start_factory_mode<
        S: Sink + Send + 'static,
        F: FnOnce() -> io::Result<S> + Send + 'static,
    >(
        factory: F,
        budget: Budget,
        mode: ArchiveMode,
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
                let mut archive = ActiveArchive::new(sink, mode);
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
            mode,
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
        self.finish_with_binding(FinishBinding::ReadOnly(binding))
    }
    pub fn finish_workload(&mut self, binding: WorkloadIndexBinding) -> io::Result<()> {
        self.finish_with_binding(FinishBinding::Workload(binding))
    }
    /// Bind workload index identities to the frozen pilot manifest and the
    /// digest produced by `BoundEnvelope::encode`; arbitrary caller hashes are
    /// not accepted on this finalization path.
    pub fn finish_pilot_workload(
        &mut self,
        manifest: &crate::workload::manifest::PilotManifest,
        session: &BoundText,
    ) -> io::Result<()> {
        let session_digest = match pilot_session_digest(manifest, session) {
            Ok(digest) => digest,
            Err(error) => {
                self.close();
                return Err(error);
            }
        };
        self.finish_workload(WorkloadIndexBinding::from_pilot(manifest, session_digest))
    }
    /// Validate and enqueue the pilot-bound session as the final primary leaf.
    pub fn enqueue_pilot_session(
        &mut self,
        manifest: &crate::workload::manifest::PilotManifest,
        session: BoundText,
    ) -> io::Result<()> {
        if let Err(error) = pilot_session_digest(manifest, &session) {
            self.close();
            return Err(error);
        }
        self.enqueue(Artifact::Session, session)
    }
    fn finish_with_binding(&mut self, binding: FinishBinding) -> io::Result<()> {
        let scope_matches = matches!(
            (self.mode, &binding),
            (ArchiveMode::ReadOnly, FinishBinding::ReadOnly(_))
                | (ArchiveMode::Workload, FinishBinding::Workload(_))
        );
        if !scope_matches {
            self.close();
            return Err(io::Error::other("writer/archive scope mismatch"));
        }
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

fn pilot_header(
    manifest: &crate::workload::manifest::PilotManifest,
    budget: &Budget,
) -> io::Result<(BoundText, BoundText)> {
    let frozen_manifest = BoundText::copy(manifest.canonical(), budget)
        .map_err(|_| io::Error::other("workload manifest evidence capacity"))?;
    let frozen_profile = BoundText::copy(manifest.profile_canonical(), budget)
        .map_err(|_| io::Error::other("workload profile evidence capacity"))?;
    Ok((frozen_manifest, frozen_profile))
}

fn pilot_session_digest(
    manifest: &crate::workload::manifest::PilotManifest,
    session: &BoundText,
) -> io::Result<crate::evidence::Digest> {
    let session_digest = session
        .session_digest()
        .ok_or_else(|| io::Error::other("workload session is not digest-bound"))?;
    let envelope = crate::evidence::decode_envelope(
        session.as_bytes(),
        session.len(),
        crate::evidence::AcceptPolicy::AllowSimulated,
    )
    .map_err(|_| io::Error::other("workload session envelope is invalid"))?;
    let draft = manifest.manifest_draft();
    let artifacts = draft.artifacts;
    if envelope.manifest.manifest_digest != crate::evidence::Obs::Known(manifest.digest())
        || envelope.manifest.profile_digest
            != crate::evidence::Obs::Known(manifest.profile_digest())
        || envelope.artifacts.source_commit != draft.source_commit
        || envelope.artifacts.worker_sha256 != crate::evidence::Obs::Known(artifacts.worker)
        || envelope.artifacts.lockfile_sha256 != crate::evidence::Obs::Known(artifacts.lockfile)
        || envelope.artifacts.sampler_sha256 != crate::evidence::Obs::Known(artifacts.sampler)
        || envelope.artifacts.ptx_sha256 != crate::evidence::Obs::Known(artifacts.ptx)
    {
        return Err(io::Error::other(
            "workload session does not match pilot manifest",
        ));
    }
    Ok(session_digest)
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
