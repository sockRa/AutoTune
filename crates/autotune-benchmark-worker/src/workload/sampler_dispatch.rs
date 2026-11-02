//! Selected sampler's one-shot settings dispatch on its serial native lane.
//! No runtime load, process launch, compute, consent or admission issuer.
use super::{
    manifest::BoundRun,
    settings::{Block, Capture, Draft},
    settings_transport::{Child, Frame, Request},
    transport::{Queue, RawReceived, SendFailure, Sender},
    wire::WorkerBinding,
};
use crate::{
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        child::NativeAdmission,
        wire::Lane,
    },
};
use autotune_probe::nvml::NvmlSampler;

pub struct Dispatch {
    binding: WorkerBinding,
    deadline_ns: u64,
    clock: Clock,
    last: Anchor,
    child: Child,
    raw: Option<RawReceived>,
    request: Option<Request>,
    capture: Option<Capture>,
    unsent: Option<Frame>,
    fault: Option<Block>,
    send_failure: Option<SendFailure>,
    sent: bool,
    attempted: bool,
    budget: Budget,
    _metadata: Reservation,
}
impl Dispatch {
    /// Freeze the child's first setup entry. A later parent request may tighten
    /// this deadline, never renew it. Entry does not prove safety or consent.
    pub fn new(
        run: &BoundRun<'_>,
        frequency: u64,
        epoch: Anchor,
        setup_entry: Anchor,
        budget: Budget,
    ) -> Result<Self, Block> {
        let binding = WorkerBinding::new(run, frequency, epoch).map_err(|_| Block::Clock)?;
        let mut clock = Clock::new(binding.monitor, epoch).map_err(|_| Block::Clock)?;
        if setup_entry.qpc_before < epoch.qpc_after {
            return Err(Block::Clock);
        }
        let entry = clock.observe(setup_entry).map_err(|_| Block::Clock)?;
        if entry < binding.origin_ns || entry >= binding.setup_boundary() {
            return Err(Block::Phase);
        }
        let deadline_ns = entry
            .checked_add(binding.plan.phase_caps_ns[1])
            .ok_or(Block::Phase)?
            .min(binding.setup_boundary());
        Ok(Self {
            binding,
            deadline_ns,
            clock,
            last: setup_entry,
            child: Child::new(run, frequency, epoch, budget.clone())?,
            raw: None,
            request: None,
            capture: None,
            unsent: None,
            fault: None,
            send_failure: None,
            sent: false,
            attempted: false,
            _metadata: budget
                .reserve(std::mem::size_of::<Self>())
                .map_err(|_| Block::Capacity)?,
            budget,
        })
    }
    pub fn raw(&self) -> Option<&RawReceived> {
        self.raw.as_ref()
    }
    pub fn capture(&self) -> Option<&Capture> {
        self.capture.as_ref()
    }
    pub fn unsent(&self) -> Option<&Frame> {
        self.unsent.as_ref()
    }
    pub fn fault(&self) -> Option<Block> {
        self.fault
    }
    pub fn send_failure(&self) -> Option<SendFailure> {
        self.send_failure
    }
    pub fn response_queued(&self) -> bool {
        self.sent
    }
    pub fn attempted(&self) -> bool {
        self.attempted
    }
    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ns
    }
    fn fail(&mut self, f: Block) {
        self.fault.get_or_insert(f);
    }
    /// Runs on idle polls too; no parent/NVML/pipe call is awaited here.
    pub fn tick(&mut self, anchor: Option<Anchor>) -> Result<(), Block> {
        let result = (|| {
            let a = anchor.ok_or(Block::Clock)?;
            if a.qpc_before < self.last.qpc_after {
                return Err(Block::Clock);
            }
            let now = self.clock.observe(a).map_err(|_| Block::Clock)?;
            self.last = a;
            if now >= self.deadline_ns {
                return Err(Block::Phase);
            }
            Ok(())
        })();
        if let Err(f) = result {
            self.fail(f);
        }
        self.fault.map_or(Ok(()), Err)
    }
    // Returning the exact charged packet avoids allocating on a rejection path.
    #[allow(clippy::result_large_err)]
    fn begin(&mut self, run: &BoundRun<'_>, raw: RawReceived) -> Result<(), RawReceived> {
        if self.attempted {
            self.fail(Block::Scope);
            return Err(raw);
        }
        self.attempted = true;
        self.raw = Some(raw);
        let result = (|| {
            if WorkerBinding::new(run, self.binding.monitor.qpc_frequency, self.binding.epoch)
                .map_err(|_| Block::Clock)?
                != self.binding
            {
                return Err(Block::Scope);
            }
            let raw = self.raw.as_ref().expect("stored first packet");
            if raw.lane() != Lane::Ordinary || raw.packet().rejection().is_some() {
                return Err(Block::Scope);
            }
            let receipt = raw.receipt();
            let request = self.child.accept(raw.packet().bytes())?;
            self.deadline_ns = self.deadline_ns.min(request.deadline_ns());
            let issued = request.issued();
            let a = receipt.ok_or(Block::Clock)?;
            if a.qpc_before < issued.qpc_after {
                return Err(Block::Clock);
            }
            let mut clock =
                Clock::new(self.binding.monitor, self.binding.epoch).map_err(|_| Block::Clock)?;
            clock.observe(issued).map_err(|_| Block::Clock)?;
            clock.observe(a).map_err(|_| Block::Clock)?;
            self.request = Some(request);
            self.tick(receipt)
        })();
        if let Err(f) = result {
            self.fail(f);
        }
        // A first malformed request stays in this owner. Only a second packet
        // is returned, avoiding an unbounded rejected-packet collection.
        Ok(())
    }
    /// Explicit fixture dispatch cannot assert native provenance. The supplied
    /// draft remains caller-owned on failure; retained artifacts are simulated.
    #[allow(clippy::result_large_err)]
    pub fn fixture<Q: Queue>(
        &mut self,
        run: &BoundRun<'_>,
        raw: RawReceived,
        draft: &Draft,
        sender: &mut Sender<Q>,
    ) -> Result<(), RawReceived> {
        self.begin(run, raw)?;
        if self.fault.is_some() {
            return Ok(());
        }
        match Capture::fixture(
            run,
            self.binding.monitor.qpc_frequency,
            self.binding.epoch,
            draft,
            &self.budget,
        ) {
            Ok(c) => self.finish(c, sender),
            Err(f) => self.fail(f),
        }
        Ok(())
    }
    /// Actual selected getters, on the already supervised sampler's serial
    /// lane. The independently owned control thread must stop this shared gate.
    /// Native scope/peer/runtime/image checks are caller prerequisites.
    #[allow(clippy::result_large_err)]
    pub fn native<Q: Queue>(
        &mut self,
        run: &BoundRun<'_>,
        raw: RawReceived,
        sampler: &mut NvmlSampler,
        admission: &NativeAdmission,
        clock: impl FnMut() -> Option<Anchor>,
        sender: &mut Sender<Q>,
    ) -> Result<(), RawReceived> {
        if let Err(raw) = self.begin(run, raw) {
            admission.stop();
            return Err(raw);
        }
        if self.fault.is_some() {
            admission.stop();
            return Ok(());
        }
        if admission.begin().is_err() {
            self.fail(Block::Stopped);
            admission.stop();
            return Ok(());
        }
        let receipt = self
            .raw
            .as_ref()
            .and_then(RawReceived::receipt)
            .expect("validated receipt");
        let captured = self
            .request
            .as_mut()
            .expect("validated request")
            .collect_native_after(
                run,
                sampler,
                (receipt, self.deadline_ns),
                (clock, || admission.stopped()),
            );
        // A returned collector (including a typed failure) is no longer in the
        // native call. complete() cannot clear a concurrently observed stop.
        admission.complete();
        match captured {
            Ok(c) => self.finish(c, sender),
            Err(f) => self.fail(f),
        }
        if self.fault.is_some() {
            admission.stop();
        }
        Ok(())
    }
    fn finish<Q: Queue>(&mut self, capture: Capture, sender: &mut Sender<Q>) {
        self.capture = Some(capture); // retain BEFORE any encode/queue request
        let capture = self.capture.as_ref().expect("retained capture");
        let receipt = self
            .raw
            .as_ref()
            .and_then(RawReceived::receipt)
            .expect("validated receipt");
        // The fixture seam is adversarial too: a supposedly readable capture
        // cannot omit child receipt chronology or borrow a later setup window.
        let mut clock =
            Clock::new(self.binding.monitor, self.binding.epoch).expect("validated binding");
        let mut last = receipt;
        let chronology = (|| {
            clock.observe(receipt).map_err(|_| Block::Clock)?;
            for o in &capture.draft().observations {
                for a in [o.before, o.after].into_iter().flatten() {
                    if a.qpc_before < last.qpc_after {
                        return Err(Block::Clock);
                    }
                    let n = clock.observe(a).map_err(|_| Block::Clock)?;
                    last = a;
                    if n >= self.deadline_ns {
                        return Err(Block::Phase);
                    }
                }
            }
            Ok(())
        })();
        // Artifacts already encode collection halts. Retain/export those raw
        // failures, but never send a readable capture hiding a dispatch-only
        // violation, regardless of the source path.
        let current_fault = capture.current().err();
        if current_fault.is_none() {
            if let Err(f) = chronology {
                self.fail(f);
                return;
            }
        }
        if chronology.is_ok() {
            self.clock = clock;
            self.last = last;
        }
        let encoded = self
            .request
            .as_mut()
            .expect("validated request")
            .respond(capture);
        if let Some(f) = current_fault {
            self.fail(f);
        }
        match encoded {
            Ok(frame) => match sender.send_settings(frame) {
                Ok(()) => self.sent = true,
                Err(frame) => {
                    self.unsent = Some(frame);
                    let cause = sender.fault(Lane::Ordinary).unwrap_or(SendFailure::Closed);
                    self.send_failure = Some(cause);
                    self.fail(if cause == SendFailure::Capacity {
                        Block::Capacity
                    } else {
                        Block::Stopped
                    });
                }
            },
            Err(f) => self.fail(f),
        }
    }
}

