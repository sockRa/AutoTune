use autotune_probe::{mock::SCENARIOS, report::*, run_mock};
use serde_json::Value;
use std::process::Command;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_autotune-probe"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn every_matrix_scenario_is_deterministic_and_cli_matches_schema() {
    for scenario in SCENARIOS {
        let first = cli(&["--mock", scenario]);
        let second = cli(&["--mock", scenario]);
        assert_eq!(first.stdout, second.stdout, "{scenario}");
        let expected_exit = match *scenario {
            "supported" | "zero-gpus" | "missing-api" | "unsupported" | "denied" | "multi-gpu" => 0,
            _ => 2,
        };
        assert_eq!(first.status.code(), Some(expected_exit), "{scenario}");
        let report: Report = serde_json::from_slice(&first.stdout).unwrap();
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.provenance, Provenance::Simulated);
        assert_eq!(report.collected_at_utc, autotune_probe::mock::TIMESTAMP);
        let serialized = serde_json::to_value(&report).unwrap();
        assert_eq!(
            serialized,
            serde_json::from_slice::<Value>(&first.stdout).unwrap()
        );
        for device in &report.devices {
            for control in device.controls.values() {
                assert_eq!(control.write_verification, WriteVerification::NotTested);
            }
            for field in device
                .identity
                .values()
                .chain(device.telemetry.values())
                .chain(device.controls.values().flat_map(|c| c.getters.values()))
            {
                assert!(!field.unit.is_empty());
            }
        }
    }
}

#[test]
fn supported_queries_retain_readings_units_and_untested_writes() {
    let report = run_mock("supported").unwrap();
    assert_eq!(report.status, CollectionStatus::Complete);
    let device = &report.devices[0];
    let temperature = &device.telemetry["temperature"];
    assert_eq!(temperature.unit, "degC");
    assert!(
        matches!(&temperature.reading, Reading::Available { value } if value == &Value::from(42))
    );
    assert_eq!(
        device.controls["power_limit"].setter_symbol_presence,
        SymbolPresence::Present
    );
    assert_eq!(
        device.controls["power_limit"].write_verification,
        WriteVerification::NotTested
    );
    for key in ["graphics_clock_offset_p0", "memory_clock_offset_p0"] {
        assert_eq!(device.controls[key].getters["current"].unit, "MHz");
        assert!(matches!(
            device.controls[key].getters["current"].reading,
            Reading::Available { .. }
        ));
        assert_eq!(
            device.controls[key].write_verification,
            WriteVerification::NotTested
        );
    }
}

#[test]
fn absent_runtime_is_not_successful_zero_device_enumeration() {
    let absent = run_mock("no-runtime").unwrap();
    assert!(absent.runtime.is_none());
    assert!(matches!(
        absent.enumeration,
        Reading::Unavailable {
            failure: Failure {
                reason: Reason::RuntimeMissing,
                ..
            }
        }
    ));
    let empty = run_mock("zero-gpus").unwrap();
    assert!(empty.runtime.is_some());
    assert!(matches!(&empty.enumeration, Reading::Available { value } if value == &Value::from(0)));
    assert!(empty.devices.is_empty());
    assert_eq!(empty.exit_code(), 0);
}

#[test]
fn failures_do_not_turn_into_zero_measurements_or_support_verdicts() {
    for (scenario, reason, code, symbol) in [
        (
            "missing-api",
            Reason::SymbolMissing,
            None,
            SymbolPresence::Absent,
        ),
        (
            "unsupported",
            Reason::Unsupported,
            Some(3),
            SymbolPresence::Present,
        ),
        (
            "denied",
            Reason::PermissionDenied,
            Some(4),
            SymbolPresence::Present,
        ),
        (
            "query-failure",
            Reason::QueryFailed,
            Some(999),
            SymbolPresence::Present,
        ),
        (
            "lost-gpu",
            Reason::GpuLost,
            Some(15),
            SymbolPresence::Present,
        ),
        (
            "invalid-value",
            Reason::InvalidValue,
            None,
            SymbolPresence::Present,
        ),
    ] {
        let report = run_mock(scenario).unwrap();
        let control = &report.devices[0].controls["power_limit"];
        let field = &control.getters["current"];
        assert_eq!(field.symbol_presence, symbol);
        assert_eq!(field.unit, "mW");
        match &field.reading {
            Reading::Unavailable { failure } => {
                assert_eq!(failure.reason, reason);
                assert_eq!(failure.vendor_status, code);
            }
            _ => panic!("{scenario} emitted a reading"),
        }
        assert!(matches!(
            report.devices[0].telemetry["temperature"].reading,
            Reading::Available { .. }
        ));
        assert_eq!(control.write_verification, WriteVerification::NotTested);
        let serialized = serde_json::to_value(control).unwrap();
        assert!(serialized.get("supported").is_none());
        assert!(serialized.get("writable").is_none());
    }
}

