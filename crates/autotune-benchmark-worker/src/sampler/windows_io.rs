//! Dedicated synchronous pipe workers, never joined on the control path.
//! CancelSynchronousIo only requests cancellation; completion remains observed
//! separately. Microsoft API contract:
//! https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelsynchronousio
use super::{
    budget::{Budget, Reservation},
    framing::{
        write_packet, FaultSlot, Framer, Phase, ReadFault, ReadStep, Rejection, Sink, Source,
    },
    wire::Lane,
};
use std::{
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::{ReadFile, WriteFile},
    System::IO::CancelSynchronousIo,
};

pub(super) struct Frame {
    pub bytes: Vec<u8>,
    _reservation: Option<Reservation>,
    pub rejection: Option<Rejection>,
    completion: Option<Arc<AtomicUsize>>,
}
impl Frame {
    pub(super) fn into_packet(mut self) -> super::framing::Packet {
        super::framing::Packet::from_parts(
            std::mem::take(&mut self.bytes),
            self._reservation.take().expect("frame charge"),
            self.rejection,
        )
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        if let Some(counter) = &self.completion {
            counter.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
pub(super) struct Transport {
    incoming: [Receiver<Frame>; 2],
    outgoing: [SyncSender<Frame>; 2],
    threads: Vec<JoinHandle<()>>,
    closed: Arc<AtomicBool>,
    error: Arc<AtomicU32>,
    budget: Budget,
    control_budget: Budget,
    _metadata: Arc<Reservation>,
    outstanding: [Arc<AtomicUsize>; 2],
    faults: [Arc<FaultSlot<Frame>>; 4],
    readers_done: [Arc<AtomicBool>; 2],
}
pub(super) struct ControlPort {
    incoming: Receiver<Frame>,
    outgoing: SyncSender<Frame>,
    budget: Budget,
    error: Arc<AtomicU32>,
    closed: Arc<AtomicBool>,
    _metadata: Arc<Reservation>,
    outstanding: Arc<AtomicUsize>,
    fault: Arc<FaultSlot<Frame>>,
    write_fault: Arc<FaultSlot<Frame>>,
}
impl ControlPort {
    pub(super) fn codec_reservation(&self) -> io::Result<Reservation> {
        self.budget
            .reserve(codec_bytes())
            .map_err(|_| io::Error::other("control codec capacity"))
    }
    pub(super) fn receive(&self) -> io::Result<Option<Frame>> {
        if let Ok(Some(frame)) = self.fault.take() {
            return Ok(Some(frame));
        }
        if let Ok(Some(frame)) = self.write_fault.take() {
            return Ok(Some(frame));
        }
        match self.incoming.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(_) => {
                let code = self.error.load(Ordering::Acquire);
                if code != 0 {
                    Err(io::Error::from_raw_os_error(code as i32))
                } else {
                    Ok(None)
                }
            }
        }
    }
    pub(super) fn send(&self, bytes: Vec<u8>) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::Error::other("transport closed"));
        }
        let reservation = self
            .budget
            .reserve(bytes.capacity())
            .map_err(|_| io::Error::other("control RAM exhausted"))?;
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        self.outgoing
            .try_send(Frame {
                bytes,
                _reservation: Some(reservation),
                rejection: None,
                completion: Some(self.outstanding.clone()),
            })
            .map_err(|_| io::Error::other("control queue full/closed"))
    }
}
fn index(lane: Lane) -> usize {
    match lane {
        Lane::Ordinary => 0,
        Lane::Control => 1,
    }
}
fn error(error: &AtomicU32, code: u32) {
    let _ = error.compare_exchange(0, code.max(1), Ordering::AcqRel, Ordering::Acquire);
}
struct PipeRead {
    pipe: OwnedHandle,
    closed: Arc<AtomicBool>,
}
impl Source for PipeRead {
    fn read(&mut self, bytes: &mut [u8]) -> ReadStep {
        if self.closed.load(Ordering::Acquire) {
            return ReadStep {
                reported: 0,
                error: Some(ERROR_OPERATION_ABORTED),
            };
        }
        let mut read = 0;
        // SAFETY: a unique reader owns this synchronous pipe and bounded writable
        // slice. Preserve BOOL failure AND its raw reported byte count together.
        let success = unsafe {
            ReadFile(
                self.pipe.as_raw_handle(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        ReadStep {
            reported: read as usize,
            error: (success == 0).then(|| unsafe { GetLastError() }),
        }
    }
}
struct ReaderDone(Arc<AtomicBool>);
impl Drop for ReaderDone {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
fn rejection_code(rejection: Rejection) -> u32 {
    match rejection.fault {
        ReadFault::Os(code) => code,
        ReadFault::Eof => ERROR_BROKEN_PIPE,
        ReadFault::Queue => ERROR_BUFFER_OVERFLOW,
        _ => ERROR_INVALID_DATA,
    }
}
struct PipeWrite<'a> {
    pipe: &'a OwnedHandle,
    closed: &'a AtomicBool,
}
impl Sink for PipeWrite<'_> {
    fn write(&mut self, bytes: &[u8]) -> ReadStep {
        if self.closed.load(Ordering::Acquire) {
            return ReadStep {
                reported: 0,
                error: Some(ERROR_OPERATION_ABORTED),
            };
        }
        let mut written = 0;
        // SAFETY: owned synchronous pipe, unique writer, immutable bounded bytes.
        let success = unsafe {
            WriteFile(
                self.pipe.as_raw_handle(),
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        ReadStep {
            reported: written as usize,
            error: (success == 0).then(|| unsafe { GetLastError() }),
        }
    }
}
impl Transport {
    pub(super) fn take_control(&mut self) -> ControlPort {
        let (_, empty) = mpsc::channel();
        ControlPort {
            incoming: std::mem::replace(&mut self.incoming[1], empty),
            outgoing: self.outgoing[1].clone(),
            budget: self.control_budget.clone(),
            error: self.error.clone(),
            closed: self.closed.clone(),
            _metadata: self._metadata.clone(),
            outstanding: self.outstanding[1].clone(),
            fault: self.faults[1].clone(),
            write_fault: self.faults[3].clone(),
        }
    }
    pub(super) fn new(
        read: [OwnedHandle; 2],
        write: [OwnedHandle; 2],
        budget: Budget,
    ) -> io::Result<Self> {
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let control_bytes =
            (p.control_queue_frames as usize * 2 + 2) * p.frame_bytes as usize + codec_bytes();
        let control_budget = budget
            .partition(control_bytes)
            .map_err(|_| io::Error::other("control reserve unavailable"))?;
        // Account application queue slots and worker bookkeeping before their
        // construction. Allocator/OS stack overhead is outside this ledger.
        let metadata = Arc::new(
            budget
                .reserve(
                    (p.ordinary_queue_frames + p.control_queue_frames) as usize
                        * std::mem::size_of::<Frame>()
                        * 8
                        + 4096,
                )
                .map_err(|_| io::Error::other("transport metadata capacity"))?,
        );
        let closed = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(AtomicU32::new(0));
        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();
        let mut threads = Vec::new();
        let faults: [Arc<FaultSlot<Frame>>; 4] =
            std::array::from_fn(|_| Arc::new(FaultSlot::new()));
        let readers_done: [Arc<AtomicBool>; 2] =
            std::array::from_fn(|_| Arc::new(AtomicBool::new(false)));
        for (i, pipe) in read.into_iter().enumerate() {
            let (tx, rx) = mpsc::sync_channel(if i == 0 {
                p.ordinary_queue_frames
            } else {
                p.control_queue_frames
            } as usize);
            incoming.push(rx);
            let b = if i == 0 {
                budget.clone()
            } else {
                control_budget.clone()
            };
            let mut framer =
                Framer::new(b).map_err(|_| io::Error::other("reader prefix reserve"))?;
            let c = closed.clone();
            let f = failure.clone();
            let storage = metadata.clone();
            let fault = faults[i].clone();
            let done = readers_done[i].clone();
            let spawned = thread::Builder::new()
                .name(format!("sampler-read-{i}"))
                .spawn(move || {
                    let _storage = storage;
                    let _done = ReaderDone(done);
                    let mut source = PipeRead { pipe, closed: c };
                    loop {
                        let (bytes, reservation, rejection) = framer.read(&mut source).into_parts();
                        let frame = Frame {
                            bytes,
                            _reservation: Some(reservation),
                            rejection,
                            completion: None,
                        };
                        if let Some(rejection) = rejection {
                            let _ = fault.store(frame);
                            error(&f, rejection_code(rejection));
                            return;
                        }
                        if let Err(failed) = tx.try_send(frame) {
                            let (mut frame, overflow) = match failed {
                                mpsc::TrySendError::Full(frame) => (frame, true),
                                mpsc::TrySendError::Disconnected(frame) => (frame, false),
                            };
                            frame.rejection = Some(Rejection {
                                fault: ReadFault::Queue,
                                phase: Phase::Queue,
                                reported: frame.bytes.len() as u64,
                                queue_overflow: overflow,
                            });
                            let _ = fault.store(frame);
                            error(
                                &f,
                                if overflow {
                                    ERROR_BUFFER_OVERFLOW
                                } else {
                                    ERROR_BROKEN_PIPE
                                },
                            );
                            return;
                        }
                    }
                });
            match spawned {
                Ok(t) => threads.push(t),
                Err(e) => {
                    cancel(&closed, &threads);
                    return Err(e);
                }
            }
        }
        for (i, pipe) in write.into_iter().enumerate() {
            let (tx, rx) = mpsc::sync_channel::<Frame>(if i == 0 {
                p.ordinary_queue_frames
            } else {
                p.control_queue_frames
            } as usize);
            outgoing.push(tx);
            let c = closed.clone();
            let f = failure.clone();
            let storage = metadata.clone();
            let fault = faults[i + 2].clone();
            let spawned = thread::Builder::new()
                .name(format!("sampler-write-{i}"))
                .spawn(move || {
                    let _storage = storage;
                    while !c.load(Ordering::Acquire) {
                        match rx.recv_timeout(Duration::from_millis(10)) {
                            Ok(mut frame) => {
                                if let Err(rejection) = write_packet(
                                    &mut PipeWrite {
                                        pipe: &pipe,
                                        closed: &c,
                                    },
                                    &frame.bytes,
                                ) {
                                    frame.rejection = Some(rejection);
                                    let _ = fault.store(frame);
                                    error(&f, rejection_code(rejection));
                                    return;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                });
            match spawned {
                Ok(t) => threads.push(t),
                Err(e) => {
                    cancel(&closed, &threads);
                    return Err(e);
                }
            }
        }
        Ok(Self {
            incoming: incoming
                .try_into()
                .map_err(|_| io::Error::other("lane count"))?,
            outgoing: outgoing
                .try_into()
                .map_err(|_| io::Error::other("lane count"))?,
            threads,
            closed,
            error: failure,
            budget,
            control_budget,
            _metadata: metadata,
            outstanding: [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))],
            faults,
            readers_done,
        })
    }
    pub(super) fn send(&self, lane: Lane, bytes: Vec<u8>) -> io::Result<()> {
        if self.error.load(Ordering::Acquire) != 0 || self.closed.load(Ordering::Acquire) {
            return Err(io::Error::other("transport failed/closed"));
        }
        let reservation = self
            .lane_budget(lane)
            .reserve(bytes.capacity())
            .map_err(|_| io::Error::other("transport RAM exhausted"))?;
        self.outstanding[index(lane)].fetch_add(1, Ordering::AcqRel);
        self.outgoing[index(lane)]
            .try_send(Frame {
                bytes,
                _reservation: Some(reservation),
                rejection: None,
                completion: Some(self.outstanding[index(lane)].clone()),
            })
            .map_err(|_| io::Error::other("outgoing queue closed/full"))
    }
    pub(super) fn codec_reservation(&self, lane: Lane) -> io::Result<Reservation> {
        self.lane_budget(lane)
            .reserve(codec_bytes())
            .map_err(|_| io::Error::other("codec capacity"))
    }
    fn lane_budget(&self, lane: Lane) -> &Budget {
        match lane {
            Lane::Ordinary => &self.budget,
            Lane::Control => &self.control_budget,
        }
    }
    pub(super) fn receive(&self) -> io::Result<Option<(Lane, Frame)>> {
        // Already received raw evidence precedes transport-loss notification.
        for i in [1, 3, 0, 2] {
            if let Ok(Some(frame)) = self.faults[i].take() {
                return Ok(Some((
                    if i % 2 == 0 {
                        Lane::Ordinary
                    } else {
                        Lane::Control
                    },
                    frame,
                )));
            }
        }
        for lane in [Lane::Control, Lane::Ordinary] {
            match self.incoming[index(lane)].try_recv() {
                Ok(frame) => return Ok(Some((lane, frame))),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {}
            }
        }
        let failure = self.error.load(Ordering::Acquire);
        if failure != 0 {
            return Err(io::Error::from_raw_os_error(failure as i32));
        }
        Ok(None)
    }
    pub(super) fn output_drained(&self) -> bool {
        self.outstanding
            .iter()
            .all(|n| n.load(Ordering::Acquire) == 0)
            && self.error.load(Ordering::Acquire) == 0
    }
    /// Check BEFORE a receive that returns empty. Once true, the readers can
    /// enqueue no further packets; that ordering prevents a false drained race.
    pub(super) fn inputs_finished(&self) -> bool {
        self.readers_done
            .iter()
            .all(|done| done.load(Ordering::Acquire))
            && self
                .outstanding
                .iter()
                .all(|n| n.load(Ordering::Acquire) == 0)
    }
    pub(super) fn completion(&self) -> [bool; 4] {
        std::array::from_fn(|i| self.threads[i].is_finished())
    }
    /// Each result retains cancellation status and observed thread completion;
    /// false remains unresolved. No wait/join, and no TerminateThread.
    pub(super) fn shutdown(&self) -> [super::runtime::IoObservation; 4] {
        let results = cancel(&self.closed, &self.threads);
        std::array::from_fn(|i| super::runtime::IoObservation {
            lane: if i % 2 == 0 {
                Lane::Ordinary
            } else {
                Lane::Control
            },
            reader: i < 2,
            finished: results[i].0,
            cancellation: results[i].1,
        })
    }
}
fn codec_bytes() -> usize {
    autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .frame_bytes as usize
        * 16
}
fn cancel(closed: &AtomicBool, threads: &[JoinHandle<()>]) -> Vec<(bool, Result<(), u32>)> {
    closed.store(true, Ordering::Release);
    threads
        .iter()
        .map(|thread| {
            // SAFETY: retained JoinHandle owns a valid thread handle. Cancellation
            // does not free the owned I/O buffers; those remain inside the worker.
            let result = if unsafe { CancelSynchronousIo(thread.as_raw_handle()) } != 0 {
                Ok(())
            } else {
                Err(unsafe { GetLastError() })
            };
            (thread.is_finished(), result)
        })
        .collect()
}
impl Drop for Transport {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
