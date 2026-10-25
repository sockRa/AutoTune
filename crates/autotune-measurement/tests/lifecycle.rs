use autotune_measurement::lifecycle::{
    Binding, Deadlines, Decision, Error, Event, Lifecycle, Outcome, Phase, Policy, Termination,
};
use autotune_measurement::Identity;
use serde_json::{json, Value};

fn identity() -> Identity {
    Identity::new("run", "workload", "v1", u64::MAX, "gpu", "pci").unwrap()
}

fn binding() -> Binding {
    Binding::new(identity(), "worker").unwrap()
}

fn model() -> Lifecycle {
    Lifecycle::new(
        binding(),
        Policy::new(64, Deadlines::new(10, 20, 30, 40, 50).unwrap()).unwrap(),
    )
}

fn append(model: &mut Lifecycle, event: Event, at: u64) -> Decision {
    model.append(&binding(), event, at).unwrap()
}

fn advance_to(model: &mut Lifecycle, phase: Phase) {
    for (event, reached) in [
        (Event::LaunchedObserved, Phase::Launched),
        (Event::ReadyObserved, Phase::Ready),
        (Event::RunningObserved, Phase::Running),
    ] {
        if model.phase() == phase {
            return;
        }
        assert_eq!(append(model, event, 0), Decision::Applied);
        assert_eq!(model.phase(), reached);
    }
    assert_eq!(model.phase(), phase);
}

fn termination(reason: Termination) -> Event {
    Event::TerminatedObserved { reason }
}

fn assert_rejected(model: &mut Lifecycle, other: &Binding, event: Event, at: u64, error: Error) {
    let mut before: Value = serde_json::from_str(&model.to_json().unwrap()).unwrap();
    before["had_rejections"] = json!(true);
    assert_eq!(model.append(other, event, at), Err(error));
    assert!(model.had_rejections());
    assert_eq!(
        serde_json::from_str::<Value>(&model.to_json().unwrap()).unwrap(),
        before
    );
}

#[test]
fn ordered_completion_and_full_deterministic_json() {
    let mut model = model();
    assert_eq!(model.phase(), Phase::Requested);
    assert_eq!(model.outcome(), None);
    assert!(model.events().is_empty());
    assert_eq!(model.binding(), &binding());
    assert_eq!(model.binding().identity(), &identity());
    assert_eq!(model.binding().direct_worker(), "worker");
    for (at, event) in [
        (1, Event::LaunchedObserved),
        (2, Event::ReadyObserved),
        (3, Event::RunningObserved),
        (4, termination(Termination::Completed)),
    ] {
        assert_eq!(append(&mut model, event, at), Decision::Applied);
    }
    assert_eq!(model.phase(), Phase::Terminated);
    assert_eq!(model.outcome(), Some(Outcome::CompletedObserved));
    let export = model.to_json().unwrap();
    assert_eq!(export, model.to_json().unwrap());
    assert_eq!(
        serde_json::from_str::<Value>(&export).unwrap(),
        json!({
            "schema_version": 1,
            "provenance": "simulated",
            "time_basis": "run_relative_monotonic_nanoseconds",
            "binding": {
                "identity": { "run_id": "run", "workload_id": "workload", "workload_version": "v1",
                    "workload_seed": u64::MAX, "gpu_uuid": "gpu", "pci_bus_id": "pci" },
                "direct_worker": "worker"
            },
            "policy": { "event_capacity": 64, "deadlines": {
                "requested_ns": 10, "launched_ns": 20, "ready_ns": 30,
                "running_ns": 40, "stop_requested_ns": 50 } },
            "requested_at_ns": 0, "last_at_ns": 4, "phase": "terminated",
            "outcome": "completed_observed", "descendants": "unknown", "orphans": "unknown",
            "resource_release": "unknown", "correctness": "not_evaluated",
            "had_rejections": false, "had_conflicts": false,
            "events": [
                { "at_ns": 1, "event": { "kind": "launched_observed" },
                    "decision": { "kind": "applied" }, "phase": "launched", "outcome": null },
                { "at_ns": 2, "event": { "kind": "ready_observed" },
                    "decision": { "kind": "applied" }, "phase": "ready", "outcome": null },
                { "at_ns": 3, "event": { "kind": "running_observed" },
                    "decision": { "kind": "applied" }, "phase": "running", "outcome": null },
                { "at_ns": 4, "event": { "kind": "terminated_observed", "reason": "completed" },
                    "decision": { "kind": "applied" }, "phase": "terminated", "outcome": "completed_observed" }
            ]
        })
    );
    for (i, record) in model.events().iter().enumerate() {
        assert_eq!(record.at_ns(), i as u64 + 1);
        assert_eq!(record.decision(), Decision::Applied);
    }
    assert_eq!(
        model.events()[3].event(),
        termination(Termination::Completed)
    );
    assert_eq!(model.events()[3].phase(), Phase::Terminated);
    assert_eq!(
        model.events()[3].outcome(),
        Some(Outcome::CompletedObserved)
    );
}

