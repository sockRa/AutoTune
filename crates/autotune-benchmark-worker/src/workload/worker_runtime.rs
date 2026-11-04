//! Bound preparation/Ready/encoded-Start/producer composition. No executable,
//! native loader or workload admission issuer. Reported Ready is not consent.
use super::{
    bootstrap::Bound,
    control::Shared,
    gate::{Action, Gate},
    producer::{Driver, FrameSink, NativeAdmission, PrepareFailure, Producer},
    transport::WorkerReceived,
    wire::{Message, WorkerBinding},
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
pub struct Session<B, C, S> {
    binding: WorkerBinding,
    shared: Arc<Shared>,
    gate: Gate,
    producer: Producer<B, C, S>,
    ready: Option<Message>,
    ready_receipt: Option<Anchor>,
    start_packet: Option<WorkerReceived>,
    rejected: Option<WorkerReceived>,
    fault: Option<Fault>,
    started: bool,
    deadline: u64,
    worker_digest: crate::evidence::Digest,
    history: super::bootstrap::EntryClock,
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
            let history = super::bootstrap::EntryClock::new(
                binding.monitor().qpc_frequency,
                binding.epoch,
                bootstrap.receipt(),
                budget,
            )
            .map_err(|e| match e {
                super::bootstrap::Error::Capacity => super::producer::Fault::Capacity,
                _ => super::producer::Fault::Clock,
            })?;
            Ok((gate, metadata, history))
        })();
        let (gate, metadata, history) = match resources {
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
            ready_receipt: None,
            start_packet: None,
            rejected: None,
            fault,
            started: false,
            deadline: bootstrap.deadline_ns(),
            worker_digest: bootstrap.run().worker_digest(),
            history,
            _metadata: metadata,
        })
    }
    pub fn producer(&self) -> &Producer<B, C, S> {
        &self.producer
    }
    pub fn ready(&self) -> Option<&Message> {
        self.ready.as_ref()
    }
    pub fn ready_receipt(&self) -> Option<Anchor> {
        self.ready_receipt
    }
    pub fn start_packet(&self) -> Option<&WorkerReceived> {
        self.start_packet.as_ref()
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
        if self.fault.is_some() || self.ready.is_some() {
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
    /// Only one ordinary Start packet is accepted; a rejected owner is not lost.
    #[allow(clippy::result_large_err)]
    pub fn packet(&mut self, raw: WorkerReceived) -> Result<(), WorkerReceived> {
        if self.rejected.is_some() {
            return Err(raw);
        }
        let valid = self.fault.is_none()
            && self.ready_receipt.is_some()
            && self.start_packet.is_none()
            && raw.raw().lane() == Lane::Ordinary
            && raw.binding().ok() == Some(self.binding)
            && matches!(raw.message(), Ok(Message::Start { .. }));
        if !valid {
            self.rejected = Some(raw);
            self.fail(Fault::Protocol);
            return Ok(());
        }
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
        Ok(())
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
            if self.shared.admission().stopped() {
                self.fail(Fault::Control);
                return;
            }
            self.producer.advance(sink);
        }
    }
    pub fn cleanup(&mut self) {
        self.producer.cleanup();
    }
    pub fn export_failure_step(
        &mut self,
        sink: &mut impl FrameSink,
    ) -> Result<bool, super::producer::Fault> {
        self.producer.export_failure_step(sink)
    }
}
