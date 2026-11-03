use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, child::NativeAdmission, wire::Lane},
    workload::{
        manifest::Artifacts,
        nvml_trace::{Fault as LedgerFault, Ledger},
        settings::{Block, Draft, Observation, Source},
        starting::{Scope, ScopeDraft, Snapshot},
        starting_bootstrap::Declaration,
        starting_control::{Completion, State},
        starting_export::{Binding, ClockFault, ControlFacts, Facts, Response},
        starting_runtime::{Fault as TraceFault, Trace},
    },
};
use autotune_probe::{
    nvml::{SamplerCall, SamplerInfo, SamplerObserver, SamplerOutput, SamplerReturn},
    report::{Field, Runtime},
};

fn d(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [3; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn budget() -> Budget {
    Budget::new(32 * 1024 * 1024).unwrap()
}
fn foreign_scope(b: &Budget) -> Scope {
    let base = scope(b);
    let mut draft = base.draft().clone();
    draft.capture_nonce = [9; 16];
    Scope::freeze(draft, b).unwrap()
}
fn scope(b: &Budget) -> Scope {
    Scope::freeze(
        ScopeDraft {
            session: [1; 16],
            capture_nonce: [2; 16],
            clock_token: [3; 16],
            frequency: 1_000_000_000,
            epoch: a(0),
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: d(4),
                lockfile: d(5),
                controller: d(6),
                worker: d(7),
                sampler: d(8),
                ptx: d(9),
                protocol: d(10),
            },
            read_approval_ref: "CPU-only response fixture".into(),
        },
        b,
    )
    .unwrap()
}
fn info(s: &Scope) -> SamplerInfo {
    SamplerInfo {
        raw_uuid: s.draft().raw_uuid.clone(),
        raw_pci: s.draft().raw_pci.clone(),
        runtime: Runtime {
            library_path: None,
            driver_version: Field::available("", 1),
            nvml_version: Field::available("", 2),
            cuda_driver_version: Field::available("", 3),
            header_api_version: 12,
        },
    }
}
fn draft(s: &Scope) -> Draft {
    Draft {
        raw_uuid: s.draft().raw_uuid.clone(),
        raw_pci: s.draft().raw_pci.clone(),
        observations: autotune_probe::nvml::SettingMetric::ALL.map(|kind| Observation {
            kind,
            before: Some(a(2000)),
            after: Some(a(2100)),
            raw: Some(Field::available(kind.unit(), 1)),
        }),
        halt: None,
    }
}
struct Fixture {
    b: Budget,
    s: Scope,
    decl: Declaration,
    ledger: Ledger,
    trace: Trace,
    snapshot: Snapshot,
}
fn fixture() -> Fixture {
    let b = budget();
    let s = scope(&b);
    let decl = Declaration::new(&s, a(1000), &b).unwrap();
    let ledger = Ledger::new(1_000_000_000, a(0), a(1000), 10_000_000_000, &b).unwrap();
    let trace = Trace::new(&s, a(1000), &b).unwrap();
    let snapshot = Snapshot::fixture(&s, a(2000), &draft(&s), &b).unwrap();
    Fixture {
        b,
        s,
        decl,
        ledger,
        trace,
        snapshot,
    }
}
fn build<'a>(
    f: &'a Fixture,
    info: Option<&'a SamplerInfo>,
    snapshot: Option<&'a Snapshot>,
    snapshot_error: Option<Block>,
    control: ControlFacts,
) -> Result<Response, Block> {
    Response::build(
        Binding {
            scope: &f.s,
            declaration: &f.decl,
            first_entry: a(1000),
            receipt: a(1100),
            deadline_ns: 10_000_000_000,
        },
        Facts {
            info,
            ledger: &f.ledger,
            snapshot,
            snapshot_error,
            trace: &f.trace,
            control,
            native_failure: None,
            clock_error: None,
            control_error: None,
            event_clone_error: None,
            unreported_completion: None,
        },
        &f.b,
    )
}
fn completion() -> ControlFacts {
    ControlFacts {
        completion: Some(Completion {
            anchor: a(5000),
            initialized_session: true,
            cleanup_attempted: true,
            cleanup_status: Some(0),
        }),
        completion_receipt: Some(a(5100)),
        finished: Some(true),
        fault: None,
        window: None,
        trigger: Some(a(4000)),
        event_status: Some(0),
        event_error: None,
        transport_error: None,
        observed_frequency: Some(1_000_000_000),
        rejected_receipt: None,
    }
}

