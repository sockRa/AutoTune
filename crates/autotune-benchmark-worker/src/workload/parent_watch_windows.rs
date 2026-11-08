//! Fixed parent controller and four independent OS request lanes. No main uses
//! this source before separately scoped process/native execution approval.
use super::{
    gate::StopWindow,
    launch_origin::Kind,
    parent_sampler::ParentSampler,
    parent_watch::{Effects, Fault, State, StopRequest, Update, Window},
    parent_watch_retirement::{self as retirement, Arbitration, Frame as RetirementFrame},
    starting_export::ClockFault,
    stop_requests::Observation,
    windows_process::StopOwner,
};
use crate::{
    monitor::{windows_clock::WindowsClock, Anchor},
    sampler::budget::{Budget, Reservation},
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc, OnceLock,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capture {
    pub anchor: Option<Anchor>,
    pub error: Option<ClockFault>,
}
fn capture(clock: &WindowsClock) -> Capture {
    match clock.capture() {
        Ok(a) => Capture {
            anchor: Some(a),
            error: None,
        },
        Err(e) => Capture {
            anchor: None,
            error: Some(e.into()),
        },
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Operation {
    pub ready: bool,
    pub requested: bool,
    pub before: Option<Capture>,
    pub returned: Option<Observation>,
    /// Some(false) means the method observed an earlier/pending shared request;
    /// these brackets cannot be presented as a fresh OS-call observation.
    pub invoked: Option<bool>,
    pub after: Option<Capture>,
    pub finished: bool,
    pub withdrawn: bool,
}
struct Slot {
    ready: AtomicBool,
    requested: AtomicBool,
    before: OnceLock<Capture>,
    returned: OnceLock<Observation>,
    invoked: OnceLock<bool>,
    after: OnceLock<Capture>,
    finished: AtomicBool,
    withdrawn: AtomicBool,
}
impl Slot {
    fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            requested: AtomicBool::new(false),
            before: OnceLock::new(),
            returned: OnceLock::new(),
            invoked: OnceLock::new(),
            after: OnceLock::new(),
            finished: AtomicBool::new(false),
            withdrawn: AtomicBool::new(false),
        }
    }
    fn observe(&self) -> Operation {
        Operation {
            ready: self.ready.load(Ordering::Acquire),
            requested: self.requested.load(Ordering::Acquire),
            before: self.before.get().copied(),
            returned: self.returned.get().copied(),
            invoked: self.invoked.get().copied(),
            after: self.after.get().copied(),
            finished: self.finished.load(Ordering::Acquire),
            withdrawn: self.withdrawn.load(Ordering::Acquire),
        }
    }
}
struct Control {
    arbitration: Arc<Arbitration>,
    retirement: OnceLock<RetirementFrame>,
    publisher_taken: AtomicBool,
    owners: [Arc<StopOwner>; 2],
    slots: [Slot; 4],
    window: Window,
    closed: AtomicBool,
    ready: AtomicBool,
    setup: AtomicBool,
    abort: OnceLock<Fault>,
    external: OnceLock<StopRequest>,
    unplanned: OnceLock<StopWindow>,
    send: SyncSender<Update>,
    pending_count: Arc<AtomicUsize>,
    _allocation: Arc<Reservation>,
    rejected: OnceLock<Update>,
    send_error: OnceLock<MailboxFault>,
}
impl Control {
    fn request_stop(&self, window: StopWindow, planned: bool) {
        if !planned {
            self.arbitration.stop();
            let _ = self.unplanned.set(window);
        }
        let _ = self.external.set(StopRequest { window, planned });
        self.close();
    }
    fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
            && !self.closed.load(Ordering::Acquire)
            && self.arbitration.active()
            && self.owners.iter().all(|o| !o.cancelled())
            && self.slots.iter().all(|s| s.ready.load(Ordering::Acquire))
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        for owner in &self.owners {
            owner.close_admission();
        }
    }
    fn apply(&self, e: Effects) {
        let (stop, terminate) = self.arbitration.dispatch(e.stop, e.terminate);
        if stop {
            self.close();
            for i in [0, 2] {
                self.slots[i].requested.store(true, Ordering::Release);
            }
        }
        if terminate {
            self.close();
            for i in [1, 3] {
                self.slots[i].requested.store(true, Ordering::Release);
            }
        }
    }
    fn abort(&self, f: Fault) {
        let _ = self.abort.set(f);
        self.apply(Effects {
            stop: true,
            terminate: true,
            finished: true,
        });
    }
    fn abandon(&self) {
        if !self.arbitration.retired() {
            self.abort(Fault::Requested);
        }
    }
}
pub struct Report {
    pub state: State,
    /// No queued command is silently discarded when the controller ends.
    pub pending: Pending<Update>,
    // Keep the original control/slot owners in the returned outcome; normal
    // controller return must not itself dispose of a last containment owner.
    _control: Arc<Control>,
}
/// The receiver keeps its prepaid mailbox allocation even after the producing
/// handle/controller has gone. This wrapper owns no OS control capability.
pub struct Pending<T> {
    receiver: Receiver<T>,
    pending_count: Arc<AtomicUsize>,
    _allocation: Arc<Reservation>,
}
impl<T> Pending<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.receiver.try_recv().inspect(|_| {
            self.pending_count.fetch_sub(1, Ordering::AcqRel);
        })
    }
    pub fn is_empty(&self) -> bool {
        self.pending_count.load(Ordering::Acquire) == 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxFault {
    Full,
    Disconnected,
}
/// Independent cancellation never acquires the collector's owner mutex.
#[derive(Clone)]
pub struct Cancellation {
    control: Arc<Control>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.control.abort(Fault::Requested);
    }
}
impl super::collector_task::Cancel for Cancellation {
    fn cancel(&self) {
        Cancellation::cancel(self);
    }
}
pub struct Handle {
    control: Arc<Control>,
    operations: [Option<JoinHandle<()>>; 4],
    controller: Option<JoinHandle<Report>>,

