//! One retained parent-to-worker sender for the fixed Windows pilot process.
//!
//! Start and NativeGrant share this owner and therefore the same ordinary
//! channel sequence. The adapter binds the process-state reads to the worker
//! whose inherited pipe owns that sender. It does not grant permission by
//! itself; ParentSampler still applies its original-context, safety, deadline
//! and one-shot checks.
use super::{
    assessment::{self, Audit, Progress},
    dispatch::FailureVector,
    gate::StopWindow,
    launch::CancelQueue,
    parent_sampler::{Fault, ParentSampler},
    parent_start,
    process_state::Processes,
    starting_bootstrap, windows_process,
    windows_transport::{ControlTransport, Receive},
    wire::{Channel, Direction},
    worker_setup_delivery::{self, ReadyDelivery, Receiver},
};
use crate::{monitor::windows_clock::WindowsClock, sampler::budget::Budget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlFault {
    Binding,
    Clock,
    Deadline,
    Protocol,
    Capacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveKind {
    Packet,
    Idle,
    Fault,
}

/// Bounded chronological record for every receive attempt on both worker
/// lanes. Packet bodies stay in their packet owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiveEvent {
    pub ordinal: u32,
    pub lane: crate::sampler::wire::Lane,
    pub kind: ReceiveKind,
    pub anchor: Option<crate::monitor::Anchor>,
    pub code: Option<i32>,
    pub packet_digest: Option<crate::evidence::Digest>,
}

/// Parent-side terminal control assessment. It validates reported chronology
/// against the same original planned stop window; it does not prove process
/// exit, GPU quiescence, or successful native cleanup beyond the report.
pub struct WorkerControlAudit {
    binding: crate::workload::wire::WorkerBinding,
    origin: crate::workload::launch_origin::Origin,
    source_clock: crate::monitor::clock::Clock,
    receipt_clock: crate::monitor::clock::Clock,
    last_source: crate::monitor::Anchor,
    stop: StopWindow,
    stop_ack: bool,
    role_finished: bool,
    native_failure: bool,
    fault: Option<ControlFault>,
    _allocation: crate::sampler::budget::Reservation,
}
impl WorkerControlAudit {
    fn new(
        binding: crate::workload::wire::WorkerBinding,
        origin: crate::workload::launch_origin::Origin,
        stop: StopWindow,
        budget: &Budget,
    ) -> Result<Self, ControlFault> {
        let allocation = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| ControlFault::Capacity)?;
        let source_clock = crate::monitor::clock::Clock::new(binding.monitor(), binding.epoch)
            .map_err(|_| ControlFault::Clock)?;
        let receipt_clock = crate::monitor::clock::Clock::new(binding.monitor(), binding.epoch)
            .map_err(|_| ControlFault::Clock)?;
        Ok(Self {
            binding,
            origin,
            source_clock,
            receipt_clock,
            last_source: binding.epoch,
            stop,
            stop_ack: false,
            role_finished: false,
            native_failure: false,
            fault: None,
            _allocation: allocation,
        })
    }

    fn observe_teardown(
        &mut self,
        evidence: &crate::workload::teardown::Evidence,
        receipt: crate::monitor::Anchor,
    ) -> bool {
        if !evidence.reports_success() {
            return false;
        }
        for entry in evidence.entries[..usize::from(evidence.len)]
            .iter()
            .flatten()
        {
            for anchor in [entry.before, entry.after].into_iter().flatten() {
                if anchor.qpc_before < self.last_source.qpc_after
                    || anchor.qpc_after > receipt.qpc_before
                    || self.source_clock.observe(anchor).is_err()
                {
                    self.fault = Some(ControlFault::Clock);
                    return false;
                }
                self.last_source = anchor;
            }
        }
        true
    }

    pub fn observe(&mut self, received: &crate::workload::transport::WorkerReceived) {
        if self.fault.is_some() {
            return;
        }
        if received.raw().lane() != crate::sampler::wire::Lane::Control
            || received.raw().origin() != Some(self.origin)
            || received.binding().ok() != Some(self.binding)
        {
            self.fault = Some(ControlFault::Binding);
            return;
        }
        let Some(receipt) = received.raw().receipt() else {
            self.fault = Some(ControlFault::Clock);
            return;
        };
        let Ok(receipt_ns) = self.receipt_clock.observe(receipt) else {
            self.fault = Some(ControlFault::Clock);
            return;
        };
        let message = match received.message() {
            Ok(message) => message,
            Err(_) => {
                self.fault = Some(ControlFault::Protocol);
                return;
            }
        };
        let (anchor, kind, traced_ns) = match message {
            crate::workload::wire::Message::NativeFailure { before, after, .. } => {
                if self.native_failure || self.role_finished || before.is_some() != after.is_some()
                {
                    self.fault = Some(ControlFault::Protocol);
                    return;
                }
                self.native_failure = true;
                if let (Some(before), Some(after)) = (before, after) {
                    if before.qpc_before < self.last_source.qpc_after
                        || after.qpc_before < before.qpc_after
                    {
                        self.fault = Some(ControlFault::Clock);
                        return;
                    }
                    let Ok(source_ns) = self.source_clock.observe(*before) else {
                        self.fault = Some(ControlFault::Clock);
                        return;
                    };
                    let Ok(after_ns) = self.source_clock.observe(*after) else {
                        self.fault = Some(ControlFault::Clock);
                        return;
                    };
                    self.last_source = *after;
                    (*after, 0, Some(after_ns.max(source_ns)))
                } else {
                    (receipt, 0, None)
                }
            }
            crate::workload::wire::Message::StopAck { anchor, .. } => {
                if self.stop_ack || self.role_finished {
                    self.fault = Some(ControlFault::Protocol);
                    return;
                }
                (*anchor, 1, None)
            }
            crate::workload::wire::Message::RoleFinished {
                anchor,
                pending,
                cleanup_attempted,
                cleanup_status,
                teardown,
            } => {
                if !self.stop_ack
                    || self.role_finished
                    || *pending
                    || !*cleanup_attempted
                    || *cleanup_status != Some(0)
                {
                    self.fault = Some(ControlFault::Protocol);
                    return;
                }
                if !self.observe_teardown(teardown, receipt) {
                    self.fault.get_or_insert(ControlFault::Protocol);
                    return;
                }
                (*anchor, 2, None)
            }
            _ => {
                self.fault = Some(ControlFault::Protocol);
                return;
            }
        };
        let has_causal_anchor = traced_ns.is_some() || kind != 0;
        let anchor_ns = if let Some(ns) = traced_ns {
            ns
        } else if kind == 0 {
            receipt_ns
        } else {
            if anchor.qpc_before < self.last_source.qpc_after {
                self.fault = Some(ControlFault::Clock);
                return;
            }
            match self.source_clock.observe(anchor) {
                Ok(ns) => {
                    self.last_source = anchor;
                    ns
                }
                Err(_) => {
                    self.fault = Some(ControlFault::Clock);
                    return;
                }
            }
        };
        if has_causal_anchor && anchor.qpc_after > receipt.qpc_before {
            self.fault = Some(ControlFault::Clock);
        } else if receipt_ns >= self.stop.cooperative_ns
            || (kind != 0 && anchor_ns < self.stop.trigger_ns)
        {
            self.fault = Some(ControlFault::Deadline);
        } else if kind == 1 {
            self.stop_ack = true;
        } else if kind == 2 {
            self.role_finished = true;
        }
    }

    pub fn stop_acknowledged(&self) -> bool {
        self.stop_ack
    }
    pub fn role_finished(&self) -> bool {
        self.role_finished
    }
    pub fn native_failure(&self) -> bool {
        self.native_failure
    }
    pub fn fault(&self) -> Option<ControlFault> {
        self.fault
    }
}

