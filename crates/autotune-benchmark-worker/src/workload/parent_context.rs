//! Cross-artifact reported references. Success is not peer authentication,
//! human consent, fresh safety, native admission or hardware qualification.
use super::{
    host_environment,
    manifest::BoundRun,
    parent_sampler::{self, ParentSampler},
    runtime_evidence,
    settings::{self, Current},
    starting::Scope,
    starting_read,
    wire::WorkerBinding,
    worker_setup_delivery::ReadyDelivery,
};
use crate::{evidence::Digest, monitor::clock::Clock};
use autotune_probe::report::{Reading, Runtime};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Binding,
    Scope,
    Host,
    Starting,
    Lifecycle,
    Nvml,
    Versions,
    Worker,
    Settings,
    Duplicate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Missing,
    Binding,
    Host(host_environment::Fault),
    Starting(settings::Block),
    Lifecycle,
    Nvml(runtime_evidence::Fault),
    Version,
    Worker(super::worker_setup_assessment::Rejection),
    Sampler(parent_sampler::Fault),
    Changed,
    Simulated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejection {
    pub part: Part,
    pub cause: Cause,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub part: Part,
    pub result: Result<(), Cause>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct References {
    pub manifest: Digest,
    pub scope: Digest,
    pub host: Option<Digest>,
    pub starting: Option<Digest>,
    pub starting_response: Option<Digest>,
    pub worker_setup: Digest,
    pub current: Option<Digest>,
    pub sampler_ready: Option<Digest>,
    pub current_response: Option<Digest>,
}
/// Reported reference consistency only; all original artifact owners remain
/// with the caller and must be retained/exported for the complete session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub references: References,
    pub current: Current,
    pub cuda_api: i32,
    pub worker_ready: crate::monitor::Anchor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub references: References,
    pub checks: [Check; 9],
    pub result: Result<Summary, Rejection>,
}
pub struct Inputs<'a, 'r> {
    pub run: &'a BoundRun<'r>,
    pub scope: &'a Scope,
    pub host: Option<&'a host_environment::Reported>,
    pub starting: Option<&'a starting_read::Receipt>,
    pub worker: &'a mut ReadyDelivery,
}
pub(super) fn screen(parent: &ParentSampler, input: Inputs<'_, '_>) -> Record {
    let Inputs {
        run,
        scope,
        host,
        starting,
        worker,
    } = input;
    let d = scope.draft();
    let binding = parent.context_binding();
    let current_receipt = parent.current().and_then(|r| r.result().ok());
    let snapshot = starting.and_then(starting_read::Receipt::snapshot);
    let refs = References {
        manifest: run.manifest_digest(),
        scope: scope.digest(),
        host: host.map(|h| h.snapshot().digest()),
        starting: snapshot.map(|s| s.snapshot().digest()),
        starting_response: starting.map(starting_read::Receipt::digest),
        worker_setup: worker.receipt().digest(),
        current: current_receipt.map(|r| r.capture().digest()),
        sampler_ready: parent.ready().map(|r| {
            Digest::of(
                crate::evidence::Domain::RawArtifact,
                r.raw().packet().bytes(),
            )
        }),
        current_response: current_receipt
            .map(|r| Digest::of(crate::evidence::Domain::RawArtifact, r.frame())),
    };
    let check_binding = if !parent.context_open() {
        Err(Cause::Sampler(
            parent.fault().unwrap_or(parent_sampler::Fault::Stopped),
        ))
    } else if WorkerBinding::new(run, d.frequency, d.epoch).ok() != Some(binding)
        || worker.receipt().binding() != binding
    {
        Err(Cause::Binding)
    } else {
        Ok(())
    };
    let check_scope = if host.is_none() || starting.is_none() || snapshot.is_none() {
        Err(Cause::Missing)
    } else if host.is_some_and(|h| h.snapshot().scope().digest() != scope.digest())
        || starting.is_some_and(|s| s.scope_digest() != scope.digest())
        || snapshot.is_some_and(|s| s.snapshot().scope().digest() != scope.digest())
    {
        Err(Cause::Binding)
    } else {
        Ok(())
    };
    let check_host = host.ok_or(Cause::Missing).and_then(|h| {
        h.snapshot()
            .check_run(run, d.frequency, d.epoch)
            .map_err(Cause::Host)
    });
    let original = snapshot.ok_or(Cause::Missing).and_then(|s| {
        s.check_run(run, d.frequency, d.epoch)
            .map_err(Cause::Starting)
    });
    let check_lifecycle = starting.ok_or(Cause::Missing).and_then(|s| {
        let r = s.summary();
        if r.response_version != 2
            || !r.lifecycle_complete
            || !r.native_abi_complete
            || r.lifecycle_timing.is_err()
            || r.native_fault.is_some()
            || r.snapshot_error.is_some()
            || r.trace_fault.is_some()
            || r.control_fault.is_some()
            || r.clock_error.is_some()
            || r.control_error.is_some()
            || r.native_failure.is_some()
            || r.cleanup_status != Some(0)
            || !r.output_drained
            || !r.control_finished
            || r.observed_frequency != Some(d.frequency)
        {
            return Err(Cause::Lifecycle);
        }
        let complete = r.completion.ok_or(Cause::Missing)?;
        let elapsed = Clock::new(binding.monitor, binding.epoch)
            .and_then(|mut c| c.observe(complete))
            .map_err(|_| Cause::Lifecycle)?;
        if elapsed > run.origin_ns() {
            return Err(Cause::Lifecycle);
        }
        let drain = r.lifecycle_timing.map_err(|_| Cause::Lifecycle)?;
        if Clock::new(binding.monitor, binding.epoch)
            .and_then(|mut c| c.observe(drain))
            .map_err(|_| Cause::Lifecycle)?
            > run.origin_ns()
        {
            return Err(Cause::Lifecycle);
        }
        if let Some(end) = snapshot.and_then(|s| {
            s.snapshot()
                .draft()
                .observations
                .last()
                .and_then(|o| o.after)
        }) {
            if complete.qpc_before < end.qpc_after {
                return Err(Cause::Lifecycle);
            }
        } else {
            return Err(Cause::Missing);
        }
        let begin = r.native_begin.ok_or(Cause::Missing)?;
        let end = r.native_end.ok_or(Cause::Missing)?;
        let settings_entry = snapshot.ok_or(Cause::Missing)?.snapshot().entry();
        let file_checks = s.runtime().ok_or(Cause::Missing)?.checks();
        let file_first = file_checks
            .first()
            .and_then(|c| c.anchor)
            .ok_or(Cause::Missing)?;
        let file_last = file_checks
            .last()
            .and_then(|c| c.anchor)
            .ok_or(Cause::Missing)?;
        if file_first.qpc_before < r.receipt.qpc_after
            || begin.qpc_before < file_last.qpc_after
            || settings_entry.qpc_before < end.qpc_after
        {
            return Err(Cause::Lifecycle);
        }
        Ok(())
    });
    let check_nvml = starting
        .and_then(starting_read::Receipt::runtime)
        .ok_or(Cause::Missing)
        .and_then(|r| r.assess(scope).map_err(Cause::Nvml));
    let setup = worker.assess_setup();
    let check_worker = setup.map(|_| ()).map_err(Cause::Worker);
    let current = current_receipt.ok_or(Cause::Missing).and_then(|r| {
        if r.fault().is_some() {
            return Err(Cause::Starting(r.fault().expect("observed fault")));
        }
        if r.capture().reported_source() != settings::Source::NativeGetterPath {
            return Err(Cause::Simulated);
        }
        r.capture().current().map_err(Cause::Starting)
    });
    let check_settings = original
        .and_then(|a| current.and_then(|b| if a == b { Ok(()) } else { Err(Cause::Changed) }));
    let versions = starting.ok_or(Cause::Missing).and_then(|s| {
        let start = s.info().ok_or(Cause::Missing)?;
        if !s.summary().runtime_calls_match {
            return Err(Cause::Version);
        }
        let later = parent.runtime().ok_or(Cause::Missing)?;
        let setup = setup.map_err(Cause::Worker)?;
        if !parent_sampler::runtime_valid(&start.runtime)
            || !parent_sampler::runtime_valid(later)
            || !same_versions(&start.runtime, later, setup.driver_version)
        {
            return Err(Cause::Version);
        }
        let path = start
            .runtime
            .library_path
            .as_deref()
            .ok_or(Cause::Missing)?;
        let requested = s
            .runtime()
            .and_then(|r| r.file())
            .and_then(|f| f.requested.as_ref())
            .ok_or(Cause::Missing)?;
        if !requested.iter().copied().eq(path.encode_utf16())
            || later.library_path.as_deref() != Some(path)
        {
            return Err(Cause::Version);
        }
        Ok(setup.driver_version)
    });
    let checks = [
        Check {
            part: Part::Binding,
            result: check_binding,
        },
        Check {
            part: Part::Scope,
            result: check_scope,
        },
        Check {
            part: Part::Host,
            result: check_host,
        },
        Check {
            part: Part::Starting,
            result: original.map(|_| ()),
        },
        Check {
            part: Part::Lifecycle,
            result: check_lifecycle,
        },
        Check {
            part: Part::Nvml,
            result: check_nvml,
        },
        Check {
            part: Part::Versions,
            result: versions.map(|_| ()),
        },
        Check {
            part: Part::Worker,
            result: check_worker,
        },
        Check {
            part: Part::Settings,
            result: check_settings,
        },
    ];
    let result = match checks.iter().find_map(|c| {
        c.result.err().map(|cause| Rejection {
            part: c.part,
            cause,
        })
    }) {
        Some(e) => {
            worker.close_reference_path();
            Err(e)
        }
        None => Ok(Summary {
            references: refs,
            current: current.expect("checked current"),
            cuda_api: versions.expect("checked versions"),
            worker_ready: setup.expect("checked setup").ready,
        }),
    };
    Record {
        references: refs,
        checks,
        result,
    }
}
fn same_versions(a: &Runtime, b: &Runtime, cuda: i32) -> bool {
    a.driver_version == b.driver_version
        && a.nvml_version == b.nvml_version
        && a.cuda_driver_version == b.cuda_driver_version
        && a.header_api_version == b.header_api_version
        && matches!(&a.cuda_driver_version.reading,Reading::Available{value} if value.as_i64()==Some(i64::from(cuda)))
}
