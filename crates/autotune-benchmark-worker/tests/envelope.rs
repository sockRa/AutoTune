use autotune_benchmark_worker::evidence::{
    decode, decode_envelope, encode, encode_envelope, AcceptPolicy, Admission, Artifacts, Clocks,
    ControlWrites, Correctness, Coverage, Digest, Dispatch, Domain, EmptyJobLifecycle, Envelope,
    EnvelopeError, EnvelopeErrorKind, Hex64, Identity, Integrity, InternalAge, Lifecycle, Manifest,
    MatchState, NativeCompletion, Object, Obs, ParseError, Provenance, Ratio, Reason, ReasonError,
    ResultEmptyJob, Retention, Role, SessionResult, Telemetry, TelemetryValue, Value, Verdict,
    WriterStatus, MAX_DEPTH, SCHEMA, VERSION,
};

/// Pinned `Domain::Session` digest of the fixture payload without `digest`.
const SESSION_DIGEST: &str = "2ffe9f82352de2704cd1aa49ec98c7215327517e44bb068e1ffe22b8c6a5795c";

const SOURCE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn reason(text: &str) -> Reason {
    Reason::new(text).expect("valid reason code")
}

fn hex64(seed: u8) -> Hex64 {
    let digit = char::from_digit(u32::from(seed % 16), 16).expect("hex digit");
    Digest::parse(&digit.to_string().repeat(64)).expect("64 lowercase hex")
}

fn dispatch(j: u64, role: Role, lanes: u64, work: u64) -> Dispatch {
    Dispatch {
        j,
        role,
        shape: "8x8".to_owned(),
        input_digest: hex64(5),
        submit_ns: Obs::Known(1_210),
        query_ns: Obs::Unavailable(reason("not_sampled")),
        readback_ns: Obs::Known(1_220),
        check_ns: Obs::Known(1_230),
        return_code: 0,
        checked_lanes: lanes,
        mismatch: 0,
        nonfinite: 0,
        work,
        output_digest: Obs::Known(hex64(7)),
        retention: Retention::DigestOnly,
    }
}

fn envelope() -> Envelope {
    let mut measured = dispatch(1, Role::Measured, 32, 200);
    measured.output_digest = Obs::Known(hex64(6));
    measured.input_digest = hex64(8);
    Envelope {
        provenance: Provenance::LiveObserved,
        identity: Identity {
            nonce: "nonce-1".to_owned(),
            nvml_uuid: Obs::Known("GPU-0000".to_owned()),
            cuda_uuid: Obs::Known("GPU-0000".to_owned()),
            pci_bus_id: Obs::Known("0000:01:00.0".to_owned()),
            context_device: Obs::Known("0".to_owned()),
            boot: Obs::Unavailable(reason("not_sampled")),
            uuid_match: MatchState::Match,
            pci_match: MatchState::Match,
        },
        artifacts: Artifacts {
            source_commit: SOURCE_COMMIT.to_owned(),
            worker_sha256: Obs::Known(hex64(0)),
            lockfile_sha256: Obs::Known(hex64(1)),
            sampler_sha256: Obs::Unavailable(reason("not_built")),
            ptx_sha256: Obs::Known(hex64(2)),
            kernel_entry: "fma_kernel".to_owned(),
            generator_version: "gen-1".to_owned(),
            oracle_version: "oracle-1".to_owned(),
        },
        manifest: Manifest {
            manifest_digest: Obs::Known(hex64(3)),
            profile_digest: Obs::Known(hex64(4)),
            shape: Obs::Known("1024x1".to_owned()),
            stop_policy: Obs::Known("pilot180s".to_owned()),
            approval_ref: Obs::Unavailable(reason("no_approval")),
            seed: Obs::Known(42),
            control_writes: ControlWrites::None,
        },
        clocks: Clocks {
            qpc_frequency: 10_000_000,
            qpc_epoch_ticks: 100,
            interrupt_ticks: 7,
            unbiased_ticks: Obs::Unavailable(reason("unsupported")),
            resolution_ns: 100,
            utc_context: Obs::Known("2026-10-26T00:00:00Z".to_owned()),
            discontinuities: 0,
        },
        lifecycle: Lifecycle {
            requested_ns: Obs::Known(1_000),
            launch_ns: Obs::Known(1_100),
            ready_ns: Obs::Unavailable(reason("not_observed")),
            start_ns: Obs::Known(1_200),
            stop_requested_ns: Obs::Known(1_300),
            exit_ns: Obs::Known(1_400),
            empty_job: EmptyJobLifecycle::ObservedNonempty,
        },
        dispatches: vec![dispatch(0, Role::Warmup, 16, 100), measured],
        telemetry: vec![
            Telemetry {
                metric: "power".to_owned(),
                unit: "W".to_owned(),
                device: "GPU-0000".to_owned(),
                value: Obs::Known(TelemetryValue::Int(200)),
                slot: 0,
                query_start_ns: Obs::Known(2_000),
                query_end_ns: Obs::Known(2_100),
                receipt_ns: Obs::Known(2_200),
                internal_age: InternalAge::Assessed,
                vendor_code: Obs::Known("0x0".to_owned()),
            },
            Telemetry {
                metric: "temp".to_owned(),
                unit: "C".to_owned(),
                device: "GPU-0000".to_owned(),
                value: Obs::Known(TelemetryValue::Ratio(
                    Ratio::new(1, 2).expect("valid ratio"),
                )),
                slot: 1,
                query_start_ns: Obs::Unavailable(reason("not_sampled")),
                query_end_ns: Obs::Unavailable(reason("not_sampled")),
                receipt_ns: Obs::Known(2_300),
                internal_age: InternalAge::Unknown,
                vendor_code: Obs::Unavailable(reason("no_vendor_code")),
            },
        ],
        integrity: Integrity {
            capacity_bytes: 4_096,
            observed_bytes: 1_024,
            failed: 0,
            late: 0,
            rejected: 0,
            overflow: 0,
            expected_slots: 2,
            observed_slots: 2,
            raw_records: 2,
            writer_status: WriterStatus::Bounded,
        },
        result: SessionResult {
            admission: Admission::Admitted,
            correctness: Correctness::Passed,
            worker_exit: Obs::Known(0),
            sampler_exit: Obs::Unavailable(reason("not_run")),
            empty_job: ResultEmptyJob::NotObserved,
            native_completion: NativeCompletion::Unknown,
            coverage: Coverage::Complete,
            limitations: Vec::new(),
            verdict: Verdict::Usable,
        },
    }
}