/// Owns the unique ordinary sender derived from one retained worker process.
/// Keep this value alive from Start through the optional one-shot NativeGrant.
pub struct ParentWorkerChannel<'a> {
    worker: &'a windows_process::Process,
    control: ControlTransport,
    control_decoder: Channel,
    control_audit: Option<WorkerControlAudit>,
    control_packets: Vec<crate::workload::transport::WorkerReceived>,
    control_packet_limit: usize,
    control_storage: crate::sampler::budget::Reservation,
    events: Vec<ReceiveEvent>,
    event_limit: usize,
    event_storage: crate::sampler::budget::Reservation,
    audit: Option<Audit>,
    assessment_fault: Option<assessment::Fault>,
    failure_vector: Option<FailureVector>,
    result_packets: Vec<crate::workload::transport::WorkerReceived>,
    result_packet_limit: usize,
    result_storage: Option<crate::sampler::budget::Reservation>,
    overflowed: bool,
    sender: crate::workload::transport::Sender<CancelQueue<windows_process::ProcessQueue<'a>>>,
}

/// One independently polled control-lane observation from the worker.
// Keep the received raw packet and its charged allocation inline across the
// caller handoff; boxing would add an unbudgeted allocation to every poll.
#[allow(clippy::large_enum_variant)]
pub enum ControlPoll {
    Packet,
    Overflow(crate::workload::transport::WorkerReceived),
    Idle {
        anchor: Option<crate::monitor::Anchor>,
    },
    Fault {
        code: Option<i32>,
        anchor: Option<crate::monitor::Anchor>,
    },
}