#[test]
fn multiple_devices_have_separate_uuid_and_pci_identity() {
    let report = serde_json::to_value(run_mock("multi-gpu").unwrap()).unwrap();
    let devices = report["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2);
    for key in ["uuid", "pci_bus_id"] {
        assert_ne!(
            devices[0]["identity"][key]["reading"]["value"],
            devices[1]["identity"][key]["reading"]["value"]
        );
    }
}

#[test]
fn unsupported_platform_is_structured_and_mock_remains_available() {
    let report = run_mock("unsupported-platform").unwrap();
    assert_eq!(report.diagnostics[0].reason, Reason::UnsupportedPlatform);
    assert_eq!(report.exit_code(), 2);
    #[cfg(not(windows))]
    {
        let live = autotune_probe::run_live();
        assert_eq!(live.provenance, Provenance::Live);
        assert_eq!(live.diagnostics[0].reason, Reason::UnsupportedPlatform);
        assert_eq!(live.exit_code(), 2);
    }
    assert_eq!(run_mock("supported").unwrap().exit_code(), 0);
}

#[test]
fn initialization_enumeration_and_handle_failure_preserve_stage() {
    let init = run_mock("init-failure").unwrap();
    assert!(init.runtime.is_none());
    let enumeration = run_mock("enumeration-failure").unwrap();
    assert!(enumeration.runtime.is_some());
    assert!(enumeration.devices.is_empty());
    let handle = run_mock("handle-failure").unwrap();
    assert_eq!(handle.status, CollectionStatus::Incomplete);
    assert_eq!(
        handle.devices[0].failure.as_ref().unwrap().reason,
        Reason::GpuLost
    );
    assert!(handle.devices[0].telemetry.is_empty());
}

#[test]
fn usage_help_and_default_mock_exit_behavior() {
    let report: Report = serde_json::from_slice(&cli(&["--mock"]).stdout).unwrap();
    assert_eq!(report.provenance, Provenance::Simulated);
    assert_eq!(cli(&["--help"]).status.code(), Some(0));
    for args in [
        &["--bad"][..],
        &["--mock", "bogus"],
        &["--help", "extra"],
        &["--mock", "supported", "extra"],
    ] {
        let output = cli(args);
        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["reason"],
            "usage_error"
        );
    }
}

#[test]
fn production_mutation_boundary_is_an_explicit_opaque_allowlist() {
    let source = include_str!("../src/nvml.rs");
    let mut symbols = vec![];
    for line in source.lines() {
        if line.contains("b\"nvmlDeviceSet") {
            symbols.push(line);
            assert!(line.contains("setter_presence(") || line.contains(".as_slice()"));
            assert!(!line.contains("get::<"));
        }
    }
    assert_eq!(symbols.len(), 4);
    assert!(!source.contains("fn nvmlDeviceSet"));
    assert!(!source.contains("nvmlDeviceReset"));
    assert!(!source.contains("Library::new("));
    for source in [
        include_str!("../src/main.rs"),
        include_str!("../src/lib.rs"),
        include_str!("../src/adapter.rs"),
        include_str!("../src/nvml.rs"),
    ] {
        assert!(!source.contains("Command::"));
        assert!(!source.contains("ShellExecute"));
        assert!(!source.contains("mem::transmute"));
    }
    let manifest = include_str!("../Cargo.toml");
    assert!(!manifest.contains("nvml-wrapper"));
}