fn document() -> String {
    encode_envelope(&envelope()).expect("valid fixture encodes")
}

fn entries(object: &Object) -> Vec<(String, Value)> {
    object
        .iter()
        .map(|(key, value)| (key.to_owned(), value.clone()))
        .collect()
}

fn object(entries: Vec<(String, Value)>) -> Object {
    let mut object = Object::new();
    for (key, value) in entries {
        object.insert(key, value).expect("unique fixture keys");
    }
    object
}

fn seal(payload: &Object) -> String {
    let mut sealed = payload.clone();
    let digest = Digest::of_value(Domain::Session, &Value::Object(payload.clone())).to_hex();
    sealed
        .insert("digest", Value::Text(digest))
        .expect("digest is appended once");
    encode(&Value::Object(sealed))
}

fn reseal(bytes: &str) -> String {
    let marker = ",\"digest\":\"";
    let index = bytes.rfind(marker).expect("sealed document");
    let prefix = &bytes[..index];
    let payload = format!("{prefix}}}");
    let value = decode(payload.as_bytes(), payload.len()).expect("canonical payload");
    let digest = Digest::of_value(Domain::Session, &value).to_hex();
    format!("{prefix},\"digest\":\"{digest}\"}}")
}

fn rebuild_top(bytes: &str, edit: impl FnOnce(&mut Vec<(String, Value)>)) -> String {
    let value = decode(bytes.as_bytes(), bytes.len()).expect("canonical document");
    let payload = value.as_object().expect("root object").without("digest");
    let mut fields = entries(&payload);
    edit(&mut fields);
    seal(&object(fields))
}

fn rebuild_section(
    bytes: &str,
    name: &str,
    edit: impl FnOnce(&mut Vec<(String, Value)>),
) -> String {
    rebuild_top(bytes, |top| {
        let (_, section) = top
            .iter_mut()
            .find(|(key, _)| key == name)
            .expect("section present");
        let mut fields = entries(section.as_object().expect("section object"));
        edit(&mut fields);
        *section = Value::Object(object(fields));
    })
}

fn mutate(bytes: &str, from: &str, to: &str) -> String {
    assert_eq!(bytes.matches(from).count(), 1, "unique fragment {from}");
    reseal(&bytes.replace(from, to))
}

fn assert_encode_error(envelope: &Envelope, field: &str, kind: EnvelopeErrorKind) {
    let error: EnvelopeError = encode_envelope(envelope).expect_err("encode must reject");
    assert_eq!(error.field(), field);
    assert_eq!(error.kind(), kind);
}

fn assert_decode_error(bytes: &str, policy: AcceptPolicy, field: &str, kind: EnvelopeErrorKind) {
    let error = decode_envelope(bytes.as_bytes(), bytes.len(), policy).expect_err("decode rejects");
    assert_eq!(error.field(), field);
    assert_eq!(error.kind(), kind);
}

fn assert_decode_error_with_max(
    bytes: &str,
    max_bytes: usize,
    field: &str,
    kind: EnvelopeErrorKind,
) {
    let error = decode_envelope(bytes.as_bytes(), max_bytes, AcceptPolicy::RequireLive)
        .expect_err("decode rejects");
    assert_eq!(error.field(), field);
    assert_eq!(error.kind(), kind);
}