#[test]
fn stop_from_every_active_phase_is_intent_and_repeats_do_not_extend_deadline() {
    for phase in [
        Phase::Requested,
        Phase::Launched,
        Phase::Ready,
        Phase::Running,
    ] {
        let mut model = model();
        advance_to(&mut model, phase);
        assert_eq!(
            append(&mut model, Event::StopRequested, 1),
            Decision::Applied
        );
        assert_eq!(model.phase(), Phase::StopRequested);
        assert_eq!(model.outcome(), None);
        for at in [1, 2, 49] {
            assert_eq!(append(&mut model, Event::StopRequested, at), Decision::NoOp);
            assert_eq!(model.outcome(), None);
        }
        assert_eq!(
            append(&mut model, Event::StopRequested, 50),
            Decision::BoundExceeded {
                phase: Phase::StopRequested,
                deadline_ns: 50
            }
        );
        assert_eq!(model.phase(), Phase::StopRequested);
        assert_eq!(model.outcome(), Some(Outcome::OutcomeUnknown));
    }
}

#[test]
fn explicit_stopped_or_completed_race_has_no_inferred_causality() {
    for phase in [Phase::Launched, Phase::Ready, Phase::Running] {
        let mut model = model();
        advance_to(&mut model, phase);
        append(&mut model, Event::StopRequested, 1);
        append(&mut model, termination(Termination::Stopped), 2);
        assert_eq!(model.phase(), Phase::Terminated);
        assert_eq!(model.outcome(), Some(Outcome::StopObserved));
    }
    let mut model = model();
    advance_to(&mut model, Phase::Running);
    append(&mut model, Event::StopRequested, 1);
    append(&mut model, termination(Termination::Completed), 2);
    assert_eq!(model.outcome(), Some(Outcome::CompletedObserved));
}

#[test]
fn never_launched_requires_explicit_assertion_and_never_fabricates_termination() {
    for stopped in [false, true] {
        let mut model = model();
        append(&mut model, Event::AdvanceTime, 1);
        assert_eq!(model.outcome(), None);
        if stopped {
            append(&mut model, Event::StopRequested, 2);
        }
        append(&mut model, Event::NeverLaunchedObserved, 3);
        assert_eq!(model.outcome(), Some(Outcome::NeverLaunched));
        assert_eq!(
            model.phase(),
            if stopped {
                Phase::StopRequested
            } else {
                Phase::Requested
            }
        );
    }
}

#[test]
fn crashes_and_lost_observation_close_unknown_without_fabricated_phase() {
    for phase in [
        Phase::Launched,
        Phase::Ready,
        Phase::Running,
        Phase::StopRequested,
    ] {
        for event in [
            termination(Termination::Crashed),
            Event::WorkerLost,
            Event::ControllerLost,
        ] {
            let mut model = model();
            advance_to(
                &mut model,
                if phase == Phase::StopRequested {
                    Phase::Launched
                } else {
                    phase
                },
            );
            if phase == Phase::StopRequested {
                append(&mut model, Event::StopRequested, 0);
            }
            append(&mut model, event, 1);
            assert_eq!(model.outcome(), Some(Outcome::OutcomeUnknown));
            assert_eq!(
                model.phase(),
                if matches!(event, Event::TerminatedObserved { .. }) {
                    Phase::Terminated
                } else {
                    phase
                }
            );
        }
    }
    for stopped in [false, true] {
        let mut model = model();
        if stopped {
            append(&mut model, Event::StopRequested, 0);
        }
        append(&mut model, Event::ControllerLost, 1);
        assert_eq!(model.outcome(), Some(Outcome::OutcomeUnknown));
        assert_eq!(
            model.phase(),
            if stopped {
                Phase::StopRequested
            } else {
                Phase::Requested
            }
        );
    }
}

