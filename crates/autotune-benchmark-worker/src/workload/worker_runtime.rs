//! Bound preparation/Ready/Start/native-grant reception/producer composition.
//! The fixed parent orchestrator is still absent. Reported Ready is not consent.
use super::{
    bootstrap::Bound,
    control::Shared,
    gate::{Action, Gate},
    producer::{Driver, FrameSink, NativeAdmission, PrepareFailure, Producer},
    transport::WorkerReceived,
    wire::{Channel, Direction, Message, WorkerBinding},
};
use crate::{
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        wire::Lane,
    },
    worker::Identity,
};
use std::sync::Arc;
#[path = "worker_receive.rs"]
mod receive;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Binding,
    Capacity,
    Protocol,
    Gate(super::gate::Fault),
    Producer(super::producer::Fault),
    Control,
}
/// Actual CUDA setup must supply these from its successful, retained mapping
/// and capability checks. Injected values remain caller reports, not proof.
#[derive(Clone, Copy)]
pub struct ReadyFacts {
    pub context: Identity,
    pub driver_version: i32,
    pub compute_major: i32,
    pub compute_minor: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupDelivery {
    pub digest: crate::evidence::Digest,
    pub sent: u32,
    pub count: u32,
}
pub struct Session<B, C, S> {
    binding: WorkerBinding,
    shared: Arc<Shared>,
    gate: Gate,
    producer: Producer<B, C, S>,
    ready: Option<Message>,
    setup_delivery: Option<SetupDelivery>,
    ready_receipt: Option<Anchor>,
    start_packet: Option<WorkerReceived>,
    native_grant_packet: Option<WorkerReceived>,
    native_admission_issued: bool,
    rejected: Option<WorkerReceived>,
    fault: Option<Fault>,
    started: bool,
    deadline: u64,
    worker_digest: crate::evidence::Digest,
    history: super::bootstrap::EntryClock,
    receiver: Channel,
    transport_fault: Option<(Option<i32>, Option<Anchor>)>,
    input_finished: bool,
    prestart: super::prestart::Report,
    _metadata: Reservation,
}
pub(super) fn handshake(
    bootstrap: &Bound,
    shared: &Shared,
) -> Result<Gate, super::producer::Fault> {
    let binding = bootstrap.binding();
    let expected = binding
        .epoch
        .interrupt_100ns
        .checked_add(bootstrap.deadline_ns() / 100);
    if expected.is_none()
        || !shared.matches(binding)
        || shared.setup_observations() != (expected, None)
        || shared.admission().stopped()
    {
        return Err(super::producer::Fault::Binding);
    }
    Gate::from_bootstrap(bootstrap).map_err(|_| super::producer::Fault::Protocol)
}
impl<
        B: Driver,
        C: FnMut() -> Option<Anchor> + 'static,
        S: FnMut() -> Option<super::gate::StopWindow> + 'static,
    > Session<B, C, S>
{
    #[allow(clippy::result_large_err)] // Original driver/callback owners, no fallible error allocation.
    pub fn prepare(
        bootstrap: &Bound,
        shared: Arc<Shared>,
        driver: B,
        clock: C,
        stop: S,
        budget: &Budget,
    ) -> Result<Self, PrepareFailure<B, C, S>> {
        let binding = bootstrap.binding();
        let resources = (|| {
            let gate = handshake(bootstrap, &shared)?;
            let metadata = budget
                .reserve(std::mem::size_of::<Self>())
                .map_err(|_| super::producer::Fault::Capacity)?;
            let tick = autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .controller_tick_ns;
            let capacity = (binding.plan.envelope_ns / tick)
                .checked_mul(2)
                .and_then(|n| n.checked_add(128))
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(super::producer::Fault::Capacity)?;
            let history = super::bootstrap::EntryClock::with_capacity(
                binding.monitor().qpc_frequency,
                binding.epoch,
                bootstrap.receipt(),
                capacity,
                budget,
            )
            .map_err(|e| match e {
                super::bootstrap::Error::Capacity => super::producer::Fault::Capacity,
                _ => super::producer::Fault::Clock,
            })?;
            let receiver =
                Channel::new(binding, Lane::Ordinary, Direction::ToWorker, budget.clone())
                    .map_err(|_| super::producer::Fault::Capacity)?;
            Ok((gate, metadata, history, receiver))
        })();
        let (gate, metadata, history, receiver) = match resources {
            Ok(v) => v,
            Err(fault) => {
                shared.close();
                return Err(PrepareFailure {
                    fault,
                    driver,
                    clock,
                    stop,
                });
            }
        };
        let mut producer = match Producer::prepare_retained(binding, driver, clock, stop, budget) {
            Ok(v) => v,
            Err(f) => {
                shared.close();
                return Err(f);
            }
        };
        let fault = producer
            .bind_control(shared.clone())
            .and_then(|()| producer.inherit_setup(bootstrap.receipt()))
            .err()
            .map(Fault::Producer);
        Ok(Self {
            binding,
            shared,
            gate,
            producer,
            ready: None,
            setup_delivery: None,
            ready_receipt: None,
            start_packet: None,
            native_grant_packet: None,
            native_admission_issued: false,
            rejected: None,
            fault,
            started: false,
            deadline: bootstrap.deadline_ns(),
            worker_digest: bootstrap.run().worker_digest(),
            history,
            receiver,
            transport_fault: None,
            input_finished: false,
            prestart: super::prestart::Report::default(),
            _metadata: metadata,
        })
    }
    pub fn producer(&self) -> &Producer<B, C, S> {
        &self.producer
    }
    pub fn ready(&self) -> Option<&Message> {
        self.ready.as_ref()
    }
    pub fn setup_delivery(&self) -> Option<SetupDelivery> {
        self.setup_delivery
    }
    /// One attempt on the producer's existing ordinary sequence. Each handoff
    /// is bracketed by the original setup clock/control gate, with no new grace.
    pub fn send_setup(
        &mut self,
        artifact: &super::worker_setup::Artifact,
        sink: &mut impl FrameSink,
        budget: &Budget,
    ) -> Result<(), Fault> {
        if self.fault.is_some() || self.ready.is_some() || self.setup_delivery.is_some() {
            return Err(self.fail(Fault::Protocol));
        }
        let count = artifact
            .canonical()
            .len()
            .div_ceil(super::worker_setup_delivery::CHUNK_BYTES) as u32;
        self.setup_delivery = Some(SetupDelivery {
            digest: artifact.digest(),
            sent: 0,
            count,
        });
        if artifact.binding() != self.binding
            || self.shared.setup_observations() != (Some(artifact.deadline_raw()), None)
        {
            return Err(self.fail(Fault::Protocol));
        }
        let chunks = super::worker_setup_delivery::Chunks::new(artifact, budget)
            .map_err(|_| self.fail(Fault::Producer(super::producer::Fault::Capacity)))?;
        for index in 0..chunks.count() {
            self.poll_setup()?;
            let message = chunks
                .message(index)
                .map_err(|_| self.fail(Fault::Protocol))?;
            self.producer
                .publish_ready(&message, sink)
                .map_err(|e| self.fail(Fault::Producer(e)))?;
            self.setup_delivery.as_mut().expect("one delivery").sent += 1;
            self.poll_setup()?;
        }
        Ok(())
    }
    pub fn ready_receipt(&self) -> Option<Anchor> {
        self.ready_receipt
    }
    pub fn start_packet(&self) -> Option<&WorkerReceived> {
        self.start_packet.as_ref()
    }
    /// Parent authorization is retained separately from Start. Digest
    /// references bind the packet, but never issue NativeAdmission by themselves.
    pub fn native_grant_packet(&self) -> Option<&WorkerReceived> {
        self.native_grant_packet.as_ref()
    }
    /// Issue workload admission only from the retained grant that already
    /// passed Start chronology, setup digest, issue/receipt freshness and the
    /// original setup deadline. The fixed parent sender is the trust boundary
    /// for its digest-only facts; this does not authenticate arbitrary input.
    #[cfg(all(windows, target_arch = "x86_64", feature = "native-cuda"))]
    pub(super) fn native_admission(
        &mut self,
        setup: &super::worker_setup::Artifact,
    ) -> Result<NativeAdmission, Fault> {
        if self.fault.is_some()
            || self.started
            || self.native_admission_issued
            || self.shared.admission().stopped()
            || self.start_packet.is_none()
        {
            return Err(Fault::Protocol);
        }
        let expected_setup = self
            .setup_delivery
            .filter(|delivery| delivery.sent == delivery.count)
            .map(|delivery| delivery.digest)
            .ok_or(Fault::Protocol)?;
        if setup.digest() != expected_setup {
            return Err(Fault::Protocol);
        }
        let grant = self.native_grant_packet.as_ref().ok_or(Fault::Protocol)?;
        if !self.grant_matches_start_and_setup(grant) {
            return Err(Fault::Protocol);
        }
        let message = grant.message().map_err(|_| Fault::Protocol)?;
        let admission = NativeAdmission::from_native_grant(self.binding, message, expected_setup)
            .ok_or(Fault::Protocol)?;
        self.native_admission_issued = true;
        Ok(admission)
    }
    pub fn rejected(&self) -> Option<&WorkerReceived> {
        self.rejected.as_ref()
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn history(&self) -> &super::bootstrap::EntryClock {
        &self.history
    }
    /// Continue from the retained final setup observation using the same epoch;
    /// this cannot reset a fault, issue consent or move a deadline.
    pub fn inherit_setup(&mut self, anchor: Anchor) -> Result<(), Fault> {
        self.observe_history(Some(anchor))?;
        self.producer
            .inherit_setup(anchor)
            .map_err(|e| self.fail(Fault::Producer(e)))
    }
    fn fail(&mut self, f: Fault) -> Fault {
        self.fault.get_or_insert(f);
        self.shared.close_at(self.binding, None);
        f
    }
    fn observe_history(&mut self, anchor: Option<Anchor>) -> Result<(), Fault> {
        self.history.observe(anchor).map(|_| ()).map_err(|e| {
            let f = if e == super::bootstrap::Error::Capacity {
                super::producer::Fault::Capacity
            } else {
                super::producer::Fault::Clock
            };
            self.fail(Fault::Producer(f))
        })
    }
    /// CPU preparation precedes this fresh Ready timestamp. Ready and dispatch
    /// reports share the producer's one output channel/sequence and failed frame.
    pub fn announce(&mut self, facts: ReadyFacts, sink: &mut impl FrameSink) -> Result<(), Fault> {
        if self.fault.is_some()
            || self.ready.is_some()
            || self.setup_delivery.is_some_and(|s| s.sent != s.count)
        {
            return Err(self.fail(Fault::Protocol));
        }
        let anchor = self.poll_setup()?;
        self.ready = Some(Message::Ready {
            anchor,
            identity: self.producer.identity(),
            context: facts.context,
            worker_digest: self.worker_digest,
            driver_version: facts.driver_version,
            compute_major: facts.compute_major,
            compute_minor: facts.compute_minor,
            prepared_lanes: self.binding.plan.shape.lanes(),
        });
        if let Err(e) = self.gate.ready(self.ready.as_ref().expect("Ready owner")) {
            return Err(self.fail(Fault::Gate(e)));
        }
        if let Err(e) = self
            .producer
            .publish_ready(self.ready.as_ref().expect("Ready owner"), sink)
        {
            return Err(self.fail(Fault::Producer(e)));
        }
        let receipt = self.poll_setup()?;
        self.ready_receipt = Some(receipt);
        if self.shared.complete_setup(self.binding, receipt).is_err() {
            return Err(self.fail(Fault::Control));
        }
        Ok(())
    }
    pub fn poll_setup(&mut self) -> Result<Anchor, Fault> {
        if let Some(f) = self.fault {
            return Err(f);
        }
        let observed = self.producer.setup_observation(self.deadline);
        self.observe_history(self.producer.latest_observation())?;
        match observed {
            Ok(a) => Ok(a),
            Err(e) => Err(self.fail(Fault::Producer(e))),
        }
    }
    /// Retain the decoded owner (including malformed bytes) before examining it.
    /// Accept Start, then one distinct setup-bound native grant on the same
    /// ordinary sequence. A rejected owner is not lost.
    #[allow(clippy::result_large_err)]
    pub fn packet(&mut self, raw: WorkerReceived) -> Result<(), WorkerReceived> {
        if self.rejected.is_some() {
            return Err(raw);
        }
        let valid_start = self.fault.is_none()
            && self.ready_receipt.is_some()
            && self.start_packet.is_none()
            && self.native_grant_packet.is_none()
            && raw.raw().lane() == Lane::Ordinary
            && raw.binding().ok() == Some(self.binding)
            && matches!(raw.message(), Ok(Message::Start { .. }));
        let valid_grant = self.fault.is_none()
            && self.start_packet.is_some()
            && self.native_grant_packet.is_none()
            && raw.raw().lane() == Lane::Ordinary
            && raw.binding().ok() == Some(self.binding)
            && matches!(raw.message(), Ok(Message::NativeGrant { .. }))
            && self.grant_matches_start_and_setup(&raw);
        let valid = valid_start || valid_grant;
        if !valid {
            self.rejected = Some(raw);
            self.fail(Fault::Protocol);
            return Ok(());
        }
        if valid_start {
            self.start_packet = Some(raw);
            let raw = self.start_packet.as_ref().expect("Start owner");
            match self.gate.accept(
                raw.message().expect("validated message"),
                raw.raw().receipt(),
            ) {
                Ok(Action::Start { .. }) => (),
                Ok(_) => {
                    self.fail(Fault::Protocol);
                }
                Err(e) => {
                    self.fail(Fault::Gate(e));
                }
            }
        } else {
            self.native_grant_packet = Some(raw);
        }
        Ok(())
    }
    fn grant_matches_start_and_setup(&self, raw: &WorkerReceived) -> bool {
        let Some(start) = self.start_packet.as_ref() else {
            return false;
        };
        let (
            Ok(Message::Start {
                issued: start_issued,
            }),
            Ok(Message::NativeGrant { issued, setup, .. }),
        ) = (start.message(), raw.message())
        else {
            return false;
        };
        let Some(expected_setup) = self.setup_delivery.map(|delivery| delivery.digest) else {
            return false;
        };
        let Some(start_receipt) = start.raw().receipt() else {
            return false;
        };
        let Some(receipt) = raw.raw().receipt() else {
            return false;
        };
        let Ok(start_received_ns) = self.binding.elapsed(start_receipt) else {
            return false;
        };
        let Ok(start_ns) = self.binding.elapsed(*start_issued) else {
            return false;
        };
        let Ok(issue_ns) = self.binding.elapsed(*issued) else {
            return false;
        };
        let Ok(receipt_ns) = self.binding.elapsed(receipt) else {
            return false;
        };
        let Some(grant_age_ns) = receipt_ns.checked_sub(issue_ns) else {
            return false;
        };
        *setup == expected_setup
            && start_ns <= start_received_ns
            && issue_ns >= start_received_ns
            && grant_age_ns
                < autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .observation_gap_ns
            && receipt_ns < self.deadline
    }
    fn take_start(&mut self) -> Result<super::gate::StartObservation, Fault> {
        if self.fault.is_some() || self.started || self.shared.admission().stopped() {
            return Err(self.fail(Fault::Control));
        }
        self.gate
            .take_start()
            .ok_or_else(|| self.fail(Fault::Protocol))
    }
    pub fn start_fixture(&mut self) -> Result<(), Fault> {
        let start = self.take_start()?;
        match self.producer.start_fixture(start) {
            Ok(()) => {
                self.started = true;
                Ok(())
            }
            Err(e) => Err(self.fail(Fault::Producer(e))),
        }
    }
    pub fn start_native(&mut self, admission: NativeAdmission) -> Result<(), Fault> {
        let grant_matches = self.native_grant_packet.as_ref().is_some_and(|grant| {
            grant
                .message()
                .is_ok_and(|message| admission.matches_grant(self.binding, message))
        });
        if !self.native_admission_issued || !grant_matches {
            return Err(self.fail(Fault::Protocol));
        }
        let start = self.take_start()?;
        match self.producer.start_native(start, admission) {
            Ok(()) => {
                self.started = true;
                Ok(())
            }
            Err(e) => Err(self.fail(Fault::Producer(e))),
        }
    }
    pub fn advance(&mut self, sink: &mut impl FrameSink) {
        if self.started && self.fault.is_none() {
            if self.shared.admission().stopped()
                && !matches!(self.shared.stop_window(self.binding), Ok(Some(_)))
            {
                self.fail(Fault::Control);
                return;
            }
            self.producer.advance(sink);
        }
    }
    pub fn cleanup(&mut self) {
        if self.started {
            self.producer.cleanup();
        } else {
            self.producer.dispose_before_start(&mut self.prestart);
        }
    }
    pub fn prestart_disposition(&self) -> &super::prestart::Report {
        &self.prestart
    }
    pub fn cleanup_observation(&self) -> Option<super::producer::Cleanup> {
        if self.prestart.requested() {
            Some(self.prestart.cleanup)
        } else {
            self.producer.cleanup_observation()
        }
    }
    pub fn teardown_fault(&self) -> Option<super::teardown::Fault> {
        if self.prestart.requested() {
            self.prestart.ledger.fault()
        } else {
            self.producer.teardown_ledger().fault()
        }
    }
    pub fn teardown_evidence(&self) -> super::teardown::Evidence {
        if self.prestart.requested() {
            self.prestart.ledger.evidence()
        } else {
            self.producer.teardown_ledger().evidence()
        }
    }
    pub fn export_failure_step(
        &mut self,
        sink: &mut impl FrameSink,
    ) -> Result<bool, super::producer::Fault> {
        self.producer.export_failure_step(sink)
    }
}
