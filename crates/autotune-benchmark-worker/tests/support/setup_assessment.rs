mod assessment_tests {
    #[test]
    fn delivered_assessment_retains_one_result_and_rejection_poison_without_losing_evidence() {
        use autotune_benchmark_worker::{
            sampler::wire::WireError,
            workload::{
                dispatch::Phase,
                wire::Message,
                worker_setup_delivery::{Chunks, Receiver},
            },
        };
        for rejected in [false, true] {
            let b = Budget::new(32 * 1024 * 1024).unwrap();
            let bind = binding(&b, 0);
            let mut f = fixture(bind);
            if rejected {
                f.entries[14].raw.as_mut().unwrap().output = Output::Integer(0);
            }
            let artifact = artifact(&f, bind, &b);
            let native = artifact.canonical().replace(
                "\"source\":\"simulated_fixture\"",
                "\"source\":\"native_setup_path\"",
            );
            assert_eq!(native.len(), artifact.canonical().len());
            let digest = Digest::of(Domain::RawArtifact, native.as_bytes());
            let chunks = Chunks::new(&artifact, &b).unwrap();
            let mut tx = super::delivery_tests::channel(bind, &b);
            let mut rx = Receiver::new(bind, a(100), 100_000_000, b.clone()).unwrap();
            for index in 0..chunks.count() {
                let frame = tx.encode(&chunks.message(index).unwrap()).unwrap();
                let value = decode(&frame.as_bytes()[4..], 64 * 1024).unwrap();
                let start = index as usize * CHUNK_BYTES;
                let end = (start + CHUNK_BYTES).min(native.len());
                let value = edit(
                    &value,
                    &["body", "data"],
                    Value::Text(super::delivery_tests::hex(&native.as_bytes()[start..end])),
                );
                let text =
                    edit(&value, &["body", "digest"], Value::Text(digest.to_hex())).to_canonical();
                let mut bytes = ((text.len() + 4) as u32).to_le_bytes().to_vec();
                bytes.extend_from_slice(text.as_bytes());
                rx.accept(super::delivery_tests::raw(
                    &bytes,
                    Some(a(20_000 + index as u64 * 1000)),
                    &b,
                ))
                .unwrap();
            }
            let frame = tx.encode(&f.ready).unwrap();
            rx.accept(super::delivery_tests::raw(
                frame.as_bytes(),
                Some(a(120_000)),
                &b,
            ))
            .unwrap();
            let mut delivery = rx.into_ready().ok().unwrap();
            assert!(delivery.assessment().is_none());
            let result = delivery.assess_setup();
            assert_eq!(delivery.assessment(), Some(result));
            assert_eq!(delivery.assess_setup(), result);
            assert_eq!(delivery.receipt().canonical(), native);
            let frame = tx
                .encode(&Message::PhaseEntered {
                    phase: Phase::Warmup,
                    anchor: a(130_000),
                })
                .unwrap();
            if rejected {
                assert_eq!(result.unwrap_err().cause, Cause::Context);
                assert!(matches!(
                    delivery.decoder().decode(frame.as_bytes()),
                    Err(WireError::Poisoned)
                ));
            } else {
                assert!(result.is_ok());
                assert!(delivery.decoder().decode(frame.as_bytes()).is_ok());
            }
        }
    }
    use super::*;
    use autotune_benchmark_worker::workload::{
        child_options::Variable,
        worker_setup_assessment::{screen, Cause, Rejection, Site, Summary},
        worker_setup_delivery::{Arrival, CHUNK_BYTES},
    };

    mod fixture_data {
        include!("setup_fixture.rs");
    }
    use fixture_data::{push, retime, Fixture};
    fn fixture(bind: WorkerBinding) -> Fixture {
        fixture_data::fixture(bind, digest(9), 65536)
    }
    fn artifact(f: &Fixture, bind: WorkerBinding, b: &Budget) -> Artifact {
        let mut facts = facts(&f.entries, &f.checks);
        facts.first = a(1000);
        facts.preparation = Preparation::Ready;
        facts.pending = false;
        facts.fault = None;
        facts.file = Some(File::Recorded(&f.file));
        facts.environment = f.environment;
        facts.driver_version = Some(12090);
        facts.module_loading_mode = Some(1);
        Artifact::recorded(bind, facts, b).unwrap()
    }
    fn claim(artifact: &Artifact, bind: WorkerBinding, b: &Budget) -> Receipt {
        let bytes = artifact.canonical().replace(
            "\"source\":\"simulated_fixture\"",
            "\"source\":\"native_setup_path\"",
        );
        assert_ne!(bytes, artifact.canonical());
        read(bytes.as_bytes(), bind, b)
    }
    fn arrivals(receipt: &Receipt) -> Vec<Arrival> {
        let count = receipt.canonical().len().div_ceil(CHUNK_BYTES);
        (0..=count)
            .map(|i| Arrival {
                receipt: Some(a(if i < count {
                    20_000 + i as u64 * 1000
                } else {
                    120_000
                })),
                frame_digest: digest(88),
                index: if i < count { Some(i as u32) } else { None },
                origin: None,
            })
            .collect()
    }
    fn assess(f: &Fixture, bind: WorkerBinding, b: &Budget) -> Result<Summary, Rejection> {
        let artifact = artifact(f, bind, b);
        let receipt = claim(&artifact, bind, b);
        screen(&receipt, bind, &f.ready, &arrivals(&receipt))
    }
    #[test]
    fn consistent_native_claim_passes_only_reported_checks_while_fixture_source_rejects() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let f = fixture(bind);
        let artifact = artifact(&f, bind, &b);
        let simulated = read(artifact.canonical().as_bytes(), bind, &b);
        assert_eq!(
            screen(&simulated, bind, &f.ready, &arrivals(&simulated))
                .unwrap_err()
                .cause,
            Cause::Simulated
        );
        drop(simulated);
        let claimed = claim(&artifact, bind, &b);
        let summary = screen(&claimed, bind, &f.ready, &arrivals(&claimed)).unwrap();
        assert_eq!(summary.selected_device, 42);
        assert_eq!(summary.selected_ordinal, 0);
        assert_eq!(summary.calls, 23);
        assert_eq!(summary.file_checks, 20);
        assert_eq!(summary.dll_bytes, 3);
    }
    #[test]
    fn missing_attempt_status_and_post_clock_failures_keep_precise_call_sites() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        for mode in 0..5 {
            let mut f = fixture(bind);
            let expected = match mode {
                0 => {
                    f.entries[2].attempted = false;
                    Cause::Attempt
                }
                1 => {
                    f.entries[2].raw = None;
                    Cause::Missing
                }
                2 => {
                    f.entries[2].after = None;
                    Cause::Clock(None)
                }
                3 => {
                    f.entries[2].raw.as_mut().unwrap().status = -71;
                    f.entries[2].after = None;
                    Cause::Status(-71)
                }
                _ => {
                    f.entries[2].operation = Operation::DriverVersion;
                    Cause::Order
                }
            };
            assert_eq!(
                assess(&f, bind, &b),
                Err(Rejection {
                    site: Site::Call(2),
                    cause: expected
                })
            );
        }
    }
    #[test]
    fn full_sequence_is_required_and_extra_entries_are_not_ignored() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let mut f = fixture(bind);
        f.entries.pop();
        assert_eq!(
            assess(&f, bind, &b).unwrap_err(),
            Rejection {
                site: Site::Call(22),
                cause: Cause::Missing
            }
        );
        let mut f = fixture(bind);
        push(&mut f.entries, Operation::Initialize, Output::None);
        retime(&mut f);
        assert_eq!(
            assess(&f, bind, &b).unwrap_err(),
            Rejection {
                site: Site::Call(23),
                cause: Cause::Trailing
            }
        );
    }
    #[test]
    fn environment_mode_version_and_declared_readback_must_match_actual_return_ledger() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        for mode in 0..5 {
            let mut f = fixture(bind);
            let expected = match mode {
                0 => {
                    f.environment.cache_disable = None;
                    Cause::Environment(Variable::CacheDisable)
                }
                1 => {
                    if let Output::Environment(ref mut r) =
                        f.entries[1].raw.as_mut().unwrap().output
                    {
                        r.units[0] = b'L' as u16;
                    }
                    Cause::Environment(Variable::ModuleLoading)
                }
                2 => {
                    f.entries[3].raw.as_mut().unwrap().output = Output::Integer(-1);
                    Cause::Version
                }
                3 => {
                    f.entries[4].raw.as_mut().unwrap().output = Output::Integer(2);
                    Cause::LoadingMode
                }
                _ => {
                    f.entries[4].raw.as_mut().unwrap().output = Output::Integer(99);
                    Cause::LoadingMode
                }
            };
            assert_eq!(assess(&f, bind, &b).unwrap_err().cause, expected);
        }
    }
    #[test]
    fn mapping_context_capability_memory_and_allocations_are_checked_before_readiness() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        for (index, output, cause) in [
            (5, Output::Integer(65), Cause::DeviceCount),
            (6, Output::Integer(-1), Cause::Mapping),
            (7, Output::Uuid([0; 16]), Cause::Mapping),
            (8, Output::Integer(99), Cause::Mapping),
            (11, Output::Integer(8), Cause::Capability),
            (14, Output::Integer(0), Cause::Context),
            (
                17,
                Output::Memory {
                    free: 1,
                    total: 1 << 30,
                },
                Cause::Memory,
            ),
            (18, Output::AllocationIsZero(true), Cause::Memory),
        ] {
            let mut f = fixture(bind);
            f.entries[index].raw.as_mut().unwrap().output = output;
            assert_eq!(assess(&f, bind, &b).unwrap_err().cause, cause);
        }
    }
    #[test]
    fn reordered_device_enumeration_is_allowed_but_duplicate_handles_and_uuids_are_not() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let mut f = fixture(bind);
        f.entries[5].raw.as_mut().unwrap().output = Output::Integer(2);
        f.entries[6].operation = Operation::Device(1);
        let mut added = Vec::new();
        push(&mut added, Operation::Device(0), Output::Integer(7));
        push(&mut added, Operation::Uuid(7), Output::Uuid([99; 16]));
        f.entries.splice(6..6, added);
        retime(&mut f);
        assert_eq!(assess(&f, bind, &b).unwrap().selected_ordinal, 1);
        f.entries[8].raw.as_mut().unwrap().output = Output::Integer(7);
        assert_eq!(assess(&f, bind, &b).unwrap_err().cause, Cause::Mapping);
        f.entries[8].raw.as_mut().unwrap().output = Output::Integer(42);
        f.entries[9].raw.as_mut().unwrap().output = Output::Uuid([99; 16]);
        assert_eq!(assess(&f, bind, &b).unwrap_err().cause, Cause::Mapping);
    }
    #[test]
    fn file_identity_hash_lifecycle_paths_and_logical_read_counts_must_be_consistent() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        for mode in 0..6 {
            let mut f = fixture(bind);
            match mode {
                0 => f.file.bound = false,
                1 => f.file.report.sha256 = None,
                2 => f.file.loaded_identity = Some(Err(Some(5))),
                3 => {
                    f.file.requested =
                        Some("C:\\Windows\\System32\\otherx.dll".encode_utf16().collect());
                }
                4 => {
                    f.file.report.bytes = 1;
                    let mut id = f.file.report.before.unwrap().unwrap();
                    id.bytes = 1;
                    f.file.report.before = Some(Ok(id));
                    f.file.report.after = Some(Ok(id));
                    f.file.loaded_identity = Some(Ok(id));
                    f.file.report.reads = 3;
                    f.checks.splice(
                        10..10,
                        [
                            FileCheck {
                                stage: Stage::ReadBefore(2),
                                anchor: Some(a(3950)),
                            },
                            FileCheck {
                                stage: Stage::ReadAfter(2),
                                anchor: Some(a(3975)),
                            },
                        ],
                    );
                }
                _ => {
                    f.checks.remove(3);
                }
            }
            let cause = assess(&f, bind, &b).unwrap_err().cause;
            assert!(matches!(
                cause,
                Cause::File(_) | Cause::FileReads | Cause::FilePath
            ));
        }
    }
    #[test]
    fn cross_file_call_clock_regression_foreign_token_and_original_expiry_reject() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        for mode in 0..4 {
            let mut f = fixture(bind);
            match mode {
                0 => f.checks[0].anchor = Some(a(2100)),
                1 => {
                    let mut v = a(6000);
                    v.clock_token = [77; 16];
                    f.entries[2].before = Some(v);
                }
                2 => {
                    f.entries[2].before = Some(a(10_000_000_000));
                }
                _ => {
                    f.entries[2].after = Some(a(5000));
                }
            }
            let cause = assess(&f, bind, &b).unwrap_err().cause;
            assert!(matches!(cause, Cause::Clock(_) | Cause::Deadline));
        }
    }
    #[test]
    fn independent_ready_and_reception_branches_allow_overlap_but_require_causal_final_receipt() {
        use autotune_benchmark_worker::workload::wire::Message;
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let mut f = fixture(bind);
        let artifact = artifact(&f, bind, &b);
        let receipt = claim(&artifact, bind, &b);
        let original = arrivals(&receipt);
        assert!(original[0].receipt.unwrap().qpc_before < 100_000);
        assert!(screen(&receipt, bind, &f.ready, &original).is_ok());
        let mut bad = original.clone();
        bad[0].receipt = Some(a(1000));
        assert!(screen(&receipt, bind, &f.ready, &bad).is_err());
        let mut bad = original.clone();
        bad.last_mut().unwrap().receipt = Some(a(90_000));
        assert!(screen(&receipt, bind, &f.ready, &bad).is_err());
        if let Message::Ready { driver_version, .. } = &mut f.ready {
            *driver_version = 12080;
        }
        assert_eq!(
            screen(&receipt, bind, &f.ready, &original)
                .unwrap_err()
                .cause,
            Cause::Version
        );
    }
    #[test]
    fn screening_allocates_no_new_model_and_rejects_a_different_parent_binding() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let other = binding(&b, 1);
        let f = fixture(bind);
        let artifact = artifact(&f, bind, &b);
        let receipt = claim(&artifact, bind, &b);
        let arrivals = arrivals(&receipt);
        let _pressure = b.reserve(32 * 1024 * 1024 - b.used()).unwrap();
        let used = b.used();
        assert!(screen(&receipt, bind, &f.ready, &arrivals).is_ok());
        assert_eq!(b.used(), used);
        assert_eq!(
            screen(&receipt, other, &f.ready, &arrivals)
                .unwrap_err()
                .cause,
            Cause::Binding
        );
    }
}
