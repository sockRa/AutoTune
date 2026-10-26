use super::{
    clock::Clock,
    limits,
    schedule::{MissedSlots, RecordError, Schedule},
    Anchor, Binding, ClockError, Metric, Record, Ticket,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Pilot,
    Acceptance,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Requested,
    Clock(ClockError),
    SessionDeadline,
    SafetyGap(Metric),
    Temperature,
    Power,
    Record(RecordError),
    EvidenceCapacity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    pub reason: StopReason,
    /// Last validated sleep-including time, never the rejected anchor's time.
    pub at_ns: u64,
    pub cooperative_deadline_ns: u64,
    pub exit_deadline_ns: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Initial,
    Poll,
    Begin(Option<Ticket>),
    Complete(Record),
    RequestStop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub anchor: Anchor,
    pub time: Result<u64, ClockError>,
    pub action: Action,
    pub missed: [Option<MissedSlots>; 6],
    pub record_error: Option<RecordError>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacity {
    pub ordinary_budget_bytes: u64,
    pub allocated_bytes: u64,
    /// Ordinary control events (successful getter begins are paired with samples).
    pub event_capacity: usize,
    pub retained_controls: usize,
    pub sample_capacity: u64,
    pub retained_samples: u64,
    pub missed_range_capacity: usize,
    pub retained_missed_ranges: usize,
    pub exhausted: bool,
}

// Routine polls must not pay for a Record-sized union or six empty ranges.
#[derive(Clone, Copy, Debug)]
struct Observation {
    sequence: u64,
    anchor: Anchor,
    time: Result<u64, ClockError>,
}
#[derive(Clone, Copy, Debug)]
enum ControlKind {
    Initial,
    Poll,
    BeginNone,
    RequestStop,
}
#[derive(Debug)]
struct ControlEvidence {
    observation: Observation,
    kind: ControlKind,
}
#[derive(Debug)]
struct SampleEvidence {
    observation: Observation,
    record: Record,
    error: Option<RecordError>,
    begin: Option<Observation>,
}
#[derive(Debug)]
struct MissEvidence {
    sequence: u64,
    range: MissedSlots,
}
#[derive(Debug)]
pub struct Report {
    binding: Binding,
    controls: Vec<ControlEvidence>,
    samples: Vec<SampleEvidence>,
    misses: Vec<MissEvidence>,
    // At most one unresolved begin, retained even across malformed returns.
    begin: Option<(Observation, Ticket)>,
    terminal_evidence: Option<Event>,
    stop: Option<Stop>,
    pending: Option<Ticket>,
    safety: [Option<Record>; 2],
    baseline_insufficient: [bool; 6],
    now_ns: u64,
    clock_valid: bool,
    session_deadline_ns: u64,
    capacity: Capacity,
}
impl Report {
    pub fn binding(&self) -> Binding {
        self.binding
    }
    /// Allocation-free, original operation order. Values are reconstructed one
    /// at a time from compact storage; no event vector is allocated.
    pub fn events(&self) -> Events<'_> {
        Events {
            report: self,
            control: 0,
            sample: 0,
            paired: 0,
            pending_seen: false,
            miss: 0,
        }
    }
    pub fn terminal_evidence(&self) -> Option<&Event> {
        self.terminal_evidence.as_ref()
    }
    pub fn stop(&self) -> Option<Stop> {
        self.stop
    }
    pub fn pending(&self) -> Option<Ticket> {
        self.pending
    }
    pub fn safety(&self) -> &[Option<Record>; 2] {
        &self.safety
    }
    pub fn baseline_insufficient(&self) -> &[bool; 6] {
        &self.baseline_insufficient
    }
    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }
    pub fn capacity(&self) -> Capacity {
        self.capacity
    }
    pub fn session_deadline_ns(&self) -> u64 {
        self.session_deadline_ns
    }
    pub fn current_clock_valid(&self) -> bool {
        self.clock_valid
    }
    pub fn fixture_compute_eligible(&self) -> bool {
        self.stop.is_none()
            && self.clock_valid
            && self.safety.iter().all(|record| {
                record.is_some_and(|r| self.now_ns - r.end_ns < limits().observation_gap_ns)
            })
    }
    /// Unknown current time is conservatively treated as deadline reached.
    pub fn cooperative_deadline_reached(&self) -> bool {
        !self.clock_valid
            || self
                .stop
                .is_some_and(|s| self.now_ns >= s.cooperative_deadline_ns)
    }
    pub fn exit_deadline_reached(&self) -> bool {
        !self.clock_valid || self.stop.is_some_and(|s| self.now_ns >= s.exit_deadline_ns)
    }
    pub fn native_admission_available(&self) -> bool {
        false
    }
    pub fn native_quiescence_known(&self) -> bool {
        false
    }
    pub fn sensor_age_known(&self) -> bool {
        false
    }
}