/// Raw ordinary receive outcome. Clock anchors and OS errors are returned to
/// the owner so it can retain them alongside packet evidence.
#[allow(clippy::large_enum_variant)]
pub enum ResultPoll {
    Packet,
    Overflow(crate::workload::transport::WorkerReceived),
    Idle {
        anchor: Option<crate::monitor::Anchor>,
    },
    Fault {
        code: Option<i32>,
        anchor: Option<crate::monitor::Anchor>,
    },
    Unavailable(assessment::Fault),
}

/// Transferable raw worker evidence retained under the same parent budget.
/// This moves into `PilotOwners::worker_report_evidence` before finalization.
pub struct WorkerReportEvidence {
    binding: crate::workload::wire::WorkerBinding,
    origin: crate::workload::launch_origin::Origin,
    result_packets: Vec<crate::workload::transport::WorkerReceived>,
    control_packets: Vec<crate::workload::transport::WorkerReceived>,
    events: Vec<ReceiveEvent>,
    audit: Option<Audit>,
    assessment_fault: Option<assessment::Fault>,
    failure_vector: Option<FailureVector>,
    control_audit: Option<WorkerControlAudit>,
    overflowed: bool,
    _result_storage: Option<crate::sampler::budget::Reservation>,
    _control_storage: crate::sampler::budget::Reservation,
    _event_storage: crate::sampler::budget::Reservation,
}
impl WorkerReportEvidence {
    pub fn binding(&self) -> crate::workload::wire::WorkerBinding {
        self.binding
    }
    pub fn origin(&self) -> crate::workload::launch_origin::Origin {
        self.origin
    }
    pub fn result_packets(&self) -> &[crate::workload::transport::WorkerReceived] {
        &self.result_packets
    }
    pub fn control_packets(&self) -> &[crate::workload::transport::WorkerReceived] {
        &self.control_packets
    }
    pub fn receive_events(&self) -> &[ReceiveEvent] {
        &self.events
    }
    pub fn progress(&self) -> Option<Progress> {
        self.audit.as_ref().map(Audit::progress)
    }
    pub fn reports_complete(&self) -> bool {
        self.audit.as_ref().is_some_and(Audit::reports_complete)
    }
    pub fn assessment_fault(&self) -> Option<assessment::Fault> {
        self.assessment_fault
            .or_else(|| self.audit.as_ref().and_then(Audit::fault))
    }
    pub fn failure_vector(&self) -> Option<&FailureVector> {
        self.failure_vector.as_ref()
    }
    pub fn control_audit(&self) -> Option<&WorkerControlAudit> {
        self.control_audit.as_ref()
    }
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }
}

