use crate::report::*;

/// Only lifecycle and getter operations. No mutation capability is exposed.
pub trait ReadOnlyAdapter {
    fn provenance(&self) -> Provenance;
    fn initialize(&mut self) -> Result<Runtime, Failure>;
    fn device_count(&self) -> Result<u32, Failure>;
    fn device(&self, index: u32) -> Result<Device, Failure>;
}

pub fn collect(adapter: &mut impl ReadOnlyAdapter, timestamp: String) -> Report {
    let mut report = Report {
        schema_version: 1,
        provenance: adapter.provenance(),
        collected_at_utc: timestamp,
        status: CollectionStatus::Failed,
        runtime: None,
        enumeration: Reading::Unavailable {
            failure: Failure::new(Reason::QueryFailed, "enumeration not attempted"),
        },
        devices: vec![],
        diagnostics: vec![],
    };
    let result = adapter.initialize().and_then(|runtime| {
        report.runtime = Some(runtime);
        adapter.device_count()
    });
    let count = match result {
        Ok(count) if count <= 1024 => count,
        Ok(_) => {
            let failure = Failure::new(
                Reason::InvalidValue,
                "device count exceeds safety bound (1024)",
            );
            report.enumeration = Reading::Unavailable {
                failure: failure.clone(),
            };
            report.diagnostics.push(failure);
            return report;
        }
        Err(failure) => {
            report.enumeration = Reading::Unavailable {
                failure: failure.clone(),
            };
            report.diagnostics.push(failure);
            return report;
        }
    };
    report.enumeration = Reading::Available {
        value: count.into(),
    };
    report.status = CollectionStatus::Complete;
    if report.runtime.as_ref().is_some_and(|r| {
        r.driver_version.incomplete()
            || r.nvml_version.incomplete()
            || r.cuda_driver_version.incomplete()
    }) {
        report.status = CollectionStatus::Incomplete;
    }
    for index in 0..count {
        let device = adapter.device(index).unwrap_or_else(|failure| Device {
            index,
            failure: Some(failure),
            identity: Default::default(),
            telemetry: Default::default(),
            controls: Default::default(),
        });
        if device.incomplete() {
            report.status = CollectionStatus::Incomplete;
        }
        report.devices.push(device);
    }
    report
}