/// Immutable, allocation-free merge of control, begin and return evidence.
pub struct Events<'a> {
    report: &'a Report,
    control: usize,
    sample: usize,
    paired: usize,
    pending_seen: bool,
    miss: usize,
}
impl Iterator for Events<'_> {
    type Item = Event;
    fn next(&mut self) -> Option<Event> {
        let r = self.report;
        while self.paired < r.samples.len() && r.samples[self.paired].begin.is_none() {
            self.paired += 1;
        }
        let control = r.controls.get(self.control).map(|e| e.observation);
        let sample = r.samples.get(self.sample).map(|e| e.observation);
        let paired = r.samples.get(self.paired).and_then(|e| e.begin);
        let pending = if self.pending_seen {
            None
        } else {
            r.begin.map(|e| e.0)
        };
        let (source, observation) = [control, sample, paired, pending]
            .into_iter()
            .enumerate()
            .filter_map(|(i, o)| o.map(|o| (i, o)))
            .min_by_key(|(_, o)| o.sequence)?;
        let (action, record_error) = match source {
            0 => {
                let action = match r.controls[self.control].kind {
                    ControlKind::Initial => Action::Initial,
                    ControlKind::Poll => Action::Poll,
                    ControlKind::BeginNone => Action::Begin(None),
                    ControlKind::RequestStop => Action::RequestStop,
                };
                self.control += 1;
                (action, None)
            }
            1 => {
                let e = &r.samples[self.sample];
                self.sample += 1;
                (Action::Complete(e.record), e.error)
            }
            2 => {
                let e = &r.samples[self.paired];
                self.paired += 1;
                (Action::Begin(Some(e.record.ticket)), None)
            }
            _ => {
                self.pending_seen = true;
                (Action::Begin(Some(r.begin.unwrap().1)), None)
            }
        };
        let mut missed = [None; 6];
        while let Some(m) = r.misses.get(self.miss) {
            if m.sequence != observation.sequence {
                break;
            }
            missed[m.range.metric as usize] = Some(m.range);
            self.miss += 1;
        }
        Some(Event {
            anchor: observation.anchor,
            time: observation.time,
            action,
            missed,
            record_error,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    Clock(ClockError),
    Capacity,
}
pub struct Controller {
    clock: Clock,
    schedule: Schedule,
    report: Report,
    sequence: u64,
}

fn reserve<T>(count: usize) -> Result<Vec<T>, InitError> {
    let mut v = Vec::new();
    v.try_reserve_exact(count)
        .map_err(|_| InitError::Capacity)?;
    Ok(v)
}
impl Controller {
    pub fn new(binding: Binding, origin: Anchor, session: Session) -> Result<Self, InitError> {
        let clock = Clock::new(binding, origin).map_err(InitError::Clock)?;
        let limits = limits();
        let ordinary_budget_bytes =
            limits.evidence_ram_bytes - limits.writer_queue_bytes - limits.failure_snapshot_bytes;
        let event_capacity =
            usize::try_from(limits.event_capacity - 1).map_err(|_| InitError::Capacity)?;
        let sample_capacity =
            usize::try_from(limits.sample_capacity - 1).map_err(|_| InitError::Capacity)?;
        let mut controls = reserve::<ControlEvidence>(event_capacity)?;
        let samples = reserve::<SampleEvidence>(sample_capacity)?;
        let misses = reserve::<MissEvidence>(event_capacity)?;
        let allocated_bytes = (std::mem::size_of::<Self>()
            + controls.capacity() * std::mem::size_of::<ControlEvidence>()
            + samples.capacity() * std::mem::size_of::<SampleEvidence>()
            + misses.capacity() * std::mem::size_of::<MissEvidence>())
            as u64;
        if allocated_bytes > ordinary_budget_bytes {
            return Err(InitError::Capacity);
        }
        controls.push(ControlEvidence {
            observation: Observation {
                sequence: 0,
                anchor: origin,
                time: Ok(0),
            },
            kind: ControlKind::Initial,
        });
        Ok(Self {
            clock,
            schedule: Schedule::new(binding),
            sequence: 1,
            report: Report {
                binding,
                controls,
                samples,
                misses,
                begin: None,
                terminal_evidence: None,
                stop: None,
                pending: None,
                safety: [None; 2],
                baseline_insufficient: [false; 6],
                now_ns: 0,
                clock_valid: true,
                session_deadline_ns: match session {
                    Session::Pilot => limits.pilot_session_ns,
                    Session::Acceptance => limits.acceptance_session_ns,
                },
                capacity: Capacity {
                    ordinary_budget_bytes,
                    allocated_bytes,
                    event_capacity,
                    retained_controls: 1,
                    sample_capacity: sample_capacity as u64,
                    retained_samples: 0,
                    missed_range_capacity: event_capacity,
                    retained_missed_ranges: 0,
                    exhausted: false,
                },
            },
        })
    }
    pub fn report(&self) -> &Report {
        &self.report
    }
    fn stop_once(&mut self, reason: StopReason) {
        let now = self.report.now_ns;
        let session = self.report.session_deadline_ns;
        self.report.stop.get_or_insert(Stop {
            reason,
            at_ns: now,
            cooperative_deadline_ns: now
                .saturating_add(limits().cooperative_stop_ns)
                .min(session),
            exit_deadline_ns: now
                .saturating_add(limits().exit_observation_ns)
                .min(session),
        });
    }
    fn observe(&mut self, anchor: Anchor, action: Action) -> Event {
        let time = self.clock.observe(anchor);
        let mut missed = [None; 6];
        match time {
            Err(error) => {
                self.report.clock_valid = false;
                self.stop_once(StopReason::Clock(error));
            }
            Ok(now) => {
                self.report.now_ns = now;
                missed = self.schedule.skip(now);
                if now >= self.report.session_deadline_ns {
                    self.stop_once(StopReason::SessionDeadline);
                } else {
                    for (i, metric) in [Metric::Temperature, Metric::Power].into_iter().enumerate()
                    {
                        let end = self.report.safety[i].map_or(0, |r| r.end_ns);
                        if now - end >= limits().observation_gap_ns {
                            self.stop_once(StopReason::SafetyGap(metric));
                        }
                    }
                }
            }
        }
        Event {
            anchor,
            time,
            action,
            missed,
            record_error: None,
        }
    }
    fn fits(&self, event: &Event) -> bool {
        let r = &self.report;
        !r.capacity.exhausted
            && r.controls.len() < r.capacity.event_capacity
            && r.misses.len() + event.missed.iter().flatten().count()
                <= r.capacity.missed_range_capacity
            && match event.action {
                Action::Complete(_) | Action::Begin(Some(_)) => {
                    r.samples.len() < r.capacity.sample_capacity as usize
                }
                _ => r.controls.len() < r.capacity.event_capacity,
            }
    }
    fn retain(&mut self, event: Event) {
        if !self.fits(&event) {
            self.report.capacity.exhausted = true;
            self.report.terminal_evidence.get_or_insert(event);
            self.stop_once(StopReason::EvidenceCapacity);
        } else {
            let observation = Observation {
                sequence: self.sequence,
                anchor: event.anchor,
                time: event.time,
            };
            for range in event.missed.into_iter().flatten() {
                self.report.misses.push(MissEvidence {
                    sequence: self.sequence,
                    range,
                });
            }
            match event.action {
                Action::Begin(Some(ticket)) => {
                    self.report.begin = Some((observation, ticket));
                }
                Action::Complete(record) => {
                    let begin = if self.report.begin.is_some_and(|(_, t)| t == record.ticket)
                        && self.schedule.pending.is_none()
                    {
                        self.report.begin.take().map(|b| b.0)
                    } else {
                        None
                    };
                    self.report.samples.push(SampleEvidence {
                        observation,
                        record,
                        error: event.record_error,
                        begin,
                    });
                }
                action => {
                    let kind = match action {
                        Action::Initial => ControlKind::Initial,
                        Action::Poll => ControlKind::Poll,
                        Action::Begin(None) => ControlKind::BeginNone,
                        Action::RequestStop => ControlKind::RequestStop,
                        _ => unreachable!(),
                    };
                    self.report
                        .controls
                        .push(ControlEvidence { observation, kind });
                }
            }
            self.sequence += 1;
        }
        self.report.pending = self.schedule.pending;
        self.report.capacity.retained_controls = self.report.controls.len();
        self.report.capacity.retained_samples = self.report.samples.len() as u64;
        self.report.capacity.retained_missed_ranges = self.report.misses.len();
    }
    pub fn poll(&mut self, anchor: Anchor) {
        let event = self.observe(anchor, Action::Poll);
        self.retain(event);
    }
    pub fn request_stop(&mut self, anchor: Anchor) {
        let event = self.observe(anchor, Action::RequestStop);
        self.stop_once(StopReason::Requested);
        self.retain(event);
    }
    pub fn begin(&mut self, anchor: Anchor) -> Option<Ticket> {
        let mut event = self.observe(anchor, Action::Begin(None));
        // Check both possible storage classes before issuing a fixture ticket.
        if !self.fits(&event)
            || self.report.samples.len() == self.report.capacity.sample_capacity as usize
        {
            self.report.capacity.exhausted = true;
            self.report.terminal_evidence.get_or_insert(event);
            self.stop_once(StopReason::EvidenceCapacity);
            return None;
        }
        let ticket = if self.report.stop.is_none() {
            self.schedule.begin(self.report.now_ns)
        } else {
            None
        };
        event.action = Action::Begin(ticket);
        self.retain(event);
        ticket
    }
    pub fn complete(&mut self, anchor: Anchor, record: Record) {
        let mut event = self.observe(anchor, Action::Complete(record));
        let validation = record.validate(self.schedule.pending, self.report.now_ns);
        event.record_error = validation.err();
        if event.time.is_ok()
            && self.schedule.pending == Some(record.ticket)
            && !matches!(validation, Err(RecordError::Ticket | RecordError::Order))
        {
            self.schedule.pending = None;
        }
        if let Err(error) = validation {
            self.report.baseline_insufficient[record.ticket.metric as usize] = true;
            if record.ticket.metric.safety() || error != RecordError::Status {
                self.stop_once(StopReason::Record(error));
            }
        } else if self.report.stop.is_none() {
            let value = record.value.expect("validated value");
            match record.ticket.metric {
                Metric::Temperature if value >= limits().temperature_stop_degc => {
                    self.stop_once(StopReason::Temperature)
                }
                Metric::Power if value as u64 >= limits().power_stop_mw => {
                    self.stop_once(StopReason::Power)
                }
                Metric::Temperature | Metric::Power => {
                    self.report.safety[record.ticket.metric as usize] = Some(record)
                }
                _ => (),
            }
        }
        self.retain(event);
    }
}