#[test]
fn valid_envelope_round_trips_and_reencodes_identically() {
    let bytes = document();
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive)
        .expect("valid envelope decodes");
    assert_eq!(decoded, envelope());
    assert_eq!(encode_envelope(&decoded).unwrap(), bytes);
    assert_eq!(encode_envelope(&decoded).unwrap(), document());

    let value = decode(bytes.as_bytes(), bytes.len()).unwrap();
    let keys = value
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, _)| key.to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec![
            "schema",
            "version",
            "provenance",
            "identity",
            "artifacts",
            "manifest",
            "clocks",
            "lifecycle",
            "dispatches",
            "telemetry",
            "integrity",
            "result",
            "digest",
        ]
    );
}

#[test]
fn golden_session_digest_is_pinned() {
    let bytes = document();
    let marker = ",\"digest\":\"";
    let index = bytes.rfind(marker).expect("digest last");
    let payload = format!("{}}}", &bytes[..index]);
    let stored = &bytes[index + marker.len()..bytes.len() - 2];
    assert_eq!(stored, SESSION_DIGEST);
    assert_eq!(stored.len(), 64);

    let value = decode(payload.as_bytes(), payload.len()).unwrap();
    assert_eq!(
        Digest::of_value(Domain::Session, &value).to_hex(),
        SESSION_DIGEST
    );
}

#[test]
fn round_trip_preserves_typed_unavailable_and_ratio() {
    let bytes = document();
    let decoded =
        decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::AllowSimulated).unwrap();
    assert_eq!(
        decoded.identity.boot,
        Obs::Unavailable(reason("not_sampled"))
    );
    assert_eq!(
        decoded.identity.boot.as_known(),
        None,
        "unavailable is never inferred as a value"
    );
    assert_eq!(
        decoded.artifacts.sampler_sha256,
        Obs::Unavailable(reason("not_built"))
    );
    assert_eq!(
        decoded.telemetry[1].value,
        Obs::Known(TelemetryValue::Ratio(Ratio::new(1, 2).unwrap()))
    );
    assert_eq!(
        decoded.telemetry[1].value.as_known(),
        Some(&TelemetryValue::Ratio(Ratio::new(1, 2).unwrap()))
    );
    assert_eq!(
        decoded.telemetry[1].vendor_code,
        Obs::Unavailable(reason("no_vendor_code"))
    );
    assert_eq!(
        decoded.result.sampler_exit,
        Obs::Unavailable(reason("not_run"))
    );
    assert!(decoded.identity.nvml_uuid.is_known());
}

#[test]
fn reason_codes_use_the_exact_lowercase_grammar() {
    assert_eq!(Reason::new(""), Err(ReasonError::Empty));
    assert_eq!(Reason::new("A"), Err(ReasonError::Character));
    assert_eq!(Reason::new("a b"), Err(ReasonError::Character));
    assert_eq!(Reason::new("é"), Err(ReasonError::Character));
    assert_eq!(Reason::new("a/b"), Err(ReasonError::Character));
    assert_eq!(Reason::new(&"a".repeat(65)), Err(ReasonError::TooLong));
    assert_eq!(Reason::new(&"a".repeat(64)).unwrap().as_str().len(), 64);
    let valid = Reason::new("a-z_0.9").unwrap();
    assert_eq!(valid.as_str(), "a-z_0.9");
}

#[test]
fn wrong_enum_spellings_and_reason_codes_are_typed_rejections() {
    let bytes = document();
    assert_decode_error(
        &mutate(
            &bytes,
            "\"uuid_match\":\"match\"",
            "\"uuid_match\":\"Match\"",
        ),
        AcceptPolicy::RequireLive,
        "uuid_match",
        EnvelopeErrorKind::UnknownVariant,
    );
    assert_decode_error(
        &mutate(
            &bytes,
            "\"boot\":{\"unavailable\":\"not_sampled\"}",
            "\"boot\":{\"unavailable\":\"Not Sampled\"}",
        ),
        AcceptPolicy::RequireLive,
        "boot",
        EnvelopeErrorKind::Reason,
    );
    assert_decode_error(
        &mutate(&bytes, "\"role\":\"warmup\"", "\"role\":\"Warmup\""),
        AcceptPolicy::RequireLive,
        "role",
        EnvelopeErrorKind::UnknownVariant,
    );
}

