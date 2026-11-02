//! Common actual inherited child startup. Returns a bound context, never native
//! workload permission. Fixed executable mains/control/native composition follow.
use super::{
    bootstrap::{Bound, EntryClock, Error as BootstrapError},
    launch::{Arguments, Role},
    transport::RawReceived,
    windows_transport::{InheritedTransport, Receive},
};
use crate::{
    monitor::{
        windows_clock::{CaptureError, WindowsClock},
        Anchor,
    },
    sampler::{
        budget::{Budget, Reservation},
        runtime::IoObservation,
        wire::Lane,
    },
};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::{GetFileType, FILE_SHARE_READ, FILE_TYPE_PIPE},
    System::Threading::WaitForSingleObject,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Role,
    Handle,
    Clock,
    Image,
    Stopped,
    Transport,
    Bootstrap(BootstrapError),
    Deadline,
}
pub struct Failure {
    fault: Fault,
    code: Option<u32>,
    clock_error: Option<CaptureError>,
    first: Option<Anchor>,
    last_poll: Option<Anchor>,
    observed_frequency: Option<u64>,
    entry_clock: Option<EntryClock>,
    raw: Option<RawReceived>,
    transport: Option<InheritedTransport>,
    image: Option<File>,
    image_path: Option<PathBuf>,
    image_digest: Option<[u8; 32]>,
    io: Option<[IoObservation; 4]>,
    metadata: Option<Reservation>,
}
impl Failure {
    pub fn fault(&self) -> Fault {
        self.fault
    }
    pub fn code(&self) -> Option<u32> {
        self.code
    }
    pub fn clock_error(&self) -> Option<CaptureError> {
        self.clock_error
    }
    pub fn first_entry(&self) -> Option<Anchor> {
        self.first
    }
    pub fn last_poll(&self) -> Option<Anchor> {
        self.last_poll
    }
    pub fn observed_frequency(&self) -> Option<u64> {
        self.observed_frequency
    }
    pub fn entry_clock(&self) -> Option<&EntryClock> {
        self.entry_clock.as_ref()
    }
    pub fn raw(&self) -> Option<&RawReceived> {
        self.raw.as_ref()
    }
    pub fn image(&self) -> (Option<&Path>, Option<[u8; 32]>) {
        (self.image_path.as_deref(), self.image_digest)
    }
    pub fn io_shutdown(&self) -> Option<&[IoObservation; 4]> {
        self.io.as_ref()
    }
    fn new() -> Self {
        Self {
            fault: Fault::Capacity,
            code: None,
            clock_error: None,
            first: None,
            last_poll: None,
            observed_frequency: None,
            entry_clock: None,
            raw: None,
            transport: None,
            image: None,
            image_path: None,
            image_digest: None,
            io: None,
            metadata: None,
        }
    }
}
pub struct Context {
    bootstrap: Bound,
    raw: RawReceived,
    transport: InheritedTransport,
    stop: OwnedHandle,
    clock: WindowsClock,
    first: Anchor,
    entry_clock: EntryClock,
    _image: File,
    image_path: PathBuf,
    image_digest: [u8; 32],
    budget: Budget,
    _metadata: Reservation,
}
impl Context {
    pub fn bootstrap(&self) -> &Bound {
        &self.bootstrap
    }
    pub fn raw(&self) -> &RawReceived {
        &self.raw
    }
    pub fn image(&self) -> (&Path, [u8; 32]) {
        (&self.image_path, self.image_digest)
    }
    pub fn first_entry(&self) -> Anchor {
        self.first
    }
    pub fn entry_clock(&self) -> &EntryClock {
        &self.entry_clock
    }
    /// Keeps the image and raw bootstrap owners alive while the caller splits
    /// independent control and composes the selected sampler/worker runtime.
    pub fn parts(
        &mut self,
    ) -> (
        &Bound,
        &mut InheritedTransport,
        &WindowsClock,
        &OwnedHandle,
        &Budget,
    ) {
        (
            &self.bootstrap,
            &mut self.transport,
            &self.clock,
            &self.stop,
            &self.budget,
        )
    }
    /// # Safety
    /// Call exactly once at process entry. The five validated argument handles
    /// must be exclusively transferred inherited endpoints, not handles already
    /// owned elsewhere in this process. Numeric validation cannot prove ownership.
    /// Caller still owes exact execution scope and native/control admission.
    #[allow(clippy::result_large_err)]
    pub unsafe fn from_inherited(
        args: &Arguments,
        expected_role: Role,
        budget: Budget,
    ) -> Result<Self, Failure> {
        let mut failure = Failure::new();
        let result = (|| -> Result<Self, Fault> {
            failure.metadata = Some(
                budget
                    .reserve(
                        1024 * 1024 + std::mem::size_of::<Self>() + std::mem::size_of::<Failure>(),
                    )
                    .map_err(|_| Fault::Capacity)?,
            );
            if args.role() != expected_role {
                return Err(Fault::Role);
            }
            let numbers = args.handles();
            // Validate every endpoint before adopting ANY ownership. The fixed
            // launcher specifies which four pipes and stop event are transferred.
            for (i, n) in numbers.iter().enumerate() {
                let mut flags = 0;
                if GetHandleInformation(*n as HANDLE, &mut flags) == 0 {
                    failure.code = Some(GetLastError());
                    return Err(Fault::Handle);
                }
                if i < 4 && GetFileType(*n as HANDLE) != FILE_TYPE_PIPE {
                    return Err(Fault::Handle);
                }
            }
            let handles = numbers.map(|n| OwnedHandle::from_raw_handle(n as HANDLE));
            for h in &handles {
                if SetHandleInformation(h.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) == 0 {
                    failure.code = Some(GetLastError());
                    return Err(Fault::Handle);
                }
            }
            let [r0, r1, w0, w1, stop] = handles;
            let clock = WindowsClock::new(args.epoch().clock_token).map_err(|e| {
                failure.clock_error = Some(e);
                Fault::Clock
            })?;
            failure.observed_frequency = Some(clock.qpc_frequency());
            if clock.qpc_frequency() != args.frequency() {
                return Err(Fault::Clock);
            }
            let first = clock.capture().map_err(|e| {
                failure.clock_error = Some(e);
                Fault::Clock
            })?;
            failure.first = Some(first);
            failure.entry_clock = Some(
                EntryClock::new(args.frequency(), args.epoch(), first, &budget)
                    .map_err(Fault::Bootstrap)?,
            );
            let p = autotune_fma_reference::profile::Profile::approved_design();
            // Before the manifest is known, use a first-actual-entry local cap.
            // A forged/stale argument epoch can never extend this finite wait.
            let local_end = first
                .interrupt_100ns
                .checked_add(p.pilot[0].phase_caps_ns[1] / 100)
                .ok_or(Fault::Deadline)?;
            failure.transport = Some(
                InheritedTransport::new([r0, r1], [w0, w1], budget.clone()).map_err(|e| {
                    failure.code = e.raw_os_error().map(|n| n as u32);
                    Fault::Transport
                })?,
            );
            let path = std::env::current_exe()
                .and_then(|p| p.canonicalize())
                .map_err(|e| {
                    failure.code = e.raw_os_error().map(|n| n as u32);
                    Fault::Image
                })?;
            failure.image_path = Some(path.clone());
            if !path.is_absolute()
                || path.file_name() != Some(std::ffi::OsStr::new(expected_role.image_name()))
            {
                return Err(Fault::Image);
            }
            failure.image = Some(
                OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ)
                    .open(&path)
                    .map_err(|e| {
                        failure.code = e.raw_os_error().map(|n| n as u32);
                        Fault::Image
                    })?,
            );
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 32768];
            loop {
                let n = failure
                    .image
                    .as_mut()
                    .expect("image retained")
                    .read(&mut buffer)
                    .map_err(|e| {
                        failure.code = e.raw_os_error().map(|n| n as u32);
                        Fault::Image
                    })?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            let image_digest: [u8; 32] = hash.finalize().into();
            failure.image_digest = Some(image_digest);
            loop {
                match WaitForSingleObject(stop.as_raw_handle(), 0) {
                    WAIT_OBJECT_0 => return Err(Fault::Stopped),
                    WAIT_TIMEOUT => (),
                    WAIT_FAILED => {
                        failure.code = Some(GetLastError());
                        return Err(Fault::Handle);
                    }
                    _ => return Err(Fault::Handle),
                }
                let mut clock_error = None;
                let received = failure
                    .transport
                    .as_ref()
                    .expect("transport retained")
                    .receive(|| clock.capture().map_err(|e| clock_error = Some(e)).ok());
                if let Some(e) = clock_error {
                    failure.clock_error = Some(e);
                }
                match received {
                    Receive::Packet(raw) => {
                        failure.last_poll = raw.receipt();
                        failure.raw = Some(raw);
                        failure
                            .entry_clock
                            .as_mut()
                            .expect("clock retained")
                            .observe(failure.last_poll)
                            .map_err(Fault::Bootstrap)?;
                        let raw = failure.raw.as_ref().expect("packet retained");
                        if raw.lane() != Lane::Ordinary || raw.packet().rejection().is_some() {
                            return Err(Fault::Transport);
                        }
                        let receipt = raw.receipt().ok_or(Fault::Clock)?;
                        if receipt.interrupt_100ns >= local_end {
                            return Err(Fault::Deadline);
                        }
                        let bootstrap = Bound::parse(
                            args,
                            expected_role,
                            raw.packet().bytes(),
                            first,
                            receipt,
                            &budget,
                        )
                        .map_err(Fault::Bootstrap)?;
                        let run = bootstrap.run();
                        let expected = match expected_role {
                            Role::Worker => run.worker_digest(),
                            Role::Sampler => run.sampler_digest(),
                        };
                        if expected.as_bytes() != &image_digest {
                            return Err(Fault::Image);
                        }
                        // Stop can race the final bootstrap check. It must be
                        // checked again by the independent native-operation gate.
                        return Ok(Self {
                            bootstrap,
                            raw: failure.raw.take().expect("retained packet"),
                            transport: failure.transport.take().expect("retained transport"),
                            stop,
                            clock,
                            first,
                            entry_clock: failure.entry_clock.take().expect("clock retained"),
                            _image: failure.image.take().expect("retained image"),
                            image_path: failure.image_path.take().expect("retained path"),
                            image_digest,
                            budget: budget.clone(),
                            _metadata: failure.metadata.take().expect("startup reservation"),
                        });
                    }
                    Receive::Idle { anchor } => {
                        failure.last_poll = anchor;
                        failure
                            .entry_clock
                            .as_mut()
                            .expect("clock retained")
                            .observe(anchor)
                            .map_err(Fault::Bootstrap)?;
                        let a = anchor.ok_or(Fault::Clock)?;
                        if a.interrupt_100ns >= local_end {
                            return Err(Fault::Deadline);
                        }
                    }
                    Receive::Fault { code, anchor } => {
                        failure.last_poll = anchor;
                        failure.code = code.map(|n| n as u32);
                        let _ = failure
                            .entry_clock
                            .as_mut()
                            .expect("clock retained")
                            .observe(anchor);
                        return Err(Fault::Transport);
                    }
                }
                std::thread::sleep(Duration::from_nanos(p.limits.controller_tick_ns));
            }
        })();
        match result {
            Ok(context) => Ok(context),
            Err(fault) => {
                failure.fault = fault;
                failure.io = failure.transport.as_ref().map(InheritedTransport::shutdown);
                Err(failure)
            }
        }
    }
}