#[test]
fn success_binds_scope_declaration_snapshot_and_completion() {
    let f = fixture();
    let info = info(&f.s);
    let response = build(&f, Some(&info), Some(&f.snapshot), None, completion()).unwrap();
    let text = response.canonical();
    assert!(text.contains("\"scope\":\"pilot_starting_settings_response\""));
    assert!(text.contains(&format!("\"request_digest\":\"{}\"", f.s.digest().to_hex())));
    assert!(text.contains(&format!("\"declaration\":\"{}\"", f.decl.tag().to_hex())));
    assert!(text.contains("\"source\":\"simulated_fixture\""));
    assert!(text.contains("\"cleanup_status\":\"0\""));
    assert!(text.contains("\"observed_frequency\":\"1000000000\""));
    assert_eq!(response.scope_digest(), f.s.digest());
    assert_eq!(response.declaration(), f.decl.tag());
    assert_eq!(
        response.digest(),
        Digest::of(
            autotune_benchmark_worker::evidence::Domain::RawArtifact,
            text.as_bytes()
        )
    );
}

#[test]
fn ledger_faults_overflow_and_rejected_returns_are_retained() {
    let mut f = fixture();
    let gate = NativeAdmission::default();
    gate.stop();
    {
        let mut o = f.ledger.observer(&gate, || Some(a(2000)));
        assert!(!o.before(SamplerCall::Count));
        assert!(!o.returned(
            SamplerCall::Count,
            SamplerReturn {
                status: 0,
                output: SamplerOutput::Count(1),
            }
        ));
    }
    assert_eq!(f.ledger.fault(), Some(LedgerFault::Stopped));
    let response = build(&f, None, None, None, ControlFacts::default()).unwrap();
    let text = response.canonical();
    assert!(text.contains("\"native_fault\":\"stopped\""));
    assert!(text.contains("\"native_rejected\":{\"call\":\"count\""));
}

#[test]
fn trace_and_snapshot_faults_are_not_hidden_or_made_successful() {
    let mut f = fixture();
    let state = State::new(&f.s, a(1000), &f.b).unwrap();
    f.trace.fail(TraceFault::Read, &state.shared());
    let response = build(&f, None, None, Some(Block::Clock), ControlFacts::default()).unwrap();
    let text = response.canonical();
    assert!(text.contains("\"fault\":\"read\""));
    assert!(text.contains("\"snapshot\":null"));
    assert!(text.contains("\"snapshot_error\":\"clock\""));
}

#[test]
fn foreign_or_stale_scope_and_declaration_are_rejected() {
    let f = fixture();
    let foreign_scope = foreign_scope(&f.b);
    let foreign_decl = Declaration::new(&foreign_scope, a(1000), &f.b).unwrap();
    let foreign_snapshot =
        Snapshot::fixture(&foreign_scope, a(2000), &draft(&foreign_scope), &f.b).unwrap();
    assert_eq!(
        Response::build(
            Binding {
                scope: &f.s,
                declaration: &foreign_decl,
                first_entry: a(1000),
                receipt: a(1100),
                deadline_ns: 10_000_000_000,
            },
            Facts {
                info: None,
                ledger: &f.ledger,
                snapshot: None,
                snapshot_error: None,
                trace: &f.trace,
                control: ControlFacts::default(),
                native_failure: None,
                clock_error: None,
                control_error: None,
                event_clone_error: None,
                unreported_completion: None,
            },
            &f.b,
        )
        .err(),
        Some(Block::Scope)
    );
    assert_eq!(
        Response::build(
            Binding {
                scope: &f.s,
                declaration: &f.decl,
                first_entry: a(1000),
                receipt: a(1100),
                deadline_ns: 10_000_000_000,
            },
            Facts {
                info: None,
                ledger: &f.ledger,
                snapshot: Some(&foreign_snapshot),
                snapshot_error: None,
                trace: &f.trace,
                control: ControlFacts::default(),
                native_failure: None,
                clock_error: None,
                control_error: None,
                event_clone_error: None,
                unreported_completion: None,
            },
            &f.b,
        )
        .err(),
        Some(Block::Scope)
    );
}

