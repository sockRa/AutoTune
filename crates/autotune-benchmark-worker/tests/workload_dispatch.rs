use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{
        budget::Budget,
        wire::{Lane, WireError},
    },
    workload::{
        dispatch::{
            Call, FailureChunk, FailureVector, FirstFailure, Operation, Phase, Record, CHUNK_LANES,
        },
        manifest::{Artifacts, PilotDraft, PilotManifest},
        wire::{Channel, Direction, Message, WorkerBinding},
    },
};
fn digest(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn manifest(b: &Budget) -> PilotManifest {
    PilotManifest::freeze(
        PilotDraft {
            session: [1; 16],
            runs: [[2; 16], [3; 16], [4; 16]],
            clock_token: [5; 16],
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: digest(6),
                lockfile: digest(7),
                controller: digest(8),
                worker: digest(9),
                sampler: digest(10),
                ptx: digest(11),
                protocol: digest(12),
            },
            starting_evidence: digest(13),
            environment_evidence: digest(14),
            approval_ref: "not execution authority".into(),
            output_root: "D:\\capture\\pilot".into(),
        },
        b,
    )
    .unwrap()
}
fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [5; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn record() -> Record {
    Record {
        dispatch: 0,
        phase: Phase::Warmup,
        seed: 0,
        lanes: 65536,
        steps: 4096,
        calls: [
            Operation::Input,
            Operation::Submit,
            Operation::Query,
            Operation::Readback,
            Operation::Check,
            Operation::Hash,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, operation)| Call {
            operation,
            before: Some(a((i as u64) * 1000)),
            after: Some(a((i as u64) * 1000 + 100)),
            status: Some(0),
        })
        .collect(),
        pending: false,
        readback_lanes: Some(65536),
        checked_lanes: 65536,
        mismatches: 0,
        nonfinite: 0,
        first_failure: None,
        verified_fma: 268435456,
        input_sha256: Some(digest(20)),
        output_sha256: Some(digest(21)),
    }
}
fn binding(m: &PilotManifest) -> WorkerBinding {
    WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap()
}
fn roundtrip(b: &Budget, binding: WorkerBinding, message: &Message) -> Message {
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let frame = tx.encode(message).unwrap();
    assert!(frame.as_bytes().len() <= 65536);
    rx.decode(frame.as_bytes()).unwrap().message().clone()
}
#[test]
fn dispatch_wire_retains_every_raw_stage_status_clock_and_exact_work() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut r = record();
    assert_eq!(
        roundtrip(&b, binding, &Message::DispatchRecord(r.clone())),
        Message::DispatchRecord(r.clone())
    );
    r.calls[1].status = Some(i32::MIN);
    r.calls[2].after = None;
    r.calls[2].status = None;
    r.calls[3].before = Some(a(100)); // contradictory order retained, not qualified
    r.pending = true;
    r.readback_lanes = None;
    r.checked_lanes = 0;
    r.verified_fma = 0;
    r.input_sha256 = None;
    r.output_sha256 = None;
    assert_eq!(
        roundtrip(&b, binding, &Message::DispatchRecord(r.clone())),
        Message::DispatchRecord(r)
    );
}
#[test]
fn wrong_phase_shape_counts_work_and_native_direction_poison_the_channel() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut invalid = Vec::new();
    let mut r = record();
    r.phase = Phase::Measured;
    invalid.push(r);
    let mut r = record();
    r.dispatch = 384;
    r.phase = Phase::Measured;
    invalid.push(r);
    let mut r = record();
    r.seed = 1;
    invalid.push(r);
    let mut r = record();
    r.steps = 16384;
    invalid.push(r);
    let mut r = record();
    r.checked_lanes = 65535;
    invalid.push(r);
    let mut r = record();
    r.mismatches = 1;
    invalid.push(r);
    let mut r = record();
    r.verified_fma += 1;
    invalid.push(r);
    let mut r = record();
    r.calls[0].before.as_mut().unwrap().clock_token = [99; 16];
    invalid.push(r);
    for r in invalid {
        let mut tx =
            Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
        assert!(tx.encode(&Message::DispatchRecord(r)).is_err());
        assert!(matches!(
            tx.encode(&Message::DispatchRecord(record())),
            Err(WireError::Poisoned)
        ));
    }
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::ToWorker, b).unwrap();
    assert!(tx.encode(&Message::DispatchRecord(record())).is_err());
}
#[test]
fn warmup_measured_boundary_and_last_dispatch_use_the_bound_plan() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    for (j, phase) in [
        (127, Phase::Warmup),
        (128, Phase::Measured),
        (383, Phase::Measured),
    ] {
        let mut r = record();
        r.dispatch = j;
        r.phase = phase;
        assert_eq!(
            roundtrip(&b, binding, &Message::DispatchRecord(r.clone())),
            Message::DispatchRecord(r)
        );
    }
}
fn failed_record(bits: &[u32]) -> Record {
    use sha2::{Digest as _, Sha256};
    let mut hash = Sha256::new();
    for n in bits {
        hash.update(n.to_le_bytes());
    }
    let mut r = record();
    r.mismatches = 1;
    r.nonfinite = 1;
    r.verified_fma = 0;
    r.first_failure = Some(FirstFailure {
        lane: 123,
        expected: 17,
        actual_bits: 0x7fc01234,
        nonfinite: true,
    });
    r.output_sha256 = Some(Digest::parse(&format!("{:x}", hash.finalize())).unwrap());
    r
}
#[test]
fn complete_failure_vector_crosses_real_frames_and_retains_budget_until_drop() {
    let b = Budget::new(12 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let failure = b.partition(4 * 1024 * 1024).unwrap();
    let mut bits = vec![0x80000000; 65536];
    bits[123] = 0x7fc01234;
    let r = failed_record(&bits);
    assert_eq!(
        roundtrip(&b, binding, &Message::DispatchRecord(r.clone())),
        Message::DispatchRecord(r.clone())
    );
    let mut vector = FailureVector::new(binding, &r, &failure).unwrap();
    let charged = failure.used();
    assert!(charged >= bits.len() * 4);
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    for (index, part) in bits.chunks(CHUNK_LANES).enumerate() {
        let frame = tx
            .encode(&Message::FailureChunk(FailureChunk {
                dispatch: 0,
                offset: (index * CHUNK_LANES) as u32,
                total_lanes: 65536,
                bits: part.to_vec(),
            }))
            .unwrap();
        let decoded = rx.decode(frame.as_bytes()).unwrap();
        let Message::FailureChunk(chunk) = decoded.message() else {
            panic!("wrong kind")
        };
        vector.accept(chunk).unwrap();
    }
    assert!(vector.complete());
    assert!(!vector.failed());
    assert_eq!(vector.bits(), bits);
    assert_eq!(failure.used(), charged);
    drop(vector);
    assert_eq!(failure.used(), 0);
}
#[test]
fn gaps_duplicates_hash_mismatch_and_capacity_preserve_failure_uncertainty() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let bits = vec![0; 65536];
    let r = failed_record(&bits);
    assert!(FailureVector::new(binding, &r, &Budget::new(16).unwrap()).is_err());
    for mode in 0..3 {
        let mut vector = FailureVector::new(binding, &r, &b).unwrap();
        let first = FailureChunk {
            dispatch: 0,
            offset: 0,
            total_lanes: 65536,
            bits: vec![0; CHUNK_LANES],
        };
        vector.accept(&first).unwrap();
        let bad = match mode {
            0 => first.clone(),
            1 => FailureChunk {
                offset: (CHUNK_LANES * 2) as u32,
                ..first.clone()
            },
            _ => FailureChunk {
                dispatch: 1,
                offset: CHUNK_LANES as u32,
                ..first.clone()
            },
        };
        assert!(vector.accept(&bad).is_err());
        assert!(vector.failed());
        assert!(!vector.complete());
        assert_eq!(vector.bits(), &bits[..CHUNK_LANES]);
        assert!(matches!(vector.accept(&first), Err(WireError::Poisoned)));
    }
    let mut vector = FailureVector::new(binding, &r, &b).unwrap();
    for index in 0..32 {
        let chunk = FailureChunk {
            dispatch: 0,
            offset: (index * CHUNK_LANES) as u32,
            total_lanes: 65536,
            bits: vec![1; CHUNK_LANES],
        };
        let result = vector.accept(&chunk);
        if index == 31 {
            assert!(result.is_err());
        } else {
            result.unwrap();
        }
    }
    assert!(vector.failed());
    assert_eq!(vector.bits().len(), 65536);
}
#[test]
fn dispatch_owned_models_release_exact_charges_and_reject_oversized_chunk_capacity() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let before = b.used();
    let frame = tx.encode(&Message::DispatchRecord(record())).unwrap();
    let message = rx.decode(frame.as_bytes()).unwrap();
    assert!(b.used() > before);
    drop(frame);
    assert!(b.used() > before);
    drop(message);
    assert_eq!(b.used(), before);
    let mut bits = Vec::with_capacity(CHUNK_LANES + 1);
    bits.push(1);
    assert!(tx
        .encode(&Message::FailureChunk(FailureChunk {
            dispatch: 0,
            offset: 0,
            total_lanes: 65536,
            bits
        }))
        .is_err());
}
#[test]
fn independent_dispatch_body_golden_pins_unknowns_without_zero_filling() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut r = record();
    r.calls.clear();
    r.pending = true;
    r.readback_lanes = None;
    r.checked_lanes = 0;
    r.verified_fma = 0;
    r.input_sha256 = None;
    r.output_sha256 = None;
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b).unwrap();
    let frame = tx.encode(&Message::DispatchRecord(r)).unwrap();
    let text = std::str::from_utf8(&frame.as_bytes()[4..]).unwrap();
    assert_eq!(
        text.split_once("\"body\":").unwrap().1,
        r#"{"dispatch":"0","phase":"warmup","seed":"0","lanes":"65536","steps":"4096","calls":[],"pending":true,"readback_lanes":null,"checked_lanes":"0","mismatches":"0","nonfinite":"0","first_failure":null,"verified_fma":"0","input_sha256":null,"output_sha256":null}}"#
    );
}
#[test]
fn largest_pilot_failure_fits_reserved_snapshot_and_preserves_every_word() {
    use sha2::{Digest as _, Sha256};
    let b = Budget::new(12 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = WorkerBinding::new(&m.bind_run(2, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
    let failure = b.partition(4 * 1024 * 1024).unwrap();
    let mut r = record();
    r.seed = 2;
    r.lanes = 262144;
    r.steps = 65536;
    r.checked_lanes = r.lanes;
    r.readback_lanes = Some(r.lanes);
    r.mismatches = r.lanes;
    r.nonfinite = r.lanes;
    r.verified_fma = 0;
    r.first_failure = Some(FirstFailure {
        lane: 0,
        expected: 0,
        actual_bits: u32::MAX,
        nonfinite: true,
    });
    let mut hash = Sha256::new();
    for _ in 0..r.lanes {
        hash.update(u32::MAX.to_le_bytes());
    }
    r.output_sha256 = Some(Digest::parse(&format!("{:x}", hash.finalize())).unwrap());
    let mut vector = FailureVector::new(binding, &r, &failure).unwrap();
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let mut rx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b).unwrap();
    for offset in (0..r.lanes).step_by(CHUNK_LANES) {
        let message = Message::FailureChunk(FailureChunk {
            dispatch: 0,
            offset,
            total_lanes: r.lanes,
            bits: vec![u32::MAX; CHUNK_LANES],
        });
        let frame = tx.encode(&message).unwrap();
        let decoded = rx.decode(frame.as_bytes()).unwrap();
        let Message::FailureChunk(chunk) = decoded.message() else {
            panic!("wrong kind")
        };
        vector.accept(chunk).unwrap();
    }
    assert!(vector.complete());
    assert_eq!(vector.bits().len(), 262144);
    assert!(vector.bits().iter().all(|n| *n == u32::MAX));
    assert!(failure.used() < 4 * 1024 * 1024);
}
#[test]
fn bounded_raw_call_trace_roundtrips_without_truncation_and_nested_schema_is_strict() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let binding = binding(&m);
    let mut r = record();
    r.calls = vec![
        Call {
            operation: Operation::Query,
            before: Some(a(0)),
            after: Some(a(100)),
            status: Some(600)
        };
        64
    ];
    assert_eq!(
        roundtrip(&b, binding, &Message::DispatchRecord(r.clone())),
        Message::DispatchRecord(r.clone())
    );
    r.calls.push(Call {
        operation: Operation::Launch,
        before: None,
        after: None,
        status: None,
    });
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    assert!(tx.encode(&Message::DispatchRecord(r)).is_err());
    let mut tx = Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
    let frame = tx.encode(&Message::DispatchRecord(record())).unwrap();
    for (from, to) in [
        ("\"checked_lanes\":\"65536\"", "\"checked_lanes\":\"65537\""),
        ("\"operation\":\"submit\"", "\"operation\":\"retry\""),
        (
            "\"verified_fma\":\"268435456\"",
            "\"verified_fma\":\"268435455\"",
        ),
        (
            "\"mismatches\":\"0\"",
            "\"mismatches\":\"0\",\"extra\":null",
        ),
        ("\"phase\":\"warmup\"", "\"phase\":\"measured\""),
    ] {
        let text = std::str::from_utf8(&frame.as_bytes()[4..])
            .unwrap()
            .replace(from, to);
        let mut bytes = ((text.len() + 4) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(text.as_bytes());
        let mut rx =
            Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
        assert!(rx.decode(&bytes).is_err());
        assert!(matches!(
            rx.decode(frame.as_bytes()),
            Err(WireError::Poisoned)
        ));
    }
}
