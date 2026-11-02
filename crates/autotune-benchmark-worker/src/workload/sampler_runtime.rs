//! Common serial sampler startup/cleanup policy. No runtime load or consent.
use super::{bootstrap::EntryClock, wire::WorkerBinding};
use crate::{
    monitor::Anchor,
    sampler::{
        budget::{Budget, Reservation},
        child::NativeAdmission,
        wire_model::string,
    },
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Arguments,
    Startup,
    Capacity,
    Control,
    Clock,
    Deadline,
    Runtime,
    Ready,
    Settings,
    Telemetry,
    Protocol,
    Cleanup,
    Drain,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Open,
    Describe,
    Cleanup,
}
#[derive(Default)]
pub struct Call {
    pub requested: bool,
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
    pub attempted: bool,
    pub returned: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Settings,
    Telemetry,
}
/// Route only; the selected full parser still validates schema/binding/sequence.
pub fn route(bytes: &[u8], budget: &Budget) -> Result<Route, Fault> {
    let p = bytes.get(..4).ok_or(Fault::Protocol)?;
    let len = u32::from_le_bytes(p.try_into().map_err(|_| Fault::Protocol)?) as usize;
    if len != bytes.len() || !(4..=64 * 1024).contains(&len) {
        return Err(Fault::Protocol);
    }
    crate::sampler::wire::validate_complexity_limit(&bytes[4..], 4096)
        .map_err(|_| Fault::Capacity)?;
    let _codec = budget
        .reserve(64 * 1024 * 16)
        .map_err(|_| Fault::Capacity)?;
    let v = crate::evidence::decode(&bytes[4..], 64 * 1024 - 4).map_err(|_| Fault::Protocol)?;
    let o = v.as_object().ok_or(Fault::Protocol)?;
    if o.get("version").and_then(|v| string(v).ok()) != Some("1") {
        return Err(Fault::Protocol);
    }
    match o.get("role").and_then(|v| string(v).ok()) {
        Some("pilot_settings_sampler") => Ok(Route::Settings),
        Some("pilot_sampler") => Ok(Route::Telemetry),
        _ => Err(Fault::Protocol),
    }
}
pub struct Trace {
    binding: WorkerBinding,
    setup_deadline_ns: u64,
    pub history: EntryClock,
    pub calls: [Call; 3],
    pub cleanup_status: Option<i32>,
    pub fault: Option<Fault>,
    pub output_drained: bool,
    pub control_finished: bool,
    pub stop_trigger: Option<Anchor>,
    pub drain_receipt: Option<Anchor>,
    pub setup_complete: Option<Anchor>,
    _metadata: Reservation,
}
impl Trace {
    pub fn new(
        binding: WorkerBinding,
        setup_entry: Anchor,
        first: Anchor,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let entry = binding.elapsed(setup_entry).map_err(|_| Fault::Clock)?;
        let first_ns = binding.elapsed(first).map_err(|_| Fault::Clock)?;
        if entry < binding.origin_ns || first_ns < entry || setup_entry.qpc_after > first.qpc_before
        {
            return Err(Fault::Clock);
        }
        let deadline = entry
            .checked_add(binding.plan.phase_caps_ns[1])
            .ok_or(Fault::Deadline)?
            .min(binding.setup_boundary());
        if first_ns >= deadline {
            return Err(Fault::Deadline);
        }
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let capacity = (binding.plan.envelope_ns / p.controller_tick_ns * 4 + 128) as usize;
        let metadata = budget
            .reserve(512 * 1024 + std::mem::size_of::<Self>())
            .map_err(|_| Fault::Capacity)?;
        let history = EntryClock::with_capacity(
            binding.monitor.qpc_frequency,
            binding.epoch,
            first,
            capacity,
            budget,
        )
        .map_err(|_| Fault::Clock)?;
        Ok(Self {
            binding,
            setup_deadline_ns: deadline,
            history,
            calls: std::array::from_fn(|_| Call::default()),
            cleanup_status: None,
            fault: None,
            output_drained: false,
            control_finished: false,
            stop_trigger: None,
            drain_receipt: None,
            setup_complete: None,
            _metadata: metadata,
        })
    }
    pub fn fail(&mut self, f: Fault, gate: &NativeAdmission) {
        self.fault.get_or_insert(f);
        gate.stop();
    }
    pub fn observe(&mut self, a: Option<Anchor>, gate: &NativeAdmission) -> Result<u64, Fault> {
        let result = self
            .history
            .observe(a)
            .map_err(|e| {
                if e == super::bootstrap::Error::Capacity {
                    Fault::Capacity
                } else {
                    Fault::Clock
                }
            })
            .and_then(|n| {
                if n >= self.binding.run_end() {
                    Err(Fault::Deadline)
                } else {
                    Ok(n)
                }
            });
        if let Err(f) = result {
            self.fail(f, gate);
        }
        result
    }
    pub fn setup(&mut self, a: Option<Anchor>, gate: &NativeAdmission) -> bool {
        match self.observe(a, gate) {
            Ok(n) if n < self.setup_deadline_ns && !gate.stopped() => true,
            Ok(_) => {
                self.fail(Fault::Deadline, gate);
                false
            }
            Err(_) => false,
        }
    }
    pub fn begin(
        &mut self,
        op: Operation,
        a: Option<Anchor>,
        cooperative_raw: u64,
        gate: &NativeAdmission,
    ) -> bool {
        let index = op as usize;
        if self.calls[index].requested {
            self.fail(Fault::Protocol, gate);
            return false;
        }
        self.calls[index].requested = true;
        self.calls[index].before = a;
        let valid = if op == Operation::Cleanup {
            self.observe(a, gate).is_ok()
                && a.is_some_and(|v| v.interrupt_100ns < cooperative_raw)
                && gate.begin_cleanup().is_ok()
        } else {
            self.fault.is_none() && self.setup(a, gate) && gate.begin().is_ok()
        };
        if !valid {
            self.fail(
                if op == Operation::Cleanup {
                    Fault::Cleanup
                } else {
                    Fault::Control
                },
                gate,
            );
            return false;
        }
        self.calls[index].attempted = true;
        true
    }
    /// The caller owns the raw native result BEFORE invoking this method.
    pub fn returned(&mut self, op: Operation, a: Option<Anchor>, gate: &NativeAdmission) {
        let index = op as usize;
        if !self.calls[index].attempted || self.calls[index].returned {
            self.fail(Fault::Protocol, gate);
            return;
        }
        self.calls[index].returned = true;
        self.calls[index].after = a;
        if let Ok(n) = self.observe(a, gate) {
            if op != Operation::Cleanup && n >= self.setup_deadline_ns {
                self.fail(Fault::Deadline, gate);
            }
        }
        gate.complete();
    }
    pub fn cleanup_returned(
        &mut self,
        a: Option<Anchor>,
        status: Option<i32>,
        cooperative_raw: u64,
        gate: &NativeAdmission,
    ) {
        if !self.calls[2].attempted || self.calls[2].returned {
            self.fail(Fault::Protocol, gate);
            return;
        }
        self.cleanup_status = status;
        self.returned(Operation::Cleanup, a, gate);
        if status != Some(0) || a.is_none_or(|v| v.interrupt_100ns >= cooperative_raw) {
            self.fail(Fault::Cleanup, gate);
        }
    }
    /// Poll booleans first, then capture/retain this receipt anchor. This method
    /// never waits and does not treat queue drain as peer receipt/durability.
    pub fn drain(
        &mut self,
        a: Option<Anchor>,
        output: bool,
        control: bool,
        cooperative_raw: u64,
        gate: &NativeAdmission,
    ) -> bool {
        self.output_drained = output;
        self.control_finished = control;
        self.drain_receipt = a;
        if self.observe(a, gate).is_err() || a.is_none_or(|v| v.interrupt_100ns >= cooperative_raw)
        {
            self.fail(Fault::Drain, gate);
            return false;
        }
        output && control
    }
}
