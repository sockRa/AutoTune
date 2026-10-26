use super::{limits, Binding};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Metric {
    Temperature,
    Power,
    GraphicsClock,
    MemoryClock,
    GpuUtilization,
    MemoryUtilization,
}
impl Metric {
    pub const ALL: [Self; 6] = [
        Self::Temperature,
        Self::Power,
        Self::GraphicsClock,
        Self::MemoryClock,
        Self::GpuUtilization,
        Self::MemoryUtilization,
    ];
    pub fn unit(self) -> Unit {
        match self {
            Self::Temperature => Unit::DegC,
            Self::Power => Unit::Milliwatt,
            Self::GraphicsClock | Self::MemoryClock => Unit::MHz,
            Self::GpuUtilization | Self::MemoryUtilization => Unit::Percent,
        }
    }
    pub fn safety(self) -> bool {
        matches!(self, Self::Temperature | Self::Power)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    DegC,
    Milliwatt,
    MHz,
    Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Available,
    Unsupported,
    Unavailable,
    Failed(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub binding: Binding,
    pub metric: Metric,
    pub slot: u64,
    pub scheduled_ns: u64,
    pub start_ns: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub ticket: Ticket,
    pub end_ns: u64,
    pub receipt_ns: u64,
    pub unit: Unit,
    pub status: Status,
    pub value: Option<i64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordError {
    Ticket,
    Order,
    Bracket,
    Transport,
    Unit,
    Status,
    Value,
}
impl Record {
    pub(super) fn validate(self, pending: Option<Ticket>, now: u64) -> Result<(), RecordError> {
        if pending != Some(self.ticket) {
            return Err(RecordError::Ticket);
        }
        if self.ticket.start_ns > self.end_ns
            || self.end_ns > self.receipt_ns
            || self.receipt_ns > now
        {
            return Err(RecordError::Order);
        }
        if self.end_ns - self.ticket.start_ns > limits().query_bracket_ns {
            return Err(RecordError::Bracket);
        }
        if self.receipt_ns - self.end_ns > limits().transport_age_ns {
            return Err(RecordError::Transport);
        }
        if self.unit != self.ticket.metric.unit() {
            return Err(RecordError::Unit);
        }
        // A value accompanying an unavailable status is contradictory evidence,
        // not a benign non-safety availability failure.
        if self.status != Status::Available {
            return Err(if self.value.is_some() {
                RecordError::Value
            } else {
                RecordError::Status
            });
        }
        let value = self.value.ok_or(RecordError::Value)?;
        if value < 0
            || (matches!(
                self.ticket.metric,
                Metric::GpuUtilization | Metric::MemoryUtilization
            ) && value > 100)
        {
            return Err(RecordError::Value);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissedSlots {
    pub metric: Metric,
    pub first: u64,
    pub last: u64,
}

pub(super) struct Schedule {
    binding: Binding,
    next: [u64; 6],
    pub pending: Option<Ticket>,
}
impl Schedule {
    pub fn new(binding: Binding) -> Self {
        Self {
            binding,
            next: [0; 6],
            pending: None,
        }
    }
    /// Retain older unstarted slots without replacing an unresolved call.
    pub fn skip(&mut self, now: u64) -> [Option<MissedSlots>; 6] {
        let latest = now / limits().cadence_ns;
        std::array::from_fn(|i| {
            if self.next[i] < latest {
                let range = MissedSlots {
                    metric: Metric::ALL[i],
                    first: self.next[i],
                    last: latest - 1,
                };
                self.next[i] = latest;
                Some(range)
            } else {
                None
            }
        })
    }
    pub fn begin(&mut self, now: u64) -> Option<Ticket> {
        if self.pending.is_some() {
            return None;
        }
        let latest = now / limits().cadence_ns;
        let i = (0..6).min_by_key(|&i| (self.next[i], i))?;
        let slot = self.next[i];
        if slot > latest {
            return None;
        }
        let scheduled_ns = slot.checked_mul(limits().cadence_ns)?;
        self.next[i] = slot.checked_add(1)?;
        let ticket = Ticket {
            binding: self.binding,
            metric: Metric::ALL[i],
            slot,
            scheduled_ns,
            start_ns: now,
        };
        self.pending = Some(ticket);
        Some(ticket)
    }
}
