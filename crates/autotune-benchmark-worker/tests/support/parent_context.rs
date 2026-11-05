mod parent_context_tests {
    #[test]
    fn separate_starting_and_setup_requests_bind_exact_scope_run_and_operations() {
        use autotune_benchmark_worker::workload::{
            consent::{Request, Setup, Stage},
            launch::{Declaration as PilotDeclaration, Role},
        };
        let h = Harness::new(200000);
        let run = h.m.bind_run(0, ORIGIN).unwrap();
        let decl = Declaration::new(&h.scope, a(2100), &h.b).unwrap();
        let read = Request::starting(&h.scope, &decl, a(2200), &h.b).unwrap();
        let worker =
            PilotDeclaration::new(&run, h.scope.draft().frequency, a(0), Role::Worker, &h.b)
                .unwrap();
        let sampler =
            PilotDeclaration::new(&run, h.scope.draft().frequency, a(0), Role::Sampler, &h.b)
                .unwrap();
        let setup = Request::setup(
            Setup {
                run: &run,
                scope: &h.scope,
                worker: &worker,
                sampler: &sampler,
                entry: a(ORIGIN + 1000),
                issued: a(ORIGIN + 2000),
            },
            &h.b,
        )
        .unwrap();
        assert_eq!(read.stage(), Stage::StartingRead);
        assert_eq!(setup.stage(), Stage::PilotSetup);
        assert_eq!(setup.scope(), h.scope.digest());
        assert_ne!(read.digest(), setup.digest());
        assert!(setup.canonical().contains(&worker.image_digest().to_hex()));
        assert!(setup.canonical().contains(&run.manifest_digest().to_hex()));
    }
    #[test]
    fn setup_request_rejects_wrong_run_role_scope_and_original_expiry() {
        use autotune_benchmark_worker::workload::{
            consent::{Fault, Request, Setup},
            launch::{Declaration as PilotDeclaration, Role},
        };
        let h = Harness::new(200000);
        let run = h.m.bind_run(0, ORIGIN).unwrap();
        let other = h.m.bind_run(1, ORIGIN).unwrap();
        let worker =
            PilotDeclaration::new(&run, h.scope.draft().frequency, a(0), Role::Worker, &h.b)
                .unwrap();
        let sampler =
            PilotDeclaration::new(&run, h.scope.draft().frequency, a(0), Role::Sampler, &h.b)
                .unwrap();
        for mode in 0..3 {
            let result = Request::setup(
                Setup {
                    run: if mode == 0 { &other } else { &run },
                    scope: &h.scope,
                    worker: if mode == 1 { &sampler } else { &worker },
                    sampler: &sampler,
                    entry: a(ORIGIN + 1000),
                    issued: a(if mode == 2 {
                        ORIGIN + 1000 + 10_000_000_000
                    } else {
                        ORIGIN + 2000
                    }),
                },
                &h.b,
            );
            assert_eq!(
                result.err(),
                Some(if mode == 2 {
                    Fault::Deadline
                } else {
                    Fault::Binding
                })
            );
        }
        let mut draft = h.scope.draft().clone();
        draft.source_commit = "b".repeat(40);
        let foreign = Scope::freeze(draft, &h.b).unwrap();
        assert_eq!(
            Request::setup(
                Setup {
                    run: &run,
                    scope: &foreign,
                    worker: &worker,
                    sampler: &sampler,
                    entry: a(ORIGIN + 1000),
                    issued: a(ORIGIN + 2000)
                },
                &h.b
            )
            .err(),
            Some(Fault::Binding)
        );
    }
    #[test]
    fn consistent_context_without_local_peer_evidence_cannot_request_workload_consent() {
        use autotune_benchmark_worker::workload::consent::{Fault, Request};
        let mut h = Harness::new(200000);
        assert!(h.check(true, true).is_ok());
        let run = h.m.bind_run(0, ORIGIN).unwrap();
        assert_eq!(
            Request::workload(&h.parent, &run, a(ORIGIN + 4000), &h.b).err(),
            Some(Fault::Context)
        );
        assert!(!h.parent.native_admission_available());
    }
    #[test]
    fn consent_capacity_rejection_keeps_existing_scope_and_declaration_unchanged() {
        use autotune_benchmark_worker::workload::consent::{Fault, Request};
        let h = Harness::new(200000);
        let decl = Declaration::new(&h.scope, a(2100), &h.b).unwrap();
        let before = h.scope.digest();
        let tag = decl.tag();
        let _pressure = h.b.reserve(32 * 1024 * 1024 - h.b.used()).unwrap();
        assert_eq!(
            Request::starting(&h.scope, &decl, a(2200), &h.b).err(),
            Some(Fault::Capacity)
        );
        assert_eq!(h.scope.digest(), before);
        assert_eq!(decl.tag(), tag);
    }
    #[test]
    fn reported_starting_timing_retains_typed_failures_without_discarding_raw_reports() {
        use autotune_benchmark_worker::workload::starting_read::TimingFault;
        for (mode, expected) in [
            (4, TimingFault::Missing),
            (5, TimingFault::Order),
            (6, TimingFault::Order),
            (7, TimingFault::Window),
            (8, TimingFault::Deadline),
            (9, TimingFault::Clock),
            (10, TimingFault::Settings),
            (11, TimingFault::Native),
            (12, TimingFault::Native),
            (13, TimingFault::Deadline),
            (14, TimingFault::Missing),
            (15, TimingFault::Window),
            (16, TimingFault::Window),
            (18, TimingFault::Order),
            (19, TimingFault::Order),
        ] {
            let mut h = Harness::new_mode(200000, Some(mode));
            assert_eq!(
                h.start.summary().lifecycle_timing,
                Err(expected),
                "mode {mode}"
            );
            let bytes = h.start.canonical().to_owned();
            assert_eq!(
                h.check(true, true).unwrap_err().part,
                Part::Lifecycle,
                "mode {mode}"
            );
            assert_eq!(h.start.canonical(), bytes);
            assert!(!h.parent.native_admission_available());
        }
    }
    #[test]
    fn drain_after_run_origin_cannot_be_used_as_a_pre_run_context() {
        let mut h = Harness::new_mode(200000, Some(17));
        assert!(h.start.summary().lifecycle_timing.is_ok());
        assert_eq!(h.check(true, true).unwrap_err().part, Part::Lifecycle);
    }
    #[test]
    fn valid_reported_timeline_keeps_original_cleanup_window_and_final_drain() {
        let mut h = Harness::new(200000);
        assert_eq!(h.start.summary().lifecycle_timing, Ok(a(7200)));
        let before = h.b.used();
        assert!(h.check(true, true).is_ok());
        assert_eq!(h.b.used(), before);
    }
    #[test]
    fn missing_runtime_or_contradictory_versions_and_lifecycle_stay_distinct() {
        for mode in 0..3 {
            let mut h = Harness::new_mode(200000, Some(mode));
            let e = h.check(true, true).unwrap_err();
            assert_eq!(
                e.part,
                if mode == 2 {
                    Part::Lifecycle
                } else {
                    Part::Versions
                }
            );
            assert_eq!(
                e.cause,
                if mode == 0 {
                    Cause::Missing
                } else if mode == 1 {
                    Cause::Version
                } else {
                    Cause::Lifecycle
                }
            );
            assert!(h.start.canonical().len() > 1000);
        }
    }
    #[test]
    fn aliased_capture_scope_and_wrong_run_cannot_rebind_parent_context() {
        let mut h = Harness::new(200000);
        let mut draft = h.scope.draft().clone();
        draft.capture_nonce = [99; 16];
        let other = Scope::freeze(draft, &h.b).unwrap();
        let snap = collect(&other, conditions(), &mut Fake::default(), &h.b);
        h.host = claimed(&other, &snap, &h.b);
        drop(snap);
        assert_eq!(h.check(true, true).unwrap_err().part, Part::Scope);
        let mut h = Harness::new(200000);
        let run = h.m.bind_run(1, ORIGIN).unwrap();
        let e = h
            .parent
            .check_context(Inputs {
                run: &run,
                scope: &h.scope,
                host: Some(&h.host),
                starting: Some(&h.start),
                worker: &mut h.worker,
            })
            .unwrap_err();
        assert_eq!(e.part, Part::Binding);
    }
    #[test]
    fn reference_composition_uses_prepaid_storage_under_full_budget_pressure() {
        let mut h = Harness::new(200000);
        let _pressure = h.b.reserve(32 * 1024 * 1024 - h.b.used()).unwrap();
        let used = h.b.used();
        assert!(h.check(true, true).is_ok());
        assert_eq!(h.b.used(), used);
        assert!(!h.parent.native_admission_available());
    }
    #[test]
    fn successful_context_is_not_overwritten_by_a_second_check() {
        let mut h = Harness::new(200000);
        assert!(h.check(true, true).is_ok());
        let first = *h.parent.context_check().unwrap();
        assert_eq!(h.check(false, false).unwrap_err().part, Part::Duplicate);
        assert_eq!(h.parent.context_check(), Some(&first));
        assert_eq!(h.parent.fault(), Some(ParentFault::Protocol));
    }
    #[test]
    fn retained_runtime_info_is_raw_and_missing_abi_history_does_not_become_complete() {
        let h = Harness::new(200000);
        assert!(h.start.summary().native_abi_complete);
        assert!(h.start.summary().runtime_calls_match);
        assert_eq!(
            h.start.info().unwrap().runtime.cuda_driver_version,
            runtime().cuda_driver_version
        );
        drop(h);
        let mut h = Harness::new_mode(200000, Some(3));
        assert!(!h.start.summary().native_abi_complete);
        assert!(!h.start.summary().runtime_calls_match);
        assert_eq!(h.check(true, true).unwrap_err().part, Part::Lifecycle);
    }
    use super::*;
    use autotune_benchmark_worker::{
        evidence::{decode, Value},
        sampler::{
            child::NativeAdmission,
            framing::{Framer, ReadStep, Source},
            wire::{Lane, WireError},
        },
        workload::{
            nvml_trace::Ledger,
            parent_context::{Cause, Inputs, Part},
            sampler_runtime::Call,
            settings::Capture,
            settings_transport::Child,
            starting_bootstrap::Declaration,
            starting_control::Completion,
            starting_export::{Binding, ControlFacts, Facts, Response},
            starting_read::Receipt as StartReceipt,
            starting_runtime::Trace,
            transport::{Queue, RawReceived, SendFailure, Sender},
            wire::{Channel, Direction, Message, WorkerBinding},
            worker_setup::{Artifact as SetupArtifact, Facts as SetupFacts, Preparation},
            worker_setup_delivery::{Chunks, ReadyDelivery, Receiver, CHUNK_BYTES},
        },
    };
    use autotune_probe::{
        nvml::{SamplerCall, SamplerInfo, SamplerObserver, SamplerOutput, SamplerReturn},
        report::Runtime,
        runtime_file::{Kind, PathReturn},
    };
    mod worker_fixture {
        include!("setup_fixture.rs");
    }
    const ORIGIN: u64 = 1_000_000;
    struct Bytes<'a>(&'a [u8]);
    impl Source for Bytes<'_> {
        fn read(&mut self, out: &mut [u8]) -> ReadStep {
            let n = out.len().min(self.0.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            ReadStep {
                reported: n,
                error: None,
            }
        }
    }
    fn raw(bytes: &[u8], at: u64, b: &Budget) -> RawReceived {
        RawReceived::new(
            Lane::Ordinary,
            Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes)),
            Some(a(at)),
        )
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|n| format!("{n:02x}")).collect()
    }
    fn edit(v: &Value, path: &[&str], replacement: Value) -> Value {
        if path.is_empty() {
            return replacement;
        }
        if let Value::Array(values) = v {
            let index: usize = path[0].parse().unwrap();
            return Value::Array(
                values
                    .iter()
                    .enumerate()
                    .map(|(i, value)| {
                        if i == index {
                            edit(value, &path[1..], replacement.clone())
                        } else {
                            value.clone()
                        }
                    })
                    .collect(),
            );
        }
        let mut result = autotune_benchmark_worker::evidence::Object::new();
        for (key, v) in v.as_object().unwrap().iter() {
            result
                .insert(
                    key,
                    if key == path[0] {
                        edit(v, &path[1..], replacement.clone())
                    } else {
                        v.clone()
                    },
                )
                .unwrap();
        }
        Value::Object(result)
    }
    fn runtime() -> Runtime {
        Runtime {
            library_path: Some("C:\\Windows\\System32\\nvml.dll".into()),
            driver_version: Field::available("text", "fixture-driver"),
            nvml_version: Field::available("text", "fixture-nvml"),
            cuda_driver_version: Field::available("CUDA encoded integer", 12090),
            header_api_version: 13,
        }
    }
    fn text(s: &str) -> [u8; 256] {
        let mut v = [0; 256];
        v[..s.len()].copy_from_slice(s.as_bytes());
        v
    }
    fn start_response(s: &Scope, b: &Budget, mode: Option<u8>) -> StartReceipt {
        let draft = Draft {
            raw_uuid: s.draft().raw_uuid.clone(),
            raw_pci: s.draft().raw_pci.clone(),
            observations: SettingMetric::ALL.map(|kind| Observation {
                kind,
                before: Some(a(5100 + kind as u64 * 200)),
                after: Some(a(5150 + kind as u64 * 200)),
                raw: Some(Field::available(
                    kind.unit(),
                    if kind == SettingMetric::PowerLimit {
                        200000
                    } else {
                        0
                    },
                )),
            }),
            halt: None,
        };
        let snap = Starting::fixture(s, a(5000), &draft, b).unwrap();
        let bytes = snap
            .canonical()
            .replace("simulated_fixture", "native_getter_path");
        let start = ReportedSnapshot::parse(
            s,
            bytes.as_bytes(),
            Digest::of(Domain::RawArtifact, bytes.as_bytes()),
            b,
        )
        .unwrap();
        drop(snap);
        let decl = Declaration::new(s, a(2100), b).unwrap();
        let mut ledger =
            Ledger::new(s.draft().frequency, a(0), a(2200), 10_000_000_000, b).unwrap();
        let bind_dummy = WorkerBinding::new(
            &PilotManifest::freeze(pilot(s, d(70), d(71)), b)
                .unwrap()
                .bind_run(0, ORIGIN)
                .unwrap(),
            s.draft().frequency,
            a(0),
        )
        .unwrap();
        let f = worker_fixture::fixture(bind_dummy, d(7), 65536);
        let mut file = f.file;
        file.kind = Kind::Nvml;
        let path: Vec<_> = "C:\\Windows\\System32\\nvml.dll".encode_utf16().collect();
        file.requested = Some(path.clone());
        file.loaded = Some(path.clone());
        file.module_path = Some(PathReturn {
            length: path.len() as u32,
            error: None,
        });
        let gate = NativeAdmission::default();
        let mut n = 2275;
        {
            let mut observer = ledger.observer(&gate, || {
                n += 25;
                Some(a(n))
            });
            for check in f.checks {
                assert!(observer.file_check(check.stage));
            }
            let mut pci = [0; 32];
            pci[..s.draft().raw_pci.len()].copy_from_slice(s.draft().raw_pci.as_bytes());
            for (call, output) in [
                (SamplerCall::Initialize, SamplerOutput::None),
                (SamplerCall::Count, SamplerOutput::Count(1)),
                (SamplerCall::Handle(0), SamplerOutput::HandleIsNull(false)),
                (
                    SamplerCall::Uuid(0),
                    SamplerOutput::Text(text(&s.draft().raw_uuid)),
                ),
                (
                    SamplerCall::Pci(0),
                    SamplerOutput::Pci {
                        legacy: [0; 16],
                        numbers: [0, 1, 2, 0, 0],
                        bus_id: pci,
                    },
                ),
                (SamplerCall::CudaVersion, SamplerOutput::Version(12090)),
                (
                    SamplerCall::DriverVersion,
                    SamplerOutput::Text(text("fixture-driver")),
                ),
                (
                    SamplerCall::NvmlVersion,
                    SamplerOutput::Text(text("fixture-nvml")),
                ),
            ] {
                assert!(observer.before(call));
                assert!(observer.returned(call, SamplerReturn { status: 0, output }));
            }
        }
        let mut trace = Trace::new(s, a(2200), b).unwrap();
        trace.calls = std::array::from_fn(|i| Call {
            requested: true,
            before: Some(a([2300, 3030, 5000, 6000][i])),
            after: Some(a([3025, 3200, 5600, 6500][i])),
            attempted: true,
            returned: true,
        });
        trace.cleanup_status = Some(0);
        trace.output_drained = true;
        trace.control_finished = true;
        trace.stop_trigger = Some(a(5800));
        trace.drain_receipt = Some(a(7200));
        let info = SamplerInfo {
            raw_uuid: s.draft().raw_uuid.clone(),
            raw_pci: s.draft().raw_pci.clone(),
            runtime: runtime(),
        };
        let response = Response::build_runtime(
            Binding {
                scope: s,
                declaration: &decl,
                first_entry: a(2100),
                receipt: a(2200),
                deadline_ns: 10_000_000_000,
            },
            Facts {
                info: Some(&info),
                ledger: &ledger,
                snapshot: Some(start.snapshot()),
                snapshot_error: None,
                trace: &trace,
                control: ControlFacts {
                    completion: Some(Completion {
                        anchor: a(7000),
                        initialized_session: true,
                        cleanup_attempted: true,
                        cleanup_status: Some(0),
                    }),
                    completion_receipt: Some(a(7100)),
                    finished: Some(true),
                    observed_frequency: Some(s.draft().frequency),
                    trigger: Some(a(5800)),
                    window: Some(autotune_benchmark_worker::workload::gate::StopWindow {
                        trigger_ns: 5800,
                        cooperative_ns: 1_000_005_800,
                        exit_ns: 3_000_005_800,
                    }),
                    ..Default::default()
                },
                native_failure: None,
                clock_error: None,
                control_error: None,
                event_clone_error: None,
                unreported_completion: None,
            },
            autotune_benchmark_worker::workload::runtime_evidence::Input::Recorded {
                file: Some(&file),
                load_error: None,
                initialization: Some(0),
            },
            b,
        )
        .unwrap();
        let bytes = response.canonical().replace(
            "\"source\":\"simulated_fixture\"",
            "\"source\":\"native_file_path\"",
        );
        let bytes = if let Some(mode) = mode {
            let doc = decode(bytes.as_bytes(), 2 * 1024 * 1024).unwrap();
            let (path, value) = match mode {
                0 => (vec!["info"], Value::Null),
                1 => {
                    let mut info = runtime();
                    info.cuda_driver_version = Field::available("CUDA encoded integer", 12080);
                    (
                        vec!["info", "runtime"],
                        Value::Text(serde_json::to_string(&info).unwrap()),
                    )
                }
                2 => (vec!["event_clone_error"], Value::U64(5)),
                3 => (vec!["native_calls"], Value::Array(Vec::new())),
                4 => (vec!["trace", "drain_receipt"], Value::Null),
                5 => (vec!["control", "completion_receipt"], av(6900)),
                6 => (vec!["trace", "calls", "3", "before"], av(5700)),
                7 => (
                    vec!["control", "window", "cooperative_ns"],
                    Value::U64(1_000_005_801),
                ),
                8 => (vec!["trace", "drain_receipt"], av(1_000_005_800)),
                9 => (
                    vec!["trace", "drain_receipt", "unbiased_100ns"],
                    Value::U64(10_000_072),
                ),
                10 => (vec!["trace", "calls", "2", "after"], av(5400)),
                11 => (vec!["trace", "calls", "1", "before"], av(3060)),
                12 => (vec!["trace", "calls", "0", "before"], av(2400)),
                13 => (vec!["trace", "calls", "1", "after"], av(10_000_000_000)),
                14 => (vec!["control", "window"], Value::Null),
                15 => (vec!["control", "trigger"], av(5700)),
                16 => (
                    vec!["control", "window", "exit_ns"],
                    Value::U64(3_000_005_801),
                ),
                17 => (vec!["trace", "drain_receipt"], av(ORIGIN + 100)),
                18 => (vec!["trace", "calls", "1", "before"], av(3000)),
                19 => (vec!["trace", "calls", "3", "after"], av(7050)),
                _ => panic!("unknown timing fixture mode"),
            };
            edit(&doc, &path, value).to_canonical()
        } else {
            bytes
        };
        StartReceipt::read(
            bytes.as_bytes(),
            s,
            &decl,
            Digest::of(Domain::RawArtifact, bytes.as_bytes()),
            10_000_000_000,
            b,
        )
        .ok()
        .unwrap()
    }
    fn av(n: u64) -> Value {
        let anchor = a(n);
        let mut value = autotune_benchmark_worker::evidence::Object::new();
        for (key, field) in [
            ("qpc_before", Value::U64(anchor.qpc_before)),
            ("qpc_after", Value::U64(anchor.qpc_after)),
            ("interrupt_100ns", Value::U64(anchor.interrupt_100ns)),
            ("unbiased_100ns", Value::U64(anchor.unbiased_100ns)),
        ] {
            value.insert(key, field).unwrap();
        }
        Value::Object(value)
    }
    fn worker(bind: WorkerBinding, b: &Budget) -> ReadyDelivery {
        let mut f = worker_fixture::fixture(bind, d(7), 65536);
        let shift = |v: Anchor| a(v.qpc_before + ORIGIN);
        for e in &mut f.entries {
            e.before = e.before.map(shift);
            e.after = e.after.map(shift);
        }
        for c in &mut f.checks {
            c.anchor = c.anchor.map(shift);
        }
        if let Message::Ready { anchor, .. } = &mut f.ready {
            *anchor = shift(*anchor);
        }
        let artifact = SetupArtifact::recorded(
            bind,
            SetupFacts {
                first: a(ORIGIN + 1000),
                deadline_raw: (ORIGIN + 1_000_000_000) / 100,
                preparation: Preparation::Ready,
                cpu_fault: None,
                native_fault: None,
                entries: &f.entries,
                pending: false,
                fault: None,
                overflow: None,
                rejected: None,
                file_checks: &f.checks,
                file_overflow: None,
                file: Some(
                    autotune_benchmark_worker::workload::worker_setup::File::Recorded(&f.file),
                ),
                load_error: None,
                environment: f.environment,
                driver_version: Some(12090),
                module_loading_mode: Some(1),
            },
            b,
        )
        .unwrap();
        let bytes = artifact.canonical().replace(
            "\"source\":\"simulated_fixture\"",
            "\"source\":\"native_setup_path\"",
        );
        assert_eq!(bytes.len(), artifact.canonical().len());
        let digest = Digest::of(Domain::RawArtifact, bytes.as_bytes());
        let chunks = Chunks::new(&artifact, b).unwrap();
        let mut tx = Channel::new(bind, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
        let mut rx =
            Receiver::new(bind, a(ORIGIN), (ORIGIN + 1_000_000_000) / 100, b.clone()).unwrap();
        for i in 0..chunks.count() {
            let frame = tx.encode(&chunks.message(i).unwrap()).unwrap();
            let value = decode(&frame.as_bytes()[4..], 64 * 1024).unwrap();
            let begin = i as usize * CHUNK_BYTES;
            let end = (begin + CHUNK_BYTES).min(bytes.len());
            let value = edit(
                &value,
                &["body", "data"],
                Value::Text(hex(&bytes.as_bytes()[begin..end])),
            );
            let text =
                edit(&value, &["body", "digest"], Value::Text(digest.to_hex())).to_canonical();
            let mut frame = ((text.len() + 4) as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(text.as_bytes());
            rx.accept(raw(&frame, ORIGIN + 200_000 + i as u64 * 1000, b))
                .unwrap();
        }
        let frame = tx.encode(&f.ready).unwrap();
        rx.accept(raw(frame.as_bytes(), ORIGIN + 300_000, b))
            .unwrap();
        rx.into_ready().ok().unwrap()
    }
    struct Harness {
        b: Budget,
        scope: Scope,
        host: Reported,
        start: StartReceipt,
        m: PilotManifest,
        parent: ParentSampler,
        worker: ReadyDelivery,
    }
    impl Harness {
        fn new(current_power: i64) -> Self {
            Self::new_mode(current_power, None)
        }
        fn new_mode(current_power: i64, mode: Option<u8>) -> Self {
            let b = Budget::new(32 * 1024 * 1024).unwrap();
            let scope = scope(&b);
            let snap = collect(&scope, conditions(), &mut Fake::default(), &b);
            let host = claimed(&scope, &snap, &b);
            drop(snap);
            let start = start_response(&scope, &b, mode);
            let m = host
                .freeze_manifest(
                    start.snapshot().unwrap(),
                    pilot(
                        &scope,
                        host.snapshot().digest(),
                        start.snapshot().unwrap().snapshot().digest(),
                    ),
                    scope.draft().frequency,
                    a(0),
                    &b,
                )
                .unwrap();
            let run = m.bind_run(0, ORIGIN).unwrap();
            let bind = WorkerBinding::new(&run, scope.draft().frequency, a(0)).unwrap();
            let worker = worker(bind, &b);
            let mut parent = ParentSampler::new(
                &run,
                scope.draft().frequency,
                a(0),
                a(ORIGIN),
                a(ORIGIN + 100),
                b.clone(),
                b.partition(2 * 1024 * 1024).unwrap(),
            )
            .unwrap();
            let mut tx =
                Channel::sampler(bind, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
            let frame = tx
                .encode(&Message::SamplerReady {
                    before: a(ORIGIN + 200),
                    after: a(ORIGIN + 300),
                    raw_uuid: run.raw_uuid().into(),
                    raw_pci: run.raw_pci().into(),
                    sampler_digest: d(8),
                    runtime: serde_json::to_string(&runtime()).unwrap(),
                })
                .unwrap();
            assert!(parent
                .receive(&run, raw(frame.as_bytes(), ORIGIN + 400, &b))
                .is_ok());
            // The request queue is captured separately; the sampler's owned
            // strict response below is an adversarial native claim, never a read.
            struct Captured<'a>(&'a mut Vec<u8>);
            impl Queue for Captured<'_> {
                fn try_send(&mut self, _: Lane, v: &[u8]) -> Result<(), SendFailure> {
                    self.0.extend_from_slice(v);
                    Ok(())
                }
            }
            let mut request_bytes = Vec::new();
            let mut out =
                Sender::new(Captured(&mut request_bytes), bind, Direction::ToWorker, &b).unwrap();
            parent.issue_settings(a(ORIGIN + 1000), &mut out).unwrap();
            drop(out);
            let mut child = Child::new(&run, scope.draft().frequency, a(0), b.clone()).unwrap();
            let mut request = child.accept(&request_bytes).unwrap();
            let draft = Draft {
                raw_uuid: run.raw_uuid().into(),
                raw_pci: run.raw_pci().into(),
                observations: SettingMetric::ALL.map(|kind| Observation {
                    kind,
                    before: Some(a(ORIGIN + 1500 + kind as u64 * 200)),
                    after: Some(a(ORIGIN + 1600 + kind as u64 * 200)),
                    raw: Some(Field::available(
                        kind.unit(),
                        if kind == SettingMetric::PowerLimit {
                            current_power
                        } else {
                            0
                        },
                    )),
                }),
                halt: None,
            };
            let capture =
                Capture::fixture(&run, scope.draft().frequency, a(0), &draft, &b).unwrap();
            let frame = request.respond(&capture).unwrap();
            let canonical = capture
                .canonical()
                .replace("simulated_fixture", "native_getter_path");
            let digest = Digest::of(Domain::RawArtifact, canonical.as_bytes());
            let text = std::str::from_utf8(&frame.as_bytes()[4..])
                .unwrap()
                .replace(&capture.digest().to_hex(), &digest.to_hex())
                .replace("simulated_fixture", "native_getter_path");
            let mut bytes = ((text.len() + 4) as u32).to_le_bytes().to_vec();
            bytes.extend_from_slice(text.as_bytes());
            assert!(parent.receive(&run, raw(&bytes, ORIGIN + 3000, &b)).is_ok());
            Self {
                b,
                scope,
                host,
                start,
                m,
                parent,
                worker,
            }
        }
        fn check(
            &mut self,
            host: bool,
            start: bool,
        ) -> Result<
            autotune_benchmark_worker::workload::parent_context::Summary,
            autotune_benchmark_worker::workload::parent_context::Rejection,
        > {
            let run = self.m.bind_run(0, ORIGIN).unwrap();
            self.parent.check_context(Inputs {
                run: &run,
                scope: &self.scope,
                host: host.then_some(&self.host),
                starting: start.then_some(&self.start),
                worker: &mut self.worker,
            })
        }
    }
    #[test]
    fn complete_claimed_context_checks_all_references_without_issuing_native_authority() {
        let mut h = Harness::new(200000);
        let result = h.check(true, true).unwrap();
        assert_eq!(result.current.power_limit_mw, 200000);
        assert_eq!(result.cuda_api, 12090);
        assert_eq!(result.references.starting_response, Some(h.start.digest()));
        assert!(h
            .parent
            .context_check()
            .unwrap()
            .checks
            .iter()
            .all(|c| c.result.is_ok()));
        assert!(!h.parent.native_admission_available());
    }
    #[test]
    fn missing_parts_retain_all_results_and_cannot_be_filled_by_retry() {
        let mut h = Harness::new(200000);
        assert!(h.check(false, false).is_err());
        let first = *h.parent.context_check().unwrap();
        assert_eq!(first.references.host, None);
        assert_eq!(first.references.starting_response, None);
        assert!(first
            .checks
            .iter()
            .any(|c| c.part == Part::Host && c.result == Err(Cause::Missing)));
        assert_eq!(h.check(true, true).unwrap_err().part, Part::Duplicate);
        assert_eq!(h.parent.context_check(), Some(&first));
        assert_eq!(h.parent.fault(), Some(ParentFault::Context));
    }
    #[test]
    fn changed_reported_settings_reject_and_preserve_the_exact_context_receipts() {
        let mut h = Harness::new(215000);
        let before = h.start.canonical().to_owned();
        let result = h.check(true, true).unwrap_err();
        assert_eq!(result.part, Part::Settings);
        assert_eq!(result.cause, Cause::Changed);
        assert_eq!(h.start.canonical(), before);
        assert_eq!(h.parent.fault(), Some(ParentFault::Context));
        let frame = Channel::new(
            WorkerBinding::new(
                &h.m.bind_run(0, ORIGIN).unwrap(),
                h.scope.draft().frequency,
                a(0),
            )
            .unwrap(),
            Lane::Ordinary,
            Direction::FromWorker,
            h.b.clone(),
        )
        .unwrap()
        .encode(&Message::PhaseEntered {
            phase: autotune_benchmark_worker::workload::dispatch::Phase::Warmup,
            anchor: a(ORIGIN + 400_000),
        })
        .unwrap();
        assert!(matches!(
            h.worker.decoder().decode(frame.as_bytes()),
            Err(WireError::Poisoned)
        ));
    }
}
