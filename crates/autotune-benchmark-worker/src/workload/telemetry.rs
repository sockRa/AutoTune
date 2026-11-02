//! Pilot-bound sequential telemetry. No runtime loading or compute authority.
use super::{
    manifest::BoundRun,
    settings,
    transport::{Queue, Sender, WorkerReceived},
    wire::{Channel, EncodedFrame, Message, WorkerBinding},
};
use crate::{
    evidence::Value,
    monitor::{clock::Clock, Anchor, Metric, Record, Status, Ticket},
    sampler::{
        self,
        budget::{Budget, Reservation},
        child::NativeAdmission,
        wire::{Lane, WireError},
        wire_model::{exact, get, object, string, uint},
    },
};
use autotune_probe::{
    nvml::NvmlSampler,
    report::{Field, Reading},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Capacity,
    Protocol,
    Clock,
    Deadline,
    Stopped,
    Replay,
    Identity,
    Value,
    DeviceLost,
    Safety,
    Send,
    Simulated,
    Transport,
}
pub(super) fn field_bytes(f: &Field) -> usize {
    f.unit.capacity()
        + match &f.reading {
            Reading::Available {
                value: serde_json::Value::String(s),
            } => s.capacity(),
            Reading::Unavailable { failure } => failure.detail.capacity(),
            _ => 0,
        }
}
pub(super) fn ticket_value(t: Ticket) -> Value {
    object([
        ("metric", Value::Text(metric_name(t.metric).into())),
        ("slot", Value::U64(t.slot)),
        ("scheduled_ns", Value::U64(t.scheduled_ns)),
        ("issued_ns", Value::U64(t.start_ns)),
    ])
}
fn metric_name(m: Metric) -> &'static str {
    match m {
        Metric::Temperature => "temperature",
        Metric::Power => "power",
        Metric::GraphicsClock => "graphics_clock",
        Metric::MemoryClock => "memory_clock",
        Metric::GpuUtilization => "gpu_utilization",
        Metric::MemoryUtilization => "memory_utilization",
    }
}
pub(super) fn parse_ticket(v: &Value, b: WorkerBinding) -> Result<Ticket, WireError> {
    let o = exact(v, &["metric", "slot", "scheduled_ns", "issued_ns"])?;
    let name = string(get(o, "metric"))?;
    Ok(Ticket {
        binding: b.monitor,
        metric: Metric::ALL
            .into_iter()
            .find(|m| metric_name(*m) == name)
            .ok_or(WireError::Body)?,
        slot: uint(get(o, "slot"))?,
        scheduled_ns: uint(get(o, "scheduled_ns"))?,
        start_ns: uint(get(o, "issued_ns"))?,
    })
}
pub(super) fn validate_ticket(t: Ticket, b: WorkerBinding) -> Result<(), WireError> {
    let cadence = autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .cadence_ns;
    if t.binding != b.monitor
        || t.slot.checked_mul(cadence) != Some(t.scheduled_ns)
        || t.start_ns < t.scheduled_ns
        || t.start_ns >= t.scheduled_ns.saturating_add(cadence)
        || t.start_ns < b.origin_ns
        || t.start_ns >= b.run_end()
    {
        return Err(WireError::Binding);
    }
    Ok(())
}
fn observe(clock: &mut Clock, last: &mut Anchor, a: Option<Anchor>) -> Result<u64, Error> {
    let a = a.ok_or(Error::Clock)?;
    if a.qpc_before < last.qpc_after {
        return Err(Error::Clock);
    }
    let n = clock.observe(a).map_err(|_| Error::Clock)?;
    *last = a;
    Ok(n)
}
fn map(b: WorkerBinding, t: Ticket, end: u64, receipt: u64, raw: &Field) -> Result<Record, Error> {
    validate_ticket(t, b).map_err(|_| Error::Protocol)?;
    let r = sampler::convert(t, end, receipt, raw).map_err(|e| {
        if e == sampler::MappingError::DeviceLost {
            Error::DeviceLost
        } else {
            Error::Value
        }
    })?;
    if r.status == Status::Available
        && r.value.is_none_or(|v| {
            v < 0
                || (matches!(t.metric, Metric::GpuUtilization | Metric::MemoryUtilization)
                    && v > 100)
        })
    {
        return Err(Error::Value);
    }
    Ok(r)
}
pub struct Entry {
    pub request: WorkerReceived,
    pub response: Option<Message>,
    pub unsent: Option<EncodedFrame>,
    pub queued: bool,
    pub fault: Option<Error>,
    pub send_error: Option<super::transport::SendFailure>,
    pub codec_error: Option<WireError>,
    _allocation: Option<Reservation>,
}
pub struct Output<'a, Q> {
    pub channel: &'a mut Channel,
    pub sender: &'a mut Sender<Q>,
}
pub struct Collector {
    binding: WorkerBinding,
    history: super::bootstrap::EntryClock,
    last_slots: [Option<u64>; 6],
    entries: Vec<Entry>,
    capacity: usize,
    fault: Option<Error>,
    budget: Budget,
    _metadata: Reservation,
}
impl Collector {
    pub fn new(binding: WorkerBinding, first: Anchor, budget: Budget) -> Result<Self, Error> {
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let history_capacity = (binding.plan.envelope_ns / p.controller_tick_ns
            + 3 * (binding.plan.envelope_ns / p.cadence_ns + 1) * 6
            + 16) as usize;
        let history = super::bootstrap::EntryClock::with_capacity(
            binding.monitor.qpc_frequency,
            binding.epoch,
            first,
            history_capacity,
            &budget,
        )
        .map_err(|e| {
            if e == super::bootstrap::Error::Capacity {
                Error::Capacity
            } else {
                Error::Clock
            }
        })?;
        let n = binding.elapsed(first).map_err(|_| Error::Clock)?;
        if n < binding.origin_ns || n >= binding.run_end() {
            return Err(Error::Deadline);
        }
        let p = autotune_fma_reference::profile::Profile::approved_design().limits;
        let capacity = (binding.plan.envelope_ns / p.cadence_ns + 1) as usize * 6;
        let bytes = capacity
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(Error::Capacity)?;
        if bytes + history_capacity * std::mem::size_of::<Anchor>() > p.metadata_bytes as usize {
            return Err(Error::Capacity);
        }
        let metadata = budget
            .reserve(bytes + std::mem::size_of::<Self>())
            .map_err(|_| Error::Capacity)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_| Error::Capacity)?;
        Ok(Self {
            binding,
            history,
            last_slots: [None; 6],
            entries,
            capacity,
            fault: None,
            budget,
            _metadata: metadata,
        })
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn fault(&self) -> Option<Error> {
        self.fault
    }
    pub fn history(&self) -> &super::bootstrap::EntryClock {
        &self.history
    }
    fn observe(&mut self, a: Option<Anchor>) -> Result<u64, Error> {
        self.history.observe(a).map_err(|e| {
            if e == super::bootstrap::Error::Capacity {
                Error::Capacity
            } else {
                Error::Clock
            }
        })
    }
    pub fn idle(&mut self, a: Option<Anchor>, admission: &NativeAdmission) -> bool {
        let result = self.observe(a).and_then(|n| {
            if n >= self.binding.run_end() {
                Err(Error::Deadline)
            } else if admission.stopped() {
                Err(Error::Stopped)
            } else {
                Ok(())
            }
        });
        if let Err(e) = result {
            self.fault.get_or_insert(e);
            admission.stop();
        }
        self.fault.is_none()
    }
    fn fail(&mut self, e: Error, admission: &NativeAdmission) {
        self.fault.get_or_insert(e);
        admission.stop();
        if let Some(entry) = self.entries.last_mut() {
            entry.fault.get_or_insert(e);
        }
    }
    /// Uses only the already selected sampler; no load, enumeration or parallel
    /// replacement query. The independent control owner shares admission.
    #[allow(clippy::result_large_err)]
    pub fn native<Q: Queue>(
        &mut self,
        run: &BoundRun<'_>,
        request: WorkerReceived,
        sampler: &NvmlSampler,
        admission: &NativeAdmission,
        clock: impl FnMut() -> Option<Anchor>,
        out: Output<'_, Q>,
    ) -> Result<(), WorkerReceived> {
        let identity = sampler.identity();
        let binding = self.binding;
        self.collect(
            request,
            admission,
            clock,
            out,
            true,
            || {
                if WorkerBinding::new(run, binding.monitor.qpc_frequency, binding.epoch).ok()
                    != Some(binding)
                    || identity != Some((run.raw_uuid(), run.raw_pci()))
                {
                    return Err(Error::Identity);
                }
                Ok(())
            },
            |metric| sampler.sample(sampler::sample_metric(metric)),
        )
    }
    /// Explicit simulation source, incapable of becoming a native report.
    #[allow(clippy::result_large_err)]
    pub fn fixture<Q: Queue>(
        &mut self,
        request: WorkerReceived,
        raw: &Field,
        admission: &NativeAdmission,
        clock: impl FnMut() -> Option<Anchor>,
        out: Output<'_, Q>,
    ) -> Result<(), WorkerReceived> {
        if !settings::bounded(raw) {
            self.fault.get_or_insert(Error::Capacity);
            admission.stop();
            return Err(request);
        }
        self.collect(
            request,
            admission,
            clock,
            out,
            false,
            || Ok(()),
            |_| raw.clone(),
        )
    }
    #[allow(clippy::result_large_err, clippy::too_many_arguments)]
    fn collect<Q: Queue>(
        &mut self,
        request: WorkerReceived,
        admission: &NativeAdmission,
        mut clock: impl FnMut() -> Option<Anchor>,
        out: Output<'_, Q>,
        native: bool,
        identity: impl FnOnce() -> Result<(), Error>,
        read: impl FnOnce(Metric) -> Field,
    ) -> Result<(), WorkerReceived> {
        if self.fault.is_some() || self.entries.len() == self.capacity {
            self.fault.get_or_insert(Error::Capacity);
            admission.stop();
            return Err(request);
        }
        self.entries.push(Entry {
            request,
            response: None,
            unsent: None,
            queued: false,
            fault: None,
            send_error: None,
            codec_error: None,
            _allocation: None,
        });
        let idx = self.entries.len() - 1;
        // All raw/native-return staging is reserved before entering a getter.
        let allocation = match self.budget.reserve(64 * 1024) {
            Ok(a) => a,
            Err(_) => {
                self.fail(Error::Capacity, admission);
                return Ok(());
            }
        };
        self.entries[idx]._allocation = Some(allocation);
        let query = (|| {
            identity()?;
            let request = &self.entries[idx].request;
            if request.binding() != Ok(self.binding) {
                return Err(Error::Protocol);
            }
            let (ticket, issued) = match request.message() {
                Ok(Message::SamplerQuery { ticket, issued }) => (*ticket, *issued),
                _ => return Err(Error::Protocol),
            };
            validate_ticket(ticket, self.binding).map_err(|_| Error::Protocol)?;
            let receipt = request.raw().receipt();
            let received = self.observe(receipt)?;
            if receipt.is_none_or(|a| issued.qpc_after > a.qpc_before)
                || self.binding.elapsed(issued).map_err(|_| Error::Clock)? != ticket.start_ns
                || received < ticket.start_ns
            {
                return Err(Error::Clock);
            }
            let previous = &mut self.last_slots[ticket.metric as usize];
            if previous.is_some_and(|p| ticket.slot <= p) {
                return Err(Error::Replay);
            }
            *previous = Some(ticket.slot);
            let before = clock();
            self.entries[idx].response = Some(Message::SamplerRecord {
                ticket,
                before,
                after: None,
                raw: None,
                native,
            });
            let now = self.observe(before)?;
            let p = autotune_fma_reference::profile::Profile::approved_design().limits;
            if now >= self.binding.run_end()
                || now >= ticket.start_ns.saturating_add(p.query_bracket_ns)
            {
                return Err(Error::Deadline);
            }
            admission.begin().map_err(|_| Error::Stopped)?;
            let raw = read(ticket.metric);
            // Own the raw return BEFORE post-call clock capture/validation.
            if let Some(Message::SamplerRecord { raw: slot, .. }) = &mut self.entries[idx].response
            {
                *slot = Some(raw);
            }
            let after = clock();
            admission.complete();
            if let Some(Message::SamplerRecord { after: slot, .. }) =
                &mut self.entries[idx].response
            {
                *slot = after;
            }
            let end = self.observe(after)?;
            if end >= self.binding.run_end()
                || end.saturating_sub(ticket.start_ns) > p.query_bracket_ns
            {
                return Err(Error::Deadline);
            }
            let raw = match self.entries[idx].response.as_ref() {
                Some(Message::SamplerRecord { raw: Some(raw), .. }) => raw,
                _ => unreachable!(),
            };
            if !settings::bounded(raw) {
                return Err(Error::Capacity);
            }
            let mapped = map(self.binding, ticket, end, end, raw)?;
            if ticket.metric.safety() && mapped.status != Status::Available {
                return Err(Error::Value);
            }
            if (ticket.metric == Metric::Temperature
                && mapped.value.is_some_and(|v| v >= p.temperature_stop_degc))
                || (ticket.metric == Metric::Power
                    && mapped.value.is_some_and(|v| v >= p.power_stop_mw as i64))
            {
                return Err(Error::Safety);
            }
            if ticket.metric != Metric::MemoryUtilization && mapped.status != Status::Available {
                return Err(Error::Value);
            }
            if admission.stopped() {
                return Err(Error::Stopped);
            }
            Ok(())
        })();
        if let Err(e) = query {
            self.fail(e, admission);
        }
        // Even an in-flight return after stop/clock loss remains exportable.
        if let Some(message) = self.entries[idx].response.as_ref() {
            match out.channel.encode(message) {
                Ok(frame) => match out.sender.send_worker(frame) {
                    Ok(()) => self.entries[idx].queued = true,
                    Err(frame) => {
                        self.entries[idx].unsent = Some(frame);
                        self.entries[idx].send_error = out.sender.fault(Lane::Ordinary);
                        self.fail(Error::Send, admission);
                    }
                },
                Err(e) => {
                    self.entries[idx].codec_error = Some(e);
                    self.fail(Error::Send, admission);
                }
            }
            // Keep a compact charge for bounded scalar fields after codec work.
            let bytes = self.entries[idx]
                .response
                .as_ref()
                .and_then(|m| {
                    if let Message::SamplerRecord { raw: Some(f), .. } = m {
                        Some(field_bytes(f))
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            if bytes <= 64 * 1024 {
                let _ = self.entries[idx]
                    ._allocation
                    .as_mut()
                    .expect("staging retained")
                    .shrink_to(bytes);
            }
        } else {
            self.entries[idx]._allocation = None;
        }
        Ok(())
    }
}
/// Parent-side retained response plus a conservative monitor conversion. A
/// reported native bit is untrusted provenance, never consent or qualification.
pub struct Receipt {
    pub raw: WorkerReceived,
    pub record: Result<Record, Error>,
}
#[cfg(windows)]
pub struct Poll {
    pub anchor: Option<Anchor>,
    pub os_code: Option<i32>,
    pub transport_fault: bool,
    pub rejected: Option<WorkerReceived>,
}
#[cfg(windows)]
impl Collector {
    /// Actual ordinary pipe -> strict query -> selected serial getter -> owned
    /// response handoff. The caller retains every Poll and typed capture error.
    #[allow(clippy::too_many_arguments)]
    pub fn poll_native<Q: Queue>(
        &mut self,
        run: &BoundRun<'_>,
        transport: &super::windows_transport::InheritedTransport,
        rx: &mut Channel,
        sampler: &NvmlSampler,
        admission: &NativeAdmission,
        mut clock: impl FnMut() -> Option<Anchor>,
        out: Output<'_, Q>,
    ) -> Poll {
        use super::windows_transport::Receive;
        match transport.receive(&mut clock) {
            Receive::Packet(raw) => {
                let anchor = raw.receipt();
                let rejected = self
                    .native(run, raw.decode_worker(rx), sampler, admission, clock, out)
                    .err();
                Poll {
                    anchor,
                    rejected,
                    os_code: None,
                    transport_fault: false,
                }
            }
            Receive::Idle { anchor } => {
                self.idle(anchor, admission);
                Poll {
                    anchor,
                    rejected: None,
                    os_code: None,
                    transport_fault: false,
                }
            }
            Receive::Fault { code, anchor } => {
                if let Err(e) = self.observe(anchor) {
                    self.fault.get_or_insert(e);
                }
                self.fault.get_or_insert(Error::Transport);
                admission.stop();
                Poll {
                    anchor,
                    rejected: None,
                    os_code: code,
                    transport_fault: true,
                }
            }
        }
    }
}
impl Receipt {
    pub fn assess(
        binding: WorkerBinding,
        expected: Ticket,
        issued: Anchor,
        raw: WorkerReceived,
    ) -> Self {
        let record = (|| {
            if raw.binding() != Ok(binding) {
                return Err(Error::Protocol);
            }
            let (ticket, before, after, field, native) = match raw.message() {
                Ok(Message::SamplerRecord {
                    ticket,
                    before,
                    after,
                    raw: Some(f),
                    native,
                }) => (*ticket, *before, *after, f, *native),
                _ => return Err(Error::Protocol),
            };
            if !native {
                return Err(Error::Simulated);
            }
            if ticket != expected {
                return Err(Error::Protocol);
            }
            if binding.elapsed(issued).map_err(|_| Error::Clock)? != ticket.start_ns {
                return Err(Error::Clock);
            }
            let mut clock = Clock::new(binding.monitor, binding.epoch).map_err(|_| Error::Clock)?;
            let mut last = binding.epoch;
            observe(&mut clock, &mut last, Some(issued))?;
            let start = observe(&mut clock, &mut last, before)?;
            let end = observe(&mut clock, &mut last, after)?;
            let receipt = observe(&mut clock, &mut last, raw.raw().receipt())?;
            let p = autotune_fma_reference::profile::Profile::approved_design().limits;
            if start < ticket.start_ns
                || end - ticket.start_ns > p.query_bracket_ns
                || receipt - end > p.transport_age_ns
                || receipt >= binding.run_end()
            {
                return Err(Error::Deadline);
            }
            map(binding, ticket, end, receipt, field)
        })();
        Self { raw, record }
    }
}