#[test]
fn all_phase_bounds_below_equal_above_and_expiry_precedes_event_semantics() {
    for (phase, deadline) in [
        (Phase::Requested, 10),
        (Phase::Launched, 20),
        (Phase::Ready, 30),
        (Phase::Running, 40),
        (Phase::StopRequested, 50),
    ] {
        for at in [deadline - 1, deadline, deadline + 1] {
            let mut model = model();
            if phase == Phase::StopRequested {
                append(&mut model, Event::StopRequested, 0);
            } else {
                advance_to(&mut model, phase);
            }
            // Even invalid semantics are retained, not applied, at/after expiry.
            let event = if at < deadline {
                Event::AdvanceTime
            } else {
                termination(Termination::Completed)
            };
            let decision = append(&mut model, event, at);
            if at < deadline {
                assert_eq!(decision, Decision::NoOp);
                assert_eq!(model.outcome(), None);
            } else {
                assert_eq!(
                    decision,
                    Decision::BoundExceeded {
                        phase,
                        deadline_ns: deadline
                    }
                );
                assert_eq!(model.outcome(), Some(Outcome::OutcomeUnknown));
            }
            assert_eq!(model.phase(), phase);
        }
    }
}

#[test]
fn expired_destination_is_retained_as_observed_phase_but_closes_unknown() {
    for (phase, event, deadlines) in [
        (
            Phase::Requested,
            Event::LaunchedObserved,
            [100, 5, 100, 100, 100],
        ),
        (
            Phase::Launched,
            Event::ReadyObserved,
            [100, 100, 5, 100, 100],
        ),
        (
            Phase::Ready,
            Event::RunningObserved,
            [100, 100, 100, 5, 100],
        ),
        (
            Phase::Requested,
            Event::StopRequested,
            [100, 100, 100, 100, 5],
        ),
        (
            Phase::Launched,
            Event::StopRequested,
            [100, 100, 100, 100, 5],
        ),
        (Phase::Ready, Event::StopRequested, [100, 100, 100, 100, 5]),
        (
            Phase::Running,
            Event::StopRequested,
            [100, 100, 100, 100, 5],
        ),
    ] {
        for at in [4, 5, 6] {
            let [a, b, c, d, e] = deadlines;
            let mut model = Lifecycle::new(
                binding(),
                Policy::new(10, Deadlines::new(a, b, c, d, e).unwrap()).unwrap(),
            );
            advance_to(&mut model, phase);
            let destination = match event {
                Event::LaunchedObserved => Phase::Launched,
                Event::ReadyObserved => Phase::Ready,
                Event::RunningObserved => Phase::Running,
                _ => Phase::StopRequested,
            };
            let decision = append(&mut model, event, at);
            if at < 5 {
                assert_eq!(decision, Decision::Applied);
                assert_eq!(model.outcome(), None);
            } else {
                assert_eq!(
                    decision,
                    Decision::BoundExceeded {
                        phase: destination,
                        deadline_ns: 5
                    }
                );
                assert_eq!(model.outcome(), Some(Outcome::OutcomeUnknown));
            }
            assert_eq!(model.phase(), destination);
            assert_eq!(model.events().last().unwrap().event(), event);
            if phase == Phase::Requested && event == Event::StopRequested {
                assert_eq!(model.events().len(), 1);
                if at < 5 {
                    for reason in [Termination::Crashed, Termination::Stopped] {
                        let event = termination(reason);
                        assert_rejected(
                            &mut model,
                            &binding(),
                            event,
                            at,
                            Error::InvalidPhase {
                                phase: Phase::StopRequested,
                                event,
                            },
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn every_terminal_outcome_freezes_phase_and_retains_all_late_events_as_conflicts() {
    let mut terminals = Vec::new();
    for reason in [
        Termination::Completed,
        Termination::Stopped,
        Termination::Crashed,
    ] {
        let mut model = model();
        advance_to(&mut model, Phase::Running);
        if reason == Termination::Stopped {
            append(&mut model, Event::StopRequested, 0);
        }
        append(&mut model, termination(reason), 0);
        terminals.push(model);
    }
    for event in [
        Event::NeverLaunchedObserved,
        Event::WorkerLost,
        Event::ControllerLost,
        Event::AdvanceTime,
    ] {
        let mut model = model();
        if event == Event::WorkerLost {
            advance_to(&mut model, Phase::Launched);
        }
        append(
            &mut model,
            event,
            if event == Event::AdvanceTime { 10 } else { 0 },
        );
        terminals.push(model);
    }
    for mut model in terminals {
        let phase = model.phase();
        let outcome = model.outcome();
        assert!(outcome.is_some());
        let original = model.events().to_vec();
        for event in [
            Event::LaunchedObserved,
            Event::ReadyObserved,
            Event::RunningObserved,
            Event::StopRequested,
            termination(Termination::Completed),
            termination(Termination::Stopped),
            termination(Termination::Crashed),
            Event::NeverLaunchedObserved,
            Event::WorkerLost,
            Event::ControllerLost,
        ] {
            assert_eq!(append(&mut model, event, 60), Decision::Conflict);
            assert_eq!(model.phase(), phase);
            assert_eq!(model.outcome(), outcome);
            assert_eq!(model.events().last().unwrap().event(), event);
        }
        assert_eq!(append(&mut model, Event::AdvanceTime, 60), Decision::NoOp);
        assert!(model.had_conflicts());
        assert!(!model.had_rejections());
        assert_eq!(&model.events()[..original.len()], original.as_slice());
        let value: Value = serde_json::from_str(&model.to_json().unwrap()).unwrap();
        assert_eq!(value["had_conflicts"], true);
        assert_eq!(
            value["events"][original.len()]["decision"],
            json!({"kind": "conflict"})
        );
    }
}

#[test]
fn invalid_transitions_reject_atomically_and_do_not_advance_time() {
    for phase in [
        Phase::Requested,
        Phase::Launched,
        Phase::Ready,
        Phase::Running,
        Phase::StopRequested,
    ] {
        let mut model = model();
        if phase == Phase::StopRequested {
            append(&mut model, Event::StopRequested, 0);
        } else {
            advance_to(&mut model, phase);
        }
        let invalid: Vec<Event> = match phase {
            Phase::Requested => vec![
                Event::ReadyObserved,
                Event::RunningObserved,
                Event::WorkerLost,
                termination(Termination::Completed),
                termination(Termination::Stopped),
                termination(Termination::Crashed),
            ],
            Phase::Launched => vec![
                Event::LaunchedObserved,
                Event::RunningObserved,
                Event::NeverLaunchedObserved,
                termination(Termination::Completed),
                termination(Termination::Stopped),
            ],
            Phase::Ready => vec![
                Event::LaunchedObserved,
                Event::ReadyObserved,
                Event::NeverLaunchedObserved,
                termination(Termination::Completed),
                termination(Termination::Stopped),
            ],
            Phase::Running => vec![
                Event::LaunchedObserved,
                Event::ReadyObserved,
                Event::RunningObserved,
                Event::NeverLaunchedObserved,
                termination(Termination::Stopped),
            ],
            _ => vec![
                Event::LaunchedObserved,
                Event::ReadyObserved,
                Event::RunningObserved,
                Event::WorkerLost,
                termination(Termination::Completed),
                termination(Termination::Stopped),
                termination(Termination::Crashed),
            ],
        };
        for event in invalid {
            assert_rejected(
                &mut model,
                &binding(),
                event,
                1,
                Error::InvalidPhase { phase, event },
            );
            if event == Event::RunningObserved && phase != Phase::Running {
                let event = termination(Termination::Completed);
                assert_rejected(
                    &mut model,
                    &binding(),
                    event,
                    1,
                    Error::InvalidPhase { phase, event },
                );
            }
        }
        append(&mut model, Event::AdvanceTime, 0);
        assert!(model.had_rejections());
    }
    // Stop after launch also forbids returning to readiness/running and never-launched assertions.
    let mut model = model();
    advance_to(&mut model, Phase::Launched);
    append(&mut model, Event::StopRequested, 0);
    for event in [
        Event::LaunchedObserved,
        Event::ReadyObserved,
        Event::RunningObserved,
        Event::NeverLaunchedObserved,
    ] {
        assert_rejected(
            &mut model,
            &binding(),
            event,
            1,
            Error::InvalidPhase {
                phase: Phase::StopRequested,
                event,
            },
        );
    }
}

#[test]
fn all_identity_fields_and_worker_are_exact_bindings() {
    let fields = ["run", "workload", "v1", "gpu", "pci"];
    for index in 0..7 {
        let mut changed = fields;
        if index < 5 {
            changed[index] = "different";
        }
        let id = Identity::new(
            changed[0],
            changed[1],
            changed[2],
            if index == 5 { 0 } else { u64::MAX },
            changed[3],
            changed[4],
        )
        .unwrap();
        let other = Binding::new(id, if index == 6 { "Worker" } else { "worker" }).unwrap();
        let mut model = model();
        assert_rejected(
            &mut model,
            &other,
            Event::LaunchedObserved,
            1,
            Error::BindingMismatch,
        );
        let event = termination(Termination::Crashed);
        assert_rejected(
            &mut model,
            &binding(),
            event,
            1,
            Error::InvalidPhase {
                phase: Phase::Requested,
                event,
            },
        );
        append(&mut model, Event::LaunchedObserved, 0);
        assert_eq!(model.phase(), Phase::Launched);
    }
}

#[test]
fn invalid_policies_labels_and_string_extremes() {
    for index in 0..5 {
        let mut values = [u64::MAX; 5];
        values[index] = 0;
        let [a, b, c, d, e] = values;
        let phase = [
            Phase::Requested,
            Phase::Launched,
            Phase::Ready,
            Phase::Running,
            Phase::StopRequested,
        ][index];
        assert_eq!(
            Deadlines::new(a, b, c, d, e),
            Err(Error::ZeroDeadline { phase })
        );
    }
    let deadlines = Deadlines::new(1, u64::MAX, 2, 1, 3).unwrap();
    assert_eq!(deadlines.for_phase(Phase::Terminated), None);
    assert_eq!(Policy::new(0, deadlines), Err(Error::ZeroCapacity));
    for label in [
        "",
        "  ",
        "\nworker",
        "worker\0",
        "\u{7f}",
        &"x".repeat(257),
        &"é".repeat(129),
    ] {
        assert_eq!(
            Binding::new(identity(), label),
            Err(Error::InvalidWorkerLabel)
        );
    }
    for label in [" worker ".to_owned(), "x".repeat(256), "é".repeat(128)] {
        assert_eq!(
            Binding::new(identity(), &label).unwrap().direct_worker(),
            label
        );
    }
    assert!(std::error::Error::source(&Error::ZeroCapacity).is_none());
    assert_eq!(Error::ZeroCapacity.to_string(), "ZeroCapacity");
}

#[test]
fn capacity_time_and_extreme_integers_are_bounded_without_eviction_or_progress() {
    let max = Deadlines::new(u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX).unwrap();
    let mut huge = Lifecycle::new(binding(), Policy::new(usize::MAX, max).unwrap());
    // A maximum policy does not request a maximum allocation.
    assert!(huge.events().is_empty());
    assert_eq!(huge.policy(), &Policy::new(usize::MAX, max).unwrap());
    append(&mut huge, Event::AdvanceTime, u64::MAX - 1);
    assert_rejected(
        &mut huge,
        &binding(),
        Event::AdvanceTime,
        0,
        Error::TimeRegression,
    );
    append(&mut huge, Event::LaunchedObserved, u64::MAX - 1);
    assert_eq!(
        append(&mut huge, termination(Termination::Crashed), u64::MAX),
        Decision::BoundExceeded {
            phase: Phase::Launched,
            deadline_ns: u64::MAX
        }
    );
    assert_eq!(huge.phase(), Phase::Launched);
    let value: Value = serde_json::from_str(&huge.to_json().unwrap()).unwrap();
    assert_eq!(value["last_at_ns"].as_u64(), Some(u64::MAX));
    assert_eq!(value["events"][0]["at_ns"].as_u64(), Some(u64::MAX - 1));
    let mut full = Lifecycle::new(binding(), Policy::new(1, max).unwrap());
    append(&mut full, Event::AdvanceTime, 1);
    assert_rejected(
        &mut full,
        &binding(),
        Event::LaunchedObserved,
        u64::MAX,
        Error::CapacityReached,
    );
    assert_eq!(full.phase(), Phase::Requested);
    assert_eq!(full.outcome(), None);
    assert_eq!(full.events().len(), 1);
    let mut terminal_full = Lifecycle::new(binding(), Policy::new(1, max).unwrap());
    append(&mut terminal_full, Event::ControllerLost, 1);
    assert_rejected(
        &mut terminal_full,
        &binding(),
        Event::LaunchedObserved,
        2,
        Error::CapacityReached,
    );
    assert!(!terminal_full.had_conflicts());
    // Terminal validation still rejects invalid time/binding rather than recording a conflict.
    assert_rejected(
        &mut huge,
        &binding(),
        Event::StopRequested,
        0,
        Error::TimeRegression,
    );
    let other = Binding::new(identity(), "other").unwrap();
    assert_rejected(
        &mut huge,
        &other,
        Event::StopRequested,
        u64::MAX,
        Error::BindingMismatch,
    );
}

#[test]
fn owned_sources_and_prior_records_are_independent_and_exports_do_not_advance_time() {
    let mut label = String::from("worker");
    let id = identity();
    let source = Binding::new(id.clone(), &label).unwrap();
    let mut model = Lifecycle::new(
        source.clone(),
        Policy::new(10, Deadlines::new(10, 20, 30, 40, 50).unwrap()).unwrap(),
    );
    label.clear();
    drop(source);
    drop(id);
    assert_eq!(model.binding(), &binding());
    let initial = model.to_json().unwrap();
    assert_eq!(initial, model.to_json().unwrap());
    append(&mut model, Event::LaunchedObserved, 1);
    let first = model.events()[0].clone();
    append(&mut model, Event::ReadyObserved, 1);
    assert_eq!(model.events()[0], first);
    assert_eq!(first.phase(), Phase::Launched);
    assert_eq!(first.outcome(), None);
    assert_eq!(model.outcome(), None);
}

#[test]
fn validation_precedes_expiry_and_terminal_time_alone_is_not_a_conflict() {
    let mut model = model();
    let other = Binding::new(identity(), "other").unwrap();
    assert_rejected(
        &mut model,
        &other,
        Event::LaunchedObserved,
        10,
        Error::BindingMismatch,
    );
    assert_eq!(model.outcome(), None);
    append(&mut model, Event::AdvanceTime, 9);
    assert_rejected(
        &mut model,
        &binding(),
        Event::LaunchedObserved,
        8,
        Error::TimeRegression,
    );
    assert_eq!(model.outcome(), None);
    append(&mut model, Event::LaunchedObserved, 10);
    let value: Value = serde_json::from_str(&model.to_json().unwrap()).unwrap();
    assert_eq!(
        value["events"][1],
        json!({
            "at_ns": 10,
            "event": { "kind": "launched_observed" },
            "decision": { "kind": "bound_exceeded", "phase": "requested", "deadline_ns": 10 },
            "phase": "requested", "outcome": "outcome_unknown"
        })
    );
    assert_eq!(
        append(&mut model, Event::AdvanceTime, u64::MAX),
        Decision::NoOp
    );
    assert!(!model.had_conflicts());
    assert_eq!(model.phase(), Phase::Requested);
    assert_eq!(model.outcome(), Some(Outcome::OutcomeUnknown));
}
