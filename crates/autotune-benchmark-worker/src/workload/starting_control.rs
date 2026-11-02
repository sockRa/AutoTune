//! Pre-manifest read control only. No run binding, native authority or OS effects.
use super::{bootstrap::EntryClock, gate::StopWindow, starting::Scope};
use crate::{
    evidence::Digest,
    monitor::{clock::Clock, Anchor},
    sampler::{
        budget::{Budget, Reservation},
        child::NativeAdmission,
    },
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Clock,
    Capacity,
    Deadline,
    Protocol,
    Transport,
    Cleanup,
}
pub struct Shared {
    scope: Digest,
    frequency: u64,
    epoch: Anchor,
    first: Anchor,
    admission: NativeAdmission,
    stop: OnceLock<(StopWindow, Option<Anchor>)>,
    fault: OnceLock<Fault>,
    cooperative_raw: AtomicU64,
    _allocation: Reservation,
}
impl Shared {
    pub fn scope_digest(&self) -> Digest {
        self.scope
    }
    pub fn admission(&self) -> &NativeAdmission {
        &self.admission
    }
    pub fn window(&self) -> Option<StopWindow> {
        self.stop.get().map(|s| s.0)
    }
    pub fn trigger(&self) -> Option<Anchor> {
        self.stop.get().and_then(|s| s.1)
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault.get().copied()
    }
    pub fn cooperative_raw(&self) -> u64 {
        self.cooperative_raw.load(Ordering::Acquire)
    }
    pub fn close(&self) {
        self.admission.stop();
    }
    pub fn fail(&self, fault: Fault) {
        self.close();
        let _ = self.fault.set(fault);
        // Unusable control clocks/transport cannot authorize cleanup entry.
        if fault != Fault::Deadline {
            self.cooperative_raw.fetch_min(0, Ordering::AcqRel);
        }
    }
    fn elapsed(&self, a: Anchor) -> Result<u64, Fault> {
        if a.qpc_before < self.first.qpc_after {
            return Err(Fault::Clock);
        }
        Clock::temporal(self.frequency, self.epoch.clock_token, self.epoch)
            .and_then(|mut c| c.observe(a))
            .map_err(|_| Fault::Clock)
    }
    fn stop_ns(&self, n: u64, anchor: Option<Anchor>) -> StopWindow {
        self.close();
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let n = n.min(p.preflight_ns);
        let w = self
            .stop
            .get_or_init(|| {
                (
                    StopWindow {
                        trigger_ns: n,
                        cooperative_ns: n
                            .saturating_add(p.cooperative_stop_ns)
                            .min(p.pilot_session_ns),
                        exit_ns: n
                            .saturating_add(p.exit_observation_ns)
                            .min(p.pilot_session_ns),
                    },
                    anchor,
                )
            })
            .0;
        self.cooperative_raw.fetch_min(
            self.epoch
                .interrupt_100ns
                .checked_add(w.cooperative_ns / 100)
                .unwrap_or(0),
            Ordering::AcqRel,
        );
        w
    }
    /// Close before validating. First stop owns the window; repeated calls can
    /// never renew it. Main-thread clocks are separate from control tick history.
    pub fn request_stop(&self, anchor: Option<Anchor>) -> Result<StopWindow, Fault> {
        self.close();
        let n = anchor.ok_or(Fault::Clock).and_then(|a| self.elapsed(a));
        match n {
            Ok(n) => {
                let p = autotune_fma_reference::profile::Profile::approved_design().limits;
                let w = self.stop_ns(n, anchor);
                if n >= p.preflight_ns {
                    self.fail(Fault::Deadline);
                }
                Ok(w)
            }
            Err(f) => {
                self.fail(f);
                Err(f)
            }
        }
    }
    /// Must be checked immediately before a separately authorized native cleanup.
    /// This check does not itself mark a call pending or grant execution consent.
    pub fn cleanup_allowed(&self, a: Anchor) -> bool {
        let valid = self.elapsed(a).is_ok_and(|n| {
            self.window()
                .is_some_and(|w| n >= w.trigger_ns && n < w.cooperative_ns)
        }) && a.interrupt_100ns < self.cooperative_raw()
            && !self.admission.pending();
        if !valid {
            self.close();
        }
        valid
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub anchor: Anchor,
    pub initialized_session: bool,
    pub cleanup_attempted: bool,
    pub cleanup_status: Option<i32>,
}
pub struct State {
    shared: Arc<Shared>,
    history: EntryClock,
    completion: Option<Completion>,
    completion_receipt: Option<Anchor>,
    finished: bool,
    _metadata: Reservation,
}
impl State {
    pub fn new(scope: &Scope, first: Anchor, budget: &Budget) -> Result<Self, Fault> {
        let d = scope.draft();
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let mut clock =
            Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| Fault::Clock)?;
        if first.qpc_before < d.epoch.qpc_after {
            return Err(Fault::Clock);
        }
        if clock.observe(first).map_err(|_| Fault::Clock)? >= p.preflight_ns {
            return Err(Fault::Deadline);
        }
        d.epoch
            .interrupt_100ns
            .checked_add((p.preflight_ns + p.exit_observation_ns) / 100)
            .ok_or(Fault::Deadline)?;
        let metadata = budget
            .reserve(std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let capacity =
            ((p.preflight_ns + p.exit_observation_ns) / p.controller_tick_ns * 2 + 16) as usize;
        let history = EntryClock::with_capacity(d.frequency, d.epoch, first, capacity, budget)
            .map_err(|_| Fault::Capacity)?;
        // Survives any Shared clone retained after State/Report is dropped.
        // Includes Arc bookkeeping and the fixed watcher/mailbox handle state.
        let shared_allocation = budget
            .reserve(std::mem::size_of::<Shared>() + 16 * 1024)
            .map_err(|_| Fault::Capacity)?;
        let shared = Arc::new(Shared {
            scope: scope.digest(),
            frequency: d.frequency,
            epoch: d.epoch,
            first,
            admission: NativeAdmission::default(),
            stop: OnceLock::new(),
            fault: OnceLock::new(),
            cooperative_raw: AtomicU64::new(u64::MAX),
            _allocation: shared_allocation,
        });
        Ok(Self {
            shared,
            history,
            completion: None,
            completion_receipt: None,
            finished: false,
            _metadata: metadata,
        })
    }
    pub fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }
    pub fn history(&self) -> &EntryClock {
        &self.history
    }
    pub fn completion(&self) -> Option<Completion> {
        self.completion
    }
    pub fn completion_receipt(&self) -> Option<Anchor> {
        self.completion_receipt
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    pub fn tick(&mut self, anchor: Option<Anchor>) -> bool {
        let n = match self.history.observe(anchor) {
            Ok(n) => n,
            Err(e) => {
                self.shared.fail(if e == super::bootstrap::Error::Capacity {
                    Fault::Capacity
                } else {
                    Fault::Clock
                });
                return false;
            }
        };
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        if n >= p.preflight_ns && self.shared.window().is_none() {
            // A delayed controller observes expiry at the original boundary,
            // never now+grace. No synthetic raw Anchor is invented for it.
            self.shared.stop_ns(p.preflight_ns, None);
            self.shared.fail(Fault::Deadline);
        }
        if self.shared.window().is_some_and(|w| n >= w.exit_ns) {
            self.shared.fail(Fault::Deadline);
            return false;
        }
        if self.shared.window().is_some_and(|w| n >= w.cooperative_ns) {
            self.shared.close();
            self.shared.cooperative_raw.fetch_min(0, Ordering::AcqRel);
        }
        self.shared.fault().is_none_or(|f| f == Fault::Deadline) && !self.finished
    }
    /// Completion is a raw child assertion, not observed process exit or a GPU
    /// fence. Retain first command and actual receipt even when contradictory.
    pub fn complete(&mut self, command: Completion, receipt: Option<Anchor>) -> Result<(), Fault> {
        self.shared.close();
        if self.completion.is_some() {
            self.shared.fail(Fault::Protocol);
            return Err(Fault::Protocol);
        }
        self.completion = Some(command);
        self.completion_receipt = receipt;
        let result = (|| {
            let a = receipt.ok_or(Fault::Clock)?;
            let n = self.history.observe(Some(a)).map_err(|_| Fault::Clock)?;
            let end = self.shared.elapsed(command.anchor)?;
            if end > n || command.anchor.qpc_after > a.qpc_before {
                return Err(Fault::Clock);
            }
            let p = autotune_fma_reference::profile::Profile::approved_design().limits;
            // Completing after the read preflight expired cannot turn a timeout
            // into success, even if its independent control tick was delayed.
            if self.shared.window().is_none() && n >= p.preflight_ns {
                self.shared.stop_ns(p.preflight_ns, None);
                return Err(Fault::Deadline);
            }
            let w = self
                .shared
                .window()
                .unwrap_or_else(|| self.shared.stop_ns(end, Some(command.anchor)));
            if n >= w.cooperative_ns || end >= w.cooperative_ns {
                return Err(Fault::Deadline);
            }
            if self.shared.admission.pending() {
                return Err(Fault::Protocol);
            }
            if if command.initialized_session {
                !command.cleanup_attempted || command.cleanup_status != Some(0)
            } else {
                command.cleanup_attempted || command.cleanup_status.is_some()
            } {
                return Err(Fault::Cleanup);
            }
            if self.shared.fault().is_some() {
                return Err(self.shared.fault().unwrap());
            }
            self.finished = true;
            Ok(())
        })();
        if let Err(f) = result {
            self.shared.fail(f);
        }
        result
    }
}
