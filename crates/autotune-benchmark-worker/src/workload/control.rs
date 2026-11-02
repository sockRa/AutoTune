//! Control-only child state, shared by the actual Windows thread and CPU tests.
use super::{
    bootstrap::EntryClock,
    gate::StopWindow,
    transport::WorkerReceived,
    wire::{Message, WorkerBinding},
};
use crate::{
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        child::NativeAdmission,
    },
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Clock,
    Protocol,
    Capacity,
    Deadline,
    Transport,
    Send,
}
pub struct Shared {
    admission: Arc<NativeAdmission>,
    cooperative_raw: AtomicU64,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            admission: Arc::new(NativeAdmission::default()),
            cooperative_raw: AtomicU64::new(u64::MAX),
        }
    }
}
impl Shared {
    pub fn admission(&self) -> &NativeAdmission {
        &self.admission
    }
    pub fn cooperative_raw(&self) -> u64 {
        self.cooperative_raw.load(Ordering::Acquire)
    }
    pub fn close(&self) {
        self.admission.stop();
    }
    pub(super) fn close_at(&self, binding: WorkerBinding, trigger: Option<Anchor>) {
        self.close();
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let raw = trigger
            .and_then(|a| binding.elapsed(a).ok())
            .and_then(|n| {
                binding.epoch.interrupt_100ns.checked_add(
                    n.saturating_add(p.cooperative_stop_ns)
                        .min(binding.run_end())
                        / 100,
                )
            })
            .unwrap_or(0);
        self.cooperative_raw.fetch_min(raw, Ordering::AcqRel);
    }
}
pub struct State {
    binding: WorkerBinding,
    shared: Arc<Shared>,
    clock: EntryClock,
    raw: Vec<WorkerReceived>,
    overflow: Option<WorkerReceived>,
    window: Option<StopWindow>,
    fault: Option<Fault>,
    ack: bool,
    finished: bool,
    _metadata: Reservation,
}
impl State {
    pub fn new(
        binding: WorkerBinding,
        first: Anchor,
        shared: Arc<Shared>,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let capacity = (binding.plan.envelope_ns / p.controller_tick_ns * 2 + 16) as usize;
        let clock = EntryClock::with_capacity(
            binding.monitor.qpc_frequency,
            binding.epoch,
            first,
            capacity,
            budget,
        )
        .map_err(|_| Fault::Capacity)?;
        let metadata = budget
            .reserve(std::mem::size_of::<Self>() + 4 * std::mem::size_of::<WorkerReceived>())
            .map_err(|_| Fault::Capacity)?;
        let mut raw = Vec::new();
        raw.try_reserve_exact(4).map_err(|_| Fault::Capacity)?;
        Ok(Self {
            binding,
            shared,
            clock,
            raw,
            overflow: None,
            window: None,
            fault: None,
            ack: false,
            finished: false,
            _metadata: metadata,
        })
    }
    pub fn window(&self) -> Option<StopWindow> {
        self.window
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn raw(&self) -> &[WorkerReceived] {
        &self.raw
    }
    pub fn overflow(&self) -> Option<&WorkerReceived> {
        self.overflow.as_ref()
    }
    pub fn clock(&self) -> &EntryClock {
        &self.clock
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    pub fn fail(&mut self, f: Fault) {
        self.shared.close();
        if f != Fault::Deadline {
            self.shared.cooperative_raw.fetch_min(0, Ordering::AcqRel);
        }
        self.fault.get_or_insert(f);
    }
    fn now(&mut self, anchor: Option<Anchor>) -> Result<u64, Fault> {
        match self.clock.observe(anchor) {
            Ok(n) => Ok(n),
            Err(_) => {
                self.fail(Fault::Clock);
                self.shared.cooperative_raw.fetch_min(0, Ordering::AcqRel);
                Err(Fault::Clock)
            }
        }
    }
    fn stop(&mut self, window: StopWindow) {
        self.shared.close();
        let window = if let Some(old) = self.window {
            StopWindow {
                trigger_ns: old.trigger_ns.min(window.trigger_ns),
                cooperative_ns: old.cooperative_ns.min(window.cooperative_ns),
                exit_ns: old.exit_ns.min(window.exit_ns),
            }
        } else {
            window
        };
        self.window = Some(window);
        let cap = self
            .binding
            .epoch
            .interrupt_100ns
            .checked_add(window.cooperative_ns / 100)
            .unwrap_or(0);
        self.shared.cooperative_raw.fetch_min(cap, Ordering::AcqRel);
    }
    pub fn event(&mut self, anchor: Option<Anchor>) -> Option<Message> {
        self.shared.close();
        let n = self.now(anchor).ok()?;
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        self.stop(StopWindow {
            trigger_ns: n,
            cooperative_ns: n
                .saturating_add(p.cooperative_stop_ns)
                .min(self.binding.run_end()),
            exit_ns: n
                .saturating_add(p.exit_observation_ns)
                .min(self.binding.run_end()),
        });
        if !self.within_window(n) {
            return None;
        }
        self.ack(anchor)
    }
    pub fn local_stop(
        &mut self,
        trigger: Option<Anchor>,
        receipt: Option<Anchor>,
    ) -> Option<Message> {
        self.shared.close_at(self.binding, trigger);
        let n = self.now(receipt).ok()?;
        let Some(t) = trigger else {
            self.fail(Fault::Clock);
            return None;
        };
        let Ok(start) = self.binding.elapsed(t) else {
            self.fail(Fault::Clock);
            return None;
        };
        if start > n || receipt.is_none_or(|a| t.qpc_after > a.qpc_before) {
            self.fail(Fault::Clock);
            return None;
        }
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        self.stop(StopWindow {
            trigger_ns: start,
            cooperative_ns: start
                .saturating_add(p.cooperative_stop_ns)
                .min(self.binding.run_end()),
            exit_ns: start
                .saturating_add(p.exit_observation_ns)
                .min(self.binding.run_end()),
        });
        if !self.within_window(n) {
            return None;
        }
        self.ack(receipt)
    }
    fn ack(&mut self, anchor: Option<Anchor>) -> Option<Message> {
        if self.ack {
            return None;
        }
        self.ack = true;
        anchor.map(|anchor| Message::StopAck {
            anchor,
            pending: self.shared.admission.pending(),
        })
    }
    #[allow(clippy::result_large_err)]
    pub fn packet(&mut self, received: WorkerReceived) -> Result<Option<Message>, WorkerReceived> {
        // A control packet can never authorize work. Close first, even when
        // malformed clocks/codec or a full raw evidence ledger prevent parsing.
        self.shared.close();
        if self.overflow.is_some() {
            return Err(received);
        }
        Ok(self.accept_packet(received))
    }
    fn accept_packet(&mut self, received: WorkerReceived) -> Option<Message> {
        let receipt = received.raw().receipt();
        let message = received.message().cloned();
        if self.raw.len() == 4 {
            self.overflow = Some(received);
            self.fail(Fault::Capacity);
            return None;
        }
        self.raw.push(received);
        let n = self.now(receipt).ok()?;
        let Ok(Message::Stop {
            trigger_ns,
            cooperative_ns,
            exit_ns,
            ..
        }) = message
        else {
            self.fail(Fault::Protocol);
            return None;
        };
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        if trigger_ns > n
            || cooperative_ns
                != trigger_ns
                    .saturating_add(p.cooperative_stop_ns)
                    .min(self.binding.run_end())
            || exit_ns
                != trigger_ns
                    .saturating_add(p.exit_observation_ns)
                    .min(self.binding.run_end())
        {
            self.fail(Fault::Protocol);
            return None;
        }
        self.stop(StopWindow {
            trigger_ns,
            cooperative_ns,
            exit_ns,
        });
        if !self.within_window(n) {
            return None;
        }
        self.ack(receipt)
    }
    fn within_window(&mut self, n: u64) -> bool {
        let raw = self.binding.epoch.interrupt_100ns.checked_add(n / 100);
        if n >= self.binding.run_end()
            || self.window.is_some_and(|w| n >= w.cooperative_ns)
            || raw.is_none_or(|r| r >= self.shared.cooperative_raw())
        {
            self.fail(Fault::Deadline);
            return false;
        }
        self.fault.is_none()
    }
    pub fn tick(&mut self, anchor: Option<Anchor>) -> bool {
        let Ok(n) = self.now(anchor) else {
            return false;
        };
        self.within_window(n)
    }
    pub fn complete(
        &mut self,
        anchor: Option<Anchor>,
        cleanup_attempted: bool,
        cleanup_status: Option<i32>,
    ) -> Option<Message> {
        self.shared.close();
        let n = self.now(anchor).ok()?;
        if self.fault.is_some() {
            return None;
        }
        if !cleanup_attempted && cleanup_status.is_some() {
            self.fail(Fault::Protocol);
            return None;
        }
        if self.finished || !self.within_window(n) {
            self.fail(Fault::Deadline);
            return None;
        }
        self.finished = true;
        anchor.map(|anchor| Message::RoleFinished {
            anchor,
            pending: self.shared.admission.pending(),
            cleanup_attempted,
            cleanup_status,
        })
    }
}
