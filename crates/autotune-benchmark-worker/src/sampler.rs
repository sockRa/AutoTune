//! Pure selective-getter/monitor conversion. No getter, clock or process runs.
//!
//! Retain the returned raw field on both success and error. A caller must close
//! admission on conversion error, e.g. with `Controller::request_stop`, rather
//! than discard the error and wait for a later successful reading. Conversion
//! does not establish live provenance, sensor age, valid timing or admission.

use crate::monitor::{Metric, Record, Status, Ticket, Unit};
use autotune_probe::{
    nvml::SampleMetric,
    report::{Field, Reading, Reason, SymbolPresence},
};

pub mod archive;
pub mod budget;
pub mod child;
#[cfg(windows)]
pub mod child_windows;
#[cfg(windows)]
pub mod controller_windows;
pub mod drain;
pub mod framing;
pub mod ingress;
pub mod manifest;
pub mod producer;
pub mod runtime;
pub mod supervisor;
#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
mod windows_io;
pub mod wire;
mod wire_model;
pub mod writer;

/// Select exactly one existing getter kind without invoking it.
pub fn sample_metric(metric: Metric) -> SampleMetric {
    match metric {
        Metric::Temperature => SampleMetric::Temperature,
        Metric::Power => SampleMetric::Power,
        Metric::GraphicsClock => SampleMetric::GraphicsClock,
        Metric::MemoryClock => SampleMetric::MemoryClock,
        Metric::GpuUtilization => SampleMetric::GpuUtilization,
        Metric::MemoryUtilization => SampleMetric::MemoryUtilization,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingError {
    Unit,
    Symbol,
    Value,
    DeviceLost,
}

/// Original owned evidence beside its conversion; neither implies acceptance.
#[derive(Debug)]
#[must_use = "retain the raw field and stop admission on conversion error"]
pub struct MappedSample {
    pub raw: Field,
    pub record: Result<Record, MappingError>,
}

/// Map exact units/status/integers, preserving all raw evidence on every branch.
///
/// Times are supplied run-relative assertions, not observed clocks. The ticket's
/// controller issue time is not a physical getter-start timestamp. The unchanged
/// controller validates ticket, ordering, conservative bracket, transport age,
/// physical ranges and freshness. No timestamp or value is repaired here.
pub fn map_field(ticket: Ticket, end_ns: u64, receipt_ns: u64, raw: Field) -> MappedSample {
    let record = convert(ticket, end_ns, receipt_ns, &raw);
    MappedSample { raw, record }
}

fn convert(
    ticket: Ticket,
    end_ns: u64,
    receipt_ns: u64,
    raw: &Field,
) -> Result<Record, MappingError> {
    if let Reading::Unavailable { failure } = &raw.reading {
        if failure.reason == Reason::GpuLost || failure.vendor_status == Some(15) {
            return Err(MappingError::DeviceLost);
        }
    }

    let unit = ticket.metric.unit();
    let expected_unit = match unit {
        Unit::DegC => "degC",
        Unit::Milliwatt => "mW",
        Unit::MHz => "MHz",
        Unit::Percent => "%",
    };
    if raw.unit != expected_unit {
        return Err(MappingError::Unit);
    }

    let (status, value) = match &raw.reading {
        Reading::Available { value } => {
            if raw.symbol_presence != SymbolPresence::Present {
                return Err(MappingError::Symbol);
            }
            let value = value.as_i64().ok_or(MappingError::Value)?;
            (Status::Available, Some(value))
        }
        Reading::Unavailable { failure } => {
            let expected_symbol = match failure.reason {
                Reason::RuntimeMissing
                | Reason::RuntimeLoadFailed
                | Reason::UnsupportedPlatform => SymbolPresence::NotChecked,
                Reason::SymbolMissing => SymbolPresence::Absent,
                Reason::Unsupported
                | Reason::PermissionDenied
                | Reason::QueryFailed
                | Reason::GpuLost
                | Reason::InvalidValue => SymbolPresence::Present,
            };
            if raw.symbol_presence != expected_symbol {
                return Err(MappingError::Symbol);
            }
            let status = match failure.reason {
                Reason::Unsupported | Reason::SymbolMissing | Reason::UnsupportedPlatform => {
                    Status::Unsupported
                }
                _ => failure
                    .vendor_status
                    .map(Status::Failed)
                    .unwrap_or(Status::Unavailable),
            };
            (status, None)
        }
    };

    Ok(Record {
        ticket,
        end_ns,
        receipt_ns,
        unit,
        status,
        value,
    })
}
