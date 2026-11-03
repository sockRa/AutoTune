use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, child::NativeAdmission},
    workload::{
        manifest::Artifacts,
        nvml_trace::Ledger,
        settings::Draft,
        starting::{Scope, ScopeDraft, Snapshot},
        starting_bootstrap::Declaration,
        starting_delivery::{Chunks, Status},
        starting_export::{Binding, ControlFacts, Facts, Response},
        starting_receive::{Fault, Receiver},
        starting_runtime::Trace,
    },
};
use autotune_probe::{
    nvml::{
        SamplerCall, SamplerInfo, SamplerObserver, SamplerOutput, SamplerReturn, SettingMetric,
    },
    report::Field,
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
fn scope(b: &Budget, nonce: u128) -> Scope {
    Scope::freeze(
        ScopeDraft {
            session: [1; 16],
            capture_nonce: nonce.to_le_bytes(),
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
            read_approval_ref: "CPU-only reception fixture".into(),
        },
        b,
    )
    .unwrap()
}
fn draft(s: &Scope) -> Draft {
    Draft {
        raw_uuid: s.draft().raw_uuid.clone(),
        raw_pci: s.draft().raw_pci.clone(),
        observations: SettingMetric::ALL.map(|kind| {
            autotune_benchmark_worker::workload::settings::Observation {
                kind,
                before: Some(a(2000)),
                after: Some(a(2100)),
                raw: Some(Field::available(kind.unit(), 1)),
            }
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
    fixture_with(320, 1)
}
fn fixture_with(entries: usize, nonce: u128) -> Fixture {
    let b = budget();
    let s = scope(&b, nonce);
    let decl = Declaration::new(&s, a(1000), &b).unwrap();
    let mut ledger = Ledger::new(1_000_000_000, a(0), a(1000), 10_000_000_000, &b).unwrap();
    if entries > 0 {
        let gate = NativeAdmission::default();
        let mut o = ledger.observer(&gate, || Some(a(2000)));
        for _ in 0..entries {
            assert!(o.before(SamplerCall::Count));
            assert!(o.returned(
                SamplerCall::Count,
                SamplerReturn {
                    status: 0,
                    output: SamplerOutput::Text([b'x'; 256]),
                }
            ));
        }
    }
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
fn response(f: &Fixture) -> Response {
    Response::build(
        Binding {
            scope: &f.s,
            declaration: &f.decl,
            first_entry: a(1000),
            receipt: a(1100),
            deadline_ns: 10_000_000_000,
        },
        Facts {
            info: None::<&SamplerInfo>,
            ledger: &f.ledger,
            snapshot: Some(&f.snapshot),
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
    .unwrap()
}
fn frames(f: &Fixture) -> (Vec<Vec<u8>>, Response) {
    let response = response(f);
    let chunks = Chunks::new(&response, &f.b).unwrap();
    let mut frames = Vec::new();
    chunks
        .send(
            |bytes| {
                frames.push(bytes.to_vec());
                Ok(())
            },
            || true,
        )
        .unwrap();
    (frames, response)
}
fn receiver(f: &Fixture) -> Receiver {
    Receiver::new(&f.decl, &f.s, 0, 10_000_000_000, &f.b).unwrap()
}

#[test]
fn complete_reception_exposes_the_exact_bound_payload() {
    let f = fixture();
    let (frames, response) = frames(&f);
    assert!(frames.len() > 1);
    let mut rx = receiver(&f);
    for (index, frame) in frames.iter().enumerate() {
        let status = rx.accept(1_000 + index as u64, frame).unwrap();
        if index + 1 == frames.len() {
            assert_eq!(
                status,
                Status::Complete {
                    bytes: response.canonical().len(),
                    digest: response.digest(),
                }
            );
        }
    }
    assert!(rx.complete());
    assert_eq!(rx.received() as usize, frames.len());
    assert_eq!(rx.canonical(), Some(response.canonical().as_bytes()));
    assert_eq!(
        rx.status(),
        Status::Complete {
            bytes: response.canonical().len(),
            digest: response.digest(),
        }
    );
}

#[test]
fn stop_before_completion_is_sticky_and_hides_every_payload() {
    let f = fixture();
    let (frames, _) = frames(&f);
    let mut rx = receiver(&f);
    rx.accept(1_000, &frames[0]).unwrap();
    rx.stop();
    assert_eq!(rx.accept(1_001, &frames[1]), Err(Fault::Stopped));
    for frame in &frames[1..] {
        let _ = rx.accept(1_002, frame);
    }
    assert!(!rx.complete());
    assert_eq!(rx.canonical(), None);
    assert_eq!(rx.fault(), Some(Fault::Stopped));
    assert!(matches!(rx.status(), Status::Incomplete { .. }));
}

#[test]
fn a_chunk_at_the_deadline_is_rejected() {
    let f = fixture();
    let (frames, _) = frames(&f);
    let mut rx = receiver(&f);
    rx.accept(9_999_999_999, &frames[0]).unwrap();
    assert_eq!(rx.accept(10_000_000_000, &frames[1]), Err(Fault::Deadline));
    assert_eq!(rx.canonical(), None);
    assert_eq!(rx.fault(), Some(Fault::Deadline));
    assert_eq!(rx.accept(10_000_000_001, &frames[1]), Err(Fault::Deadline));
}

#[test]
fn a_receiving_clock_regression_is_a_sticky_fault() {
    let f = fixture();
    let (frames, _) = frames(&f);
    let mut rx = receiver(&f);
    rx.accept(2_000, &frames[0]).unwrap();
    assert_eq!(rx.accept(1_999, &frames[1]), Err(Fault::Clock));
    assert_eq!(rx.accept(2_001, &frames[1]), Err(Fault::Clock));
    assert_eq!(rx.canonical(), None);
    assert!(!rx.complete());
}

#[test]
fn a_foreign_chunk_is_a_sticky_assembly_fault() {
    let mine = fixture();
    let other = fixture_with(320, 9);
    let (foreign, _) = frames(&other);
    let (own, _) = frames(&mine);
    let mut rx = receiver(&mine);
    rx.accept(1_000, &own[0]).unwrap();
    assert_eq!(
        rx.accept(1_001, &foreign[1]),
        Err(Fault::Assembly(
            autotune_benchmark_worker::workload::starting_delivery::Fault::Scope
        ))
    );
    assert_eq!(rx.accept(1_002, &own[1]), Err(rx.fault().unwrap()));
    assert_eq!(rx.canonical(), None);
}

#[test]
fn a_duplicate_chunk_is_a_gap_fault_and_does_not_resume() {
    let f = fixture();
    let (frames, _) = frames(&f);
    let mut rx = receiver(&f);
    rx.accept(1_000, &frames[0]).unwrap();
    let duplicate = rx.accept(1_001, &frames[0]);
    assert_eq!(
        duplicate,
        Err(Fault::Assembly(
            autotune_benchmark_worker::workload::starting_delivery::Fault::Gap
        ))
    );
    assert!(rx.accept(1_002, &frames[1]).is_err());
    assert_eq!(rx.canonical(), None);
}

#[test]
fn a_chunk_after_completion_is_rejected_without_replacing_the_payload() {
    let f = fixture();
    let (frames, response) = frames(&f);
    let mut rx = receiver(&f);
    let last = frames.len() - 1;
    for (index, frame) in frames.iter().enumerate() {
        rx.accept(1_000 + index as u64, frame).unwrap();
    }
    assert!(rx.complete());
    assert_eq!(
        rx.accept(2_000, &frames[last]),
        Err(Fault::Assembly(
            autotune_benchmark_worker::workload::starting_delivery::Fault::Stopped
        ))
    );
    assert_eq!(rx.fault(), None);
    assert_eq!(rx.canonical(), Some(response.canonical().as_bytes()));
}

#[test]
fn a_deadline_that_does_not_follow_the_epoch_is_rejected() {
    let f = fixture();
    assert_eq!(
        Receiver::new(&f.decl, &f.s, 5, 5, &f.b).err(),
        Some(Fault::Deadline)
    );
    assert_eq!(
        Receiver::new(&f.decl, &f.s, 6, 5, &f.b).err(),
        Some(Fault::Deadline)
    );
    assert!(Receiver::new(&f.decl, &f.s, 5, 6, &f.b).is_ok());
}

#[test]
fn capacity_is_reserved_before_any_chunk_is_accepted() {
    let f = fixture();
    let tiny = Budget::new(1024).unwrap();
    assert_eq!(
        Receiver::new(&f.decl, &f.s, 0, 10_000_000_000, &tiny).err(),
        Some(Fault::Assembly(
            autotune_benchmark_worker::workload::starting_delivery::Fault::Capacity
        ))
    );
}