#[test]
fn reported_sampler_identity_must_match_the_frozen_scope() {
    let f = fixture();
    let mut wrong = info(&f.s);
    wrong.raw_uuid = "GPU-ffffffff-ffff-ffff-ffff-ffffffffffff".into();
    assert_eq!(
        build(&f, Some(&wrong), None, None, ControlFacts::default()).err(),
        Some(Block::Scope)
    );
}

#[test]
fn missing_finalization_capacity_and_native_session_are_explicit() {
    let f = fixture();
    let mut control = completion();
    control.completion = None;
    control.finished = None;
    control.completion_receipt = None;
    let response = build(&f, None, None, None, control).unwrap();
    assert!(response.canonical().contains("\"completion\":null"));
    assert!(response.canonical().contains("\"finished\":null"));
    assert_eq!(
        Response::build(
            Binding {
                scope: &f.s,
                declaration: &f.decl,
                first_entry: a(1000),
                receipt: a(1100),
                deadline_ns: 10_000_000_000,
            },
            Facts {
                info: None,
                ledger: &f.ledger,
                snapshot: None,
                snapshot_error: None,
                trace: &f.trace,
                control: ControlFacts::default(),
                native_failure: None,
                clock_error: Some(ClockFault::ZeroToken),
                control_error: None,
                event_clone_error: None,
                unreported_completion: None,
            },
            &Budget::new(1).unwrap(),
        )
        .err(),
        Some(Block::Capacity)
    );
}

#[test]
fn canonical_bytes_and_digest_are_deterministic() {
    let f = fixture();
    let info = info(&f.s);
    let one = build(&f, Some(&info), Some(&f.snapshot), None, completion()).unwrap();
    let two = build(&f, Some(&info), Some(&f.snapshot), None, completion()).unwrap();
    assert_eq!(one.canonical(), two.canonical());
    assert_eq!(one.digest(), two.digest());
}

#[test]
fn ledger_capacity_is_reflected_in_the_export_bound() {
    let b = budget();
    let s = scope(&b);
    let decl = Declaration::new(&s, a(1000), &b).unwrap();
    let trace = Trace::new(&s, a(1000), &b).unwrap();
    let gate = NativeAdmission::default();
    let mut ledger = Ledger::new(1_000_000_000, a(0), a(1000), 10_000_000_000, &b).unwrap();
    let mut ns = 2000;
    {
        let mut o = ledger.observer(&gate, || {
            ns += 1000;
            Some(a(ns))
        });
        for _ in 0..(1 + 1 + 1024 * 3 + 3 + 1) {
            if !o.before(SamplerCall::Count) {
                break;
            }
            assert!(o.returned(
                SamplerCall::Count,
                SamplerReturn {
                    status: 0,
                    output: SamplerOutput::Count(1)
                }
            ));
        }
    }
    assert_eq!(ledger.fault(), Some(LedgerFault::Capacity));
    let response = Response::build(
        Binding {
            scope: &s,
            declaration: &decl,
            first_entry: a(1000),
            receipt: a(1100),
            deadline_ns: 10_000_000_000,
        },
        Facts {
            info: None,
            ledger: &ledger,
            snapshot: None,
            snapshot_error: None,
            trace: &trace,
            control: ControlFacts::default(),
            native_failure: None,
            clock_error: None,
            control_error: None,
            event_clone_error: None,
            unreported_completion: None,
        },
        &b,
    )
    .unwrap();
    let text = response.canonical();
    assert!(text.contains("\"native_fault\":\"capacity\""));
    assert!(text.contains("\"native_overflow\":{\"call\":\"count\""));
    assert!(text.len() > 1024);
    assert!(text.len() < 2 * 1024 * 1024);
}

#[test]
fn unused_fixture_fields_stay_publicly_coherent() {
    // Keeps the fixture's direct source tag available without inventing native
    // provenance in a CPU-only test.
    let f = fixture();
    assert_eq!(f.snapshot.source(), Source::SimulatedFixture);
    let _ = Lane::Ordinary;
}