#[test]
fn top_level_key_mutations_are_typed_rejections() {
    let bytes = document();

    let missing_digest = {
        let value = decode(bytes.as_bytes(), bytes.len()).expect("canonical document");
        let payload = value.as_object().expect("root object").without("digest");
        encode(&Value::Object(payload))
    };
    assert_decode_error(
        &missing_digest,
        AcceptPolicy::RequireLive,
        "digest",
        EnvelopeErrorKind::FieldCount,
    );

    let missing_section = rebuild_top(&bytes, |top| {
        top.retain(|(key, _)| key != "telemetry");
    });
    assert_decode_error(
        &missing_section,
        AcceptPolicy::RequireLive,
        "telemetry",
        EnvelopeErrorKind::FieldOrder,
    );

    let unknown = bytes.replace("\"schema\":", "\"schema_x\":");
    assert_decode_error(
        &unknown,
        AcceptPolicy::RequireLive,
        "schema",
        EnvelopeErrorKind::FieldName,
    );

    let extra = format!("{},\"extra\":\"x\"}}", &bytes[..bytes.len() - 1]);
    assert_decode_error(
        &extra,
        AcceptPolicy::RequireLive,
        "digest",
        EnvelopeErrorKind::ExtraField,
    );

    let reordered = rebuild_top(&bytes, |top| {
        let identity = top.iter().position(|(key, _)| key == "identity").unwrap();
        let artifacts = top.iter().position(|(key, _)| key == "artifacts").unwrap();
        top.swap(identity, artifacts);
    });
    assert_decode_error(
        &reordered,
        AcceptPolicy::RequireLive,
        "identity",
        EnvelopeErrorKind::FieldOrder,
    );

    let duplicate = bytes.replace(
        "\"schema\":\"autotune.live-session\",",
        "\"schema\":\"autotune.live-session\",\"schema\":\"autotune.live-session\",",
    );
    assert_decode_error(
        &duplicate,
        AcceptPolicy::RequireLive,
        "envelope",
        EnvelopeErrorKind::Parse,
    );
}

#[test]
fn section_key_mutations_are_typed_rejections() {
    let bytes = document();

    let missing = rebuild_section(&bytes, "identity", |fields| {
        fields.retain(|(key, _)| key != "pci_match");
    });
    assert_decode_error(
        &missing,
        AcceptPolicy::RequireLive,
        "pci_match",
        EnvelopeErrorKind::FieldCount,
    );

    let unknown = rebuild_section(&bytes, "identity", |fields| {
        for (key, _) in fields.iter_mut() {
            if key == "nvml_uuid" {
                *key = "nvml_uuid_x".to_owned();
            }
        }
    });
    assert_decode_error(
        &unknown,
        AcceptPolicy::RequireLive,
        "nvml_uuid",
        EnvelopeErrorKind::FieldName,
    );

    let extra = rebuild_section(&bytes, "identity", |fields| {
        fields.push(("extra".to_owned(), Value::Text("x".to_owned())));
    });
    assert_decode_error(
        &extra,
        AcceptPolicy::RequireLive,
        "pci_match",
        EnvelopeErrorKind::ExtraField,
    );

    let reordered = rebuild_section(&bytes, "identity", |fields| fields.swap(0, 1));
    assert_decode_error(
        &reordered,
        AcceptPolicy::RequireLive,
        "nonce",
        EnvelopeErrorKind::FieldOrder,
    );

    let duplicate = bytes.replace(
        "\"nonce\":\"nonce-1\",",
        "\"nonce\":\"nonce-1\",\"nonce\":\"nonce-1\",",
    );
    assert_decode_error(
        &duplicate,
        AcceptPolicy::RequireLive,
        "envelope",
        EnvelopeErrorKind::Parse,
    );
}

#[test]
fn noncanonical_decimals_and_ratios_are_rejected() {
    let bytes = document();
    for (from, to, field, kind) in [
        (
            "\"qpc_frequency\":\"10000000\"",
            "\"qpc_frequency\":\"+10000000\"",
            "qpc_frequency",
            EnvelopeErrorKind::Integer,
        ),
        (
            "\"qpc_epoch_ticks\":\"100\"",
            "\"qpc_epoch_ticks\":\"0100\"",
            "qpc_epoch_ticks",
            EnvelopeErrorKind::Integer,
        ),
        (
            "\"return_code\":\"0\",\"checked_lanes\":\"32\"",
            "\"return_code\":\"-0\",\"checked_lanes\":\"32\"",
            "return_code",
            EnvelopeErrorKind::Integer,
        ),
        (
            "{\"num\":\"1\",\"den\":\"2\"}",
            "{\"num\":\"+1\",\"den\":\"2\"}",
            "value",
            EnvelopeErrorKind::Ratio,
        ),
        (
            "{\"num\":\"1\",\"den\":\"2\"}",
            "{\"num\":\"1\",\"den\":\"0\"}",
            "value",
            EnvelopeErrorKind::Ratio,
        ),
    ] {
        assert_decode_error(
            &mutate(&bytes, from, to),
            AcceptPolicy::RequireLive,
            field,
            kind,
        );
    }
}

#[test]
fn wrong_schema_and_version_are_rejected_after_the_digest() {
    let bytes = document();
    assert_decode_error(
        &mutate(
            &bytes,
            "\"schema\":\"autotune.live-session\"",
            "\"schema\":\"autotune.live-session.v2\"",
        ),
        AcceptPolicy::RequireLive,
        "schema",
        EnvelopeErrorKind::Schema,
    );
    assert_decode_error(
        &mutate(&bytes, "\"version\":\"1\"", "\"version\":\"2\""),
        AcceptPolicy::RequireLive,
        "version",
        EnvelopeErrorKind::Version,
    );
}

