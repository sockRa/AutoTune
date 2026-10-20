pub mod adapter;
pub mod mock;
pub mod nvml;
pub mod report;

pub fn run_live() -> report::Report {
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp is representable");
    adapter::collect(&mut nvml::NvmlAdapter::default(), timestamp)
}

pub fn run_mock(scenario: &str) -> Option<report::Report> {
    Some(adapter::collect(
        &mut mock::MockAdapter::new(scenario)?,
        mock::TIMESTAMP.into(),
    ))
}
