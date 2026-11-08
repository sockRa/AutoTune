//! Actual independent control thread; no CUDA/NVML calls, forced thread stop or
//! blocking joins. A returned report is not process exit or native quiescence.
use super::{
    control::{Fault, Shared, State},
    launch::Role,
    transport::Sender,
    windows_transport::{ControlTransport, Receive},
    wire::{Channel, Direction, EncodedFrame, Message, WorkerBinding},
};
use crate::{
    monitor::windows_clock::WindowsClock,
    sampler::{
        budget::{Budget, Reservation},
        wire::Lane,
    },
};
use std::{
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows_sys::Win32::{Foundation::*, System::Threading::WaitForSingleObject};
pub struct Report {
    pub state: State,
    pub unsent: Option<EncodedFrame>,
    pub unencoded: Option<Message>,
    pub codec_error: Option<crate::sampler::wire::WireError>,
    pub send_error: Option<super::transport::SendFailure>,
    pub native_failure: Option<Message>,
    pub rejected: Option<super::transport::WorkerReceived>,
    pub rejected_message: Option<Message>,
    /// Retains the bounded mailbox itself, including any send racing thread exit.
    pub pending_commands: mpsc::Receiver<Command>,
    pub event_status: Option<u32>,
    pub event_error: Option<u32>,
    pub transport_error: Option<i32>,
    pub clock_error: Option<crate::monitor::windows_clock::CaptureError>,
    pub observed_frequency: Option<u64>,
    _metadata: Reservation,
}
#[allow(clippy::large_enum_variant)] // Four inline slots, prepaid in report metadata.
pub enum Command {
    Stop(Option<crate::monitor::Anchor>),
    Failure(Message),
    Finished {
        cleanup_attempted: bool,
        cleanup_status: Option<i32>,
        teardown: super::teardown::Evidence,
    },
}
pub struct Handle {
    shared: Arc<Shared>,
    ready: Arc<AtomicBool>,
    send: SyncSender<Command>,
    thread: Option<JoinHandle<Report>>,
    binding: WorkerBinding,
}
impl Handle {
    /// All allocation/codec/thread setup precedes ready(). A failed startup
    /// closes shared native admission; main must never enter a driver first.
    pub fn start(
        port: ControlTransport,
        event: OwnedHandle,
        binding: WorkerBinding,
        role: Role,
        first: crate::monitor::Anchor,
        budget: Budget,
    ) -> Result<Self, Fault> {
        let shared = Arc::new(Shared::default());
        let ready = Arc::new(AtomicBool::new(false));
        let state = State::new(binding, first, shared.clone(), &budget)?;
        let metadata = budget
            .reserve(std::mem::size_of::<Report>() + 16 * 1024)
            .map_err(|_| Fault::Capacity)?;
        let control_budget = port.budget();
        let rx = if role == Role::Sampler {
            Channel::sampler(
                binding,
                Lane::Control,
                Direction::ToWorker,
                control_budget.clone(),
            )
        } else {
            Channel::new(
                binding,
                Lane::Control,
                Direction::ToWorker,
                control_budget.clone(),
            )
        }
        .map_err(|_| Fault::Capacity)?;
        let tx = if role == Role::Sampler {
            Channel::sampler(
                binding,
                Lane::Control,
                Direction::FromWorker,
                control_budget.clone(),
            )
        } else {
            Channel::new(
                binding,
                Lane::Control,
                Direction::FromWorker,
                control_budget.clone(),
            )
        }
        .map_err(|_| Fault::Capacity)?;
        let signal = shared.clone();
        let started = ready.clone();
        let (command_send, receive) = mpsc::sync_channel(4);
        let spawned = thread::Builder::new()
            .name("pilot-control".into())
            .spawn(move || {
                let mut report = Report {
                    state,
                    unsent: None,
                    unencoded: None,
                    codec_error: None,
                    send_error: None,
                    native_failure: None,
                    rejected: None,
                    rejected_message: None,
                    pending_commands: receive,
                    event_status: None,
                    event_error: None,
                    transport_error: None,
                    clock_error: None,
                    observed_frequency: None,
                    _metadata: metadata,
                };
                let mut rx = rx;
                let mut tx = tx;
                let mut work = || {
                    let clock = match WindowsClock::new(binding.monitor().clock_token) {
                        Ok(c) => {
                            report.observed_frequency = Some(c.qpc_frequency());
                            if c.qpc_frequency() != binding.monitor().qpc_frequency {
                                report.state.fail(Fault::Clock);
                                return;
                            }
                            c
                        }
                        Err(e) => {
                            report.clock_error = Some(e);
                            report.state.fail(Fault::Clock);
                            return;
                        }
                    };
                    let mut sender =
                        match port.sender(binding, Direction::FromWorker, &control_budget) {
                            Ok(s) => s,
                            Err(_) => {
                                report.state.fail(Fault::Capacity);
                                return;
                            }
                        };
                    started.store(true, Ordering::Release);
                    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
                    loop {
                        let status = unsafe { WaitForSingleObject(event.as_raw_handle(), 0) };
                        if status == WAIT_FAILED {
                            report.event_status = Some(status);
                            report.event_error = Some(unsafe { GetLastError() });
                            report.state.fail(Fault::Transport);
                            break;
                        }
                        if !matches!(status, WAIT_OBJECT_0 | WAIT_TIMEOUT) {
                            report.event_status = Some(status);
                            report.state.fail(Fault::Transport);
                            break;
                        }
                        if status == WAIT_OBJECT_0 {
                            report.event_status.get_or_insert(status);
                            if report.state.window().is_none() {
                                let anchor = capture(&clock, &mut report);
                                if let Some(message) = report.state.event(anchor) {
                                    if !send(&mut report, &mut tx, &mut sender, &message) {
                                        break;
                                    }
                                }
                            }
                        }
                        if report.state.fault().is_some() {
                            break;
                        }
                        let mut clock_error = None;
                        let received = port
                            .receive(|| clock.capture().map_err(|e| clock_error = Some(e)).ok());
                        if let Some(e) = clock_error {
                            report.clock_error = Some(e);
                        }
                        let output = match received {
                            Receive::Packet(raw) => {
                                match report.state.packet(raw.decode_worker(&mut rx)) {
                                    Ok(m) => m,
                                    Err(raw) => {
                                        report.rejected = Some(raw);
                                        report.state.fail(Fault::Capacity);
                                        None
                                    }
                                }
                            }
                            Receive::Idle { anchor } => {
                                if status == WAIT_OBJECT_0 && report.state.window().is_none() {
                                    report.event_status = Some(status);
                                    report.state.event(anchor)
                                } else {
                                    if !report.state.tick(anchor) {
                                        break;
                                    }
                                    None
                                }
                            }
                            Receive::Fault { code, anchor } => {
                                report.transport_error = code;
                                report.state.fail(Fault::Transport);
                                let _ = report.state.tick(anchor);
                                break;
                            }
                        };
                        if let Some(message) = output {
                            if !send(&mut report, &mut tx, &mut sender, &message) {
                                break;
                            }
                        }
                        if report.state.fault().is_some() {
                            break;
                        }
                        match report.pending_commands.try_recv() {
                            Ok(Command::Stop(trigger)) => {
                                let anchor = capture(&clock, &mut report);
                                if let Some(message) = report.state.local_stop(trigger, anchor) {
                                    if !send(&mut report, &mut tx, &mut sender, &message) {
                                        break;
                                    }
                                }
                            }
                            Ok(Command::Failure(message)) => {
                                if report.native_failure.is_some() {
                                    report.rejected_message = Some(message);
                                    report.state.fail(Fault::Protocol);
                                    break;
                                }
                                report.native_failure = Some(message);
                                signal.close();
                                let message = report
                                    .native_failure
                                    .as_ref()
                                    .expect("failure retained")
                                    .clone();
                                if let Message::NativeFailure { after, .. } = &message {
                                    let anchor = capture(&clock, &mut report);
                                    if let Some(ack) = report.state.local_stop(*after, anchor) {
                                        if !send(&mut report, &mut tx, &mut sender, &ack) {
                                            break;
                                        }
                                    }
                                }
                                if !send(&mut report, &mut tx, &mut sender, &message) {
                                    break;
                                }
                            }
                            Ok(Command::Finished {
                                cleanup_attempted,
                                cleanup_status,
                                teardown,
                            }) => {
                                let anchor = capture(&clock, &mut report);
                                if let Some(message) = report.state.complete_with_teardown(
                                    anchor,
                                    cleanup_attempted,
                                    cleanup_status,
                                    teardown,
                                ) {
                                    let _ = send(&mut report, &mut tx, &mut sender, &message);
                                }
                                break;
                            }
                            Err(TryRecvError::Disconnected) => {
                                report.state.fail(Fault::Transport);
                                break;
                            }
                            Err(TryRecvError::Empty) => (),
                        }
                        thread::sleep(Duration::from_nanos(p.controller_tick_ns));
                    }
                };
                work();
                signal.close();
                report
            });
        let thread = match spawned {
            Ok(t) => t,
            Err(_) => {
                shared.close();
                return Err(Fault::Transport);
            }
        };
        Ok(Self {
            shared,
            ready,
            send: command_send,
            thread: Some(thread),
            binding,
        })
    }
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire) && !self.shared.admission().stopped()
    }
    pub fn thread_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
    pub fn request_stop(&self, trigger: Option<crate::monitor::Anchor>) -> Result<(), Fault> {
        self.shared.close_at(self.binding, trigger);
        if self.thread_finished() {
            return Err(Fault::Transport);
        }
        self.send
            .try_send(Command::Stop(trigger))
            .map_err(|_| Fault::Transport)
    }
    #[allow(clippy::result_large_err)] // Preserve the original bounded failure owner.
    pub fn failure(&self, message: Message) -> Result<(), Message> {
        if !matches!(&message,Message::NativeFailure{detail,..} if detail.capacity()<=1024) {
            return Err(message);
        }
        if let Message::NativeFailure { after, .. } = &message {
            self.shared.close_at(self.binding, *after);
        }
        if self.thread_finished() {
            return Err(message);
        }
        self.send
            .try_send(Command::Failure(message))
            .map_err(|e| match e {
                mpsc::TrySendError::Full(Command::Failure(m))
                | mpsc::TrySendError::Disconnected(Command::Failure(m)) => m,
                _ => unreachable!(),
            })
    }
    pub fn complete(
        &self,
        cleanup_attempted: bool,
        cleanup_status: Option<i32>,
    ) -> Result<(), Fault> {
        self.complete_with_teardown(
            cleanup_attempted,
            cleanup_status,
            super::teardown::Evidence::default(),
        )
    }
    pub fn complete_with_teardown(
        &self,
        cleanup_attempted: bool,
        cleanup_status: Option<i32>,
        teardown: super::teardown::Evidence,
    ) -> Result<(), Fault> {
        self.shared.close();
        if self.thread_finished() {
            return Err(Fault::Transport);
        }
        self.send
            .try_send(Command::Finished {
                cleanup_attempted,
                cleanup_status,
                teardown,
            })
            .map_err(|_| Fault::Transport)
    }
    pub fn take_report(&mut self) -> Result<Option<Report>, Fault> {
        if !self.thread_finished() {
            return Ok(None);
        }
        let Some(thread) = self.thread.take() else {
            return Ok(None);
        };
        thread.join().map(Some).map_err(|_| Fault::Transport)
    }
}
fn capture(clock: &WindowsClock, report: &mut Report) -> Option<crate::monitor::Anchor> {
    match clock.capture() {
        Ok(a) => Some(a),
        Err(e) => {
            report.clock_error.get_or_insert(e);
            None
        }
    }
}
fn send<Q: super::transport::Queue>(
    report: &mut Report,
    channel: &mut Channel,
    sender: &mut Sender<Q>,
    message: &Message,
) -> bool {
    match channel.encode(message) {
        Ok(frame) => match sender.send_worker(frame) {
            Ok(()) => true,
            Err(frame) => {
                report.send_error = sender.fault(Lane::Control);
                report.unsent = Some(frame);
                report.state.fail(Fault::Send);
                false
            }
        },
        Err(error) => {
            report.codec_error = Some(error);
            report.unencoded = Some(message.clone());
            report.state.fail(Fault::Send);
            false
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.shared.close();
    }
}
