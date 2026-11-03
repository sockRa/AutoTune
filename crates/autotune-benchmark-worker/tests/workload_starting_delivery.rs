use autotune_benchmark_worker::{
    evidence::{Digest, Domain},
    monitor::Anchor,
    sampler::{budget::Budget, child::NativeAdmission},
    workload::{
        manifest::Artifacts,
        nvml_trace::Ledger,
        settings::Draft,
        starting::{Scope, ScopeDraft, Snapshot},
        starting_bootstrap::Declaration,
        starting_delivery::{self, Assembler, Chunks, Fault, Status},
        starting_export::{Binding, ControlFacts, Facts, Response},
        starting_runtime::Trace,
        transport::SendFailure,
    },
};
use autotune_probe::{
    nvml::{
        SamplerCall, SamplerInfo, SamplerObserver, SamplerOutput, SamplerReturn, SettingMetric,
    },
    report::Field,
};
use std::ffi::OsString;

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
            read_approval_ref: "CPU-only delivery fixture".into(),
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
    fixture_with(0)
}
fn fixture_with(entries: usize) -> Fixture {
    let b = budget();
    let s = scope(&b);
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
fn bulk() -> Fixture {
    fixture_with(320)
}
/// Replace a same-length ASCII token without corrupting the binary prefix.
fn replace_in_place(bytes: &mut [u8], from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let mut replaced = 0;
    for i in 0..=bytes.len() - from.len() {
        if &bytes[i..i + from.len()] == from {
            bytes[i..i + from.len()].copy_from_slice(to);
            replaced += 1;
        }
    }
    assert!(replaced > 0);
}
fn crafted(f: &Fixture, index: u64, count: u64, total: u64, data: &[u8]) -> Vec<u8> {
    let header = format!(
        "{{\"version\":\"1\",\"role\":\"pilot_starting_response_delivery\",\"declaration\":\"{}\",\"scope\":\"{}\",\"digest\":\"{}\",\"index\":\"{index}\",\"count\":\"{count}\",\"total_len\":\"{total}\",\"data_len\":\"{}\",\"offset\":\"0\"}}",
        f.decl.tag().to_hex(),
        f.s.digest().to_hex(),
        d(7).to_hex(),
        data.len()
    );
    let total_len = 8 + header.len() + data.len();
    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(&(total_len as u32).to_le_bytes());
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(data);
    bytes
}

#[test]
fn complete_response_round_trips_with_exact_bytes_and_digest() {
    let f = fixture();
    let (frames, response) = frames(&f);
    let chunks = Chunks::new(&response, &f.b).unwrap();
    assert_eq!(frames.len() as u32, chunks.count());
    assert!(frames.iter().all(|f| f.len() <= 64 * 1024));
    let mut a = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    for (i, frame) in frames.iter().enumerate() {
        let (index, complete) = a.accept(frame).unwrap();
        assert_eq!(index as usize, i);
        assert_eq!(complete, i + 1 == frames.len());
    }
    assert!(a.complete());
    assert_eq!(a.canonical(), Some(response.canonical().as_bytes()));
    assert_eq!(
        a.status(),
        Status::Complete {
            bytes: response.canonical().len(),
            digest: response.digest()
        }
    );
    assert_eq!(
        response.digest(),
        Digest::of(Domain::RawArtifact, a.canonical().unwrap())
    );
}

#[test]
fn multi_chunk_response_reassembles_in_order() {
    let f = bulk();
    let (frames, response) = frames(&f);
    assert!(frames.len() > 1, "fixture must exceed one chunk");
    let mut a = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    for frame in &frames {
        a.accept(frame).unwrap();
    }
    assert!(a.complete());
    assert_eq!(a.canonical(), Some(response.canonical().as_bytes()));
    assert_eq!(a.received() as usize, frames.len());
}

#[test]
fn gap_and_duplicate_chunks_are_sticky() {
    let f = bulk();
    let (frames, _) = frames(&f);
    assert!(frames.len() > 2);
    let mut a = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    a.accept(&frames[0]).unwrap();
    assert_eq!(a.accept(&frames[2]), Err(Fault::Gap));
    assert_eq!(a.accept(&frames[1]), Err(Fault::Gap));
    assert!(matches!(
        a.status(),
        Status::Faulted {
            fault: Fault::Gap,
            ..
        }
    ));
    assert_eq!(a.canonical(), None);

    let mut b = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    b.accept(&frames[0]).unwrap();
    assert_eq!(b.accept(&frames[0]), Err(Fault::Gap));
}

#[test]
fn incomplete_stream_retains_explicit_status() {
    let f = bulk();
    let (frames, _) = frames(&f);
    assert!(frames.len() > 1);
    let mut a = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    for frame in frames.iter().take(frames.len() - 1) {
        a.accept(frame).unwrap();
    }
    assert!(!a.complete());
    let Status::Incomplete {
        received,
        count,
        bytes,
    } = a.status()
    else {
        panic!("expected explicit incomplete status");
    };
    assert_eq!(received as usize + 1, frames.len());
    assert_eq!(count as usize, frames.len());
    assert!(bytes > 0);
    assert_eq!(a.canonical(), None);
}

#[test]
fn foreign_declaration_and_scope_are_rejected() {
    let f = fixture();
    let mut foreign = f.s.draft().clone();
    foreign.capture_nonce = [9; 16];
    let foreign_scope = Scope::freeze(foreign, &f.b).unwrap();
    let foreign_decl = Declaration::new(&foreign_scope, a(1000), &f.b).unwrap();
    assert_eq!(
        Assembler::new(&foreign_decl, &f.s, &f.b).err(),
        Some(Fault::Scope)
    );
    let (frames, _) = frames(&f);
    let mut frame = frames[0].clone();
    replace_in_place(
        &mut frame,
        f.decl.tag().to_hex().as_bytes(),
        foreign_decl.tag().to_hex().as_bytes(),
    );
    let mut a = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    assert_eq!(a.accept(&frame), Err(Fault::Scope));

    let mut frame = frames[0].clone();
    replace_in_place(
        &mut frame,
        f.s.digest().to_hex().as_bytes(),
        foreign_scope.digest().to_hex().as_bytes(),
    );
    let mut b = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    assert_eq!(b.accept(&frame), Err(Fault::Scope));
}

#[test]
fn digest_and_structural_contradictions_fail_closed() {
    let f = bulk();
    let (frames, response) = frames(&f);
    let mut a = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    for frame in frames.iter().take(frames.len() - 1) {
        a.accept(frame).unwrap();
    }
    let mut corrupt = frames.last().unwrap().clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert_eq!(a.accept(&corrupt), Err(Fault::Digest));
    assert_eq!(a.canonical(), None);
    assert!(matches!(
        a.status(),
        Status::Faulted {
            fault: Fault::Digest,
            ..
        }
    ));

    // Declared lengths/counts outside the fixed response bound are rejected
    // before any body allocation.
    let over = starting_export_cap() as u64 + 1;
    let mut b = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    assert_eq!(
        b.accept(&crafted(&f, 0, 1, over, response.canonical().as_bytes())),
        Err(Fault::Frame)
    );
    let mut c = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    assert_eq!(
        c.accept(&crafted(&f, 0, 0, 100, b"x".as_slice())),
        Err(Fault::Frame)
    );
    let mut e = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    assert_eq!(
        e.accept(&crafted(&f, 5, 1, 100, b"x".as_slice())),
        Err(Fault::Frame)
    );
}
fn starting_export_cap() -> usize {
    autotune_benchmark_worker::workload::starting_export::CAP
}

#[test]
fn payload_must_still_bind_parent_scope_and_declaration() {
    let f = fixture();
    let response = response(&f);
    let payload = response.canonical().as_bytes();
    assert!(payload.len() < 64 * 1024 - 1024);
    let resign = |bytes: &[u8]| Digest::of(Domain::RawArtifact, bytes);

    let mut declaration = payload.to_vec();
    replace_in_place(
        &mut declaration,
        f.decl.tag().to_hex().as_bytes(),
        d(9).to_hex().as_bytes(),
    );
    let frame = crafted(&f, 0, 1, declaration.len() as u64, &declaration);
    let frame = resign_frame(frame, resign(&declaration));
    let mut a = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    assert_eq!(a.accept(&frame), Err(Fault::Binding));
    assert_eq!(a.canonical(), None);

    let mut request = payload.to_vec();
    replace_in_place(
        &mut request,
        f.s.digest().to_hex().as_bytes(),
        d(8).to_hex().as_bytes(),
    );
    let frame = crafted(&f, 0, 1, request.len() as u64, &request);
    let frame = resign_frame(frame, resign(&request));
    let mut b = Assembler::new(&f.decl, &f.s, &f.b).unwrap();
    assert_eq!(b.accept(&frame), Err(Fault::Binding));
}
/// Replace the transfer digest in an already-built single-chunk frame.
fn resign_frame(mut frame: Vec<u8>, digest: Digest) -> Vec<u8> {
    let header_len = u32::from_le_bytes(frame[4..8].try_into().unwrap()) as usize;
    let header = String::from_utf8_lossy(&frame[8..8 + header_len]).into_owned();
    let old = header
        .split("\"digest\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("digest field");
    let replaced = header.replace(old, &digest.to_hex());
    assert_eq!(replaced.len(), header.len());
    frame[8..8 + header_len].copy_from_slice(replaced.as_bytes());
    frame
}

#[test]
fn sender_stops_at_the_first_failed_handoff() {
    let f = fixture();
    let response = response(&f);
    let chunks = Chunks::new(&response, &f.b).unwrap();
    assert!(chunks.count() >= 1);
    let mut offered = 0;
    let fault = chunks
        .send(
            |_| {
                offered += 1;
                Err(SendFailure::Queue)
            },
            || true,
        )
        .unwrap_err();
    assert_eq!(offered, 1);
    assert_eq!(fault.index(), 0);
    assert_eq!(fault.sent(), 0);
    assert_eq!(fault.cause(), SendFailure::Queue);
}

#[test]
fn chunk_bounds_cover_the_largest_response() {
    let data: usize = 64 * 1024 - 1024;
    let cap: usize = 2 * 1024 * 1024;
    assert_eq!(starting_delivery::chunk_count(0), 0);
    assert_eq!(starting_delivery::chunk_count(1), 1);
    assert_eq!(starting_delivery::chunk_count(data), 1);
    assert_eq!(starting_delivery::chunk_count(data + 1), 2);
    assert_eq!(starting_delivery::max_chunks(), cap.div_ceil(data) as u32);
    assert!((starting_delivery::max_chunks() as usize - 1) * data < cap);
}

#[test]
fn assembler_reserves_before_accepting_any_chunk() {
    let f = fixture();
    let tiny = Budget::new(1024).unwrap();
    assert_eq!(
        Assembler::new(&f.decl, &f.s, &tiny).err(),
        Some(Fault::Capacity)
    );
    assert_eq!(
        Chunks::new(&response(&f), &tiny).err(),
        Some(Fault::Capacity)
    );
}

#[test]
fn executable_routes_only_the_exact_starting_token() {
    let starting = vec![OsString::from("--pilot-starting-v1"); 14];
    assert!(autotune_benchmark_worker::workload::starting_entry::is_starting(&starting));
    assert!(
        !autotune_benchmark_worker::workload::starting_entry::is_starting(&[OsString::from(
            "--pilot-sampler-v1"
        )])
    );
    assert!(!autotune_benchmark_worker::workload::starting_entry::is_starting(&[]));
    assert!(
        !autotune_benchmark_worker::workload::starting_entry::is_starting(&[OsString::from(
            "--pilot-starting-v1 "
        )])
    );
}

#[test]
fn actual_entry_signatures_compile_without_invocation() {
    let _: unsafe fn() -> Result<
        autotune_benchmark_worker::workload::starting_entry::Outcome,
        autotune_benchmark_worker::workload::starting_entry::EntryError,
    > = autotune_benchmark_worker::workload::starting_entry::entry;
}