fn reserve_packets(
    budget: &Budget,
    count: usize,
) -> Result<
    (
        Vec<crate::workload::transport::WorkerReceived>,
        crate::sampler::budget::Reservation,
    ),
    crate::workload::transport::SendFailure,
> {
    let bytes = count
        .checked_mul(std::mem::size_of::<
            crate::workload::transport::WorkerReceived,
        >())
        .ok_or(crate::workload::transport::SendFailure::Capacity)?;
    let reservation = budget
        .reserve(bytes)
        .map_err(|_| crate::workload::transport::SendFailure::Capacity)?;
    let mut packets = Vec::new();
    packets
        .try_reserve_exact(count)
        .map_err(|_| crate::workload::transport::SendFailure::Capacity)?;
    Ok((packets, reservation))
}

fn reserve_events(
    budget: &Budget,
    count: usize,
) -> Result<
    (Vec<ReceiveEvent>, crate::sampler::budget::Reservation),
    crate::workload::transport::SendFailure,
> {
    let bytes = count
        .checked_mul(std::mem::size_of::<ReceiveEvent>())
        .ok_or(crate::workload::transport::SendFailure::Capacity)?;
    let reservation = budget
        .reserve(bytes)
        .map_err(|_| crate::workload::transport::SendFailure::Capacity)?;
    let mut events = Vec::new();
    events
        .try_reserve_exact(count)
        .map_err(|_| crate::workload::transport::SendFailure::Capacity)?;
    Ok((events, reservation))
}

impl<'a> ParentWorkerChannel<'a> {
    /// Takes the worker's single sender. A second channel cannot be created
    /// from this process owner, preventing sequence restarts between frames.
    pub fn new(
        worker: &'a mut windows_process::Process,
        budget: &Budget,
        event_capacity: usize,
    ) -> Result<Self, crate::workload::transport::SendFailure> {
        let control_budget = worker.reserved_control_budget();
        let control_decoder = Channel::new(
            worker.binding(),
            crate::sampler::wire::Lane::Control,
            Direction::FromWorker,
            control_budget,
        )
        .map_err(|_| crate::workload::transport::SendFailure::Capacity)?;
        let max_events = usize::try_from(
            autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .event_capacity,
        )
        .map_err(|_| crate::workload::transport::SendFailure::Capacity)?;
        if event_capacity < 2 || event_capacity > max_events {
            return Err(crate::workload::transport::SendFailure::Capacity);
        }
        // One event slot is held for the terminal assessment record in the
        // aggregate raw artifact; poll events consume the caller's remainder.
        let event_limit = event_capacity - 1;
        let (events, event_storage) = reserve_events(budget, event_limit)?;
        let (control_packets, control_storage) = reserve_packets(budget, 3)?;
        let control = worker.split_control()?;
        let sender = worker.sender(budget)?;
        Ok(Self {
            worker,
            control,
            control_decoder,
            control_audit: None,
            control_packets,
            control_packet_limit: 3,
            control_storage,
            events,
            event_limit,
            event_storage,
            audit: None,
            assessment_fault: None,
            failure_vector: None,
            result_packets: Vec::new(),
            result_packet_limit: 0,
            result_storage: None,
            overflowed: false,
            sender,
        })
    }

    fn record_event(
        &mut self,
        lane: crate::sampler::wire::Lane,
        kind: ReceiveKind,
        anchor: Option<crate::monitor::Anchor>,
        code: Option<i32>,
        packet_digest: Option<crate::evidence::Digest>,
    ) {
        if self.events.len() >= self.event_limit {
            self.overflowed = true;
            self.assessment_fault
                .get_or_insert(assessment::Fault::Capacity);
            if let Some(audit) = self.audit.as_mut() {
                audit.close(assessment::Fault::Capacity);
            }
            if let Some(audit) = self.control_audit.as_mut() {
                audit.fault.get_or_insert(ControlFault::Capacity);
            }
            return;
        }
        let ordinal = self.events.len() as u32;
        self.events.push(ReceiveEvent {
            ordinal,
            lane,
            kind,
            anchor,
            code,
            packet_digest,
        });
    }

