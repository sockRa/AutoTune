//! Pure synthetic telemetry recording. No clocks, hardware, workloads or I/O.
pub mod energy;
pub mod lifecycle;
pub mod metadata;
pub mod repeat;
pub mod summary;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Identity {
    run_id: String,
    workload_id: String,
    workload_version: String,
    workload_seed: u64,
    gpu_uuid: String,
    pci_bus_id: String,
}

impl Identity {
    pub fn new(
        run_id: &str,
        workload_id: &str,
        workload_version: &str,
        workload_seed: u64,
        gpu_uuid: &str,
        pci_bus_id: &str,
    ) -> Result<Self, Error> {
        for (field, value) in [
            ("run_id", run_id),
            ("workload_id", workload_id),
            ("workload_version", workload_version),
            ("gpu_uuid", gpu_uuid),
            ("pci_bus_id", pci_bus_id),
        ] {
            if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err(Error::InvalidIdentity { field });
            }
        }
        Ok(Self {
            run_id: run_id.into(),
            workload_id: workload_id.into(),
            workload_version: workload_version.into(),
            workload_seed,
            gpu_uuid: gpu_uuid.into(),
            pci_bus_id: pci_bus_id.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Policy {
    sample_capacity: usize,
    maximum_age_ns: u64,
}

impl Policy {
    pub fn new(sample_capacity: usize, maximum_age_ns: u64) -> Result<Self, Error> {
        if sample_capacity == 0 {
            return Err(Error::ZeroCapacity);
        }
        if maximum_age_ns == 0 {
            return Err(Error::ZeroMaximumAge);
        }
        Ok(Self {
            sample_capacity,
            maximum_age_ns,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Temperature,
    GraphicsClock,
    MemoryClock,
    GpuUtilization,
    MemoryUtilization,
    Power,
}

impl Metric {
    pub fn unit(self) -> Unit {
        match self {
            Self::Temperature => Unit::DegreesCelsius,
            Self::GraphicsClock | Self::MemoryClock => Unit::Megahertz,
            Self::GpuUtilization | Self::MemoryUtilization => Unit::Percent,
            Self::Power => Unit::Milliwatts,
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn accepts(self, value: i64) -> bool {
        match self {
            Self::Temperature => true,
            Self::GpuUtilization | Self::MemoryUtilization => (0..=100).contains(&value),
            _ => value >= 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Unit {
    #[serde(rename = "degC")]
    DegreesCelsius,
    #[serde(rename = "MHz")]
    Megahertz,
    #[serde(rename = "%")]
    Percent,
    #[serde(rename = "mW")]
    Milliwatts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    Unsupported,
    PermissionDenied,
    QueryFailed,
    DeviceLost,
    NotSampled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Reading {
    Available { value: i64 },
    Unavailable { reason: UnavailableReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Sample {
    pub metric: Metric,
    pub unit: Unit,
    pub reading: Reading,
    pub observation_ns: u64,
    pub receipt_ns: u64,
    pub freshness: Freshness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidIdentity { field: &'static str },
    ZeroCapacity,
    ZeroMaximumAge,
    BindingMismatch,
    InvalidValue { metric: Metric },
    ObservationAfterReceipt,
    ReceiptRegression,
    ObservationNotIncreasing { metric: Metric },
    CapacityReached,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Simulated,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBasis {
    RunRelativeMonotonicNanoseconds,
}

/// Identity and policy are fixed at construction; only accepted samples affect ordering.
#[derive(Debug, Serialize)]
pub struct Recorder {
    schema_version: u32,
    provenance: Provenance,
    time_basis: TimeBasis,
    identity: Identity,
    policy: Policy,
    had_rejections: bool,
    samples: Vec<Sample>,
    #[serde(skip)]
    last_observations: [Option<u64>; 6],
    #[serde(skip)]
    last_receipt: Option<u64>,
}

impl Recorder {
    pub fn new(identity: Identity, policy: Policy) -> Self {
        Self {
            schema_version: 1,
            provenance: Provenance::Simulated,
            time_basis: TimeBasis::RunRelativeMonotonicNanoseconds,
            identity,
            policy,
            had_rejections: false,
            samples: Vec::new(),
            last_observations: [None; 6],
            last_receipt: None,
        }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn had_rejections(&self) -> bool {
        self.had_rejections
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn append(
        &mut self,
        binding: &Identity,
        metric: Metric,
        reading: Reading,
        observation_ns: u64,
        receipt_ns: u64,
    ) -> Result<(), Error> {
        let result = self.try_append(binding, metric, reading, observation_ns, receipt_ns);
        if result.is_err() {
            self.had_rejections = true;
        }
        result
    }

    fn try_append(
        &mut self,
        binding: &Identity,
        metric: Metric,
        reading: Reading,
        observation_ns: u64,
        receipt_ns: u64,
    ) -> Result<(), Error> {
        if binding != &self.identity {
            return Err(Error::BindingMismatch);
        }
        if self.samples.len() >= self.policy.sample_capacity {
            return Err(Error::CapacityReached);
        }
        if let Reading::Available { value } = reading {
            if !metric.accepts(value) {
                return Err(Error::InvalidValue { metric });
            }
        }
        let age = receipt_ns
            .checked_sub(observation_ns)
            .ok_or(Error::ObservationAfterReceipt)?;
        if self.last_receipt.is_some_and(|last| receipt_ns < last) {
            return Err(Error::ReceiptRegression);
        }
        if self.last_observations[metric.index()].is_some_and(|last| observation_ns <= last) {
            return Err(Error::ObservationNotIncreasing { metric });
        }
        self.samples.push(Sample {
            metric,
            unit: metric.unit(),
            reading,
            observation_ns,
            receipt_ns,
            freshness: if age <= self.policy.maximum_age_ns {
                Freshness::Fresh
            } else {
                Freshness::Stale
            },
        });
        self.last_observations[metric.index()] = Some(observation_ns);
        self.last_receipt = Some(receipt_ns);
        Ok(())
    }
}