#[test]
fn digest_mismatch_rejects_without_partial_acceptance() {
    let bytes = document();
    let tampered = bytes.replace("\"seed\":\"42\"", "\"seed\":\"43\"");
    assert_ne!(tampered, bytes);
    assert_decode_error(
        &tampered,
        AcceptPolicy::RequireLive,
        "digest",
        EnvelopeErrorKind::DigestMismatch,
    );

    let mut flipped = bytes.clone();
    let marker = ",\"digest\":\"";
    let start = bytes.rfind(marker).expect("digest last") + marker.len();
    let replacement = if bytes.as_bytes()[start] == b'0' {
        "1"
    } else {
        "0"
    };
    flipped.replace_range(start..start + 1, replacement);
    assert_ne!(flipped, bytes);
    assert_decode_error(
        &flipped,
        AcceptPolicy::RequireLive,
        "digest",
        EnvelopeErrorKind::DigestMismatch,
    );
}

#[test]
fn simulated_fixture_requires_an_accepting_policy() {
    let mut fixture = envelope();
    fixture.provenance = Provenance::SimulatedFixture;
    let bytes = encode_envelope(&fixture).expect("simulated fixture encodes");
    assert_decode_error(
        &bytes,
        AcceptPolicy::RequireLive,
        "provenance",
        EnvelopeErrorKind::Provenance,
    );
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::AllowSimulated)
        .expect("explicit policy accepts the fixture");
    assert_eq!(decoded.provenance, Provenance::SimulatedFixture);
}

#[test]
fn full_success_retention_is_never_encoded_or_decoded() {
    let mut passed = envelope();
    passed.dispatches[1].retention = Retention::FullSuccess;
    assert_encode_error(
        &passed,
        "retention",
        EnvelopeErrorKind::RetentionFullSuccess,
    );

    let bytes = document();
    let measured = format!(
        "\"output_digest\":\"{}\",\"retention\":\"digest_only\"",
        hex64(6).to_hex()
    );
    let passed = measured.replace("digest_only", "full_success");
    assert_decode_error(
        &mutate(&bytes, &measured, &passed),
        AcceptPolicy::RequireLive,
        "retention",
        EnvelopeErrorKind::RetentionFullSuccess,
    );
}

#[test]
fn unbounded_writer_status_is_never_encoded_or_decoded() {
    let mut unbounded = envelope();
    unbounded.integrity.writer_status = WriterStatus::Unbounded;
    assert_encode_error(
        &unbounded,
        "writer_status",
        EnvelopeErrorKind::WriterUnbounded,
    );

    let bytes = document();
    assert_decode_error(
        &mutate(
            &bytes,
            "\"writer_status\":\"bounded\"",
            "\"writer_status\":\"unbounded\"",
        ),
        AcceptPolicy::RequireLive,
        "writer_status",
        EnvelopeErrorKind::WriterUnbounded,
    );
}

#[test]
fn dispatch_order_and_counts_are_validated() {
    let bytes = document();
    assert_decode_error(
        &mutate(&bytes, "\"j\":\"1\"", "\"j\":\"0\""),
        AcceptPolicy::RequireLive,
        "j",
        EnvelopeErrorKind::CrossField,
    );

    let mut not_first = envelope();
    not_first.dispatches[0].j = 2;
    not_first.dispatches[1].j = 3;
    assert_encode_error(&not_first, "j", EnvelopeErrorKind::CrossField);

    let mut mismatch = envelope();
    mismatch.dispatches[1].mismatch = 33;
    assert_encode_error(&mismatch, "mismatch", EnvelopeErrorKind::CrossField);

    let mut nonfinite = envelope();
    nonfinite.dispatches[1].nonfinite = 33;
    assert_encode_error(&nonfinite, "nonfinite", EnvelopeErrorKind::CrossField);

    let mut empty_failure = envelope();
    empty_failure.dispatches[0].retention = Retention::FullFailure;
    assert_encode_error(&empty_failure, "retention", EnvelopeErrorKind::CrossField);

    let mut explained_failure = envelope();
    explained_failure.dispatches[1].mismatch = 1;
    explained_failure.dispatches[1].retention = Retention::FullFailure;
    explained_failure.result.verdict = Verdict::Inconclusive;
    assert!(encode_envelope(&explained_failure).is_ok());
}

#[test]
fn telemetry_counts_slots_and_uniqueness_are_validated() {
    let mut over_expected = envelope();
    over_expected.integrity.observed_slots = 3;
    assert_encode_error(
        &over_expected,
        "observed_slots",
        EnvelopeErrorKind::CrossField,
    );

    let mut count_mismatch = envelope();
    count_mismatch.integrity.observed_slots = 1;
    assert_encode_error(
        &count_mismatch,
        "observed_slots",
        EnvelopeErrorKind::CrossField,
    );

    // Aggregate expected counts do not declare per-metric slot-index ranges.
    let mut sparse = envelope();
    sparse.telemetry[1].slot = u64::MAX;
    let bytes = encode_envelope(&sparse).expect("sparse metric slot encodes");
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive)
        .expect("sparse metric slot decodes");
    assert_eq!(decoded, sparse);

    let mut duplicate = envelope();
    duplicate.telemetry[1].metric = "power".to_owned();
    duplicate.telemetry[1].slot = 0;
    assert_encode_error(&duplicate, "slot", EnvelopeErrorKind::CrossField);

    let mut over_bytes = envelope();
    over_bytes.integrity.observed_bytes = 5_000;
    assert_encode_error(&over_bytes, "observed_bytes", EnvelopeErrorKind::CrossField);
}

