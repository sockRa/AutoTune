use autotune_benchmark_worker::{
    monitor::{Anchor, Binding, Metric},
    sampler::{
        supervisor::{Effect, Fault, Input, ProcessObservation, Selection, Supervisor},
        wire::{Message, WireBinding},
    },
    worker::DiscoveryBinding,
};
use autotune_probe::report::Field;

const UUID: &str = "GPU-03030303-0303-0303-0303-030303030303";
const PCI: &str = "00000000:01:02.0";
fn binding() -> WireBinding {
    WireBinding {
        monitor: Binding {
            session: [1; 16],
            run: [2; 16],
            device: DiscoveryBinding::parse(UUID, PCI).unwrap().identity,
            clock_token: [4; 16],
            qpc_frequency: 1_000_000_000,
        },
        manifest_digest: [5; 32],
    }
}
fn anchor(ns: u64) -> Anchor {
    Anchor {
        clock_token: [4; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn supervisor() -> Supervisor {
    Supervisor::new(
        binding(),
        anchor(0),
        Selection {
            raw_uuid: UUID.into(),
            raw_pci: PCI.into(),
            sampler_digest: [6; 32],
        },
    )
    .unwrap()
}
fn ready() -> Message {
    Message::Ready {
        raw_uuid: UUID.into(),
        raw_pci: PCI.into(),
        sampler_digest: [6; 32],
        frequency: 1_000_000_000,
        anchor: anchor(1_000),
        runtime: "observed runtime".into(),
    }
}
fn boot(s: &mut Supervisor) -> Vec<Effect> {
    s.advance(Some(anchor(0)), Input::Launched { creation: 123 });
    s.advance(Some(anchor(2_000)), Input::Message(ready()))
}
fn query(effects: Vec<Effect>) -> autotune_benchmark_worker::monitor::Ticket {
    effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Send(Message::Query(t)) => Some(t),
            _ => None,
        })
        .expect("query issued")
}

#[test]
fn requires_launch_and_fresh_exact_ready_before_one_query() {
    let mut s = supervisor();
    assert!(!s
        .advance(Some(anchor(0)), Input::Tick)
        .iter()
        .any(|e| matches!(e, Effect::Send(Message::Query(_)))));
    let first = query(boot(&mut s));
    assert_eq!(first.metric, Metric::Temperature);
    assert!(!s
        .advance(Some(anchor(10_000_000)), Input::Tick)
        .iter()
        .any(|e| matches!(e, Effect::Send(Message::Query(_)))));
    assert_eq!(s.report().pending(), Some(first));
    assert!(!s.report().native_admission_available());
}

#[test]
fn pending_getter_does_not_block_ticks_force_or_unknown_finalization() {
    let mut s = supervisor();
    let t = query(boot(&mut s));
    let effects = s.advance(Some(anchor(750_000_000)), Input::Tick);
    assert!(effects.iter().any(|e| matches!(e, Effect::SignalStop)));
    assert_eq!(s.report().pending(), Some(t));
    let terminal = *s.terminal().unwrap();
    assert_eq!(terminal.at_ns, 750_000_000);
    assert!(s
        .advance(Some(anchor(1_750_000_000)), Input::Tick)
        .iter()
        .any(|e| matches!(e, Effect::Terminate)));
    assert!(s
        .advance(Some(anchor(3_750_000_000)), Input::Tick)
        .iter()
        .any(|e| matches!(e, Effect::Finalize)));
    assert!(!s.exit_confirmed());
    assert_eq!(*s.terminal().unwrap(), terminal);
}

#[test]
fn actual_query_start_and_controller_receipt_are_retained_separately() {
    let mut s = supervisor();
    let t = query(boot(&mut s));
    s.advance(
        Some(anchor(30_000)),
        Input::Message(Message::MetricRecord {
            ticket: t,
            start: anchor(10_000),
            end: anchor(20_000),
            raw: Field::available("degC", 42),
        }),
    );
    let sample = s.samples().next().unwrap();
    assert_eq!(sample.ticket.start_ns, 2_000);
    assert_eq!(sample.start, anchor(10_000));
    assert_eq!(sample.end, anchor(20_000));
    assert_eq!(sample.receipt, anchor(30_000));
    assert!(s.terminal().is_none());
}

#[test]
fn actual_start_before_issue_stops_without_refreshing_safety() {
    let mut s = supervisor();
    let t = query(boot(&mut s));
    s.advance(
        Some(anchor(30_000)),
        Input::Message(Message::MetricRecord {
            ticket: t,
            start: anchor(1_000),
            end: anchor(20_000),
            raw: Field::available("degC", 42),
        }),
    );
    assert_eq!(s.terminal().unwrap().reason, Fault::Timing);
    assert!(s.report().safety()[0].is_none());
    assert_eq!(s.samples().count(), 1);
}

#[test]
fn missing_clock_requests_immediate_termination_without_a_fabricated_anchor() {
    let mut s = supervisor();
    boot(&mut s);
    let effects = s.advance(None, Input::Tick);
    assert!(effects.iter().any(|e| matches!(e, Effect::SignalStop)));
    assert!(effects.iter().any(|e| matches!(e, Effect::Terminate)));
    assert_eq!(s.terminal().unwrap().reason, Fault::ClockCapture);
    assert_eq!(s.terminal().unwrap().at_ns, 2_000);
    assert!(!s.exit_confirmed());
}

#[test]
fn eof_done_and_termination_success_do_not_prove_exit_or_reopen_session() {
    let mut s = supervisor();
    boot(&mut s);
    s.advance(Some(anchor(10_000)), Input::Fault(Fault::Transport));
    let terminal = *s.terminal().unwrap();
    s.advance(
        Some(anchor(20_000)),
        Input::Process(ProcessObservation {
            creation: 123,
            wait_status: 258,
            wait_error: None,
            exit_code: Some(0),
            exit_error: None,
            termination: Some(Ok(())),
            active_processes: Some(0),
            accounting_error: None,
        }),
    );
    assert!(!s.exit_confirmed());
    s.advance(
        Some(anchor(30_000)),
        Input::Process(ProcessObservation {
            creation: 123,
            wait_status: 0,
            wait_error: None,
            exit_code: Some(0),
            exit_error: None,
            termination: None,
            active_processes: Some(0),
            accounting_error: None,
        }),
    );
    assert!(s.exit_confirmed());
    assert_eq!(*s.terminal().unwrap(), terminal);
}

#[test]
fn stop_before_ready_and_duplicate_or_foreign_ready_are_terminal() {
    let mut s = supervisor();
    s.advance(Some(anchor(0)), Input::Stop);
    assert!(!boot(&mut s)
        .iter()
        .any(|e| matches!(e, Effect::Send(Message::Query(_)))));
    let mut s = supervisor();
    boot(&mut s);
    s.advance(Some(anchor(10_000)), Input::Message(ready()));
    assert_eq!(s.terminal().unwrap().reason, Fault::Protocol);
    let mut s = supervisor();
    s.advance(Some(anchor(0)), Input::Launched { creation: 123 });
    let mut wrong = ready();
    if let Message::Ready { raw_pci, .. } = &mut wrong {
        *raw_pci = "00000000:02:02.0".into();
    }
    s.advance(Some(anchor(2_000)), Input::Message(wrong));
    assert_eq!(s.terminal().unwrap().reason, Fault::Identity);
}

#[test]
fn late_exit_after_observation_timeout_is_supplemental_only() {
    let mut s = supervisor();
    boot(&mut s);
    s.advance(Some(anchor(10_000)), Input::Stop);
    s.advance(Some(anchor(3_000_010_000)), Input::Tick);
    s.advance(
        Some(anchor(3_000_020_000)),
        Input::Process(ProcessObservation {
            creation: 123,
            wait_status: 0,
            wait_error: None,
            exit_code: Some(0),
            exit_error: None,
            termination: None,
            active_processes: Some(0),
            accounting_error: None,
        }),
    );
    assert!(!s.exit_confirmed());
    assert!(matches!(
        s.traces().last().unwrap().input,
        Input::Process(_)
    ));
}

#[test]
fn invalid_clock_has_an_independent_observation_deadline_input() {
    let mut s = supervisor();
    boot(&mut s);
    s.advance(None, Input::Tick);
    let effects = s.advance(None, Input::ObservationExpired);
    assert!(effects.iter().any(|e| matches!(e, Effect::Finalize)));
    assert!(!s.exit_confirmed());
}

#[test]
fn sampler_transport_and_raw_retention_cannot_spend_the_same_remaining_ram() {
    let mut s = supervisor();
    let budget = s.budget();
    let left = 32 * 1024 * 1024 - budget.used();
    let _transport = budget.reserve(left).unwrap();
    s.advance(Some(anchor(0)), Input::Launched { creation: 123 });
    s.advance(Some(anchor(2_000)), Input::Message(ready()));
    assert_eq!(s.terminal().unwrap().reason, Fault::Capacity);
    assert!(!s.report().native_admission_available());
}

#[test]
fn combined_host_observation_retains_resources_and_closes_on_unknown_commit() {
    use autotune_benchmark_worker::sampler::runtime::ResourceObservation;
    let mut s = supervisor();
    let resources = ResourceObservation {
        commit_bytes: None,
        user_100ns: Some(1),
        kernel_100ns: Some(1),
        memory_error: Some(5),
        time_error: None,
    };
    s.advance(
        Some(anchor(100)),
        Input::Host {
            watchdog_100ns: 1,
            resources,
            sampler_resources: None,
            process: None,
        },
    );
    assert_eq!(s.terminal().unwrap().reason, Fault::Resources);
    match s.traces().last().unwrap().input {
        Input::Host {
            watchdog_100ns,
            resources: r,
            ..
        } => {
            assert_eq!(watchdog_100ns, 1);
            assert_eq!(r, resources);
        }
        _ => panic!("raw host observation lost"),
    }
}

#[test]
fn uncertain_query_send_stops_and_preserves_the_pending_ticket_and_raw_code() {
    use autotune_benchmark_worker::sampler::runtime::Operation;
    let mut s = supervisor();
    let ticket = query(boot(&mut s));
    s.advance(
        Some(anchor(3_000)),
        Input::Operation {
            operation: Operation::QuerySend,
            result: Err(109),
        },
    );
    assert_eq!(s.terminal().unwrap().reason, Fault::Transport);
    assert_eq!(s.report().pending(), Some(ticket));
    assert!(matches!(
        s.traces().last().unwrap().input,
        Input::Operation {
            result: Err(109),
            ..
        }
    ));
}
#[test]
fn rejected_frame_closes_admission_and_keeps_exact_partial_bytes() {
    use autotune_benchmark_worker::sampler::{
        framing::{Phase, ReadFault, Rejection},
        wire::Lane,
    };
    let mut s = supervisor();
    let t = query(boot(&mut s));
    s.advance(
        Some(anchor(3_000)),
        Input::Rejected {
            lane: Lane::Ordinary,
            bytes: vec![8, 0, 0, 0, 123],
            rejection: Rejection {
                fault: ReadFault::Os(995),
                phase: Phase::Body,
                reported: 1,
                queue_overflow: false,
            },
        },
    );
    assert_eq!(s.terminal().unwrap().reason, Fault::Transport);
    assert_eq!(s.report().pending(), Some(t));
    match &s.traces().last().unwrap().input {
        Input::Rejected {
            bytes, rejection, ..
        } => {
            assert_eq!(bytes, &[8, 0, 0, 0, 123]);
            assert_eq!(rejection.reported, 1);
        }
        _ => panic!("raw rejection lost"),
    }
}
#[test]
fn io_cancellation_return_does_not_establish_thread_or_process_exit() {
    use autotune_benchmark_worker::sampler::{runtime::IoObservation, wire::Lane};
    let mut s = supervisor();
    boot(&mut s);
    s.advance(
        Some(anchor(3_000)),
        Input::IoShutdown(IoObservation {
            lane: Lane::Ordinary,
            reader: false,
            finished: false,
            cancellation: Ok(()),
        }),
    );
    assert!(!s.exit_confirmed());
    match s.traces().last().unwrap().input {
        Input::IoShutdown(o) => {
            assert!(!o.finished);
            assert_eq!(o.cancellation, Ok(()));
        }
        _ => panic!("thread observation lost"),
    }
}

#[test]
fn aggregate_metadata_stops_when_shared_ram_fills_before_metadata_ceiling() {
    let mut s = supervisor();
    // The monitor and trace storage already consume most ordinary RAM. These
    // late records hit shared RAM before 4 MiB of metadata, so they must stop
    // without pretending that the independent metadata ceiling was reached.
    for _ in 0..200 {
        s.advance(
            Some(anchor(0)),
            Input::Message(Message::Ready {
                raw_uuid: UUID.into(),
                raw_pci: PCI.into(),
                sampler_digest: [6; 32],
                frequency: 1_000_000_000,
                anchor: anchor(0),
                runtime: "x".repeat(50_000),
            }),
        );
    }
    assert!(s.retention_overflow());
    assert_eq!(
        s.traces()
            .iter()
            .filter(|t| matches!(t.input, Input::MetadataOverflow { .. }))
            .count(),
        0
    );
    let retained: usize = s
        .traces()
        .iter()
        .filter_map(|t| match &t.input {
            Input::Message(Message::Ready { runtime, .. }) => Some(runtime.len()),
            _ => None,
        })
        .sum();
    assert!(retained < 4 * 1024 * 1024);
    assert!(retained > 0);
    assert!(s.budget().used() <= 32 * 1024 * 1024);
    let retained_count = s.traces().len();
    // Later valid-sized payloads cannot reopen admission or evict retained
    // evidence to make room. Allocation failure stays explicitly sticky.
    s.advance(
        Some(anchor(0)),
        Input::Message(Message::Ready {
            raw_uuid: UUID.into(),
            raw_pci: PCI.into(),
            sampler_digest: [6; 32],
            frequency: 1_000_000_000,
            anchor: anchor(0),
            runtime: "x".repeat(50_000),
        }),
    );
    assert_eq!(s.traces().len(), retained_count);
    assert!(s.terminal().is_some());
}

#[test]
fn query_elapsed_times_cannot_hide_reversed_or_overlapping_qpc_brackets() {
    for (start, end) in [
        (
            Anchor {
                qpc_before: 1500,
                qpc_after: 1500,
                ..anchor(2000)
            },
            anchor(3000),
        ),
        (
            Anchor {
                qpc_after: 4000,
                ..anchor(3000)
            },
            Anchor {
                qpc_before: 3500,
                qpc_after: 4500,
                ..anchor(4000)
            },
        ),
    ] {
        let mut s = supervisor();
        let ticket = query(boot(&mut s));
        s.advance(
            Some(anchor(6000)),
            Input::Message(Message::MetricRecord {
                ticket,
                start,
                end,
                raw: Field::available("degC", 42),
            }),
        );
        assert_eq!(s.terminal().unwrap().reason, Fault::Timing);
        assert_eq!(s.report().pending(), Some(ticket));
        assert!(matches!(
            s.traces().last().unwrap().input,
            Input::Message(Message::MetricRecord { .. })
        ));
    }
}

#[test]
fn reserved_rejected_evidence_reaches_metadata_limit_before_failure_pool_exhaustion() {
    use autotune_benchmark_worker::sampler::{
        framing::{Framer, ReadStep, Source},
        ingress::Received,
        wire::{Channel, Lane},
    };
    let mut s = supervisor();
    // Populate ordinary RAM without exhausting it, then transfer already
    // charged failure snapshots through the real ingress/retention seam.
    for _ in 0..40 {
        let mut message = ready();
        if let Message::Ready { runtime, .. } = &mut message {
            *runtime = "x".repeat(50_000);
        }
        s.advance(Some(anchor(0)), Input::Message(message));
    }
    assert!(!s.retention_overflow());
    struct FailedBody {
        body: bool,
    }
    impl Source for FailedBody {
        fn read(&mut self, out: &mut [u8]) -> ReadStep {
            if !self.body {
                self.body = true;
                out.copy_from_slice(&50_004u32.to_le_bytes());
                ReadStep {
                    reported: 4,
                    error: None,
                }
            } else {
                out.fill(0xff);
                ReadStep {
                    reported: out.len(),
                    error: Some(995),
                }
            }
        }
    }
    let mut decoder = Channel::new(binding(), Lane::Ordinary).unwrap();
    for _ in 0..100 {
        let packet = Framer::new(s.failure_budget())
            .unwrap()
            .read(&mut FailedBody { body: false });
        Received::decode(Lane::Ordinary, packet, &mut decoder, Err(()))
            .apply(&mut s, Some(anchor(0)));
    }
    assert!(s.retention_overflow());
    let markers: Vec<_> = s
        .traces()
        .iter()
        .filter_map(|t| match t.input {
            Input::MetadataOverflow { attempted_bytes } => Some(attempted_bytes),
            _ => None,
        })
        .collect();
    assert_eq!(markers, [50_004]);
    // This time neither the ordinary nor the reserved failure pool is full:
    // only the aggregate metadata ceiling rejected the next owned snapshot.
    let _ordinary_room = s.budget().reserve(100_000).unwrap();
    let _failure_room = s.failure_budget().reserve(1_000_000).unwrap();
    assert!(s
        .traces()
        .iter()
        .any(|t| matches!(t.input, Input::Rejected { .. })));
}

#[test]
fn failed_launch_keeps_cleanup_facts_and_never_establishes_successful_launch_or_exit() {
    let mut s = supervisor();
    let process = ProcessObservation {
        creation: 0,
        wait_status: 258,
        wait_error: None,
        exit_code: None,
        exit_error: None,
        termination: Some(Err(5)),
        active_processes: Some(0),
        accounting_error: None,
    };
    let effects = s.advance(
        Some(anchor(100)),
        Input::LaunchFailure {
            stage: "assign_job",
            error: 5,
            assigned: false,
            creation: None,
            process,
        },
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::SignalStop)));
    assert_eq!(s.terminal().unwrap().reason, Fault::Transport);
    assert!(s.launch_ns().is_none());
    assert!(!s.exit_confirmed());
    assert!(s.report().pending().is_none());
    s.advance(
        Some(anchor(200)),
        Input::Process(ProcessObservation {
            wait_status: 0,
            exit_code: Some(96),
            termination: Some(Ok(())),
            ..process
        }),
    );
    assert!(!s.exit_confirmed());
    assert_eq!(s.terminal().unwrap().at_ns, 100);
    let raw = autotune_benchmark_worker::sampler::producer::raw_events(&s).unwrap();
    assert!(raw
        .contains("\"stage\":\"assign_job\",\"error\":\"5\",\"assigned\":false,\"creation\":null"));
    assert!(raw.contains("\"termination\":\"5\""));
    assert!(raw.contains("\"termination\":\"requested\""));
}