#[test]
fn collection_bounds_counts_and_continues_after_a_failed_device() {
    use autotune_probe::adapter::{collect, ReadOnlyAdapter};
    struct Script {
        count: u32,
    }
    impl ReadOnlyAdapter for Script {
        fn provenance(&self) -> Provenance {
            Provenance::Simulated
        }
        fn initialize(&mut self) -> Result<Runtime, Failure> {
            use autotune_probe::mock::MockAdapter;
            MockAdapter::new("supported").unwrap().initialize()
        }
        fn device_count(&self) -> Result<u32, Failure> {
            Ok(self.count)
        }
        fn device(&self, index: u32) -> Result<Device, Failure> {
            if index == 0 {
                Err(Failure::vendor(15, "lost first adapter"))
            } else {
                Ok(autotune_probe::mock::fixture_device(index))
            }
        }
    }
    let report = collect(
        &mut Script { count: 2 },
        autotune_probe::mock::TIMESTAMP.into(),
    );
    assert_eq!(report.status, CollectionStatus::Incomplete);
    assert_eq!(report.devices.len(), 2);
    assert!(report.devices[0].failure.is_some());
    assert_eq!(report.devices[1].index, 1);
    assert!(!report.devices[1].identity.is_empty());
    let invalid = collect(
        &mut Script { count: u32::MAX },
        autotune_probe::mock::TIMESTAMP.into(),
    );
    assert_eq!(invalid.status, CollectionStatus::Failed);
    assert_eq!(invalid.diagnostics[0].reason, Reason::InvalidValue);
    assert!(invalid.devices.is_empty());
}

#[test]
fn unavailable_runtime_versions_make_successful_enumeration_incomplete() {
    use autotune_probe::adapter::{collect, ReadOnlyAdapter};
    struct VersionFailure(&'static str);
    impl ReadOnlyAdapter for VersionFailure {
        fn provenance(&self) -> Provenance {
            Provenance::Simulated
        }
        fn initialize(&mut self) -> Result<Runtime, Failure> {
            let mut runtime = autotune_probe::mock::MockAdapter::new("supported")
                .unwrap()
                .initialize()?;
            let field = match self.0 {
                "driver" => &mut runtime.driver_version,
                "nvml" => &mut runtime.nvml_version,
                "cuda" => &mut runtime.cuda_driver_version,
                _ => unreachable!(),
            };
            *field = Field::unavailable(&field.unit, Failure::vendor(999, self.0));
            Ok(runtime)
        }
        fn device_count(&self) -> Result<u32, Failure> {
            Ok(1)
        }
        fn device(&self, index: u32) -> Result<Device, Failure> {
            Ok(autotune_probe::mock::fixture_device(index))
        }
    }
    for version in ["driver", "nvml", "cuda"] {
        let report = collect(
            &mut VersionFailure(version),
            autotune_probe::mock::TIMESTAMP.into(),
        );
        assert_eq!(report.status, CollectionStatus::Incomplete, "{version}");
        assert_eq!(report.exit_code(), 2, "{version}");
        assert!(
            matches!(&report.enumeration, Reading::Available { value } if value == &Value::from(1))
        );
        let runtime = report.runtime.as_ref().unwrap();
        for (name, field) in [
            ("driver", &runtime.driver_version),
            ("nvml", &runtime.nvml_version),
            ("cuda", &runtime.cuda_driver_version),
        ] {
            match &field.reading {
                Reading::Unavailable { failure } => {
                    assert_eq!(name, version);
                    assert_eq!(failure.vendor_status, Some(999));
                }
                Reading::Available { .. } => assert_ne!(name, version),
            }
        }
        assert_eq!(report.devices.len(), 1);
        assert!(!report.devices[0].incomplete());
        assert!(
            matches!(&report.devices[0].telemetry["temperature"].reading, Reading::Available { value } if value == &Value::from(42))
        );
    }
}