    joined_operations: [Option<Result<(), Fault>>; 4],
}
#[allow(clippy::large_enum_variant)] // No new allocation on startup budget failure.
pub enum Retained {
    Owners([StopOwner; 2]),
    Partial(Handle),
}
pub struct StartFailure {
    pub fault: Fault,
    pub error: Option<std::io::Error>,
    pub retained: Retained,
}
impl Handle {
    pub fn cancellation(&self) -> Cancellation {
        Cancellation {
            control: self.control.clone(),
        }
    }
    #[allow(clippy::result_large_err)] // Failure transfers all original owners.
    pub fn start(
        parent: &ParentSampler,
        owners: [StopOwner; 2],
        clock: &WindowsClock,
        budget: Budget,
    ) -> Result<Self, StartFailure> {
        let binding = parent.context_binding();
        let window = Window {
            binding,
            setup_deadline_ns: parent.consent_window().1,
        };
        let validate = (|| {
            let _codec = budget
                .reserve(64 * 1024 * 16)
                .map_err(|_| Fault::Capacity)?;
            if !parent.context_open()
                || (clock.qpc_frequency(), clock.clock_token())
                    != (binding.monitor().qpc_frequency, binding.epoch.clock_token)
                || owners[0].origin().pid() == owners[1].origin().pid()
            {
                return Err(Fault::Binding);
            }
            for (i, owner) in owners.iter().enumerate() {
                let o = owner.origin();
                if owner.cancelled()
                    || o.kind() != [Kind::Sampler, Kind::Worker][i]
                    || o.binding() != binding.transport_digest()
                    || o.image() != binding.images()[i]
                {
                    return Err(Fault::Binding);
                }
            }
            let first = *parent.history().raw().last().ok_or(Fault::Clock)?;
            let state = State::new(window, first, &budget)?;
            let metadata = budget
                .reserve(
                    std::mem::size_of::<Control>()
                        + std::mem::size_of::<Self>()
                        + 4 * std::mem::size_of::<Update>()
                        + 4096,
                )
                .map_err(|_| Fault::Capacity)?;
            Ok((state, metadata))
        })();
        let (state, metadata) = match validate {
            Ok(v) => v,
            Err(f) => {
                return Err(StartFailure {
                    fault: f,
                    error: None,
                    retained: Retained::Owners(owners),
                })
            }
        };
        let (send, receive) = mpsc::sync_channel(4);
        let pending_count = Arc::new(AtomicUsize::new(0));
        let control = Arc::new(Control {
            arbitration: Arc::new(Arbitration::new()),
            retirement: OnceLock::new(),
            publisher_taken: AtomicBool::new(false),
            owners: owners.map(Arc::new),
            slots: std::array::from_fn(|_| Slot::new()),
            window,
            closed: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            setup: AtomicBool::new(false),
            abort: OnceLock::new(),
            external: OnceLock::new(),
            unplanned: OnceLock::new(),
            send,
            pending_count,
            _allocation: Arc::new(metadata),
            rejected: OnceLock::new(),
            send_error: OnceLock::new(),
        });
        let mut handle = Self {
            control,
            operations: std::array::from_fn(|_| None),
            controller: None,

            joined_operations: [None; 4],
        };
        if !handle
            .control
            .owners
            .iter()
            .all(|o| o.bind_arbitration(handle.control.arbitration.clone()))
        {
            handle.control.abort(Fault::Binding);
            return Err(StartFailure {
                fault: Fault::Binding,
                error: None,
                retained: Retained::Partial(handle),
            });
        }
        for index in 0..4 {
            let owned = handle.control.clone();
            let clock = clock.clone();
            match thread::Builder::new()
                .name(format!("pilot-parent-request-{index}"))
                .spawn(move || operation(owned, index, clock))
            {
                Ok(t) => handle.operations[index] = Some(t),
                Err(e) => {
                    handle.control.abort(Fault::Thread);
                    return Err(StartFailure {
                        fault: Fault::Thread,
                        error: Some(e),
                        retained: Retained::Partial(handle),
                    });
                }
            }
        }
        let owned = handle.control.clone();
        let clock = clock.clone();
        match thread::Builder::new()
            .name("pilot-parent-controller".into())
            .spawn(move || controller(owned, state, receive, clock))
        {
            Ok(t) => handle.controller = Some(t),
            Err(e) => {
                handle.control.abort(Fault::Thread);
                return Err(StartFailure {
                    fault: Fault::Thread,
                    error: Some(e),
                    retained: Retained::Partial(handle),
                });
            }
        }
        Ok(handle)
    }
    pub fn ready(&self) -> bool {
        self.control.ready()
    }
    pub fn closed(&self) -> bool {
        self.control.closed.load(Ordering::Acquire)
    }
    pub fn setup_observed(&self) -> bool {
        self.control.setup.load(Ordering::Acquire)
    }
    pub fn operations(&self) -> [Operation; 4] {
        std::array::from_fn(|i| self.control.slots[i].observe())
    }
    pub fn rejected_update(&self) -> Option<Update> {
        self.control.rejected.get().copied()
    }
    pub fn mailbox_fault(&self) -> Option<MailboxFault> {
        self.control.send_error.get().copied()
    }
    pub fn requested_window(&self) -> Option<StopWindow> {
        self.control.external.get().map(|r| r.window)
    }
    pub fn unplanned_window(&self) -> Option<StopWindow> {
        self.control.unplanned.get().copied()
    }
    pub fn abort_fault(&self) -> Option<Fault> {
        self.control.abort.get().copied()
    }
    pub fn origins(&self) -> [super::launch_origin::Origin; 2] {
        self.control.owners.each_ref().map(|o| o.origin())
    }
    pub fn owner_requests(&self) -> [[Observation; 2]; 2] {
        self.control.owners.each_ref().map(|o| o.requests())
    }
    /// Join only independently observed finished lanes; keep pending thread
    /// handles and their owner/ledger references without a blocking join.
    pub fn poll_operations(&mut self) -> [Option<Result<(), Fault>>; 4] {
        for i in 0..4 {
            if self.operations[i]
                .as_ref()
                .is_some_and(JoinHandle::is_finished)
            {
                self.joined_operations[i] = Some(
                    self.operations[i]
                        .take()
                        .expect("observed lane")
                        .join()
                        .map_err(|_| Fault::Thread),
                );
            }
        }
        self.joined_operations
    }
    pub fn operation_threads_finished(&self) -> [Option<bool>; 4] {
        std::array::from_fn(|i| self.operations[i].as_ref().map(JoinHandle::is_finished))
    }
    pub fn request_stop(&self, window: StopWindow) {
        self.control.request_stop(window, false);
    }
    pub fn retired(&self) -> bool {
        self.control.arbitration.retired()
    }
    pub fn retirement(&self) -> Option<&RetirementFrame> {
        self.control.retirement.get()
    }
    /// One normal-retirement observation, off the controller deadline path.
    /// Caller retains both original processes, collector and returned reports.
    /// Invoke after complete child lifecycle/exit observation, never as a poll
    /// or a substitute for workload/correctness/native cleanup acceptance.
    pub fn retire(
        &mut self,
        parent: &ParentSampler,
        sampler: &super::windows_process::Process,
        worker: &super::windows_process::Process,
        clock: &WindowsClock,
    ) -> Result<(), retirement::Fault> {
        self.retire_into(parent, sampler, worker, clock, &mut None)
    }
    pub(super) fn retire_into(
        &mut self,
        parent: &ParentSampler,
        sampler: &super::windows_process::Process,
        worker: &super::windows_process::Process,
        clock: &WindowsClock,
        progress: &mut Option<RetirementFrame>,
    ) -> Result<(), retirement::Fault> {
        if self.control.retirement.get().is_some() {
            return Err(retirement::Fault::Duplicate);
        }
        let stop = parent.stop().unwrap_or(StopWindow {
            trigger_ns: 0,
            cooperative_ns: 0,
            exit_ns: 0,
        });
        let expected = self.origins();
        let context = parent.context_binding() == self.control.window.binding
            && parent.consent_window().1 == self.control.window.setup_deadline_ns
            && parent.fault().is_none()
            && parent.is_finishing()
            && parent.reported_finished()
            && parent.peer_check().is_some_and(|p| {
                p.result.is_ok() && [p.peers[1].expected, p.peers[2].expected] == expected
            })
            && parent
                .start_attempt()
                .is_some_and(|a| a.queued && a.result.is_ok())
            && self
                .control
                .external
                .get()
                .is_some_and(|r| r.planned && r.window == stop)
            && [sampler.origin(), worker.origin()] == expected
            && !self.control.arbitration.retired()
            && self.control.arbitration.active()
            && self.control.slots.iter().all(|s| {
                !s.requested.load(Ordering::Acquire) || s.finished.load(Ordering::Acquire)
            })
            && self.control.abort.get().is_none();
        retirement::collect_into(
            self.control.window,
            stop,
            expected,
            [sampler, worker],
            clock,
            context,
            progress,
        );
        let frame = progress.expect("completed retirement observation");
        let result = frame.result();
        let _ = self.control.retirement.set(frame);
        if result.is_err() {
            self.control.abort(Fault::Retirement);
        }
        result
    }
    pub(super) fn publisher(&self, window: Window) -> Result<Publisher, Fault> {
        if window != self.control.window {
            return Err(Fault::Binding);
        }
        if self.control.publisher_taken.swap(true, Ordering::AcqRel) {
            self.control.abort(Fault::Binding);
            return Err(Fault::Binding);
        }
        Ok(Publisher {
            control: self.control.clone(),
            state: super::parent_watch_publication::Publication::new(window, self.origins()),
        })
    }
    /// Only a confirmed finished controller is joined; pending operation lanes
    /// and original owners remain retained by this handle after its report.
    pub fn poll(&mut self) -> Option<Result<Report, Fault>> {
        if !self
            .controller
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            return None;
        }
        Some(
            self.controller
                .take()
                .expect("observed thread")
                .join()
                .map_err(|_| Fault::Thread),
        )
    }
}
pub(super) struct Publisher {
    control: Arc<Control>,
    state: super::parent_watch_publication::Publication,
}
struct Endpoint<'a>(&'a Control);
impl super::parent_watch_publication::Endpoint for Endpoint<'_> {
    fn ready(&self) -> bool {
        self.0.ready()
    }
    fn closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }
    fn send(&mut self, update: Update) -> Result<(), Fault> {
        self.0.pending_count.fetch_add(1, Ordering::AcqRel);
        if let Err(e) = self.0.send.try_send(update) {
            self.0.pending_count.fetch_sub(1, Ordering::AcqRel);
            let (fault, rejected) = match e {
                mpsc::TrySendError::Full(v) => (MailboxFault::Full, v),
                mpsc::TrySendError::Disconnected(v) => (MailboxFault::Disconnected, v),
            };
            let _ = self.0.rejected.set(rejected);
            let _ = self.0.send_error.set(fault);
            return Err(Fault::Transport);
        }
        Ok(())
    }
    fn stop(&mut self, window: StopWindow, planned: bool) {
        self.0.request_stop(window, planned);
    }
    fn abort(&mut self, fault: Fault) {
        self.0.abort(fault);
    }
}
impl Publisher {
    pub(super) fn status(&self) -> super::parent_watch_publication::Status {
        self.state.status()
    }
    pub(super) fn ready(
        &self,
        window: Window,
        peers: Option<[super::launch_origin::Origin; 2]>,
    ) -> bool {
        self.state.ready(window, peers, &Endpoint(&self.control))
    }
    pub(super) fn publish(
        &mut self,
        facts: super::parent_watch_publication::Facts,
    ) -> Result<(), Fault> {
        self.state.publish(facts, &mut Endpoint(&self.control))
    }
    pub(super) fn stop(&self, window: StopWindow) {
        self.control.request_stop(window, false);
    }
}
impl Drop for Publisher {
    fn drop(&mut self) {
        self.control.abandon();
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.control.abandon();
    }
}
struct Emergency {
    control: Arc<Control>,
    armed: bool,
}
impl Drop for Emergency {
    fn drop(&mut self) {
        if self.armed {
            self.control.abort(Fault::Thread);
        }
    }
}
fn operation(control: Arc<Control>, index: usize, clock: WindowsClock) {
    let mut emergency = Emergency {
        control: control.clone(),
        armed: true,
    };
    let slot = &control.slots[index];
    slot.ready.store(true, Ordering::Release);
    let tick = autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .controller_tick_ns;
    while !slot.requested.load(Ordering::Acquire) {
        if control.arbitration.retired() {
            slot.withdrawn.store(true, Ordering::Release);
            slot.finished.store(true, Ordering::Release);
            emergency.armed = false;
            return;
        }
        thread::sleep(Duration::from_nanos(tick));
    }
    let _ = slot.before.set(capture(&clock));
    let owner = &control.owners[index / 2];
    let (invoked, observed) = if index.is_multiple_of(2) {
        owner.request_stop_attempt()
    } else {
        owner.request_termination_attempt()
    };
    // Owner's ledger and this slot retain the native return before post-clock.
    let _ = slot.invoked.set(invoked);
    let _ = slot.returned.set(observed);
    let _ = slot.after.set(capture(&clock));
    slot.finished.store(true, Ordering::Release);
    emergency.armed = false;
}
fn controller(
    control: Arc<Control>,
    mut state: State,
    pending: Receiver<Update>,
    clock: WindowsClock,
) -> Report {
    let mut external_observed = false;
    let mut unplanned_observed = false;
    let mut emergency = Emergency {
        control: control.clone(),
        armed: true,
    };
    let tick = autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .controller_tick_ns;
    loop {
        let raw = capture(&clock);
        let mut effects = state.tick(raw.anchor, raw.error);
        control.apply(effects);
        // Legacy launch/process requests share the arbiter. Observe owner
        // cancellation before closed so a concurrent planned close cannot be
        // mistaken for an unexplained cancellation.
        if control.owners.iter().any(|o| o.cancelled()) && !control.closed.load(Ordering::Acquire) {
            effects = state.abort(Fault::Requested);
            control.apply(effects);
        }
        if let Some(fault) = control.abort.get() {
            effects = state.abort(*fault);
            control.apply(effects);
        }
        if effects.finished {
            break;
        }
        if !external_observed {
            if let Some(window) = control.external.get() {
                external_observed = true;
                // The publication can race the preceding tick. Observe its
                // receipt afterwards, just as for a mailbox command.
                let raw = capture(&clock);
                let observed = state.tick(raw.anchor, raw.error);
                control.apply(observed);
                effects.finished |= observed.finished;
                if effects.finished {
                    break;
                }
                let requested = state.request_stop_kind(*window);
                control.apply(requested);
                effects.finished |= requested.finished;
            }
        }
        if !unplanned_observed {
            if let Some(window) = control.unplanned.get() {
                unplanned_observed = true;
                let raw = capture(&clock);
                let observed = state.tick(raw.anchor, raw.error);
                control.apply(observed);
                effects.finished |= observed.finished;
                if effects.finished {
                    break;
                }
                let requested = state.request_stop(*window);
                control.apply(requested);
                effects.finished |= requested.finished;
            }
        }
        if let Some(frame) = control.retirement.get() {
            let raw = capture(&clock);
            let eligible = state
                .observe_retirement(frame, raw.anchor, raw.error)
                .is_ok()
                && control.abort.get().is_none()
                && control.slots.iter().all(|s| {
                    !s.requested.load(Ordering::Acquire) || s.finished.load(Ordering::Acquire)
                })
                && control.owners.iter().all(|o| {
                    frame.certifies(o.origin())
                        && !o.requests()[1].requested
                        && o.requests()
                            .iter()
                            .all(|r| !r.requested || r.returned == Some(Ok(())))
                });
            if eligible && control.arbitration.retire().is_ok() {
                control.close();
                for owner in &control.owners {
                    owner.retire_observed(frame);
                }
                state.commit_retirement();
                break;
            }
            let rejected = state.abort(Fault::Retirement);
            control.apply(rejected);
            break;
        }
        if state.stop_window().is_none() {
            for _ in 0..4 {
                match pending.try_recv() {
                    Ok(update) => {
                        control.pending_count.fetch_sub(1, Ordering::AcqRel);
                        // Receipt time follows this exact try_recv, never a
                        // tick captured before a concurrently arriving update.
                        let raw = capture(&clock);
                        effects = state.tick(raw.anchor, raw.error);
                        control.apply(effects);
                        let already_stopped = state.stop_window().is_some();
                        if let Err(f) = state.update(update) {
                            if !already_stopped {
                                effects = state.abort(f);
                                control.apply(effects);
                            }
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        effects = state.abort(Fault::Transport);
                        control.apply(effects);
                        break;
                    }
                }
            }
        }
        control
            .setup
            .store(state.setup_observed(), Ordering::Release);
        if effects.finished {
            break;
        }
        control.ready.store(true, Ordering::Release);
        thread::sleep(Duration::from_nanos(tick));
    }
    emergency.armed = false;
    Report {
        state,
        pending: Pending {
            receiver: pending,
            pending_count: control.pending_count.clone(),
            _allocation: control._allocation.clone(),
        },
        _control: control.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_mailbox_keeps_prepaid_charge_after_producing_owners_are_gone() {
        let b = Budget::new(4096).unwrap();
        let charge = Arc::new(b.reserve(512).unwrap());
        let (send, receiver) = mpsc::sync_channel(4);
        send.try_send(7u8).unwrap();
        let pending = Pending {
            receiver,
            pending_count: Arc::new(AtomicUsize::new(1)),
            _allocation: charge.clone(),
        };
        drop(charge);
        drop(send);
        assert_eq!(b.used(), 512);
        assert_eq!(pending.try_recv(), Ok(7));
        assert_eq!(pending.try_recv(), Err(TryRecvError::Disconnected));
        drop(pending);
        assert_eq!(b.used(), 0);
    }
}