#[cfg(windows)]
pub struct Poll {
    /// Post-poll clock even when no packet arrived; None is an observed failure.
    pub anchor: Option<Anchor>,
    pub transport_fault: bool,
    pub os_code: Option<i32>,
    pub idle: bool,
    /// Second/replayed packet stays caller-owned rather than replacing the
    /// first retained request. Caller must export it and close the session.
    pub rejected: Option<RawReceived>,
}
#[cfg(windows)]
impl Dispatch {
    /// Concrete inherited-pipe -> selected getters -> response-queue composition.
    /// Keep the split control owner on its independent thread with `admission`.
    /// This step may block in NVML; it must never run on the parent/control path.
    pub fn poll_native<Q: Queue>(
        &mut self,
        run: &BoundRun<'_>,
        transport: &super::windows_transport::InheritedTransport,
        sampler: &mut NvmlSampler,
        admission: &NativeAdmission,
        mut clock: impl FnMut() -> Option<Anchor>,
        sender: &mut Sender<Q>,
    ) -> Poll {
        use super::windows_transport::Receive;
        match transport.receive(&mut clock) {
            Receive::Packet(raw) => {
                let anchor = raw.receipt();
                let rejected = self
                    .native(run, raw, sampler, admission, clock, sender)
                    .err();
                if rejected.is_some() {
                    admission.stop();
                }
                Poll {
                    anchor,
                    transport_fault: false,
                    os_code: None,
                    idle: false,
                    rejected,
                }
            }
            Receive::Idle { anchor } => {
                if self.tick(anchor).is_err() {
                    admission.stop();
                }
                Poll {
                    anchor,
                    transport_fault: false,
                    os_code: None,
                    idle: true,
                    rejected: None,
                }
            }
            Receive::Fault { code, anchor } => {
                self.fail(Block::Stopped);
                admission.stop();
                Poll {
                    anchor,
                    transport_fault: true,
                    os_code: code,
                    idle: false,
                    rejected: None,
                }
            }
        }
    }
}