#[test]
fn usable_verdict_requires_complete_passing_evidence() {
    let cases: &[fn(&mut Envelope)] = &[
        |e| e.result.admission = Admission::Blocked,
        |e| e.result.admission = Admission::NotQualified,
        |e| e.result.correctness = Correctness::Failed,
        |e| e.result.correctness = Correctness::NotChecked,
        |e| e.result.coverage = Coverage::Incomplete,
        |e| e.result.coverage = Coverage::Unknown,
        |e| e.result.limitations.push(reason("sim_partial")),
        |e| e.dispatches[1].role = Role::Warmup,
        |e| e.dispatches[1].mismatch = 1,
        |e| e.dispatches[1].checked_lanes = 0,
        |e| e.dispatches[1].return_code = 1,
        |e| e.result.worker_exit = Obs::Unavailable(reason("not_observed")),
        |e| e.result.worker_exit = Obs::Known(1),
        |e| e.result.sampler_exit = Obs::Known(1),
        |e| e.integrity.failed = 1,
        |e| {
            e.telemetry.truncate(1);
            e.integrity.observed_slots = 1;
        },
        |e| e.identity.pci_match = MatchState::Mismatch,
    ];
    for edit in cases {
        let mut invalid = envelope();
        edit(&mut invalid);
        assert_encode_error(&invalid, "verdict", EnvelopeErrorKind::CrossField);
    }

    let bytes = document();
    assert_decode_error(
        &mutate(
            &bytes,
            "\"admission\":\"admitted\"",
            "\"admission\":\"blocked\"",
        ),
        AcceptPolicy::RequireLive,
        "verdict",
        EnvelopeErrorKind::CrossField,
    );
}

#[test]
fn extreme_integers_survive_in_observations() {
    let mut extremes = envelope();
    extremes.manifest.seed = Obs::Known(u64::MAX);
    extremes.clocks.qpc_epoch_ticks = u64::MAX;
    extremes.clocks.discontinuities = u64::MAX;
    extremes.lifecycle.exit_ns = Obs::Known(u64::MAX);
    extremes.dispatches[0].return_code = i64::MIN;
    extremes.dispatches[1].return_code = i64::MAX;
    extremes.dispatches[1].work = u64::MAX;
    extremes.telemetry[0].value = Obs::Known(TelemetryValue::Int(i64::MAX));
    extremes.telemetry[1].value = Obs::Known(TelemetryValue::Int(i64::MIN));
    extremes.result.verdict = Verdict::Inconclusive;

    let bytes = encode_envelope(&extremes).expect("extremes encode");
    let decoded =
        decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive).unwrap();
    assert_eq!(decoded, extremes);
    assert_eq!(decoded.manifest.seed, Obs::Known(u64::MAX));
    assert_eq!(decoded.dispatches[0].return_code, i64::MIN);
    assert_eq!(
        decoded.telemetry[1].value,
        Obs::Known(TelemetryValue::Int(i64::MIN))
    );
    assert_eq!(encode_envelope(&decoded).unwrap(), bytes);
}

