//! Independent pre-manifest event/clock watcher. Never calls NVML/CUDA or joins
//! a live thread. Parent process/job observation and response/export are separate.
use super::{
    starting::Scope,
    starting_control::{Completion, Fault, Shared, State},
    transport::RawReceived,
    windows_transport::{ControlTransport, Receive},
};
use crate::{
    monitor::{
        windows_clock::{CaptureError, WindowsClock},
        Anchor,
    },
    sampler::budget::{Budget, Reservation},
};
use std::{
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows_sys::Win32::{Foundation::*, System::Threading::WaitForSingleObject};
pub struct Report {
    pub state: State,
    pub control: ControlTransport,
    pub rejected: Option<RawReceived>,
    /// Retains a completion racing thread exit, rather than discarding it.
    pub pending_completion: CompletionMailbox,
    pub event_status: Option<u32>,
    pub event_error: Option<u32>,
    pub transport_error: Option<i32>,
    pub clock_error: Option<CaptureError>,
    pub observed_frequency: Option<u64>,
    _metadata: Reservation,
}
pub struct CompletionMailbox {
    receive: mpsc::Receiver<Completion>,
    _allocation: Reservation,
}
impl CompletionMailbox {
    pub fn try_recv(&self) -> Result<Completion, TryRecvError> {
        self.receive.try_recv()
    }
}
pub struct Handle {
    shared: Arc<Shared>,
    ready: Arc<AtomicBool>,
    completion_sent: AtomicBool,
    send: SyncSender<Completion>,
    thread: Option<JoinHandle<Report>>,
}
impl Handle {
    /// Start before opening the native runtime. A ready watcher has checked its
    /// actual clock/frequency and owns the split control lane and stop event.
    pub fn start(
        port: ControlTransport,
        event: OwnedHandle,
        scope: &Scope,
        first: Anchor,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let state = State::new(scope, first, budget)?;
        let shared = state.shared();
        let metadata = budget
            .reserve(std::mem::size_of::<Report>() + 16 * 1024)
            .map_err(|_| Fault::Capacity)?;
        let mailbox_allocation = budget.reserve(16 * 1024).map_err(|_| Fault::Capacity)?;
        let ready = Arc::new(AtomicBool::new(false));
        let signal = shared.clone();
        let started = ready.clone();
        let frequency = scope.draft().frequency;
        let token = scope.draft().clock_token;
        let (send, receive) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("starting-control".into())
            .spawn(move || {
                let mut report = Report {
                    state,
                    control: port,
                    rejected: None,
                    pending_completion: CompletionMailbox {
                        receive,
                        _allocation: mailbox_allocation,
                    },
                    event_status: None,
                    event_error: None,
                    transport_error: None,
                    clock_error: None,
                    observed_frequency: None,
                    _metadata: metadata,
                };
                let mut run = || {
                    let clock = match WindowsClock::new(token) {
                        Ok(c) => {
                            report.observed_frequency = Some(c.qpc_frequency());
                            if c.qpc_frequency() != frequency {
                                signal.fail(Fault::Clock);
                                return;
                            }
                            c
                        }
                        Err(e) => {
                            report.clock_error = Some(e);
                            signal.fail(Fault::Clock);
                            return;
                        }
                    };
                    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
                    loop {
                        let status = unsafe { WaitForSingleObject(event.as_raw_handle(), 0) };
                        if !matches!(status, WAIT_OBJECT_0 | WAIT_TIMEOUT) {
                            report.event_status = Some(status);
                            if status == WAIT_FAILED {
                                report.event_error = Some(unsafe { GetLastError() });
                            }
                            signal.fail(Fault::Transport);
                            break;
                        }
                        let mut capture_error = None;
                        let received = report
                            .control
                            .receive(|| clock.capture().map_err(|e| capture_error = Some(e)).ok());
                        if let Some(e) = capture_error {
                            report.clock_error.get_or_insert(e);
                        }
                        let anchor = match received {
                            Receive::Packet(raw) => {
                                // This event-only scope defines no parent control
                                // packets. Pilot-role Stop/Start frames cannot leak
                                // through into a pre-manifest read declaration.
                                let a = raw.receipt();
                                report.rejected = Some(raw);
                                signal.fail(Fault::Protocol);
                                let _ = report.state.tick(a);
                                break;
                            }
                            Receive::Fault { code, anchor } => {
                                report.transport_error = code;
                                signal.fail(Fault::Transport);
                                let _ = report.state.tick(anchor);
                                break;
                            }
                            Receive::Idle { anchor } => anchor,
                        };
                        if status == WAIT_OBJECT_0 {
                            report.event_status.get_or_insert(status);
                            if signal.window().is_none() {
                                let _ = signal.request_stop(anchor);
                            }
                        }
                        // Retain this poll's actual clock even when completion
                        // is already queued; its separate receipt is captured
                        // after mailbox dequeue below.
                        let keep_running = report.state.tick(anchor);
                        let command = match report.pending_completion.try_recv() {
                            Ok(c) => Some(c),
                            Err(TryRecvError::Empty) => None,
                            Err(TryRecvError::Disconnected) => {
                                signal.fail(Fault::Transport);
                                break;
                            }
                        };
                        if let Some(command) = command {
                            let mut error = None;
                            let receipt = clock.capture().map_err(|e| error = Some(e)).ok();
                            if let Some(e) = error {
                                report.clock_error.get_or_insert(e);
                            }
                            let _ = report.state.complete(command, receipt);
                            break;
                        }
                        if !keep_running {
                            break;
                        }
                        // Readiness follows the first actual event/clock tick,
                        // so a delayed thread cannot authorize an expired open.
                        if !signal.admission().stopped() {
                            started.store(true, Ordering::Release);
                        }
                        thread::sleep(Duration::from_nanos(p.controller_tick_ns));
                    }
                };
                run();
                signal.close();
                report
            })
            .map_err(|_| {
                shared.fail(Fault::Capacity);
                Fault::Capacity
            })?;
        Ok(Self {
            shared,
            ready,
            completion_sent: AtomicBool::new(false),
            send,
            thread: Some(thread),
        })
    }
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
            && !self.thread_finished()
            && self.shared.fault().is_none()
            && !self.shared.admission().stopped()
    }
    pub fn thread_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
    pub fn request_stop(&self, anchor: Option<Anchor>) -> Result<super::gate::StopWindow, Fault> {
        self.shared.request_stop(anchor)
    }
    /// Retain the exact original completion on a failed/duplicate handoff.
    pub fn complete(&self, completion: Completion) -> Result<(), Completion> {
        self.shared.close();
        if self.completion_sent.swap(true, Ordering::AcqRel) {
            self.shared.fail(Fault::Protocol);
            return Err(completion);
        }
        if self.thread_finished() {
            self.shared.fail(Fault::Transport);
            return Err(completion);
        }
        self.send.try_send(completion).map_err(|e| {
            self.shared.fail(Fault::Transport);
            match e {
                TrySendError::Full(c) | TrySendError::Disconnected(c) => c,
            }
        })
    }
    pub fn take_report(&mut self) -> Result<Option<Report>, Fault> {
        if !self.thread_finished() {
            return Ok(None);
        }
        let Some(thread) = self.thread.take() else {
            return Ok(None);
        };
        thread.join().map(Some).map_err(|_| {
            self.shared.fail(Fault::Transport);
            Fault::Transport
        })
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.shared.close();
    }
}
