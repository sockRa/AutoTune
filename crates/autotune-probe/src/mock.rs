use crate::{adapter::ReadOnlyAdapter, report::*};
use std::collections::BTreeMap;

pub const TIMESTAMP: &str = "2026-10-15T00:00:00Z";
pub const SCENARIOS: &[&str] = &[
    "supported",
    "no-runtime",
    "zero-gpus",
    "missing-api",
    "unsupported",
    "denied",
    "multi-gpu",
    "query-failure",
    "lost-gpu",
    "invalid-value",
    "unsupported-platform",
    "init-failure",
    "enumeration-failure",
    "handle-failure",
];

pub struct MockAdapter {
    scenario: String,
}
impl MockAdapter {
    pub fn new(scenario: &str) -> Option<Self> {
        SCENARIOS.contains(&scenario).then(|| Self {
            scenario: scenario.into(),
        })
    }
}

fn fields(entries: &[(&str, &str, i64)]) -> BTreeMap<String, Field> {
    entries
        .iter()
        .map(|(name, unit, value)| ((*name).into(), Field::available(unit, *value)))
        .collect()
}

pub fn fixture_device(index: u32) -> Device {
    let mut controls = BTreeMap::new();
    for (name, unit, current, min, max, default) in [
        ("power_limit", "mW", 215000, 125000, 250000, Some(215000)),
        ("graphics_clock_offset", "MHz", 0, -200, 1000, None),
        ("memory_clock_offset", "MHz", 0, -500, 2000, None),
        ("graphics_clock_offset_p0", "MHz", 0, -200, 1000, None),
        ("memory_clock_offset_p0", "MHz", 0, -500, 2000, None),
    ] {
        let mut getters = fields(&[
            ("current", unit, current),
            ("minimum", unit, min),
            ("maximum", unit, max),
        ]);
        if let Some(value) = default {
            getters.insert("default".into(), Field::available(unit, value));
        }
        controls.insert(
            name.into(),
            Control {
                setter_symbol_presence: SymbolPresence::Present,
                write_verification: WriteVerification::NotTested,
                getters,
            },
        );
    }
    Device {
        index,
        failure: None,
        identity: [
            (
                "name".into(),
                Field::available("text", "Simulated NVIDIA GPU"),
            ),
            (
                "uuid".into(),
                Field::available("text", format!("GPU-SIMULATED-{index}")),
            ),
            (
                "pci_bus_id".into(),
                Field::available("text", format!("00000000:{:02x}:00.0", index + 1)),
            ),
        ]
        .into(),
        telemetry: fields(&[
            ("temperature", "degC", 42),
            ("graphics_clock", "MHz", 1605),
            ("memory_clock", "MHz", 7000),
            ("gpu_utilization", "%", 10),
            ("memory_utilization", "%", 5),
            ("power", "mW", 65000),
        ]),
        controls,
    }
}

impl ReadOnlyAdapter for MockAdapter {
    fn provenance(&self) -> Provenance {
        Provenance::Simulated
    }
    fn initialize(&mut self) -> Result<Runtime, Failure> {
        match self.scenario.as_str() {
            "no-runtime" => {
                return Err(Failure::new(
                    Reason::RuntimeMissing,
                    "simulated missing runtime",
                ))
            }
            "unsupported-platform" => {
                return Err(Failure::new(
                    Reason::UnsupportedPlatform,
                    "simulated non-Windows platform",
                ))
            }
            "init-failure" => return Err(Failure::vendor(9, "nvmlInit_v2")),
            _ => (),
        }
        Ok(Runtime {
            library_path: None,
            driver_version: Field::available("text", "SIMULATED"),
            nvml_version: Field::available("text", "SIMULATED"),
            cuda_driver_version: Field::available("CUDA encoded integer", 12000),
            header_api_version: 13,
        })
    }
    fn device_count(&self) -> Result<u32, Failure> {
        match self.scenario.as_str() {
            "zero-gpus" => Ok(0),
            "multi-gpu" => Ok(2),
            "enumeration-failure" => Err(Failure::vendor(999, "nvmlDeviceGetCount_v2")),
            _ => Ok(1),
        }
    }
    fn device(&self, index: u32) -> Result<Device, Failure> {
        if self.scenario == "handle-failure" {
            return Err(Failure::vendor(15, "nvmlDeviceGetHandleByIndex_v2"));
        }
        let mut device = fixture_device(index);
        let failure = match self.scenario.as_str() {
            "missing-api" => Some(Failure::new(
                Reason::SymbolMissing,
                "nvmlDeviceGetPowerManagementLimit",
            )),
            "unsupported" => Some(Failure::vendor(3, "nvmlDeviceGetPowerManagementLimit")),
            "denied" => Some(Failure::vendor(4, "nvmlDeviceGetPowerManagementLimit")),
            "query-failure" => Some(Failure::vendor(999, "nvmlDeviceGetPowerManagementLimit")),
            "lost-gpu" => Some(Failure::vendor(15, "nvmlDeviceGetPowerManagementLimit")),
            "invalid-value" => Some(Failure::new(Reason::InvalidValue, "invalid power limit")),
            _ => None,
        };
        if let Some(failure) = failure {
            device
                .controls
                .get_mut("power_limit")
                .unwrap()
                .getters
                .insert("current".into(), Field::unavailable("mW", failure));
        }
        Ok(device)
    }
}