#[test]
fn field_value_rules_are_typed_rejections() {
    let mut empty_nonce = envelope();
    empty_nonce.identity.nonce.clear();
    assert_encode_error(&empty_nonce, "nonce", EnvelopeErrorKind::TextEmpty);

    let mut long_nonce = envelope();
    long_nonce.identity.nonce = "a".repeat(129);
    assert_encode_error(&long_nonce, "nonce", EnvelopeErrorKind::TextLength);

    let mut control_nonce = envelope();
    control_nonce.identity.nonce = "bad\u{1}".to_owned();
    assert_encode_error(&control_nonce, "nonce", EnvelopeErrorKind::TextControl);

    let mut non_ascii_nonce = envelope();
    non_ascii_nonce.identity.nonce = "é".to_owned();
    assert_encode_error(&non_ascii_nonce, "nonce", EnvelopeErrorKind::TextNonAscii);

    let mut uppercase_commit = envelope();
    uppercase_commit.artifacts.source_commit = "A".repeat(40);
    assert_encode_error(&uppercase_commit, "source_commit", EnvelopeErrorKind::Hex);

    let mut empty_kernel = envelope();
    empty_kernel.artifacts.kernel_entry.clear();
    assert_encode_error(&empty_kernel, "kernel_entry", EnvelopeErrorKind::TextEmpty);

    let mut control_kernel = envelope();
    control_kernel.artifacts.kernel_entry = "k\u{1}".to_owned();
    assert_encode_error(
        &control_kernel,
        "kernel_entry",
        EnvelopeErrorKind::TextControl,
    );

    let mut zero_frequency = envelope();
    zero_frequency.clocks.qpc_frequency = 0;
    assert_encode_error(&zero_frequency, "qpc_frequency", EnvelopeErrorKind::Range);

    let mut zero_resolution = envelope();
    zero_resolution.clocks.resolution_ns = 0;
    assert_encode_error(&zero_resolution, "resolution_ns", EnvelopeErrorKind::Range);

    let mut zero_capacity = envelope();
    zero_capacity.integrity.capacity_bytes = 0;
    assert_encode_error(&zero_capacity, "capacity_bytes", EnvelopeErrorKind::Range);

    let mut empty_metric = envelope();
    empty_metric.telemetry[0].metric.clear();
    assert_encode_error(&empty_metric, "metric", EnvelopeErrorKind::TextEmpty);

    let mut control_unit = envelope();
    control_unit.telemetry[0].unit = "W\u{1}".to_owned();
    assert_encode_error(&control_unit, "unit", EnvelopeErrorKind::TextControl);

    let bytes = document();
    assert_decode_error(
        &mutate(&bytes, "\"nonce\":\"nonce-1\"", "\"nonce\":\"\""),
        AcceptPolicy::RequireLive,
        "nonce",
        EnvelopeErrorKind::TextEmpty,
    );
    assert_decode_error(
        &mutate(
            &bytes,
            &format!("\"input_digest\":\"{}\"", hex64(5).to_hex()),
            &format!("\"input_digest\":\"{}\"", "A".repeat(64)),
        ),
        AcceptPolicy::RequireLive,
        "input_digest",
        EnvelopeErrorKind::Hex,
    );
    assert_decode_error(
        &mutate(
            &bytes,
            &format!("\"source_commit\":\"{SOURCE_COMMIT}\""),
            &format!("\"source_commit\":\"{}\"", "A".repeat(40)),
        ),
        AcceptPolicy::RequireLive,
        "source_commit",
        EnvelopeErrorKind::Hex,
    );
}

#[test]
fn unavailable_rejects_an_extra_key_and_digest_precedes_sections() {
    let bytes = document();
    let extra = mutate(
        &bytes,
        "\"boot\":{\"unavailable\":\"not_sampled\"}",
        "\"boot\":{\"unavailable\":\"not_sampled\",\"extra\":\"x\"}",
    );
    assert_decode_error(
        &extra,
        AcceptPolicy::RequireLive,
        "unavailable",
        EnvelopeErrorKind::ExtraField,
    );

    // A section edit without resealing must fail on the digest, not on the
    // section, proving the digest check runs first.
    let unsealed = bytes.replace("\"role\":\"warmup\"", "\"role\":\"Warmup\"");
    assert_decode_error(
        &unsealed,
        AcceptPolicy::RequireLive,
        "digest",
        EnvelopeErrorKind::DigestMismatch,
    );
}

#[test]
fn stopped_dispatch_retains_late_check_without_upgrading_outcome() {
    let mut stopped = envelope();
    stopped.result.verdict = Verdict::Inconclusive;
    stopped.dispatches[1].check_ns = Obs::Known(1_350);
    let bytes = encode_envelope(&stopped).expect("in-flight check after stop is retained");
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive)
        .expect("stopped evidence decodes");
    assert_eq!(decoded, stopped);

    stopped.dispatches[1].check_ns = Obs::Known(1_500);
    let bytes = encode_envelope(&stopped).expect("supplemental post-exit check is retained");
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive)
        .expect("supplemental evidence decodes");
    assert_eq!(decoded, stopped);
    assert_eq!(decoded.result.verdict, Verdict::Inconclusive);

    // Submission can race admission closure; the snapshot does not identify
    // admission time or prove terminal-history immutability.
    stopped.dispatches[1].submit_ns = Obs::Known(1_310);
    stopped.dispatches[1].readback_ns = Obs::Known(1_320);
    stopped.dispatches[1].mismatch = 1;
    stopped.dispatches[1].retention = Retention::FullFailure;
    stopped.result.correctness = Correctness::Failed;
    stopped.result.verdict = Verdict::Invalid;
    let bytes = encode_envelope(&stopped).expect("racing submission and late failure are retained");
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive)
        .expect("invalid supplemental evidence decodes");
    assert_eq!(decoded, stopped);
    assert_eq!(decoded.result.verdict, Verdict::Invalid);
}

#[test]
fn aggregate_observations_count_independent_metric_slot_pairs() {
    let mut paired = envelope();
    paired.telemetry[1].slot = 0;
    let bytes = encode_envelope(&paired).expect("two metric-slot pairs encode");
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive)
        .expect("two metric-slot pairs decode");
    assert_eq!(decoded, paired);

    paired.integrity.observed_slots = 1;
    paired.result.verdict = Verdict::Inconclusive;
    assert_encode_error(&paired, "observed_slots", EnvelopeErrorKind::CrossField);
    assert_decode_error(
        &mutate(
            &bytes,
            "\"observed_slots\":\"2\"",
            "\"observed_slots\":\"1\"",
        ),
        AcceptPolicy::RequireLive,
        "observed_slots",
        EnvelopeErrorKind::CrossField,
    );

    paired.integrity.observed_slots = 2;
    paired.telemetry[1].metric = "power".to_owned();
    assert_encode_error(&paired, "slot", EnvelopeErrorKind::CrossField);
    assert_decode_error(
        &mutate(&bytes, "\"metric\":\"temp\"", "\"metric\":\"power\""),
        AcceptPolicy::RequireLive,
        "slot",
        EnvelopeErrorKind::CrossField,
    );
}