    /// Poll one packet from the same retained worker process and feed the
    /// original strict setup receiver. Idle polls still tick its frozen
    /// deadline. On failure `receiver` remains owned by the caller with its
    /// first fault and rejected/raw packet intact.
    pub fn poll_setup(
        &mut self,
        receiver: &mut Option<Receiver>,
        clock: &WindowsClock,
    ) -> Result<Option<ReadyDelivery>, worker_setup_delivery::Fault> {
        let Some(current) = receiver.as_mut() else {
            return Err(worker_setup_delivery::Fault::Protocol);
        };
        match self.worker.receive(|| clock.capture().ok()) {
            Receive::Packet(raw) => {
                let anchor = raw.receipt();
                self.record_event(
                    crate::sampler::wire::Lane::Ordinary,
                    ReceiveKind::Packet,
                    anchor,
                    None,
                    Some(crate::evidence::Digest::of(
                        crate::evidence::Domain::RawArtifact,
                        raw.packet().bytes(),
                    )),
                );
                current.accept(raw)?
            }
            Receive::Idle { anchor } => {
                self.record_event(
                    crate::sampler::wire::Lane::Ordinary,
                    ReceiveKind::Idle,
                    anchor,
                    None,
                    None,
                );
                current.tick(anchor)?
            }
            Receive::Fault { code, anchor } => {
                self.record_event(
                    crate::sampler::wire::Lane::Ordinary,
                    ReceiveKind::Fault,
                    anchor,
                    code,
                    None,
                );
                current.transport_failed(code, anchor);
                return Err(current
                    .fault()
                    .unwrap_or(worker_setup_delivery::Fault::Transport));
            }
        }
        if current.ready().is_none() || current.receipt().is_none() {
            return Ok(None);
        }
        let completed = receiver.take().expect("setup receiver is retained");
        match completed.into_ready() {
            Ok(ready) => Ok(Some(ready)),
            Err(rejected) => {
                let fault = rejected
                    .fault()
                    .unwrap_or(worker_setup_delivery::Fault::Protocol);
                *receiver = Some(rejected);
                Err(fault)
            }
        }
    }

    /// Poll only worker control messages. Ordinary setup/result packets remain
    /// on `poll_setup`'s reader and cannot be consumed by this path.
    pub fn poll_control(&mut self, clock: &WindowsClock) -> ControlPoll {
        match self.control.receive(|| clock.capture().ok()) {
            Receive::Packet(raw) => {
                self.record_event(
                    crate::sampler::wire::Lane::Control,
                    ReceiveKind::Packet,
                    raw.receipt(),
                    None,
                    Some(crate::evidence::Digest::of(
                        crate::evidence::Domain::RawArtifact,
                        raw.packet().bytes(),
                    )),
                );
                let received = raw
                    .stamp_process(self.worker.origin())
                    .decode_worker(&mut self.control_decoder);
                if let Some(audit) = self.control_audit.as_mut() {
                    audit.observe(&received);
                }
                if self.control_packets.len() == self.control_packet_limit {
                    self.overflowed = true;
                    self.assessment_fault
                        .get_or_insert(assessment::Fault::Capacity);
                    if let Some(audit) = self.control_audit.as_mut() {
                        audit.fault.get_or_insert(ControlFault::Capacity);
                    }
                    ControlPoll::Overflow(received)
                } else {
                    self.control_packets.push(received);
                    ControlPoll::Packet
                }
            }
            Receive::Idle { anchor } => {
                self.record_event(
                    crate::sampler::wire::Lane::Control,
                    ReceiveKind::Idle,
                    anchor,
                    None,
                    None,
                );
                ControlPoll::Idle { anchor }
            }
            Receive::Fault { code, anchor } => {
                self.record_event(
                    crate::sampler::wire::Lane::Control,
                    ReceiveKind::Fault,
                    anchor,
                    code,
                    None,
                );
                ControlPoll::Fault { code, anchor }
            }
        }
    }

