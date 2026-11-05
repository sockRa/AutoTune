//! One-shot disposition of a successfully prepared, never-started owner.
//! No compute admission, new grace, partial-setup guessing or allocation.
use super::{
    control::Shared,
    gate::StopWindow,
    producer::{Cleanup, Driver, Fault},
    teardown::{self, Check},
    wire::WorkerBinding,
};
use crate::monitor::{clock::Clock, Anchor};
use std::sync::Arc;
pub struct Report {
    pub binding: Option<WorkerBinding>,
    pub seed: Option<Anchor>,
    pub window: Option<StopWindow>,
    pub cleanup: Cleanup,
    pub ledger: teardown::Ledger,
    pub fault: Option<Fault>,
    requested: bool,
}
impl Default for Report {
    fn default() -> Self {
        Self {
            binding: None,
            seed: None,
            window: None,
            cleanup: Cleanup {
                attempted: false,
                before: None,
                after: None,
                result: None,
            },
            ledger: teardown::Ledger::default(),
            fault: None,
            requested: false,
        }
    }
}
impl Report {
    pub fn requested(&self) -> bool {
        self.requested
    }
    pub(super) fn reject(&mut self, binding: WorkerBinding, seed: Option<Anchor>, fault: Fault) {
        if self.requested {
            return;
        }
        self.requested = true;
        self.binding = Some(binding);
        self.seed = seed;
        self.fault = Some(fault);
    }
    /// Caller must retain a successful setup owner with no prior dispatch.
    /// Actual CUDA close also checks returned idle and forbids any retry.
    pub(super) fn run<B: Driver, F: FnMut() -> Check>(
        &mut self,
        binding: WorkerBinding,
        shared: Arc<Shared>,
        driver: &mut B,
        seed: Option<Anchor>,
        mut capture: F,
    ) {
        if self.requested {
            return;
        }
        self.requested = true;
        self.binding = Some(binding);
        self.seed = seed;
        if !shared.matches(binding)
            || driver.identity() != binding.monitor().device
            || driver.shape() != binding.plan.shape
        {
            self.fault = Some(Fault::Binding);
            return;
        }
        self.window = shared.stop_window(binding).ok().flatten();
        let Some(window) = self.window else {
            self.fault = Some(Fault::Admission);
            return;
        };
        if !shared.admission().stopped() || shared.admission().pending() {
            self.fault = Some(Fault::Admission);
            return;
        }
        let Some(mut last) = seed else {
            self.fault = Some(Fault::Clock);
            return;
        };
        let mut validator = match Clock::new(binding.monitor(), binding.epoch) {
            Ok(v) => v,
            Err(_) => {
                self.fault = Some(Fault::Clock);
                return;
            }
        };
        if last.qpc_before < binding.epoch.qpc_after || validator.observe(last).is_err() {
            self.fault = Some(Fault::Clock);
            return;
        }
        let mut check = || {
            let mut raw = capture();
            let checked = raw.anchor.ok_or(teardown::Fault::Clock).and_then(|a| {
                if a.qpc_before < last.qpc_after {
                    return Err(teardown::Fault::Clock);
                }
                let n = validator.observe(a).map_err(|_| teardown::Fault::Clock)?;
                last = a;
                if n < window.trigger_ns {
                    return Err(teardown::Fault::Protocol);
                }
                if n >= window.cooperative_ns
                    || n >= window.exit_ns
                    || n >= binding.run_end()
                    || a.interrupt_100ns >= shared.cooperative_raw()
                {
                    return Err(teardown::Fault::Deadline);
                }
                if shared.stop_window(binding).is_err() {
                    return Err(teardown::Fault::Protocol);
                }
                Ok(())
            });
            if raw.fault.is_none() {
                raw.fault = checked.err();
            }
            raw
        };
        let before = check();
        self.cleanup.before = before.anchor;
        if let Some(f) = before.fault {
            self.fault = Some(mapped(f));
            return;
        }
        self.cleanup.attempted = true;
        // Both the ledger and summary live in the caller's prepaid report before
        // callbacks/native entry; a post-return unwind cannot erase raw returns.
        let result =
            driver.close(&mut self.ledger.controlled(&mut check, Some(shared.admission())));
        self.cleanup.result = Some(result);
        let after = check();
        self.cleanup.after = after.anchor;
        self.ledger.finish(result.is_ok(), after);
        self.fault = self
            .ledger
            .fault()
            .map(mapped)
            .or_else(|| result.err().map(Fault::Native));
    }
}
fn mapped(f: teardown::Fault) -> Fault {
    match f {
        teardown::Fault::Clock => Fault::Clock,
        teardown::Fault::Deadline => Fault::Deadline,
        teardown::Fault::Protocol => Fault::Protocol,
        _ => Fault::Native(crate::worker::Fault {
            operation: "prestart teardown",
            raw_status: match f {
                teardown::Fault::Native(n) => n,
                _ => -2,
            },
        }),
    }
}
