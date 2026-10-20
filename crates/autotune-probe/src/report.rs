use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    RuntimeMissing,
    RuntimeLoadFailed,
    SymbolMissing,
    Unsupported,
    PermissionDenied,
    QueryFailed,
    GpuLost,
    InvalidValue,
    UnsupportedPlatform,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub reason: Reason,
    pub vendor_status: Option<i32>,
    pub detail: String,
}

impl Failure {
    pub fn new(reason: Reason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            vendor_status: None,
            detail: detail.into(),
        }
    }
    pub fn vendor(status: i32, operation: &str) -> Self {
        Self {
            reason: match status {
                3 => Reason::Unsupported,
                4 => Reason::PermissionDenied,
                15 => Reason::GpuLost,
                _ => Reason::QueryFailed,
            },
            vendor_status: Some(status),
            detail: operation.into(),
        }
    }
    pub fn incomplete(&self) -> bool {
        matches!(
            self.reason,
            Reason::QueryFailed | Reason::GpuLost | Reason::InvalidValue
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolPresence {
    Present,
    Absent,
    NotChecked,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Reading {
    Available { value: Value },
    Unavailable { failure: Failure },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    pub unit: String,
    pub symbol_presence: SymbolPresence,
    pub reading: Reading,
}

impl Field {
    pub fn available(unit: &str, value: impl Into<Value>) -> Self {
        Self {
            unit: unit.into(),
            symbol_presence: SymbolPresence::Present,
            reading: Reading::Available {
                value: value.into(),
            },
        }
    }
    pub fn unavailable(unit: &str, failure: Failure) -> Self {
        let symbol_presence = match failure.reason {
            Reason::SymbolMissing => SymbolPresence::Absent,
            Reason::RuntimeMissing | Reason::RuntimeLoadFailed | Reason::UnsupportedPlatform => {
                SymbolPresence::NotChecked
            }
            _ => SymbolPresence::Present,
        };
        Self {
            unit: unit.into(),
            symbol_presence,
            reading: Reading::Unavailable { failure },
        }
    }
    pub fn incomplete(&self) -> bool {
        matches!(&self.reading, Reading::Unavailable { failure } if failure.incomplete())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteVerification {
    NotTested,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Control {
    pub setter_symbol_presence: SymbolPresence,
    pub write_verification: WriteVerification,
    pub getters: BTreeMap<String, Field>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub index: u32,
    pub failure: Option<Failure>,
    pub identity: BTreeMap<String, Field>,
    pub telemetry: BTreeMap<String, Field>,
    pub controls: BTreeMap<String, Control>,
}

impl Device {
    pub fn incomplete(&self) -> bool {
        self.failure.is_some()
            || self
                .identity
                .values()
                .chain(self.telemetry.values())
                .any(Field::incomplete)
            || self
                .controls
                .values()
                .flat_map(|c| c.getters.values())
                .any(Field::incomplete)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Live,
    Simulated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionStatus {
    Complete,
    Incomplete,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Runtime {
    pub library_path: Option<String>,
    pub driver_version: Field,
    pub nvml_version: Field,
    pub cuda_driver_version: Field,
    pub header_api_version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub provenance: Provenance,
    pub collected_at_utc: String,
    pub status: CollectionStatus,
    pub runtime: Option<Runtime>,
    pub enumeration: Reading,
    pub devices: Vec<Device>,
    pub diagnostics: Vec<Failure>,
}

impl Report {
    pub fn exit_code(&self) -> u8 {
        if self.status == CollectionStatus::Complete {
            0
        } else {
            2
        }
    }
}
