mod worker_setup_tests {
    include!("worker_setup_delivery.rs");
    use super::*;
    use autotune_benchmark_worker::{
        evidence::{decode, Domain, Value},
        workload::{
            child_options,
            nvml_trace::FileCheck,
            runtime_evidence::LoadError,
            setup_model::{self, Entry, Fault as TraceFault, Operation, Output, Return},
            wire::WorkerBinding,
            worker_setup::{Artifact, Facts, Fault, File, Preparation, Receipt, CAP},
        },
    };
    use autotune_probe::runtime_file::{FileRecord, Kind, Report, Stage, MAX_CHECKS};
    fn binding(b: &Budget, run: usize) -> WorkerBinding {
        let m = manifest(b);
        WorkerBinding::new(&m.bind_run(run, 0).unwrap(), 1_000_000_000, a(0)).unwrap()
    }
    fn facts<'a>(entries: &'a [Entry], checks: &'a [FileCheck]) -> Facts<'a> {
        Facts {
            first: a(100),
            deadline_raw: 100_000_000,
            preparation: Preparation::CudaFailed,
            cpu_fault: None,
            native_fault: None,
            entries,
            pending: true,
            fault: Some(TraceFault::Clock),
            overflow: None,
            rejected: None,
            file_checks: checks,
            file_overflow: None,
            file: None,
            load_error: None,
            environment: Default::default(),
            driver_version: None,
            module_loading_mode: None,
        }
    }
    fn read(bytes: &[u8], binding: WorkerBinding, b: &Budget) -> Receipt {
        Receipt::read(
            bytes,
            binding,
            100_000_000,
            Digest::of(Domain::RawArtifact, bytes),
            b,
        )
        .ok()
        .unwrap()
    }
    fn edit(value: &Value, path: &[&str], replacement: Value) -> Value {
        if path.is_empty() {
            return replacement;
        }
        let object = value.as_object().unwrap();
        let mut out = autotune_benchmark_worker::evidence::Object::new();
        for (key, value) in object.iter() {
            out.insert(
                key,
                if key == path[0] {
                    edit(value, &path[1..], replacement.clone())
                } else {
                    value.clone()
                },
            )
            .unwrap();
        }
        Value::Object(out)
    }
    #[test]
    fn worker_setup_roundtrips_every_operation_output_and_raw_fault_without_success_inference() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let ops = [
            Operation::Environment(child_options::Variable::CacheDisable),
            Operation::Environment(child_options::Variable::ModuleLoading),
            Operation::Initialize,
            Operation::DriverVersion,
            Operation::ModuleLoadingMode,
            Operation::DeviceCount,
            Operation::Device(u32::MAX),
            Operation::Uuid(i32::MIN),
            Operation::Attribute {
                device: i32::MAX,
                attribute: i32::MIN,
            },
            Operation::ContextCreate,
            Operation::ContextDevice,
            Operation::ModuleLoad,
            Operation::Function,
            Operation::MemoryInfo,
            Operation::Allocate(u8::MAX),
            Operation::StreamCreate,
        ];
        let env = child_options::Read {
            length: u32::MAX,
            error: Some(u32::MAX),
            units: [0xd800; 16],
        };
        let outputs = [
            Output::None,
            Output::Integer(i32::MIN),
            Output::Uuid([0xff; 16]),
            Output::HandleIsNull(false),
            Output::AllocationIsZero(true),
            Output::Memory {
                free: usize::MAX,
                total: 1,
            },
            Output::Environment(env),
        ];
        let entries: Vec<_> = ops
            .into_iter()
            .enumerate()
            .map(|(i, operation)| Entry {
                operation,
                before: Some(a(200)),
                after: None,
                attempted: i % 2 == 0,
                raw: Some(Return {
                    status: i32::MIN,
                    output: outputs[i % outputs.len()],
                }),
            })
            .collect();
        let mut f = facts(&entries, &[]);
        f.environment.cache_disable = Some(env);
        f.load_error = Some(LoadError {
            known: true,
            code: Some(i32::MIN),
        });
        f.driver_version = Some(-1);
        f.module_loading_mode = Some(i32::MAX);
        f.overflow = Some((Operation::Device(u32::MAX), None));
        f.rejected = Some((
            Operation::Allocate(255),
            Return {
                status: -99,
                output: Output::None,
            },
            Some(a(10)),
        ));
        let artifact = Artifact::recorded(bind, f, &b).unwrap();
        let receipt = read(artifact.canonical().as_bytes(), bind, &b);
        let r = receipt.reported();
        assert!(r.simulated);
        assert_eq!(r.entries, entries);
        assert!(r.pending);
        assert_eq!(r.environment.cache_disable, Some(env));
        assert_eq!(r.driver_version, Some(-1));
        assert_eq!(r.module_loading_mode, Some(i32::MAX));
        assert!(r.overflow.is_some());
        assert!(r.rejected.is_some());
        assert_eq!(r.fault, Some(TraceFault::Clock));
        assert_eq!(receipt.digest(), artifact.digest());
        assert_eq!(receipt.canonical(), artifact.canonical());
    }
    #[test]
    fn worker_setup_retains_partial_file_paths_native_error_and_distinct_cpu_failure() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let file = FileRecord {
            kind: Kind::Cuda,
            requested: Some(vec![0xd800, 0, 0xffff]),
            loaded: None,
            directory: None,
            open: Some(Err(Some(-5))),
            report: Report::default(),
            module_path: None,
            loaded_open: None,
            loaded_identity: None,
            fault: None,
            held: false,
            bound: false,
        };
        let native = autotune_benchmark_worker::worker::Fault {
            operation: "cuModuleGetLoadingMode",
            raw_status: -71,
        };
        let mut f = facts(&[], &[]);
        f.preparation = Preparation::CpuFailed;
        f.cpu_fault = Some("native");
        f.native_fault = Some(&native);
        f.file = Some(File::Recorded(&file));
        f.load_error = Some(LoadError {
            known: false,
            code: None,
        });
        let artifact = Artifact::recorded(bind, f, &b).unwrap();
        let receipt = read(artifact.canonical().as_bytes(), bind, &b);
        let r = receipt.reported();
        assert_eq!(r.file.as_ref(), Some(&file));
        assert_eq!(r.preparation, Preparation::CpuFailed);
        assert_eq!(r.cpu_fault.as_deref(), Some("native"));
        assert_eq!(r.native_fault, Some((native.operation.into(), -71)));
        assert_eq!(
            r.load_error,
            Some(LoadError {
                known: false,
                code: None
            })
        );
    }
    #[test]
    fn worker_setup_requires_exact_parent_run_deadline_and_delivered_digest() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let other = binding(&b, 1);
        let artifact = Artifact::recorded(bind, facts(&[], &[]), &b).unwrap();
        let bytes = artifact.canonical().as_bytes();
        assert!(matches!(
            Receipt::read(bytes, other, 100_000_000, artifact.digest(), &b),
            Err(Fault::Binding)
        ));
        assert!(matches!(
            Receipt::read(bytes, bind, 100_000_001, artifact.digest(), &b),
            Err(Fault::Deadline)
        ));
        assert!(matches!(
            Receipt::read(bytes, bind, 100_000_000, digest(99), &b),
            Err(Fault::Digest)
        ));
    }
    #[test]
    fn worker_setup_reader_rejects_extra_keys_enum_spelling_width_and_union_contradictions() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let mut f = facts(&[], &[]);
        f.environment.cache_disable = Some(child_options::Read {
            length: 1,
            error: None,
            units: [0; 16],
        });
        let artifact = Artifact::recorded(bind, f, &b).unwrap();
        let v = decode(artifact.canonical().as_bytes(), CAP).unwrap();
        for (path, replacement) in [
            (vec!["version"], Value::Text("2".into())),
            (vec!["source"], Value::Text("qualified".into())),
            (vec!["preparation"], Value::Text("successful".into())),
            (vec!["driver_version"], Value::I64(i64::MAX)),
            (vec!["cpu_fault"], Value::Text("retry".into())),
            (
                vec!["environment", "cache_disable", "length"],
                Value::U64(u64::MAX),
            ),
            (
                vec!["environment", "cache_disable", "utf16le"],
                Value::Text("00".repeat(31)),
            ),
            (vec!["fault", "status"], Value::I64(-5)),
        ] {
            let bytes = edit(&v, &path, replacement).to_canonical();
            assert!(
                Receipt::read(
                    bytes.as_bytes(),
                    bind,
                    100_000_000,
                    Digest::of(Domain::RawArtifact, bytes.as_bytes()),
                    &b
                )
                .is_err(),
                "{path:?}"
            );
        }
        let bytes = artifact
            .canonical()
            .replacen("{\"version\"", "{\"extra\":null,\"version\"", 1);
        assert!(Receipt::read(
            bytes.as_bytes(),
            bind,
            100_000_000,
            Digest::of(Domain::RawArtifact, bytes.as_bytes()),
            &b
        )
        .is_err());
    }
    #[test]
    fn worker_setup_full_file_history_with_overflow_and_full_setup_ledger_fits_without_eviction() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let entries = vec![
            Entry {
                operation: Operation::ModuleLoadingMode,
                before: None,
                after: Some(a(u64::MAX - 100)),
                attempted: true,
                raw: Some(Return {
                    status: -71,
                    output: Output::Integer(-1)
                })
            };
            setup_model::CAP
        ];
        let checks: Vec<_> = (0..MAX_CHECKS)
            .map(|n| FileCheck {
                stage: Stage::ReadAfter(n as u32),
                anchor: Some(a(n as u64 * 100)),
            })
            .collect();
        let mut f = facts(&entries, &checks);
        f.file_overflow = Some(FileCheck {
            stage: Stage::LoadedMetadataAfter,
            anchor: None,
        });
        let artifact = Artifact::recorded(bind, f, &b).unwrap();
        assert!(artifact.canonical().len() <= CAP);
        let receipt = read(artifact.canonical().as_bytes(), bind, &b);
        assert_eq!(receipt.reported().entries, entries);
        assert_eq!(receipt.reported().file_checks, checks);
        assert!(receipt.reported().file_overflow.is_some());
    }
    #[test]
    fn worker_setup_capacity_rejection_preserves_inputs_and_releases_reservations() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let too_many = vec![
            Entry {
                operation: Operation::Initialize,
                before: None,
                after: None,
                attempted: false,
                raw: None
            };
            setup_model::CAP + 1
        ];
        let used = b.used();
        assert!(matches!(
            Artifact::recorded(bind, facts(&too_many, &[]), &b),
            Err(Fault::Capacity)
        ));
        assert_eq!(too_many.len(), setup_model::CAP + 1);
        assert_eq!(b.used(), used);
        let artifact = Artifact::recorded(bind, facts(&[], &[]), &b).unwrap();
        let used = b.used();
        {
            let receipt = read(artifact.canonical().as_bytes(), bind, &b);
            assert!(b.used() > used);
            drop(receipt);
        }
        assert_eq!(b.used(), used);
        let tiny = Budget::new(1).unwrap();
        assert!(matches!(
            Receipt::read(
                artifact.canonical().as_bytes(),
                bind,
                100_000_000,
                artifact.digest(),
                &tiny
            ),
            Err(Fault::Capacity)
        ));
        assert_eq!(tiny.used(), 0);
        let _pressure = b.reserve(32 * 1024 * 1024 - b.used()).unwrap();
        let used = b.used();
        assert!(matches!(
            Artifact::recorded(bind, facts(&[], &[]), &b),
            Err(Fault::Capacity)
        ));
        assert_eq!(b.used(), used);
    }
    #[test]
    fn worker_setup_bad_clock_and_native_source_claim_remain_reported_without_issuing_admission() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let mut clock = a(200);
        clock.clock_token = [99; 16];
        let checks = [FileCheck {
            stage: Stage::LibraryLoadAfter,
            anchor: Some(clock),
        }];
        let artifact = Artifact::recorded(bind, facts(&[], &checks), &b).unwrap();
        let value = decode(artifact.canonical().as_bytes(), CAP).unwrap();
        let bytes =
            edit(&value, &["source"], Value::Text("native_setup_path".into())).to_canonical();
        let receipt = read(bytes.as_bytes(), bind, &b);
        assert!(!receipt.reported().simulated);
        assert_eq!(receipt.reported().file_checks, checks);
        assert_eq!(receipt.reported().preparation, Preparation::CudaFailed);
        assert!(receipt.reported().pending);
    }
}
