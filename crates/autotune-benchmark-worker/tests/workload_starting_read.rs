use autotune_benchmark_worker::{
    evidence::{Digest, Domain, Value},
    monitor::Anchor,
    sampler::{budget::Budget, child::NativeAdmission},
    workload::{
        manifest::Artifacts,
        nvml_trace::Ledger,
        settings::{Block, Draft, Observation},
        starting::{Scope, ScopeDraft, Snapshot},
        starting_bootstrap::Declaration,
        starting_control::Completion,
        starting_export::{Binding, ControlFacts, Facts, Response},
        starting_read::{Fault, Receipt},
        starting_runtime::{Fault as TraceFault, Trace},
    },
};
use autotune_probe::{
    nvml::{SamplerCall, SamplerInfo, SamplerObserver, SamplerOutput, SamplerReturn},
    report::{Field, Runtime},
};
include!("support/starting_runtime_export.rs");

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
fn read(f: &Fixture, r: &Response) -> Receipt {
    Receipt::read(
        r.canonical().as_bytes(),
        &f.s,
        &f.decl,
        r.digest(),
        10_000_000_000,
        &f.b,
    )
    .ok()
    .unwrap()
}
fn obj(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut o = autotune_benchmark_worker::evidence::Object::new();
    for (k, v) in fields {
        o.insert(k, v).unwrap();
    }
    Value::Object(o)
}
fn edit(v: &Value, path: &[&str], replacement: Value) -> Value {
    if path.is_empty() {
        return replacement;
    }
    match v {
        Value::Object(o) => {
            let mut result = autotune_benchmark_worker::evidence::Object::new();
            for (k, v) in o.iter() {
                result
                    .insert(
                        k,
                        if k == path[0] {
                            edit(v, &path[1..], replacement.clone())
                        } else {
                            v.clone()
                        },
                    )
                    .unwrap();
            }
            Value::Object(result)
        }
        Value::Array(a) => Value::Array(
            a.iter()
                .enumerate()
                .map(|(i, v)| {
                    if i.to_string() == path[0] {
                        edit(v, &path[1..], replacement.clone())
                    } else {
                        v.clone()
                    }
                })
                .collect(),
        ),
        _ => panic!("invalid fixture edit"),
    }
}
fn changed(f: &Fixture, r: &Response, path: &[&str], replacement: Value) -> Result<Receipt, Fault> {
    let v = autotune_benchmark_worker::evidence::decode(r.canonical().as_bytes(), 2 * 1024 * 1024)
        .unwrap();
    let bytes = edit(&v, path, replacement).to_canonical();
    let digest = Digest::of(Domain::RawArtifact, bytes.as_bytes());
    Receipt::read(
        bytes.as_bytes(),
        &f.s,
        &f.decl,
        digest,
        10_000_000_000,
        &f.b,
    )
}
#[test]
fn actual_export_roundtrips_reported_facts_and_restores_original_clock_token() {
    let f = fixture();
    let i = info(&f.s);
    let r = build(&f, Some(&i), Some(&f.snapshot), None, completion()).unwrap();
    let receipt = read(&f, &r);
    assert_eq!(receipt.canonical(), r.canonical());
    assert_eq!(receipt.digest(), r.digest());
    assert_eq!(receipt.scope_digest(), f.s.digest());
    assert_eq!(receipt.declaration(), f.decl.tag());
    assert_eq!(receipt.summary().epoch, a(0));
    assert_eq!(receipt.summary().first_entry, a(1000));
    assert_eq!(receipt.summary().receipt, a(1100));
    assert_eq!(receipt.summary().completion, Some(a(5000)));
    assert_eq!(receipt.summary().snapshot, Some(f.snapshot.digest()));
    assert_eq!(
        receipt.snapshot().unwrap().snapshot().canonical(),
        f.snapshot.canonical()
    );
    assert_eq!(
        receipt.snapshot().unwrap().snapshot().source(),
        autotune_benchmark_worker::workload::settings::Source::SimulatedFixture
    );
    assert_eq!(receipt.summary().observed_frequency, Some(1_000_000_000));
}
#[test]
fn missing_values_and_failed_native_facts_stay_reported_without_success_inference() {
    let mut f = fixture();
    f.trace.fault = Some(TraceFault::Read);
    f.trace.cleanup_status = Some(-99);
    let gate = NativeAdmission::default();
    let mut times = [Some(a(1200)), None].into_iter();
    let mut observer = f.ledger.observer(&gate, || times.next().flatten());
    assert!(observer.before(SamplerCall::Count));
    assert!(!observer.returned(
        SamplerCall::Count,
        SamplerReturn {
            status: -99,
            output: SamplerOutput::Count(u32::MAX)
        }
    ));
    drop(observer);
    let r = build(
        &f,
        None,
        None,
        Some(Block::Missing),
        ControlFacts::default(),
    )
    .unwrap();
    let receipt = read(&f, &r);
    assert_eq!(receipt.summary().native_fault, Some("native:-99".into()));
    assert_eq!(receipt.summary().native_calls, 1);
    assert_eq!(receipt.summary().cleanup_status, Some(-99));
    assert_eq!(receipt.summary().trace_fault, Some("read".into()));
    assert_eq!(receipt.summary().snapshot, None);
    assert_eq!(receipt.summary().completion, None);
    let receipt = changed(
        &f,
        &r,
        &["native_failure"],
        obj([
            ("reason", Value::Text("query_failed".into())),
            ("vendor_status", Value::I64(-99)),
            ("detail", Value::Text("raw failure".into())),
        ]),
    )
    .unwrap_or_else(|_| panic!("valid failure"));
    assert_eq!(
        receipt.summary().native_failure,
        Some("query_failed".into())
    );
    let receipt = changed(
        &f,
        &r,
        &["clock_error"],
        obj([
            ("kind", Value::Text("counter_failed".into())),
            ("at", Value::Text("before".into())),
            ("code", Value::U64(5)),
        ]),
    )
    .unwrap_or_else(|_| panic!("valid clock error"));
    assert_eq!(receipt.summary().clock_error, Some("counter_failed".into()));
}
#[test]
fn rejects_foreign_scope_declaration_digest_and_original_read_deadline() {
    let f = fixture();
    let r = build(&f, None, None, None, ControlFacts::default()).unwrap();
    let foreign = foreign_scope(&f.b);
    let foreign_decl = Declaration::new(&foreign, a(1000), &f.b).unwrap();
    assert!(matches!(
        Receipt::read(
            r.canonical().as_bytes(),
            &f.s,
            &foreign_decl,
            r.digest(),
            10_000_000_000,
            &f.b
        ),
        Err(Fault::Declaration)
    ));
    assert!(matches!(
        Receipt::read(
            r.canonical().as_bytes(),
            &f.s,
            &f.decl,
            d(11),
            10_000_000_000,
            &f.b
        ),
        Err(Fault::Digest)
    ));
    assert!(matches!(
        Receipt::read(
            r.canonical().as_bytes(),
            &f.s,
            &f.decl,
            r.digest(),
            14_000_000_000,
            &f.b
        ),
        Err(Fault::Deadline)
    ));
    assert!(changed(&f, &r, &["request_digest"], Value::Text(d(11).to_hex())).is_err());
    assert!(changed(&f, &r, &["declaration"], Value::Text(d(11).to_hex())).is_err());
    assert!(changed(&f, &r, &["frequency"], Value::U64(99)).is_err());
    assert!(changed(&f, &r, &["epoch", "interrupt_100ns"], Value::U64(99)).is_err());
    assert!(changed(&f, &r, &["first_entry", "qpc_before"], Value::U64(0)).is_err());
}
#[test]
fn canonical_shape_and_nested_snapshot_source_and_schema_are_verified() {
    let f = fixture();
    let r = build(&f, None, Some(&f.snapshot), None, completion()).unwrap();
    let spaced = format!(" {}", r.canonical());
    assert!(Receipt::read(
        spaced.as_bytes(),
        &f.s,
        &f.decl,
        Digest::of(Domain::RawArtifact, spaced.as_bytes()),
        10_000_000_000,
        &f.b
    )
    .is_err());
    assert!(changed(
        &f,
        &r,
        &["trace", "calls", "0", "operation"],
        Value::Text("describe".into())
    )
    .is_err());
    assert!(changed(
        &f,
        &r,
        &["snapshot", "source"],
        Value::Text("native_getter_path".into())
    )
    .is_err());
    assert!(changed(&f, &r, &["snapshot", "digest"], Value::Text(d(20).to_hex())).is_err());
    let malformed = "{}";
    let digest = Digest::of(Domain::RawArtifact, malformed.as_bytes());
    assert!(changed(
        &f,
        &r,
        &["snapshot"],
        obj([
            ("canonical", Value::Text(malformed.into())),
            ("digest", Value::Text(digest.to_hex())),
            ("source", Value::Text("simulated_fixture".into()))
        ])
    )
    .is_err());
}
#[test]
fn exact_abi_buffer_shapes_and_numeric_widths_are_required() {
    let f = fixture();
    let r = build(&f, None, None, None, ControlFacts::default()).unwrap();
    let native = |output| {
        Value::Array(vec![obj([
            ("call", Value::Text("pci:0".into())),
            ("before", Value::Null),
            ("after", Value::Null),
            ("attempted", Value::Bool(true)),
            ("raw", obj([("status", Value::I64(-5)), ("output", output)])),
        ])])
    };
    assert!(changed(
        &f,
        &r,
        &["native_calls"],
        native(obj([
            ("kind", Value::Text("text".into())),
            ("hex", Value::Text("00".into()))
        ]))
    )
    .is_err());
    assert!(changed(
        &f,
        &r,
        &["native_calls"],
        native(obj([
            ("kind", Value::Text("count".into())),
            ("value", Value::U64(u64::MAX))
        ]))
    )
    .is_err());
    let pci = |numbers| {
        obj([
            ("kind", Value::Text("pci".into())),
            ("legacy", Value::Text("00".repeat(16))),
            ("numbers", Value::Array(numbers)),
            ("bus_id", Value::Text("00".repeat(32))),
        ])
    };
    assert!(changed(
        &f,
        &r,
        &["native_calls"],
        native(pci(vec![Value::U64(0); 4]))
    )
    .is_err());
    assert!(changed(
        &f,
        &r,
        &["native_calls"],
        native(pci(vec![Value::U64(0); 5]))
    )
    .is_ok());
    assert!(changed(&f, &r, &["native_fault"], Value::Text("native:-05".into())).is_err());
}
#[test]
fn bounded_reader_capacity_and_complexity_fail_before_acceptance() {
    let f = fixture();
    let r = build(&f, None, None, None, ControlFacts::default()).unwrap();
    assert!(matches!(
        Receipt::read(
            r.canonical().as_bytes(),
            &f.s,
            &f.decl,
            r.digest(),
            10_000_000_000,
            &Budget::new(1).unwrap()
        ),
        Err(Fault::Capacity)
    ));
    assert!(changed(
        &f,
        &r,
        &["native_calls"],
        Value::Array(vec![Value::Null; 131_073])
    )
    .is_err());
    let used = f.b.used();
    {
        let receipt = read(&f, &r);
        assert!(f.b.used() > used);
        drop(receipt);
    }
    assert_eq!(f.b.used(), used);
    let mut i = info(&f.s);
    i.runtime.driver_version = Field::available(
        "text",
        serde_json::Value::Array(vec![serde_json::Value::Null; 512]),
    );
    let r = build(&f, Some(&i), None, None, ControlFacts::default()).unwrap();
    assert!(matches!(
        Receipt::read(
            r.canonical().as_bytes(),
            &f.s,
            &f.decl,
            r.digest(),
            10_000_000_000,
            &f.b
        ),
        Err(Fault::Capacity)
    ));
}
#[test]
fn nonzero_original_epoch_is_retained_and_receipt_deadline_equality_rejected() {
    let b = budget();
    let base = scope(&b);
    let mut d = base.draft().clone();
    d.epoch = a(7_000_000_000);
    let s = Scope::freeze(d, &b).unwrap();
    let decl = Declaration::new(&s, a(7_000_001_000), &b).unwrap();
    let ledger = Ledger::new(
        s.draft().frequency,
        s.draft().epoch,
        a(7_000_001_000),
        10_000_000_000,
        &b,
    )
    .unwrap();
    let trace = Trace::new(&s, a(7_000_001_000), &b).unwrap();
    let response = |receipt| {
        Response::build(
            Binding {
                scope: &s,
                declaration: &decl,
                first_entry: a(7_000_001_000),
                receipt,
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
        .unwrap()
    };
    let r = response(a(16_999_999_900));
    let receipt = Receipt::read(
        r.canonical().as_bytes(),
        &s,
        &decl,
        r.digest(),
        10_000_000_000,
        &b,
    )
    .ok()
    .unwrap();
    assert_eq!(receipt.summary().epoch, a(7_000_000_000));
    assert_eq!(receipt.summary().receipt, a(16_999_999_900));
    drop(receipt);
    drop(r);
    let r = response(a(17_000_000_000));
    assert!(matches!(
        Receipt::read(
            r.canonical().as_bytes(),
            &s,
            &decl,
            r.digest(),
            10_000_000_000,
            &b
        ),
        Err(Fault::Deadline)
    ));
}
#[test]
fn session_requires_verified_delivery_before_strict_reading() {
    use autotune_benchmark_worker::workload::{
        starting_delivery::Chunks, starting_session::Session,
    };
    let f = fixture();
    let r = build(&f, None, None, None, ControlFacts::default()).unwrap();
    let mut session = Session::new(&f.decl, &f.s, &f.b).unwrap();
    assert!(matches!(
        session.read_report(&f.s, &f.decl, &f.b),
        Err(Fault::Incomplete)
    ));
    Chunks::new(&r, &f.b)
        .unwrap()
        .send(
            |bytes| {
                session.accept(10_000, bytes);
                Ok(())
            },
            || true,
        )
        .unwrap();
    assert!(session.complete());
    let receipt = session.read_report(&f.s, &f.decl, &f.b).ok().unwrap();
    assert_eq!(receipt.digest(), r.digest());
}
#[test]
fn largest_ledger_export_fits_reader_representation_and_budget() {
    let mut f = fixture();
    let gate = NativeAdmission::default();
    let mut observer = f.ledger.observer(&gate, || Some(a(2000)));
    for _ in 0..3077 {
        assert!(observer.before(SamplerCall::Count));
        assert!(observer.returned(
            SamplerCall::Count,
            SamplerReturn {
                status: 0,
                output: SamplerOutput::Count(1)
            }
        ));
    }
    drop(observer);
    let r = build(&f, None, None, None, ControlFacts::default()).unwrap();
    assert_eq!(read(&f, &r).summary().native_calls, 3077);
}