    /// Bind terminal worker control to the parent's already-issued planned
    /// stop. No audit can be restarted or rebased after its first packet.
    pub fn begin_control_audit(
        &mut self,
        parent: &ParentSampler,
        budget: &Budget,
    ) -> Result<(), ControlFault> {
        if self.control_audit.is_some() {
            return Err(ControlFault::Protocol);
        }
        let stop = parent.stop().ok_or(ControlFault::Protocol)?;
        let mut audit =
            WorkerControlAudit::new(self.worker.binding(), self.worker.origin(), stop, budget)?;
        for received in &self.control_packets {
            audit.observe(received);
        }
        self.control_audit = Some(audit);
        Ok(())
    }

    pub fn control_audit(&self) -> Option<&WorkerControlAudit> {
        self.control_audit.as_ref()
    }

    /// Ends transport ownership and transfers bounded raw reports plus their
    /// assessments for inclusion in the session finalizer's evidence owner.
    pub fn into_evidence(self) -> WorkerReportEvidence {
        WorkerReportEvidence {
            binding: self.worker.binding(),
            origin: self.worker.origin(),
            result_packets: self.result_packets,
            control_packets: self.control_packets,
            events: self.events,
            audit: self.audit,
            assessment_fault: self.assessment_fault,
            failure_vector: self.failure_vector,
            control_audit: self.control_audit,
            overflowed: self.overflowed,
            _result_storage: self.result_storage,
            _control_storage: self.control_storage,
            _event_storage: self.event_storage,
        }
    }

    /// Start report assessment only after the caller has confirmed Start was
    /// admitted. The raw packets remain returned to the caller for bounded
    /// evidence retention and export.
    pub fn begin_result_audit(
        &mut self,
        issued: crate::monitor::Anchor,
        budget: &Budget,
    ) -> Result<(), assessment::Fault> {
        if self.audit.is_some() {
            return Err(assessment::Fault::Closed);
        }
        let count = self
            .worker
            .binding()
            .max_result_messages()
            .ok_or(assessment::Fault::Capacity)?;
        let audit = Audit::new(self.worker.binding(), issued, budget)?;
        let (result_packets, reservation) =
            reserve_packets(budget, count).map_err(|_| assessment::Fault::Capacity)?;
        self.result_packets = result_packets;
        self.result_packet_limit = count;
        self.result_storage = Some(reservation);
        self.audit = Some(audit);
        Ok(())
    }

