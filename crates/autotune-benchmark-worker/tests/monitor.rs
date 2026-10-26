use autotune_benchmark_worker::{
    monitor::{
        clock::ticks_to_ns,
        controller::{Action, InitError},
        schedule::RecordError,
        Anchor, Binding, ClockError, Controller, Metric, Record, Session, Status, StopReason,
        Ticket, Unit,
    },
    worker::Identity,
};
use autotune_fma_reference::profile::Profile;

const MS: u64 = 1_000_000;
fn binding() -> Binding {
    Binding {
        session: [1; 16],
        run: [2; 16],
        device: Identity {
            uuid: [3; 16],
            domain: 0,
            bus: 1,
            device: 2,
        },
        clock_token: [4; 16],
        qpc_frequency: 1_000_000_000,
    }
}
fn anchor(ns: u64) -> Anchor {
    Anchor {
        clock_token: binding().clock_token,
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn controller() -> Controller {
    Controller::new(binding(), anchor(0), Session::Acceptance).unwrap()
}
fn record(ticket: Ticket, end_ns: u64, receipt_ns: u64, value: i64) -> Record {
    Record {
        ticket,
        end_ns,
        receipt_ns,
        unit: ticket.metric.unit(),
        status: Status::Available,
        value: Some(value),
    }
}
fn safe_pair(c: &mut Controller, ns: u64) {
    for value in [50, 150_000] {
        let ticket = c.begin(anchor(ns)).unwrap();
        c.complete(anchor(ns), record(ticket, ns, ns, value));
    }
}
fn reason(c: &Controller) -> StopReason {
    c.report().stop().unwrap().reason
}

#[test]
fn binding_validation_and_fixed_report_identity() {
    let mut variants = [binding(); 6];
    variants[0].session = [0; 16];
    variants[1].run = [0; 16];
    variants[2].device.uuid = [0; 16];
    variants[3].device.device = 32;
    variants[4].clock_token = [0; 16];
    variants[5].qpc_frequency = 0;
    for b in variants {
        assert!(matches!(
            Controller::new(b, anchor(0), Session::Pilot),
            Err(InitError::Clock(_))
        ));
    }
    let c = controller();
    assert_eq!(c.report().binding(), binding());
    assert!(!c.report().fixture_compute_eligible());
    assert_eq!(c.report().events().next().unwrap().anchor, anchor(0));
}

#[test]
fn checked_tick_rounding_and_overflow() {
    assert_eq!(ticks_to_ns(1, 3, false), Ok(333_333_333));
    assert_eq!(ticks_to_ns(1, 3, true), Ok(333_333_334));
    assert_eq!(ticks_to_ns(u64::MAX, 1, false), Err(ClockError::Overflow));
    assert_eq!(ticks_to_ns(1, 0, true), Err(ClockError::Binding));
    let mut c = controller();
    let mut a = anchor(0);
    a.interrupt_100ns = u64::MAX;
    a.unbiased_100ns = u64::MAX;
    c.poll(a);
    assert_eq!(reason(&c), StopReason::Clock(ClockError::Overflow));
    assert_eq!(c.report().events().last().unwrap().anchor, a);
}

#[test]
fn bracket_and_elapsed_uncertainty_boundaries() {
    let mut first = anchor(0);
    first.qpc_after = MS;
    let mut c = Controller::new(binding(), first, Session::Pilot).unwrap();
    let mut next = anchor(10 * MS);
    next.qpc_after += MS;
    next.interrupt_100ns = 16 * MS / 100;
    next.unbiased_100ns = next.interrupt_100ns;
    c.poll(next);
    assert_eq!(c.report().stop(), None);
    let mut too_wide = first;
    too_wide.qpc_after += 1;
    assert!(matches!(
        Controller::new(binding(), too_wide, Session::Pilot),
        Err(InitError::Clock(ClockError::Bracket))
    ));
    let mut b = binding();
    b.qpc_frequency = 3;
    let mut a = anchor(0);
    a.qpc_after = 1;
    assert!(matches!(
        Controller::new(b, a, Session::Pilot),
        Err(InitError::Clock(ClockError::Bracket))
    ));
}

#[test]
fn clock_faults_are_sticky_and_preserve_raw_anchors() {
    for expected in [
        ClockError::Epoch,
        ClockError::Order,
        ClockError::Regression,
        ClockError::Disagreement,
        ClockError::Suspend,
    ] {
        let mut c = controller();
        c.poll(anchor(10 * MS));
        let mut a = anchor(20 * MS);
        match expected {
            ClockError::Epoch => a.clock_token = [5; 16],
            ClockError::Order => a.qpc_after = a.qpc_before - 1,
            ClockError::Regression => a = anchor(9 * MS),
            ClockError::Disagreement => {
                a.interrupt_100ns += 5 * MS / 100 + 1;
                a.unbiased_100ns = a.interrupt_100ns;
            }
            ClockError::Suspend => a.unbiased_100ns -= 5 * MS / 100 + 1,
            _ => unreachable!(),
        }
        c.poll(a);
        assert_eq!(reason(&c), StopReason::Clock(expected));
        let stop = c.report().stop().unwrap();
        assert_eq!(
            (
                c.report().now_ns(),
                stop.at_ns,
                stop.cooperative_deadline_ns,
                stop.exit_deadline_ns
            ),
            (10 * MS, 10 * MS, 1010 * MS, 3010 * MS)
        );
        assert!(c.report().cooperative_deadline_reached());
        assert!(c.report().exit_deadline_reached());
        assert_eq!(c.report().events().last().unwrap().anchor, a);
        c.poll(anchor(30 * MS));
        assert_eq!(c.report().stop(), Some(stop));
        assert_eq!(c.report().events().last().unwrap().time, Err(expected));
        assert!(c.begin(anchor(30 * MS)).is_none());
        assert!(!c.report().fixture_compute_eligible());
    }
}

#[test]
fn exact_suspend_divergence_passes_but_cumulative_drift_does_not() {
    let mut c = controller();
    let mut a = anchor(10 * MS);
    a.unbiased_100ns -= 5 * MS / 100;
    c.poll(a);
    assert_eq!(c.report().stop(), None);
    let mut b = anchor(20 * MS);
    b.unbiased_100ns -= 10 * MS / 100;
    c.poll(b);
    assert_eq!(reason(&c), StopReason::Clock(ClockError::Suspend));
}

#[test]
fn lower_interval_clamps_zero_and_agreement_lower_boundary_passes() {
    let mut first = anchor(0);
    first.qpc_after = MS;
    let mut c = Controller::new(binding(), first, Session::Pilot).unwrap();
    let mut a = anchor(0);
    a.qpc_after = MS;
    c.poll(a);
    assert_eq!(c.report().stop(), None);
    a = anchor(10 * MS);
    a.interrupt_100ns = 4 * MS / 100;
    a.unbiased_100ns = a.interrupt_100ns;
    c.poll(a);
    assert_eq!(c.report().stop(), None);
    let mut c = Controller::new(binding(), first, Session::Pilot).unwrap();
    a.interrupt_100ns -= 1;
    a.unbiased_100ns = a.interrupt_100ns;
    c.poll(a);
    assert_eq!(reason(&c), StopReason::Clock(ClockError::Disagreement));
}

#[test]
fn six_absolute_schedules_fixed_ties_and_no_catchup() {
    let mut c = controller();
    for metric in Metric::ALL {
        let t = c.begin(anchor(0)).unwrap();
        assert_eq!(
            (t.metric, t.slot, t.scheduled_ns, t.start_ns),
            (metric, 0, 0, 0)
        );
        c.complete(anchor(0), record(t, 0, 0, 1));
    }
    assert!(c.begin(anchor(249 * MS)).is_none());
    let t = c.begin(anchor(500 * MS)).unwrap();
    assert_eq!(
        (t.metric, t.slot, t.scheduled_ns),
        (Metric::Temperature, 2, 500 * MS)
    );
    for (i, range) in c
        .report()
        .events()
        .last()
        .unwrap()
        .missed
        .iter()
        .enumerate()
    {
        let range = range.unwrap();
        assert_eq!(
            (range.metric, range.first, range.last),
            (Metric::ALL[i], 1, 1)
        );
    }
    let missed = c.report().events().last().unwrap().missed;
    c.complete(anchor(500 * MS), record(t, 500 * MS, 500 * MS, 50));
    let begin = c
        .report()
        .events()
        .find(|e| e.action == Action::Begin(Some(t)))
        .unwrap();
    assert_eq!(begin.missed, missed);
    assert_eq!(c.report().events().last().unwrap().missed, [None; 6]);
    assert_eq!(c.begin(anchor(500 * MS)).unwrap().metric, Metric::Power);
}

#[test]
fn unresolved_getter_never_replaced_and_initial_gap_stops() {
    let mut c = controller();
    let ticket = c.begin(anchor(0)).unwrap();
    c.poll(anchor(500 * MS));
    assert_eq!(c.report().pending(), Some(ticket));
    let ranges = c.report().events().last().unwrap().missed;
    assert_eq!((ranges[0].unwrap().first, ranges[0].unwrap().last), (1, 1));
    assert_eq!((ranges[1].unwrap().first, ranges[1].unwrap().last), (0, 1));
    assert!(c.begin(anchor(500 * MS)).is_none());
    c.poll(anchor(750 * MS));
    assert_eq!(reason(&c), StopReason::SafetyGap(Metric::Temperature));
    assert_eq!(c.report().pending(), Some(ticket));
    let stop = c.report().stop();
    c.complete(anchor(751 * MS), record(ticket, 0, 0, 50));
    assert_eq!(c.report().pending(), None);
    assert_eq!(c.report().stop(), stop);
    assert!(!c.report().fixture_compute_eligible());
}

#[test]
fn fixture_eligibility_expires_from_query_end_not_receipt() {
    let mut c = controller();
    let t = c.begin(anchor(0)).unwrap();
    c.complete(anchor(100 * MS), record(t, 0, 100 * MS, 50));
    assert!(!c.report().fixture_compute_eligible());
    let t = c.begin(anchor(100 * MS)).unwrap();
    c.complete(anchor(100 * MS), record(t, 100 * MS, 100 * MS, 150_000));
    assert!(c.report().fixture_compute_eligible());
    assert!(!c.report().native_admission_available());
    assert!(!c.report().native_quiescence_known());
    assert!(!c.report().sensor_age_known());
    c.poll(anchor(750 * MS - 100));
    assert!(c.report().fixture_compute_eligible());
    c.poll(anchor(750 * MS));
    assert!(!c.report().fixture_compute_eligible());
    assert_eq!(reason(&c), StopReason::SafetyGap(Metric::Temperature));
}

#[test]
fn either_safety_metric_can_expire_and_neither_is_optional() {
    let mut c = controller();
    safe_pair(&mut c, 0);
    let t = c.begin(anchor(500 * MS)).unwrap();
    c.complete(anchor(500 * MS), record(t, 500 * MS, 500 * MS, 50));
    c.poll(anchor(750 * MS));
    assert_eq!(reason(&c), StopReason::SafetyGap(Metric::Power));
}

#[test]
fn threshold_equality_stops_with_first_anchor_and_fixed_deadlines() {
    for (temperature, power, expected) in [
        (75, 1, StopReason::Temperature),
        (74, 200_000, StopReason::Power),
    ] {
        let mut c = controller();
        let t = c.begin(anchor(0)).unwrap();
        c.complete(anchor(0), record(t, 0, 0, temperature));
        if c.report().stop().is_none() {
            let t = c.begin(anchor(0)).unwrap();
            c.complete(anchor(0), record(t, 0, 0, power));
        }
        assert_eq!(reason(&c), expected);
        let stop = c.report().stop().unwrap();
        assert_eq!(
            (
                stop.at_ns,
                stop.cooperative_deadline_ns,
                stop.exit_deadline_ns
            ),
            (0, 1000 * MS, 3000 * MS)
        );
        c.request_stop(anchor(500 * MS));
        c.poll(anchor(1000 * MS));
        assert!(c.report().cooperative_deadline_reached());
        assert!(!c.report().exit_deadline_reached());
        c.poll(anchor(3000 * MS));
        assert!(c.report().exit_deadline_reached());
        assert_eq!(c.report().stop(), Some(stop));
        assert!(c.begin(anchor(3000 * MS)).is_none());
    }
    let mut c = controller();
    safe_pair(&mut c, 0);
    assert!(c.report().fixture_compute_eligible());
}

#[test]
fn query_bracket_and_transport_equality_pass_excess_rejects() {
    for (end, receipt, expected) in [
        (50 * MS, 150 * MS, None),
        (50 * MS + 100, 50 * MS + 100, Some(RecordError::Bracket)),
        (0, 100 * MS + 100, Some(RecordError::Transport)),
    ] {
        let mut c = controller();
        let t = c.begin(anchor(0)).unwrap();
        c.complete(anchor(receipt), record(t, end, receipt, 50));
        assert_eq!(c.report().events().last().unwrap().record_error, expected);
        assert_eq!(c.report().safety()[0].is_some(), expected.is_none());
    }
}

#[test]
fn returned_ticket_must_match_every_bound_field() {
    for field in 0..11 {
        let mut c = controller();
        let t = c.begin(anchor(0)).unwrap();
        let mut wrong = t;
        match field {
            0 => wrong.binding.session[0] ^= 1,
            1 => wrong.binding.run[0] ^= 1,
            2 => wrong.binding.device.uuid[0] ^= 1,
            3 => wrong.binding.device.bus ^= 1,
            4 => wrong.binding.clock_token[0] ^= 1,
            5 => wrong.binding.qpc_frequency += 1,
            6 => wrong.metric = Metric::Power,
            7 => wrong.slot += 1,
            8 => wrong.scheduled_ns += 1,
            9 => wrong.start_ns += 1,
            10 => wrong.binding.device.domain += 1,
            _ => unreachable!(),
        }
        let raw = record(wrong, 0, 0, 50);
        c.complete(anchor(0), raw);
        assert_eq!(reason(&c), StopReason::Record(RecordError::Ticket));
        assert_eq!(c.report().pending(), Some(t));
        assert_eq!(
            c.report().events().last().unwrap().action,
            Action::Complete(raw)
        );
        assert!(c.begin(anchor(0)).is_none());
        let stop = c.report().stop();
        c.complete(anchor(0), record(t, 0, 0, 50));
        assert_eq!(c.report().pending(), None);
        assert_eq!(c.report().stop(), stop);
        assert!(!c.report().fixture_compute_eligible());
        assert_eq!(
            c.report()
                .events()
                .filter(|e| e.action == Action::Begin(Some(t)))
                .count(),
            1
        );
        assert!(c
            .report()
            .events()
            .any(|e| e.action == Action::Complete(raw)
                && e.record_error == Some(RecordError::Ticket)));
        assert_eq!(
            c.report().events().last().unwrap().action,
            Action::Complete(record(t, 0, 0, 50))
        );
    }
}

#[test]
fn malformed_time_unit_status_and_value_are_preserved() {
    for field in 0..9 {
        let mut c = controller();
        let t = c.begin(anchor(10 * MS)).unwrap();
        let mut raw = record(t, 10 * MS, 10 * MS, 50);
        let expected = match field {
            0 => {
                raw.end_ns -= 1;
                RecordError::Order
            }
            1 => {
                raw.receipt_ns -= 1;
                RecordError::Order
            }
            2 => {
                raw.receipt_ns += 1;
                RecordError::Order
            }
            3 => {
                raw.unit = Unit::MHz;
                RecordError::Unit
            }
            4 => {
                raw.status = Status::Unsupported;
                raw.value = None;
                RecordError::Status
            }
            5 => {
                raw.status = Status::Unavailable;
                raw.value = None;
                RecordError::Status
            }
            6 => {
                raw.status = Status::Failed(-77);
                raw.value = None;
                RecordError::Status
            }
            7 => {
                raw.value = None;
                RecordError::Value
            }
            8 => {
                raw.value = Some(-1);
                RecordError::Value
            }
            _ => unreachable!(),
        };
        c.complete(anchor(10 * MS), raw);
        assert_eq!(reason(&c), StopReason::Record(expected));
        assert_eq!(
            c.report().events().last().unwrap().action,
            Action::Complete(raw)
        );
        assert!(!c.report().fixture_compute_eligible());
        assert!(c.report().baseline_insufficient()[0]);
    }
}

#[test]
fn non_safety_unavailability_is_separate_baseline_insufficiency() {
    for status in [
        Status::Unavailable,
        Status::Unsupported,
        Status::Failed(-77),
    ] {
        let mut c = controller();
        safe_pair(&mut c, 0);
        for metric in Metric::ALL.into_iter().skip(2) {
            let t = c.begin(anchor(0)).unwrap();
            assert_eq!(t.metric, metric);
            let mut raw = record(t, 0, 0, 1);
            raw.status = status;
            raw.value = None;
            c.complete(anchor(0), raw);
            assert!(c.report().fixture_compute_eligible());
            assert!(c.report().baseline_insufficient()[metric as usize]);
            let event = c.report().events().last().unwrap();
            assert_eq!(event.action, Action::Complete(raw));
            assert_eq!(event.record_error, Some(RecordError::Status));
        }
        assert_eq!(c.report().stop(), None);
    }
}

#[test]
fn physical_negative_values_and_utilization_range_reject() {
    for metric in Metric::ALL.into_iter().skip(2) {
        for value in [-1, 101] {
            let mut c = controller();
            safe_pair(&mut c, 0);
            loop {
                let t = c.begin(anchor(0)).unwrap();
                let v = if t.metric == metric { value } else { 1 };
                c.complete(anchor(0), record(t, 0, 0, v));
                if t.metric == metric {
                    break;
                }
            }
            let invalid =
                value < 0 || matches!(metric, Metric::GpuUtilization | Metric::MemoryUtilization);
            assert_eq!(c.report().stop().is_some(), invalid);
        }
    }
    let mut c = controller();
    safe_pair(&mut c, 0);
    for _ in 2..6 {
        let t = c.begin(anchor(0)).unwrap();
        c.complete(anchor(0), record(t, 0, 0, 100));
    }
    assert!(c.report().fixture_compute_eligible());
}

#[test]
fn completion_at_gap_cannot_refresh_or_reopen() {
    let mut c = controller();
    safe_pair(&mut c, 0);
    let t = c.begin(anchor(700 * MS)).unwrap();
    c.complete(anchor(750 * MS), record(t, 750 * MS, 750 * MS, 50));
    assert_eq!(reason(&c), StopReason::SafetyGap(Metric::Temperature));
    assert_eq!(c.report().safety()[0].unwrap().end_ns, 0);
    assert!(!c.report().fixture_compute_eligible());
}

#[test]
fn whole_session_equality_wins_and_stop_deadlines_are_capped() {
    for (session, deadline) in [
        (Session::Pilot, 180_000 * MS),
        (Session::Acceptance, 600_000 * MS),
    ] {
        let mut c = Controller::new(binding(), anchor(0), session).unwrap();
        assert_eq!(c.report().session_deadline_ns(), deadline);
        c.poll(anchor(deadline));
        assert_eq!(reason(&c), StopReason::SessionDeadline);
        let stop = c.report().stop().unwrap();
        assert_eq!(
            (stop.cooperative_deadline_ns, stop.exit_deadline_ns),
            (deadline, deadline)
        );
        assert!(c.report().exit_deadline_reached());
    }
    let mut c = Controller::new(binding(), anchor(0), Session::Pilot).unwrap();
    for ns in (0..180_000 * MS).step_by(250 * MS as usize) {
        safe_pair(&mut c, ns);
    }
    c.request_stop(anchor(179_900 * MS));
    let stop = c.report().stop().unwrap();
    assert_eq!(stop.reason, StopReason::Requested);
    assert_eq!(
        (stop.cooperative_deadline_ns, stop.exit_deadline_ns),
        (180_000 * MS, 180_000 * MS)
    );
}

#[test]
fn event_capacity_retains_first_overflow_without_eviction_or_reopening() {
    let mut c = controller();
    safe_pair(&mut c, 0);
    let capacity = c.report().capacity();
    let limits = Profile::approved_design().limits;
    assert!(
        capacity.allocated_bytes
            <= limits.evidence_ram_bytes
                - limits.writer_queue_bytes
                - limits.failure_snapshot_bytes
    );
    assert!((capacity.event_capacity as u64) < limits.event_capacity);
    let first = c.report().events().next().unwrap();
    while c.report().capacity().retained_controls < capacity.event_capacity {
        c.poll(anchor(0));
    }
    assert!(c.begin(anchor(0)).is_none());
    assert_eq!(reason(&c), StopReason::EvidenceCapacity);
    let terminal = *c.report().terminal_evidence().unwrap();
    let stop = c.report().stop();
    for _ in 0..10 {
        c.request_stop(anchor(0));
    }
    assert_eq!(c.report().stop(), stop);
    assert_eq!(c.report().events().next(), Some(first));
    assert_eq!(
        c.report().capacity().retained_controls,
        capacity.event_capacity
    );
    assert_eq!(c.report().terminal_evidence(), Some(&terminal));
    assert_eq!(
        c.report().capacity().allocated_bytes,
        capacity.allocated_bytes
    );
    assert!(c.report().capacity().exhausted);
    assert!(!c.report().fixture_compute_eligible());
}

#[test]
fn sample_capacity_preserves_rejected_raw_and_original_stop() {
    let mut c = controller();
    let t = c.begin(anchor(0)).unwrap();
    c.request_stop(anchor(0));
    let stop = c.report().stop();
    let raw = record(t, 0, 0, 50);
    for _ in 0..=Profile::approved_design().limits.sample_capacity {
        c.complete(anchor(0), raw);
    }
    assert!(c.report().capacity().exhausted);
    assert_eq!(
        c.report().capacity().retained_samples,
        Profile::approved_design().limits.sample_capacity - 1
    );
    assert_eq!(
        c.report().terminal_evidence().unwrap().action,
        Action::Complete(raw)
    );
    assert_eq!(c.report().stop(), stop);
    assert!(c.report().safety()[0].is_none());
    assert!(!c.report().fixture_compute_eligible());
}

#[test]
fn malformed_time_or_clock_cannot_resolve_pending_uncertainty() {
    let mut c = controller();
    let t = c.begin(anchor(10 * MS)).unwrap();
    c.complete(anchor(10 * MS), record(t, 0, 0, 50));
    assert_eq!(c.report().pending(), Some(t));
    assert_eq!(reason(&c), StopReason::Record(RecordError::Order));
    let mut c = controller();
    let t = c.begin(anchor(0)).unwrap();
    let mut bad = anchor(0);
    bad.clock_token = [9; 16];
    c.complete(bad, record(t, 0, 0, 50));
    assert_eq!(c.report().pending(), Some(t));
    assert!(!c.report().current_clock_valid());
    assert_eq!(reason(&c), StopReason::Clock(ClockError::Epoch));
}

#[test]
fn every_mutating_operation_checks_anchor_and_nonzero_origin_is_relative() {
    for operation in 0..3 {
        let mut c = controller();
        let t = c.begin(anchor(0)).unwrap();
        let mut bad = anchor(0);
        bad.clock_token = [8; 16];
        match operation {
            0 => {
                assert!(c.begin(bad).is_none());
            }
            1 => c.complete(bad, record(t, 0, 0, 50)),
            2 => c.request_stop(bad),
            _ => unreachable!(),
        }
        assert_eq!(reason(&c), StopReason::Clock(ClockError::Epoch));
    }
    let mut c = Controller::new(binding(), anchor(123 * MS), Session::Pilot).unwrap();
    let ticket = c.begin(anchor(133 * MS)).unwrap();
    assert_eq!(ticket.start_ns, 10 * MS);
    assert_eq!(ticket.scheduled_ns, 0);
    assert_eq!(c.report().events().next().unwrap().anchor, anchor(123 * MS));
}

#[test]
fn complete_acceptance_trace_at_ten_ms_control_and_six_250_ms_queries() {
    let mut c = controller();
    assert_eq!(
        Profile::approved_design().limits.controller_tick_ns,
        10 * MS
    );
    assert_eq!(Profile::approved_design().limits.cadence_ns, 250 * MS);
    assert_eq!(c.report().session_deadline_ns(), 600_000 * MS);
    let allocated = c.report().capacity().allocated_bytes;
    for tick in 0..=60_000 {
        let ns = tick * 10 * MS;
        c.poll(anchor(ns));
        if tick == 60_000 {
            break;
        }
        if tick % 25 == 0 {
            for (i, metric) in Metric::ALL.into_iter().enumerate() {
                let start = ns + i as u64 * MS;
                let end = start + MS;
                let ticket = c.begin(anchor(start)).unwrap();
                assert_eq!(
                    (ticket.metric, ticket.slot, ticket.scheduled_ns),
                    (metric, tick / 25, ns)
                );
                let value = if metric == Metric::Power { 150_000 } else { 50 };
                c.complete(anchor(end), record(ticket, end, end, value));
            }
        }
        assert_eq!(c.report().stop(), None, "tick {tick}");
        assert!(c.report().fixture_compute_eligible());
    }
    assert_eq!(reason(&c), StopReason::SessionDeadline);
    assert_eq!(c.report().stop().unwrap().at_ns, 600_000 * MS);
    let capacity = c.report().capacity();
    assert!(!capacity.exhausted);
    assert_eq!(capacity.allocated_bytes, allocated);
    assert_eq!(capacity.retained_controls, 60_002);
    assert_eq!(capacity.retained_samples, 14_400);
    assert_eq!(capacity.retained_missed_ranges, 0);
    let mut events = c.report().events();
    assert_eq!(events.next().unwrap().action, Action::Initial);
    for tick in 0..=60_000 {
        let ns = tick * 10 * MS;
        let poll = events.next().unwrap();
        assert_eq!(
            (poll.anchor, poll.time, poll.action),
            (anchor(ns), Ok(ns), Action::Poll)
        );
        if tick < 60_000 && tick % 25 == 0 {
            for i in 0..6 {
                let start = ns + i * MS;
                let begin = events.next().unwrap();
                assert_eq!((begin.anchor, begin.time), (anchor(start), Ok(start)));
                let Action::Begin(Some(ticket)) = begin.action else {
                    panic!("missing paired begin")
                };
                let returned = events.next().unwrap();
                let end = start + MS;
                assert_eq!((returned.anchor, returned.time), (anchor(end), Ok(end)));
                let value = if ticket.metric == Metric::Power {
                    150_000
                } else {
                    50
                };
                assert_eq!(
                    returned.action,
                    Action::Complete(record(ticket, end, end, value))
                );
                assert_eq!(returned.record_error, None);
                assert_eq!(returned.missed, [None; 6]);
            }
        }
    }
    assert_eq!(events.next(), None);
}

#[test]
fn unavailable_status_value_consistency_including_non_safety() {
    for status in [
        Status::Unsupported,
        Status::Unavailable,
        Status::Failed(-77),
    ] {
        for value in [Some(-1), Some(0), Some(101)] {
            let mut c = controller();
            safe_pair(&mut c, 0);
            let t = c.begin(anchor(0)).unwrap();
            assert_eq!(t.metric, Metric::GraphicsClock);
            let mut raw = record(t, 0, 0, 1);
            raw.status = status;
            raw.value = value;
            c.complete(anchor(0), raw);
            assert_eq!(reason(&c), StopReason::Record(RecordError::Value));
            assert_eq!(
                c.report().events().last().unwrap().action,
                Action::Complete(raw)
            );
            assert!(!c.report().fixture_compute_eligible());
        }
    }
}

#[test]
fn non_ghz_scaled_nonzero_origin_fractional_elapsed_bounds() {
    // A 3 MHz QPC has fractional-nanosecond ticks. Absolute QPC and host
    // origins deliberately differ; only differences have meaning.
    let mut b = binding();
    b.qpc_frequency = 3_000_000;
    let origin = Anchor {
        clock_token: b.clock_token,
        qpc_before: 123_457,
        qpc_after: 123_458,
        interrupt_100ns: 9_000_000,
        unbiased_100ns: 8_000_000,
    };
    for (elapsed_100ns, expected) in [
        (50_000, None),
        (150_006, None),
        (150_007, Some(ClockError::Disagreement)),
        (49_999, Some(ClockError::Disagreement)),
    ] {
        let mut c = Controller::new(b, origin, Session::Acceptance).unwrap();
        // lower=floor(30000/3e6 s)=10ms, upper=ceil(30002/...)=10000667ns.
        let later = Anchor {
            qpc_before: origin.qpc_after + 30_000,
            qpc_after: origin.qpc_after + 30_001,
            interrupt_100ns: origin.interrupt_100ns + elapsed_100ns,
            unbiased_100ns: origin.unbiased_100ns + elapsed_100ns,
            ..origin
        };
        c.poll(later);
        assert_eq!(
            c.report().events().last().unwrap().time,
            expected.map_or(Ok(elapsed_100ns * 100), Err)
        );
        assert_eq!(
            c.report().stop().map(|s| s.reason),
            expected.map(StopReason::Clock)
        );
    }
    let mut c = Controller::new(b, origin, Session::Acceptance).unwrap();
    let later = Anchor {
        qpc_before: origin.qpc_before + 30_000,
        qpc_after: origin.qpc_after + 30_000,
        interrupt_100ns: origin.interrupt_100ns + 100_000,
        unbiased_100ns: origin.unbiased_100ns + 100_000,
        ..origin
    };
    let ticket = c.begin(later).unwrap();
    assert_eq!(ticket.start_ns, 10 * MS);
    c.complete(later, record(ticket, 10 * MS, 10 * MS, 50));
    assert_eq!(c.report().safety()[0].unwrap().end_ns, 10 * MS);
}

#[test]
fn blocked_non_safety_getter_expires_after_eligibility() {
    let mut c = controller();
    safe_pair(&mut c, 0);
    assert!(c.report().fixture_compute_eligible());
    let ticket = c.begin(anchor(0)).unwrap();
    assert_eq!(ticket.metric, Metric::GraphicsClock);
    c.poll(anchor(750 * MS - 100));
    assert!(c.report().fixture_compute_eligible());
    assert!(c.begin(anchor(750 * MS - 100)).is_none());
    c.poll(anchor(750 * MS));
    assert_eq!(reason(&c), StopReason::SafetyGap(Metric::Temperature));
    assert_eq!(c.report().pending(), Some(ticket));
    let stop = c.report().stop();
    c.complete(anchor(751 * MS), record(ticket, 0, 0, 1000));
    assert_eq!(c.report().pending(), None);
    assert_eq!(c.report().stop(), stop);
    assert!(!c.report().fixture_compute_eligible());
}

#[test]
fn neighboring_safety_thresholds_gaps_and_initial_absent_power() {
    for temperature in [74, 75, 76] {
        for power in [199_999, 200_000, 200_001] {
            let mut c = controller();
            let t = c.begin(anchor(0)).unwrap();
            c.complete(anchor(0), record(t, 0, 0, temperature));
            if temperature >= 75 {
                assert_eq!(reason(&c), StopReason::Temperature);
                continue;
            }
            let t = c.begin(anchor(0)).unwrap();
            c.complete(anchor(0), record(t, 0, 0, power));
            assert_eq!(
                c.report().stop().map(|s| s.reason),
                (power >= 200_000).then_some(StopReason::Power)
            );
            assert_eq!(c.report().fixture_compute_eligible(), power < 200_000);
        }
    }
    for age in [750 * MS - 100, 750 * MS, 750 * MS + 100] {
        for absent_power in [false, true] {
            let mut c = controller();
            if absent_power {
                let t = c.begin(anchor(500 * MS)).unwrap();
                c.complete(anchor(500 * MS), record(t, 500 * MS, 500 * MS, 50));
            } else {
                safe_pair(&mut c, 0);
            }
            c.poll(anchor(age));
            assert_eq!(
                c.report().stop().map(|s| s.reason),
                (age >= 750 * MS).then_some(StopReason::SafetyGap(if absent_power {
                    Metric::Power
                } else {
                    Metric::Temperature
                }))
            );
            assert_eq!(
                c.report().fixture_compute_eligible(),
                !absent_power && age < 750 * MS
            );
        }
    }
}

#[test]
fn paired_begin_survives_interleaved_rejected_returns_and_missed_ranges() {
    let mut c = controller();
    let t = c.begin(anchor(10 * MS)).unwrap();
    c.complete(anchor(10 * MS), record(t, 0, 0, 50)); // malformed, cannot resolve
    c.poll(anchor(500 * MS));
    c.complete(anchor(500 * MS), record(t, 10 * MS, 10 * MS, 50));
    let events: Vec<_> = c.report().events().collect();
    assert_eq!(events.len(), 5);
    assert_eq!(events[1].action, Action::Begin(Some(t)));
    assert_eq!(events[2].record_error, Some(RecordError::Order));
    assert_eq!(events[3].action, Action::Poll);
    assert_eq!(events[3].missed[0].unwrap().first, 1);
    assert_eq!(
        events[4].action,
        Action::Complete(record(t, 10 * MS, 10 * MS, 50))
    );
    assert_eq!(c.report().pending(), None);
}

#[test]
fn missed_range_capacity_preserves_terminal_batch_without_eviction() {
    let mut c = controller();
    c.request_stop(anchor(0));
    let stop = c.report().stop();
    let max = c.report().capacity().missed_range_capacity;
    for slot in 1..=(max as u64 / 6 + 1) {
        c.poll(anchor(slot * 250 * MS));
    }
    assert!(c.report().capacity().exhausted);
    assert!(c.report().capacity().retained_missed_ranges <= max);
    let terminal = *c.report().terminal_evidence().unwrap();
    assert_eq!(terminal.missed.iter().flatten().count(), 6);
    let count = c.report().events().count();
    c.poll(anchor(600_000 * MS));
    assert_eq!(c.report().events().count(), count);
    assert_eq!(c.report().terminal_evidence(), Some(&terminal));
    assert_eq!(c.report().stop(), stop);
}

#[test]
fn poll_overflow_closes_open_eligibility_with_fixed_first_deadlines() {
    let mut c = controller();
    safe_pair(&mut c, 0);
    while c.report().capacity().retained_controls < c.report().capacity().event_capacity {
        c.poll(anchor(0));
    }
    assert!(c.report().fixture_compute_eligible());
    assert_eq!(c.report().stop(), None);
    c.poll(anchor(10 * MS));
    assert_eq!(reason(&c), StopReason::EvidenceCapacity);
    assert!(!c.report().fixture_compute_eligible());
    let stop = c.report().stop().unwrap();
    assert_eq!(
        (
            stop.at_ns,
            stop.cooperative_deadline_ns,
            stop.exit_deadline_ns
        ),
        (10 * MS, 1010 * MS, 3010 * MS)
    );
    let terminal = *c.report().terminal_evidence().unwrap();
    assert_eq!(
        (terminal.action, terminal.anchor),
        (Action::Poll, anchor(10 * MS))
    );
    c.request_stop(anchor(20 * MS));
    c.poll(anchor(3010 * MS));
    assert_eq!(c.report().stop(), Some(stop));
    assert_eq!(c.report().terminal_evidence(), Some(&terminal));
    assert!(c.report().exit_deadline_reached());
}

#[test]
fn cumulative_qpc_disagreement_without_suspend_is_sticky() {
    let mut c = controller();
    safe_pair(&mut c, 0);
    let mut a = anchor(10 * MS);
    a.interrupt_100ns = 14 * MS / 100;
    a.unbiased_100ns = a.interrupt_100ns;
    c.poll(a);
    assert_eq!(c.report().stop(), None);
    assert_eq!(c.report().now_ns(), 14 * MS);
    assert!(c.report().fixture_compute_eligible());
    let mut b = anchor(20 * MS);
    b.interrupt_100ns = 28 * MS / 100;
    b.unbiased_100ns = b.interrupt_100ns;
    c.poll(b); // consecutive drift is 4 ms, origin-relative drift is 8 ms
    assert_eq!(reason(&c), StopReason::Clock(ClockError::Disagreement));
    let stop = c.report().stop().unwrap();
    assert_eq!(stop.at_ns, 14 * MS);
    assert_eq!(c.report().events().last().unwrap().anchor, b);
    c.poll(anchor(30 * MS));
    assert_eq!(c.report().now_ns(), 14 * MS);
    assert_eq!(c.report().stop(), Some(stop));
    assert!(!c.report().fixture_compute_eligible());
    assert_eq!(
        c.report().events().last().unwrap().time,
        Err(ClockError::Disagreement)
    );
}

#[test]
fn individual_qpc_after_interrupt_and_unbiased_regressions() {
    for field in 0..3 {
        let mut c = controller();
        let mut trusted = anchor(10 * MS);
        trusted.qpc_after += MS;
        c.poll(trusted);
        assert_eq!(c.report().stop(), None);
        let mut bad = anchor(10 * MS);
        bad.qpc_after = trusted.qpc_after;
        match field {
            0 => bad.qpc_after -= 100,
            1 => bad.interrupt_100ns -= 1,
            2 => bad.unbiased_100ns -= 1,
            _ => unreachable!(),
        }
        assert!(bad.qpc_before <= bad.qpc_after);
        c.poll(bad);
        assert_eq!(reason(&c), StopReason::Clock(ClockError::Regression));
        assert_eq!(c.report().now_ns(), 10 * MS);
        assert_eq!(c.report().stop().unwrap().at_ns, 10 * MS);
        assert_eq!(c.report().events().last().unwrap().anchor, bad);
    }
}

#[test]
fn pending_getter_crosses_independent_acceptance_deadline() {
    let mut c = controller();
    for ns in (0..600_000 * MS).step_by(250 * MS as usize) {
        safe_pair(&mut c, ns);
    }
    assert!(c.report().fixture_compute_eligible());
    let start = 599_999 * MS;
    let t = c.begin(anchor(start)).unwrap();
    assert_eq!(t.metric, Metric::GraphicsClock);
    c.poll(anchor(600_000 * MS - 100));
    assert!(c.report().fixture_compute_eligible());
    c.poll(anchor(600_000 * MS));
    assert_eq!(reason(&c), StopReason::SessionDeadline);
    let stop = c.report().stop().unwrap();
    assert_eq!(
        (
            stop.at_ns,
            stop.cooperative_deadline_ns,
            stop.exit_deadline_ns
        ),
        (600_000 * MS, 600_000 * MS, 600_000 * MS)
    );
    assert_eq!(c.report().pending(), Some(t));
    c.complete(anchor(600_001 * MS), record(t, start, start, 1000));
    assert_eq!(c.report().pending(), None);
    assert_eq!(c.report().stop(), Some(stop));
    assert!(!c.report().fixture_compute_eligible());
    assert!(c.begin(anchor(600_001 * MS)).is_none());
}

#[test]
fn sample_overflow_keeps_unresolved_begin_and_terminal_authentic_return() {
    let mut c = controller();
    let t = c.begin(anchor(0)).unwrap();
    let mut wrong = t;
    wrong.binding.run[0] ^= 1;
    let forged = record(wrong, 0, 0, 50);
    c.complete(anchor(0), forged);
    assert_eq!(reason(&c), StopReason::Record(RecordError::Ticket));
    let stop = c.report().stop();
    while c.report().capacity().retained_samples < c.report().capacity().sample_capacity {
        c.complete(anchor(0), forged);
    }
    assert_eq!(c.report().pending(), Some(t));
    assert!(!c.report().capacity().exhausted);
    let raw = record(t, 0, 0, 50);
    c.complete(anchor(0), raw);
    assert!(c.report().capacity().exhausted);
    assert_eq!(c.report().pending(), None);
    let terminal = *c.report().terminal_evidence().unwrap();
    assert_eq!(terminal.action, Action::Complete(raw));
    assert_eq!(terminal.record_error, None);
    let mut events = c.report().events();
    assert_eq!(events.next().unwrap().action, Action::Initial);
    assert_eq!(events.next().unwrap().action, Action::Begin(Some(t)));
    assert_eq!(events.count() as u64, c.report().capacity().sample_capacity);
    assert_eq!(
        c.report()
            .events()
            .filter(|e| e.action == Action::Begin(Some(t)))
            .count(),
        1
    );
    assert!(c
        .report()
        .events()
        .filter(|e| matches!(e.action, Action::Complete(_)))
        .all(
            |e| e.action == Action::Complete(forged) && e.record_error == Some(RecordError::Ticket)
        ));
    assert_eq!(c.report().stop(), stop);
    assert!(!c.report().fixture_compute_eligible());
    assert!(c.begin(anchor(0)).is_none());
    assert_eq!(c.report().terminal_evidence(), Some(&terminal));
}

#[test]
fn duplicate_and_unsolicited_returns_close_otherwise_open_eligibility() {
    for duplicate in [false, true] {
        let mut c = controller();
        safe_pair(&mut c, 0);
        let safety = *c.report().safety();
        let raw = if duplicate {
            safety[0].unwrap()
        } else {
            let mut unsolicited = safety[0].unwrap();
            unsolicited.ticket.metric = Metric::GraphicsClock;
            unsolicited.unit = Unit::MHz;
            unsolicited
        };
        assert!(c.report().fixture_compute_eligible());
        assert_eq!(c.report().pending(), None);
        c.complete(anchor(0), raw);
        assert_eq!(reason(&c), StopReason::Record(RecordError::Ticket));
        assert!(!c.report().fixture_compute_eligible());
        assert_eq!(*c.report().safety(), safety);
        assert_eq!(c.report().pending(), None);
        let event = c.report().events().last().unwrap();
        assert_eq!(event.action, Action::Complete(raw));
        assert_eq!(event.record_error, Some(RecordError::Ticket));
    }
}

#[test]
fn control_exhaustion_retains_terminal_return_and_unpaired_begin() {
    let mut c = controller();
    safe_pair(&mut c, 0);
    let t = c.begin(anchor(0)).unwrap();
    while c.report().capacity().retained_controls < c.report().capacity().event_capacity {
        c.poll(anchor(0));
    }
    let capacity = c.report().capacity();
    assert!(capacity.retained_samples < capacity.sample_capacity);
    assert!(!capacity.exhausted);
    assert!(c.report().fixture_compute_eligible());
    assert_eq!(c.report().pending(), Some(t));
    let before: Vec<_> = c.report().events().collect();
    let raw = record(t, 0, 0, 1000);
    c.complete(anchor(10 * MS), raw);
    assert_eq!(reason(&c), StopReason::EvidenceCapacity);
    let stop = c.report().stop();
    assert!(c.report().capacity().exhausted);
    assert_eq!(c.report().pending(), None);
    assert_eq!(
        c.report().capacity().retained_samples,
        capacity.retained_samples
    );
    let terminal = *c.report().terminal_evidence().unwrap();
    assert_eq!(
        (terminal.action, terminal.record_error),
        (Action::Complete(raw), None)
    );
    assert_eq!(c.report().events().collect::<Vec<_>>(), before);
    assert_eq!(
        c.report()
            .events()
            .filter(|e| e.action == Action::Begin(Some(t)))
            .count(),
        1
    );
    c.request_stop(anchor(20 * MS));
    assert!(c.begin(anchor(20 * MS)).is_none());
    assert_eq!(c.report().stop(), stop);
    assert_eq!(c.report().terminal_evidence(), Some(&terminal));
    assert_eq!(c.report().events().collect::<Vec<_>>(), before);
}