#[test]
fn lifecycle_and_dispatch_timings_must_be_ordered_and_submit_not_before_start() {
    let mut backward_exit = envelope();
    backward_exit.lifecycle.exit_ns = Obs::Known(1_050);
    assert_encode_error(&backward_exit, "exit_ns", EnvelopeErrorKind::CrossField);

    let mut backward_readback = envelope();
    backward_readback.dispatches[1].query_ns = Obs::Known(1_240);
    backward_readback.dispatches[1].readback_ns = Obs::Known(1_220);
    assert_encode_error(
        &backward_readback,
        "readback_ns",
        EnvelopeErrorKind::CrossField,
    );

    let mut early_submit = envelope();
    early_submit.dispatches[1].submit_ns = Obs::Known(1_100);
    assert_encode_error(&early_submit, "submit_ns", EnvelopeErrorKind::CrossField);

    let mut backward_check = envelope();
    backward_check.dispatches[1].check_ns = Obs::Known(1_210);
    assert_encode_error(&backward_check, "check_ns", EnvelopeErrorKind::CrossField);

    let mut backward_receipt = envelope();
    backward_receipt.telemetry[0].receipt_ns = Obs::Known(2_050);
    assert_encode_error(
        &backward_receipt,
        "receipt_ns",
        EnvelopeErrorKind::CrossField,
    );
}

#[test]
fn telemetry_internal_age_and_multi_metric_slots_are_validated() {
    let mut unassessed = envelope();
    unassessed.telemetry[1].internal_age = InternalAge::Assessed;
    assert_encode_error(&unassessed, "internal_age", EnvelopeErrorKind::CrossField);

    // Two metrics may share a numeric slot: each pair counts independently.
    let mut multi_metric = envelope();
    multi_metric.telemetry[1].slot = 0;
    multi_metric.integrity.expected_slots = 2;
    multi_metric.integrity.observed_slots = 2;
    multi_metric.result.verdict = Verdict::Inconclusive;
    let bytes = encode_envelope(&multi_metric).expect("multi-metric slot encodes");
    let decoded = decode_envelope(bytes.as_bytes(), bytes.len(), AcceptPolicy::RequireLive)
        .expect("multi-metric slot decodes");
    assert_eq!(decoded.telemetry.len(), 2);
    assert_eq!(decoded.integrity.observed_slots, 2);
}

#[test]
fn retention_and_empty_job_consistency_are_validated() {
    let mut missing_digest = envelope();
    missing_digest.dispatches[0].output_digest = Obs::Unavailable(reason("not_read"));
    assert_encode_error(
        &missing_digest,
        "output_digest",
        EnvelopeErrorKind::CrossField,
    );

    let mut empty_mismatch = envelope();
    empty_mismatch.result.empty_job = ResultEmptyJob::Observed;
    assert_encode_error(&empty_mismatch, "empty_job", EnvelopeErrorKind::CrossField);

    let mut nonfinite_verdict = envelope();
    nonfinite_verdict.dispatches[1].nonfinite = 1;
    assert_encode_error(&nonfinite_verdict, "verdict", EnvelopeErrorKind::CrossField);

    let mut unmatched_identity = envelope();
    unmatched_identity.identity.nvml_uuid = Obs::Unavailable(reason("not_sampled"));
    assert_encode_error(
        &unmatched_identity,
        "uuid_match",
        EnvelopeErrorKind::CrossField,
    );
}

#[test]
fn schema_and_version_constants_match_the_document() {
    assert_eq!(SCHEMA, "autotune.live-session");
    assert_eq!(VERSION, "1");
    assert!(document().starts_with(&format!(
        "{{\"schema\":\"{SCHEMA}\",\"version\":\"{VERSION}\","
    )));
}

#[test]
fn byte_and_depth_bounds_use_the_phase1_reader() {
    let bytes = document();
    assert_eq!(
        decode(bytes.as_bytes(), bytes.len() - 1),
        Err(ParseError::Oversize)
    );
    assert_decode_error_with_max(
        &bytes,
        bytes.len() - 1,
        "envelope",
        EnvelopeErrorKind::Parse,
    );

    let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
    assert_eq!(decode(deep.as_bytes(), deep.len()), Err(ParseError::Depth));
    assert_decode_error(
        &deep,
        AcceptPolicy::RequireLive,
        "envelope",
        EnvelopeErrorKind::Parse,
    );

    // A scalar root is a typed root rejection, not a section parse.
    let scalar = "\"text\"";
    assert_decode_error(
        scalar,
        AcceptPolicy::RequireLive,
        "envelope",
        EnvelopeErrorKind::Root,
    );
}
