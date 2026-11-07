//! Independent parent deadline policy. Requests and observations are not exit
//! proof, GPU quiescence, human consent or NativeAdmission.
use super::{
    bootstrap::EntryClock, gate::StopWindow, starting_export::ClockFault, wire::WorkerBinding,
};
use crate::{
    monitor::{clock::Clock, Anchor, Metric, Record, Status},
    sampler::budget::{Budget, Reservation},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Binding,
    Clock,
    Capacity,
    SetupDeadline,
    RunDeadline,
    Safety,
    Requested,
    Transport,
    Thread,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub binding: WorkerBinding,
    pub setup_deadline_ns: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Update {
    pub anchor: Anchor,
    pub setup: bool,
    pub safety: [Option<Record>; 2],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub update: Update,
    pub receipt: Option<Anchor>,
    pub result: Result<(), Fault>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub stop: bool,
    pub terminate: bool,
    pub finished: bool,
}
pub struct State {
    window: Window,
    history: EntryClock,
    now: u64,
    last: Anchor,
    receipt: Option<Anchor>,
    setup: bool,
    safety: [Option<Record>; 2],
    fault: Option<Fault>,
    stop: Option<StopWindow>,
    updates: Vec<Receipt>,
    capacity: usize,
    overflow: Option<Receipt>,
    capture_error: Option<ClockFault>,
    stop_sent: bool,
    terminate_sent: bool,
    done: bool,
    _allocation: Reservation,
}
impl State {
    pub fn new(window: Window, first: Anchor, budget: &Budget) -> Result<Self, Fault> {
        let binding = window.binding;
        let now = binding.elapsed(first).map_err(|_| Fault::Clock)?;
        if window.setup_deadline_ns <= binding.origin_ns
            || window.setup_deadline_ns > binding.setup_boundary()
            || now < binding.origin_ns
            || now >= window.setup_deadline_ns
        {
            return Err(Fault::Binding);
        }
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let count = ((binding.plan.envelope_ns / p.cadence_ns + 1) * 6 + 8) as usize;
        let bytes = count * std::mem::size_of::<Receipt>() + std::mem::size_of::<Self>();
        if bytes > p.metadata_bytes as usize {
            return Err(Fault::Capacity);
        }
        let allocation = budget.reserve(bytes).map_err(|_| Fault::Capacity)?;
        let mut updates = Vec::new();
        updates
            .try_reserve_exact(count)
            .map_err(|_| Fault::Capacity)?;
        let history = EntryClock::with_capacity(
            binding.monitor().qpc_frequency,
            binding.epoch,
            first,
            (binding.plan.envelope_ns / p.controller_tick_ns + count as u64 + 128) as usize,
            budget,
        )
        .map_err(|_| Fault::Capacity)?;
        Ok(Self {
            window,
            history,
            now,
            last: first,
            receipt: Some(first),
            setup: false,
            safety: [None; 2],
            fault: None,
            stop: None,
            updates,
            capacity: count,
            overflow: None,
            capture_error: None,
            stop_sent: false,
            terminate_sent: false,
            done: false,
            _allocation: allocation,
        })
    }
    pub fn window(&self) -> Window {
        self.window
    }
    pub fn history(&self) -> &EntryClock {
        &self.history
    }
    pub fn updates(&self) -> &[Receipt] {
        &self.updates
    }
    pub fn overflow(&self) -> Option<Receipt> {
        self.overflow
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn stop_window(&self) -> Option<StopWindow> {
        self.stop
    }
    pub fn setup_observed(&self) -> bool {
        self.setup
    }
    pub fn capture_error(&self) -> Option<ClockFault> {
        self.capture_error
    }
    fn fail(&mut self, fault: Fault, trigger: u64) {
        self.fault.get_or_insert(fault);
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        if self.stop.is_none_or(|w| trigger < w.trigger_ns) {
            self.stop = Some(StopWindow {
                trigger_ns: trigger,
                cooperative_ns: trigger
                    .saturating_add(p.cooperative_stop_ns)
                    .min(self.window.binding.run_end()),
                exit_ns: trigger
                    .saturating_add(p.exit_observation_ns)
                    .min(self.window.binding.run_end()),
            });
        }
    }
    pub fn tick(&mut self, anchor: Option<Anchor>, error: Option<ClockFault>) -> Effects {
        if self.done {
            return Effects {
                finished: true,
                ..Effects::default()
            };
        }
        self.receipt = anchor;
        self.capture_error = self.capture_error.or(error);
        let observed = self.history.observe(anchor);
        if error.is_some() || observed.is_err() {
            self.fail(Fault::Clock, self.now);
            // Unknown current time cannot buy cooperative grace.
            return self.effects(true);
        }
        self.now = observed.expect("checked clock");
        self.last = anchor.expect("observed anchor");
        if self.now >= self.window.binding.run_end() {
            self.fail(Fault::RunDeadline, self.window.binding.run_end());
        }
        if !self.setup && self.now >= self.window.setup_deadline_ns {
            self.fail(Fault::SetupDeadline, self.window.setup_deadline_ns);
        }
        if self.setup {
            let gap = autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .observation_gap_ns;
            let deadline = self
                .safety
                .iter()
                .flatten()
                .map(|r| r.end_ns.saturating_add(gap))
                .min();
            if self.safety.iter().any(Option::is_none) {
                self.fail(Fault::Safety, self.now);
            } else if let Some(deadline) = deadline {
                if self.now >= deadline {
                    self.fail(Fault::Safety, deadline);
                }
            }
        }
        self.effects(false)
    }
    pub fn update(&mut self, update: Update) -> Result<(), Fault> {
        let result = (|| {
            if self.fault.is_some() || self.stop.is_some() {
                return Err(Fault::Requested);
            }
            let binding = self.window.binding;
            let mut clock =
                Clock::new(binding.monitor(), binding.epoch).map_err(|_| Fault::Clock)?;
            let issued = clock.observe(update.anchor).map_err(|_| Fault::Clock)?;
            if update.anchor.qpc_after > self.last.qpc_before || issued > self.now {
                return Err(Fault::Clock);
            }
            if update.setup && (self.setup || self.now >= self.window.setup_deadline_ns) {
                return Err(Fault::SetupDeadline);
            }
            if !update.setup && !self.setup {
                return Err(Fault::Binding);
            }
            let p = autotune_fma_reference::profile::Profile::approved_design().limits;
            for (i, raw) in update.safety.iter().enumerate() {
                let r = raw.ok_or(Fault::Safety)?;
                let metric = [Metric::Temperature, Metric::Power][i];
                let limit = if i == 0 {
                    p.temperature_stop_degc
                } else {
                    p.power_stop_mw as i64
                };
                if r.ticket.binding != binding.monitor()
                    || r.ticket.metric != metric
                    || r.unit != metric.unit()
                    || r.status != Status::Available
                    || r.value.is_none_or(|v| v < 0 || v >= limit)
                    || r.ticket.start_ns > r.end_ns
                    || r.end_ns > r.receipt_ns
                    || r.receipt_ns > issued
                    || r.end_ns - r.ticket.start_ns > p.query_bracket_ns
                    || r.receipt_ns - r.end_ns > p.transport_age_ns
                    || self.now.saturating_sub(r.end_ns) >= p.observation_gap_ns
                    || self.safety[i]
                        .is_some_and(|old| r.end_ns < old.end_ns || r.ticket.slot < old.ticket.slot)
                {
                    return Err(Fault::Safety);
                }
            }
            Ok(())
        })();
        let receipt = Receipt {
            update,
            receipt: self.receipt,
            result,
        };
        if self.updates.len() == self.capacity {
            if self.overflow.is_none() {
                self.overflow = Some(receipt);
            }
            self.fail(Fault::Capacity, self.now);
            return Err(Fault::Capacity);
        }
        self.updates.push(receipt);
        if let Err(f) = result {
            self.fail(f, self.now);
            return Err(f);
        }
        self.setup |= update.setup;
        self.safety = update.safety;
        Ok(())
    }
    pub fn request_stop(&mut self, window: StopWindow) -> Effects {
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        if window.trigger_ns > self.now
            || window.cooperative_ns
                != window
                    .trigger_ns
                    .saturating_add(p.cooperative_stop_ns)
                    .min(self.window.binding.run_end())
            || window.exit_ns
                != window
                    .trigger_ns
                    .saturating_add(p.exit_observation_ns)
                    .min(self.window.binding.run_end())
        {
            return self.abort(Fault::Binding);
        }
        self.fail(Fault::Requested, window.trigger_ns);
        self.effects(false)
    }
    pub fn abort(&mut self, fault: Fault) -> Effects {
        self.fail(fault, self.now);
        self.effects(true)
    }
    fn effects(&mut self, immediate: bool) -> Effects {
        let Some(w) = self.stop else {
            return Effects::default();
        };
        let stop = !self.stop_sent;
        self.stop_sent = true;
        let terminate = !self.terminate_sent && (immediate || self.now >= w.cooperative_ns);
        self.terminate_sent |= terminate;
        // Completing this controller is explicitly not an exit observation.
        self.done |= immediate || self.now >= w.exit_ns;
        Effects {
            stop,
            terminate,
            finished: self.done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        evidence::{Digest, Domain},
        monitor::Ticket,
        workload::manifest::{Artifacts, PilotDraft, PilotManifest},
    };
    fn a(ns: u64) -> Anchor {
        Anchor {
            clock_token: [5; 16],
            qpc_before: ns,
            qpc_after: ns,
            interrupt_100ns: ns / 100,
            unbiased_100ns: ns / 100,
        }
    }
    fn state() -> (State, Budget) {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let d = Digest::of(Domain::RawArtifact, b"CPU parent watchdog model");
        let m = PilotManifest::freeze(
            PilotDraft {
                session: [1; 16],
                runs: [[2; 16], [3; 16], [4; 16]],
                clock_token: [5; 16],
                raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
                raw_pci: "0000:01:02.0".into(),
                source_commit: "a".repeat(40),
                artifacts: Artifacts {
                    source_tree: d,
                    lockfile: d,
                    controller: d,
                    worker: d,
                    sampler: d,
                    ptx: d,
                    protocol: d,
                },
                starting_evidence: d,
                environment_evidence: d,
                approval_ref: "model has no execution authority".into(),
                output_root: "D:\\capture\\pilot".into(),
            },
            &b,
        )
        .unwrap();
        let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
        let x = State::new(
            Window {
                binding,
                setup_deadline_ns: 10_000_000_000,
            },
            a(0),
            &b,
        )
        .unwrap();
        drop(m);
        (x, b)
    }
    fn update(state: &State, ns: u64, setup: bool) -> Update {
        Update {
            anchor: a(ns + 100),
            setup,
            safety: std::array::from_fn(|i| {
                let metric = [Metric::Temperature, Metric::Power][i];
                Some(Record {
                    ticket: Ticket {
                        binding: state.window.binding.monitor(),
                        metric,
                        slot: ns / 250_000_000,
                        scheduled_ns: ns,
                        start_ns: ns,
                    },
                    end_ns: ns,
                    receipt_ns: ns,
                    unit: metric.unit(),
                    status: Status::Available,
                    value: Some(if i == 0 { 50 } else { 100_000 }),
                })
            }),
        }
    }
    fn setup(state: &mut State, ns: u64) {
        let u = update(state, ns, true);
        assert_eq!(state.tick(Some(a(ns + 200)), None), Effects::default());
        state.update(u).unwrap();
    }
    #[test]
    fn setup_expiry_preserves_original_one_and_three_second_windows() {
        let (mut s, _) = state();
        let d = s.window.setup_deadline_ns;
        assert_eq!(
            s.tick(Some(a(d)), None),
            Effects {
                stop: true,
                terminate: false,
                finished: false
            }
        );
        assert_eq!(
            s.stop_window(),
            Some(StopWindow {
                trigger_ns: d,
                cooperative_ns: d + 1_000_000_000,
                exit_ns: d + 3_000_000_000
            })
        );
        assert_eq!(
            s.tick(Some(a(d + 1_000_000_000)), None),
            Effects {
                stop: false,
                terminate: true,
                finished: false
            }
        );
        assert_eq!(
            s.tick(Some(a(d + 3_000_000_000)), None),
            Effects {
                stop: false,
                terminate: false,
                finished: true
            }
        );
        assert_eq!(s.fault(), Some(Fault::SetupDeadline));
    }
    #[test]
    fn late_tick_requests_both_operations_without_new_grace() {
        let (mut s, _) = state();
        let d = s.window.setup_deadline_ns;
        assert_eq!(
            s.tick(Some(a(d + 5_000_000_000)), None),
            Effects {
                stop: true,
                terminate: true,
                finished: true
            }
        );
        assert_eq!(s.stop_window().unwrap().trigger_ns, d);
    }
    #[test]
    fn stop_repeats_cannot_extend_and_earlier_original_window_only_tightens() {
        let (mut s, _) = state();
        s.tick(Some(a(5_000_000_000)), None);
        let w = StopWindow {
            trigger_ns: 4_000_000_000,
            cooperative_ns: 5_000_000_000,
            exit_ns: 7_000_000_000,
        };
        assert!(s.request_stop(w).terminate);
        s.request_stop(StopWindow {
            trigger_ns: 5_000_000_000,
            cooperative_ns: 6_000_000_000,
            exit_ns: 8_000_000_000,
        });
        assert_eq!(s.stop_window(), Some(w));
        s.request_stop(StopWindow {
            trigger_ns: 1_000_000_000,
            cooperative_ns: 2_000_000_000,
            exit_ns: 4_000_000_000,
        });
        assert_eq!(s.stop_window().unwrap().trigger_ns, 1_000_000_000);
    }
    #[test]
    fn missing_clock_keeps_unknown_receipt_and_forces_without_grace() {
        let (mut s, _) = state();
        s.tick(Some(a(1000)), None);
        let e = ClockFault::CounterFailed {
            before: true,
            code: 87,
        };
        assert_eq!(
            s.tick(None, Some(e)),
            Effects {
                stop: true,
                terminate: true,
                finished: true
            }
        );
        let u = update(&s, 1000, true);
        assert_eq!(s.update(u), Err(Fault::Requested));
        assert_eq!(s.updates().last().unwrap().receipt, None);
        assert_eq!(s.capture_error(), Some(e));
        assert!(s.history().missing());
    }
    #[test]
    fn setup_handoff_uses_fresh_safety_then_expires_at_the_original_sample_gap() {
        let (mut s, _) = state();
        setup(&mut s, 1_000_000_000);
        assert!(s.setup_observed());
        assert_eq!(s.tick(Some(a(1_749_999_900)), None), Effects::default());
        assert!(s.tick(Some(a(1_750_000_000)), None).stop);
        assert_eq!(s.fault(), Some(Fault::Safety));
        assert_eq!(s.stop_window().unwrap().trigger_ns, 1_750_000_000);
    }
    #[test]
    fn valid_new_samples_extend_safety_only_and_do_not_rebase_run_end() {
        let (mut s, _) = state();
        setup(&mut s, 250_000_000);
        let end = s.window.binding.run_end();
        for ns in (500_000_000..end).step_by(250_000_000) {
            let u = update(&s, ns, false);
            assert_eq!(s.tick(Some(a(ns + 200)), None), Effects::default());
            s.update(u).unwrap();
        }
        assert!(s.tick(Some(a(end)), None).terminate);
        assert_eq!(s.fault(), Some(Fault::RunDeadline));
        assert_eq!(s.stop_window().unwrap().trigger_ns, end);
    }
    #[test]
    fn malformed_safety_and_future_commands_are_retained_and_rejected() {
        for mode in 0..10 {
            let (mut s, _) = state();
            let mut u = update(&s, 1000, true);
            s.tick(Some(a(1200)), None);
            let mut r = u.safety[0].unwrap();
            match mode {
                0 => u.safety[0] = None,
                1 => {
                    r.value = Some(75);
                    u.safety[0] = Some(r)
                }
                2 => {
                    r.status = Status::Unavailable;
                    u.safety[0] = Some(r)
                }
                3 => {
                    r.ticket.metric = Metric::Power;
                    u.safety[0] = Some(r)
                }
                4 => {
                    r.ticket.binding.run = [9; 16];
                    u.safety[0] = Some(r)
                }
                5 => {
                    r.receipt_ns = 1200;
                    u.safety[0] = Some(r)
                }
                6 => {
                    r.ticket.start_ns = 1100;
                    u.safety[0] = Some(r)
                }
                7 => {
                    r.value = Some(-1);
                    u.safety[0] = Some(r)
                }
                8 => u.anchor = a(1300),
                _ => {
                    r.unit = Metric::Power.unit();
                    u.safety[0] = Some(r)
                }
            }
            assert!(s.update(u).is_err());
            assert_eq!(s.updates()[0].update, u);
            assert!(!s.setup_observed());
        }
    }
    #[test]
    fn expired_or_repeated_setup_and_pre_setup_updates_fail_closed() {
        let (mut s, _) = state();
        let u = update(&s, 1000, false);
        s.tick(Some(a(1200)), None);
        assert_eq!(s.update(u), Err(Fault::Binding));
        let (mut s, _) = state();
        setup(&mut s, 1000);
        let u = update(&s, 2000, true);
        s.tick(Some(a(2200)), None);
        assert_eq!(s.update(u), Err(Fault::SetupDeadline));
        let (mut s, _) = state();
        let u = update(&s, 1000, true);
        s.tick(Some(a(10_000_000_000)), None);
        assert_eq!(s.update(u), Err(Fault::Requested));
    }
    #[test]
    fn old_values_cannot_repair_stopped_or_regressing_safety() {
        let (mut s, _) = state();
        setup(&mut s, 500_000_000);
        let u = update(&s, 250_000_000, false);
        s.tick(Some(a(600_000_000)), None);
        assert_eq!(s.update(u), Err(Fault::Safety));
        let good = update(&s, 600_000_000, false);
        assert_eq!(s.update(good), Err(Fault::Requested));
        assert_eq!(s.fault(), Some(Fault::Safety));
    }
    #[test]
    fn clock_regression_token_suspend_and_wide_bracket_never_renew_windows() {
        for mode in 0..4 {
            let (mut s, _) = state();
            s.tick(Some(a(1000)), None);
            let mut bad = a(2000);
            match mode {
                0 => bad = a(900),
                1 => bad.clock_token = [6; 16],
                2 => bad.unbiased_100ns += 100_000,
                _ => bad.qpc_after += 1_000_001,
            }
            assert!(s.tick(Some(bad), None).terminate);
            assert_eq!(s.fault(), Some(Fault::Clock));
        }
    }
    #[test]
    fn fixed_capacity_preserves_first_overflow_and_releases_all_reservations() {
        let (mut s, b) = state();
        setup(&mut s, 1000);
        let mut u = update(&s, 1000, false);
        while s.updates.len() < s.capacity {
            s.update(u).unwrap();
        }
        assert_eq!(s.update(u), Err(Fault::Capacity));
        let first = s.overflow();
        u.anchor = a(1100);
        assert_eq!(s.update(u), Err(Fault::Capacity));
        assert_eq!(s.overflow(), first);
        assert!(b.used() > 0);
        drop(s);
        assert_eq!(b.used(), 0);
    }
    #[test]
    fn malformed_stop_window_cannot_create_renewable_grace() {
        let (mut s, _) = state();
        s.tick(Some(a(1000)), None);
        assert_eq!(
            s.request_stop(StopWindow {
                trigger_ns: 2000,
                cooperative_ns: 3_000_000_000,
                exit_ns: 5_000_000_000
            }),
            Effects {
                stop: true,
                terminate: true,
                finished: true
            }
        );
        assert_eq!(s.fault(), Some(Fault::Binding));
    }
    #[test]
    fn racing_stop_publication_needs_its_actual_post_publication_receipt_clock() {
        let (mut s, _) = state();
        s.tick(Some(a(1000)), None);
        let published = StopWindow {
            trigger_ns: 2000,
            cooperative_ns: 1_000_002_000,
            exit_ns: 3_000_002_000,
        };
        s.tick(Some(a(3000)), None);
        assert_eq!(
            s.request_stop(published),
            Effects {
                stop: true,
                terminate: false,
                finished: false
            }
        );
        assert_eq!(s.fault(), Some(Fault::Requested));
        assert_eq!(s.stop_window(), Some(published));
        assert_eq!(s.history().raw().last(), Some(&a(3000)));
    }
}
