mod runtime_export_tests {
    use super::*;
    use autotune_benchmark_worker::workload::{
        runtime_evidence::{self, Input, LoadError, Source as RuntimeSource},
        starting_delivery::Chunks,
        starting_session::Session,
    };
    use autotune_probe::runtime_file::{
        Fault as ReadFault, FileRecord, Identity, ImageFault, Kind, MAX_CHECKS, MAX_PATH_UNITS,
        PathReturn, Report, Stage,
    };
    fn file() -> FileRecord {
        let identity = Identity {
            volume_serial: u32::MAX,
            index: u64::MAX - 1,
            bytes: 3,
            last_write: u64::MAX - 2,
            attributes: 0x20,
            file_type: 1,
            file_type_error: None,
        };
        let requested = "C:\\Windows\\System32\\nvml.dll"
            .encode_utf16()
            .collect::<Vec<_>>();
        FileRecord {
            kind: Kind::Nvml,
            requested: Some(requested.clone()),
            loaded: Some(requested.clone()),
            directory: Some(PathReturn {
                length: 19,
                error: None,
            }),
            open: Some(Ok(())),
            report: Report {
                before: Some(Ok(identity)),
                after: Some(Ok(identity)),
                reads: 2,
                last_read: Some(Ok(0)),
                bytes: 3,
                sha256: Some(
                    *Digest::parse(
                        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                    )
                    .unwrap()
                    .as_bytes(),
                ),
                fault: None,
            },
            module_path: Some(PathReturn {
                length: requested.len() as u32,
                error: None,
            }),
            loaded_open: Some(Ok(())),
            loaded_identity: Some(Ok(identity)),
            fault: None,
            held: true,
            bound: true,
        }
    }
    fn checks(f: &mut Fixture) {
        let gate = NativeAdmission::default();
        let mut n = 2000;
        let mut o = f.ledger.observer(&gate, || {
            n += 100;
            Some(a(n))
        });
        for s in [
            Stage::DirectoryBefore,
            Stage::DirectoryAfter,
            Stage::OpenBefore,
            Stage::OpenAfter,
            Stage::MetadataBefore,
            Stage::MetadataAfter,
            Stage::ReadBefore(0),
            Stage::ReadAfter(0),
            Stage::ReadBefore(1),
            Stage::ReadAfter(1),
            Stage::FinalMetadataBefore,
            Stage::FinalMetadataAfter,
            Stage::LibraryLoadBefore,
            Stage::LibraryLoadAfter,
            Stage::ModulePathBefore,
            Stage::ModulePathAfter,
            Stage::LoadedOpenBefore,
            Stage::LoadedOpenAfter,
            Stage::LoadedMetadataBefore,
            Stage::LoadedMetadataAfter,
        ] {
            assert!(o.file_check(s));
        }
    }
    fn build2(
        f: &Fixture,
        file: Option<&FileRecord>,
        load_error: Option<LoadError>,
        initialization: Option<i32>,
    ) -> Result<Response, Block> {
        Response::build_runtime(
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
                clock_error: None,
                control_error: None,
                event_clone_error: None,
                unreported_completion: None,
            },
            Input::Recorded {
                file,
                load_error,
                initialization,
            },
            &f.b,
        )
    }
    fn claim(f: &Fixture, r: &Response) -> Receipt {
        changed(
            f,
            r,
            &["runtime", "source"],
            Value::Text("native_file_path".into()),
        )
        .unwrap()
    }
    #[test]
    fn v2_roundtrip_retains_file_integer_widths_clock_history_and_simulated_source() {
        let mut f = fixture();
        checks(&mut f);
        let file = file();
        let r = build2(&f, Some(&file), None, Some(0)).unwrap();
        let receipt = read(&f, &r);
        assert_eq!(receipt.summary().response_version, 2);
        let raw = receipt.runtime().unwrap();
        assert_eq!(raw.file(), Some(&file));
        assert_eq!(raw.initialization(), Some(0));
        assert_eq!(raw.checks(), f.ledger.file_checks());
        assert_eq!(raw.source(), RuntimeSource::SimulatedFixture);
        assert_eq!(raw.assess(&f.s), Err(runtime_evidence::Fault::Simulated));
        assert!(r.canonical().contains("18446744073709551614"));
        assert!(r.canonical().contains("\"sha256\":\"ba7816"));
    }
    #[test]
    fn legacy_v1_stays_explicitly_without_runtime_and_cannot_be_downgraded_or_promoted() {
        let f = fixture();
        let old = build(&f, None, None, None, ControlFacts::default()).unwrap();
        let before = old.canonical().to_owned();
        let old_read = read(&f, &old);
        assert_eq!(old_read.summary().response_version, 1);
        assert!(old_read.runtime().is_none());
        drop(old_read);
        let new = build2(&f, None, None, None).unwrap();
        let new_read = read(&f, &new);
        assert_eq!(new_read.summary().response_version, 2);
        assert!(new_read.runtime().unwrap().file().is_none());
        drop(new_read);
        assert!(changed(&f, &new, &["version"], Value::Text("1".into())).is_err());
        assert!(changed(&f, &old, &["version"], Value::Text("2".into())).is_err());
        assert_eq!(
            build(&f, None, None, None, ControlFacts::default())
                .unwrap()
                .canonical(),
            before
        );
    }
    #[test]
    fn failed_and_unknown_file_loader_init_facts_roundtrip_without_success_inference() {
        for error in [
            LoadError {
                known: true,
                code: Some(i32::MIN),
            },
            LoadError {
                known: false,
                code: None,
            },
        ] {
            let f = fixture();
            let mut file = file();
            file.report.last_read = Some(Err(Some(-7)));
            file.report.bytes = 1;
            file.report.sha256 = None;
            file.report.fault = Some(ReadFault::Read);
            file.fault = Some(ImageFault::Measurement);
            file.loaded = None;
            file.bound = false;
            let r = build2(&f, Some(&file), Some(error), Some(999)).unwrap();
            let receipt = read(&f, &r);
            let raw = receipt.runtime().unwrap();
            assert_eq!(raw.file(), Some(&file));
            assert_eq!(raw.load_error(), Some(error));
            assert_eq!(raw.initialization(), Some(999));
            assert_eq!(
                claim(&f, &r).runtime().unwrap().assess(&f.s),
                Err(runtime_evidence::Fault::Loader)
            );
        }
    }
    #[test]
    fn utf16_paths_keep_unpaired_surrogates_without_lossy_replacement() {
        let f = fixture();
        let mut file = file();
        file.requested = Some(vec![67, 58, 92, 0xd800, 0x0061]);
        file.loaded = Some(vec![67, 58, 92, 0xdc00, 0x0061]);
        file.module_path = Some(PathReturn {
            length: 5,
            error: None,
        });
        let r = build2(&f, Some(&file), None, None).unwrap();
        let receipt = read(&f, &r);
        assert_eq!(receipt.runtime().unwrap().file(), Some(&file));
        assert!(r.canonical().contains("43003a005c0000d86100"));
        assert!(!r.canonical().contains('\u{fffd}'));
    }
    #[test]
    fn strict_runtime_shapes_reject_widths_unknown_fields_paths_and_loader_contradiction() {
        let f = fixture();
        let file = file();
        let r = build2(&f, Some(&file), None, Some(0)).unwrap();
        for (path, value) in [
            (vec!["runtime", "version"], Value::Text("3".into())),
            (vec!["runtime", "source"], Value::Text("approved".into())),
            (
                vec!["runtime", "file", "kind"],
                Value::Text("plugin".into()),
            ),
            (
                vec!["runtime", "file", "requested_utf16le"],
                Value::Text("d800".repeat(MAX_PATH_UNITS + 1)),
            ),
            (
                vec!["runtime", "file", "loaded_utf16le"],
                Value::Text("0G00".into()),
            ),
            (
                vec!["runtime", "file", "measurement", "reads"],
                Value::U64(u32::MAX as u64 + 1),
            ),
            (
                vec![
                    "runtime",
                    "file",
                    "measurement",
                    "before",
                    "value",
                    "attributes",
                ],
                Value::U64(u32::MAX as u64 + 1),
            ),
            (
                vec!["runtime", "file", "measurement", "last_read", "error"],
                Value::I64(5),
            ),
            (
                vec!["runtime", "load_error"],
                obj([("known", Value::Bool(false)), ("code", Value::I64(5))]),
            ),
        ] {
            assert!(changed(&f, &r, &path, value).is_err(), "{path:?}");
        }
        let bytes = r.canonical().replacen(
            "\"runtime\":{\"version\":\"1\"",
            "\"runtime\":{\"extra\":null,\"version\":\"1\"",
            1,
        );
        assert_ne!(bytes, r.canonical());
        assert!(
            Receipt::read(
                bytes.as_bytes(),
                &f.s,
                &f.decl,
                Digest::of(Domain::RawArtifact, bytes.as_bytes()),
                10_000_000_000,
                &f.b
            )
            .is_err()
        );
    }
    #[test]
    fn reported_consistency_screen_requires_full_transcript_not_only_bound_flag() {
        let mut f = fixture();
        checks(&mut f);
        let file = file();
        let r = build2(&f, Some(&file), None, Some(0)).unwrap();
        assert_eq!(claim(&f, &r).runtime().unwrap().assess(&f.s), Ok(())); // Reported consistency only, no permit.
        for (path, value) in [
            (vec!["runtime", "file", "bound"], Value::Bool(false)),
            (vec!["runtime", "file", "held"], Value::Bool(false)),
            (
                vec!["runtime", "file", "measurement", "bytes"],
                Value::U64(2),
            ),
            (
                vec!["runtime", "file", "loaded_identity", "value", "index"],
                Value::U64(99),
            ),
            (vec!["runtime", "file", "directory", "error"], Value::U64(5)),
            (vec!["runtime", "file_checks"], Value::Array(Vec::new())),
        ] {
            let value2 = edit(
                &autotune_benchmark_worker::evidence::decode(
                    r.canonical().as_bytes(),
                    autotune_benchmark_worker::workload::starting_export::CAP,
                )
                .unwrap(),
                &path,
                value,
            );
            let claimed = edit(
                &value2,
                &["runtime", "source"],
                Value::Text("native_file_path".into()),
            )
            .to_canonical();
            let receipt = Receipt::read(
                claimed.as_bytes(),
                &f.s,
                &f.decl,
                Digest::of(Domain::RawArtifact, claimed.as_bytes()),
                10_000_000_000,
                &f.b,
            )
            .unwrap();
            assert!(receipt.runtime().unwrap().assess(&f.s).is_err());
        }
    }
    #[test]
    fn failed_clock_token_and_deadline_anchors_are_retained_as_raw_evidence() {
        for mode in 0..3 {
            let mut f = fixture();
            let gate = NativeAdmission::default();
            let anchor = match mode {
                0 => None,
                1 => Some(a(10_000_000_000)),
                _ => {
                    let mut v = a(2000);
                    v.clock_token = [9; 16];
                    Some(v)
                }
            };
            assert!(
                !f.ledger
                    .observer(&gate, || anchor)
                    .file_check(Stage::MetadataBefore)
            );
            let file = file();
            let r = build2(&f, Some(&file), None, Some(0)).unwrap();
            let receipt = read(&f, &r);
            assert_eq!(receipt.runtime().unwrap().checks()[0].anchor, anchor);
            assert!(receipt.summary().native_fault.is_some());
            assert!(claim(&f, &r).runtime().unwrap().assess(&f.s).is_err());
        }
    }
    #[test]
    fn maximum_file_history_and_overflow_roundtrip_without_eviction() {
        let mut f = fixture();
        let gate = NativeAdmission::default();
        let mut n = 2000;
        let mut o = f.ledger.observer(&gate, || {
            n += 100;
            Some(a(n))
        });
        for index in 0..MAX_CHECKS {
            assert!(o.file_check(Stage::ReadBefore(index as u32)));
        }
        assert!(!o.file_check(Stage::FinalMetadataAfter));
        drop(o);
        let r = build2(&f, None, None, None).unwrap();
        let receipt = read(&f, &r);
        let raw = receipt.runtime().unwrap();
        assert_eq!(raw.checks().len(), MAX_CHECKS);
        assert_eq!(raw.checks(), f.ledger.file_checks());
        assert_eq!(raw.overflow(), f.ledger.file_overflow());
        assert!(r.canonical().len() <= autotune_benchmark_worker::workload::starting_export::CAP);
    }
    #[test]
    fn v2_runtime_survives_the_owned_chunk_delivery_and_parent_session() {
        let mut f = fixture();
        checks(&mut f);
        let file = file();
        let r = build2(&f, Some(&file), None, Some(0)).unwrap();
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
        let receipt = session.read_report(&f.s, &f.decl, &f.b).unwrap();
        assert_eq!(receipt.summary().response_version, 2);
        assert_eq!(receipt.runtime().unwrap().file(), Some(&file));
        assert_eq!(receipt.digest(), r.digest());
    }
    #[test]
    fn runtime_capacity_failure_neither_grows_input_history_nor_invents_payload() {
        let mut f = fixture();
        checks(&mut f);
        let file = file();
        let r = build2(&f, Some(&file), None, Some(0)).unwrap();
        let tiny = Budget::new(1).unwrap();
        assert!(matches!(
            Receipt::read(
                r.canonical().as_bytes(),
                &f.s,
                &f.decl,
                r.digest(),
                10_000_000_000,
                &tiny
            ),
            Err(Fault::Capacity)
        ));
        assert_eq!(tiny.used(), 0);
        let count = f.ledger.file_checks().len();
        let _pressure = f.b.reserve(32 * 1024 * 1024 - f.b.used()).unwrap();
        let used = f.b.used();
        assert!(matches!(
            build2(&f, Some(&file), None, Some(0)),
            Err(Block::Capacity)
        ));
        assert_eq!(f.b.used(), used);
        assert_eq!(f.ledger.file_checks().len(), count);
    }
    #[test]
    fn combined_maximum_histories_fail_explicitly_without_truncating_either_ledger() {
        let mut f = fixture();
        let gate = NativeAdmission::default();
        let mut o = f.ledger.observer(&gate, || Some(a(2000)));
        for _ in 0..3077 {
            assert!(o.before(SamplerCall::Count));
            assert!(o.returned(
                SamplerCall::Count,
                SamplerReturn {
                    status: 0,
                    output: SamplerOutput::Count(1),
                }
            ));
        }
        for index in 0..MAX_CHECKS {
            assert!(o.file_check(Stage::ReadBefore(index as u32)));
        }
        drop(o);
        let used = f.b.used();
        assert!(matches!(build2(&f, None, None, None), Err(Block::Capacity)));
        assert_eq!(f.b.used(), used);
        assert_eq!(f.ledger.entries().len(), 3077);
        assert_eq!(f.ledger.file_checks().len(), MAX_CHECKS);
    }
}