    /// Poll one ordinary result packet after setup. Idle observations continue
    /// the audit deadline; transport/decode/assessment faults are sticky. The
    /// packet is still returned on an assessment rejection so the caller can
    /// retain the raw evidence instead of losing the first failure.
    pub fn poll_result(
        &mut self,
        setup: &mut ReadyDelivery,
        clock: &WindowsClock,
        budget: &Budget,
    ) -> ResultPoll {
        if self.audit.is_none() {
            return ResultPoll::Unavailable(assessment::Fault::Closed);
        }
        match self.worker.receive(|| clock.capture().ok()) {
            Receive::Idle { anchor } => {
                self.record_event(
                    crate::sampler::wire::Lane::Ordinary,
                    ReceiveKind::Idle,
                    anchor,
                    None,
                    None,
                );
                let audit = self.audit.as_mut().expect("audit checked above");
                if let Err(fault) = audit.tick(anchor) {
                    self.assessment_fault.get_or_insert(fault);
                }
                ResultPoll::Idle { anchor }
            }
            Receive::Fault { code, anchor } => {
                self.record_event(
                    crate::sampler::wire::Lane::Ordinary,
                    ReceiveKind::Fault,
                    anchor,
                    code,
                    None,
                );
                let audit = self.audit.as_mut().expect("audit checked above");
                audit.close(assessment::Fault::Closed);
                self.assessment_fault
                    .get_or_insert(assessment::Fault::Closed);
                ResultPoll::Fault { code, anchor }
            }
            Receive::Packet(raw) => {
                self.record_event(
                    crate::sampler::wire::Lane::Ordinary,
                    ReceiveKind::Packet,
                    raw.receipt(),
                    None,
                    Some(crate::evidence::Digest::of(
                        crate::evidence::Domain::RawArtifact,
                        raw.packet().bytes(),
                    )),
                );
                let audit = self.audit.as_mut().expect("audit checked above");
                let received = raw
                    .stamp_process(self.worker.origin())
                    .decode_worker(setup.decoder());
                let result = match received.message() {
                    Ok(crate::workload::wire::Message::DispatchRecord(record))
                        if record.mismatches != 0 =>
                    {
                        if self.failure_vector.is_none() {
                            match FailureVector::new(self.worker.binding(), record, budget) {
                                Ok(vector) => self.failure_vector = Some(vector),
                                Err(crate::sampler::wire::WireError::Capacity) => {
                                    self.assessment_fault
                                        .get_or_insert(assessment::Fault::Capacity);
                                }
                                Err(_) => {
                                    self.assessment_fault
                                        .get_or_insert(assessment::Fault::Check);
                                }
                            }
                        }
                        audit.observe(
                            received.message().expect("decoded result"),
                            received.raw().receipt(),
                        )
                    }
                    Ok(crate::workload::wire::Message::FailureChunk(chunk)) => {
                        if let Some(vector) = self.failure_vector.as_mut() {
                            if vector.accept(chunk).is_err() {
                                self.assessment_fault
                                    .get_or_insert(assessment::Fault::Check);
                            }
                        } else {
                            self.assessment_fault
                                .get_or_insert(assessment::Fault::Check);
                        }
                        Ok(audit.progress())
                    }
                    Ok(message) => audit.observe(message, received.raw().receipt()),
                    Err(_) => {
                        audit.close(assessment::Fault::Binding);
                        Err(assessment::Fault::Binding)
                    }
                };
                if let Err(fault) = result {
                    self.assessment_fault.get_or_insert(fault);
                }
                if self.result_packets.len() == self.result_packet_limit {
                    self.overflowed = true;
                    self.assessment_fault
                        .get_or_insert(assessment::Fault::Capacity);
                    audit.close(assessment::Fault::Capacity);
                    ResultPoll::Overflow(received)
                } else {
                    self.result_packets.push(received);
                    ResultPoll::Packet
                }
            }
        }
    }

    pub fn result_progress(&self) -> Option<Progress> {
        self.audit.as_ref().map(Audit::progress)
    }

    pub fn assessment_fault(&self) -> Option<assessment::Fault> {
        self.assessment_fault
            .or_else(|| self.audit.as_ref().and_then(Audit::fault))
    }

    pub fn failure_vector(&self) -> Option<&FailureVector> {
        self.failure_vector.as_ref()
    }

    /// Make the one Start attempt using the exact worker that owns this pipe.
    pub fn queue_start(
        &mut self,
        parent: &mut ParentSampler,
        starting: &windows_process::Process<starting_bootstrap::Declaration>,
        sampler: &windows_process::Process,
        setup: &mut ReadyDelivery,
        clock: &WindowsClock,
    ) -> Result<(), parent_start::Fault> {
        parent.queue_start(
            Processes {
                starting,
                sampler,
                worker: self.worker,
                setup,
            },
            clock,
            &mut self.sender,
        )
    }

    /// Collect a fresh retained-process frame and queue NativeGrant on the
    /// same sender/sequence as Start. The caller must keep this blocking OS
    /// observation off the independent deadline-controller lane.
    pub fn collect_and_queue_native_grant(
        &mut self,
        parent: &mut ParentSampler,
        starting: &windows_process::Process<starting_bootstrap::Declaration>,
        sampler: &windows_process::Process,
        setup: &mut ReadyDelivery,
        clock: &WindowsClock,
    ) -> Result<(), Fault> {
        parent.collect_and_queue_native_grant(
            Processes {
                starting,
                sampler,
                worker: self.worker,
                setup,
            },
            clock,
            &mut self.sender,
        )
    }
}
