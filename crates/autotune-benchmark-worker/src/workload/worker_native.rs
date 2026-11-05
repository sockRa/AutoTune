//! Actual opt-in CUDA setup -> retained producer preparation. No executable or
//! workload admission issuer; invocation needs separate exact native consent.
use super::{
    bootstrap::Bound,
    control::Shared,
    cuda_setup_trace::Ledger,
    gate::StopWindow,
    producer::{Driver, PrepareFailure},
    worker_runtime::{ReadyFacts, Session},
};
use crate::{
    cuda::{CudaBackend, SetupFailure},
    monitor::Anchor,
    sampler::budget::{Budget, Reservation},
};
use std::sync::Arc;
#[allow(clippy::large_enum_variant)] // Original inline failure owners; no error-path allocation.
pub enum Preparation<C, S> {
    Ready {
        session: Session<CudaBackend, C, S>,
        facts: ReadyFacts,
    },
    SetupFailed {
        failure: SetupFailure,
        clock: C,
        stop: S,
    },
    PrepareFailed(PrepareFailure<CudaBackend, C, S>),
}
pub struct Prepared<C, S> {
    pub first: Anchor,
    pub setup: Ledger,
    pub result: Preparation<C, S>,
    pub prestart: super::prestart::Report,
    binding: super::wire::WorkerBinding,
    _metadata: Reservation,
}
impl<C: FnMut() -> Option<Anchor>, S> Prepared<C, S> {
    /// Only a successful CUDA open followed by failed CPU preparation qualifies.
    /// Failed/partial setup remains opaque and is never guessed safe to destroy.
    pub fn cleanup_unprepared(&mut self, shared: Arc<Shared>) {
        if let Preparation::PrepareFailed(f) = &mut self.result {
            let seed = self.setup.entries().last().and_then(|e| e.after);
            self.prestart
                .run(self.binding, shared, &mut f.driver, seed, || {
                    super::teardown::Check {
                        anchor: (f.clock)(),
                        fault: None,
                    }
                });
        }
    }
}
pub struct Failure<C, S> {
    pub fault: super::cuda_setup_trace::Fault,
    pub first: Option<Anchor>,
    pub clock: C,
    pub stop: S,
}
#[allow(clippy::result_large_err)] // Return original callback owners without another allocation.
/// # Safety
/// Only the fixed, independently supervised worker may call this once under the
/// exact approved native setup/manifest/device/artifact/consent scope. Bootstrap
/// and Ready are not authorization. No compute occurs here; Start and a future
/// parent-qualified NativeAdmission remain mandatory before dispatch.
pub unsafe fn prepare<
    C: FnMut() -> Option<Anchor> + 'static,
    S: FnMut() -> Option<StopWindow> + 'static,
>(
    bootstrap: &Bound,
    shared: Arc<Shared>,
    mut clock: C,
    stop: S,
    budget: &Budget,
) -> Result<Prepared<C, S>, Failure<C, S>> {
    // Validate worker role and the exact frozen bootstrap/control deadline
    // before even opening a library, not merely after CUDA setup succeeds.
    if super::worker_runtime::handshake(bootstrap, &shared).is_err() {
        shared.close();
        return Err(Failure {
            fault: super::cuda_setup_trace::Fault::Binding,
            first: None,
            clock,
            stop,
        });
    }
    let metadata = match budget.reserve(std::mem::size_of::<Prepared<C, S>>()) {
        Ok(v) => v,
        Err(_) => {
            shared.close();
            return Err(Failure {
                fault: super::cuda_setup_trace::Fault::Capacity,
                first: None,
                clock,
                stop,
            });
        }
    };
    let first = clock();
    let Some(anchor) = first else {
        shared.close_at(bootstrap.binding(), None);
        return Err(Failure {
            fault: super::cuda_setup_trace::Fault::Clock,
            first,
            clock,
            stop,
        });
    };
    let b = bootstrap.binding();
    let valid = (|| {
        let mut validator = crate::monitor::clock::Clock::new(b.monitor(), b.epoch).ok()?;
        validator.observe(bootstrap.receipt()).ok()?;
        if anchor.qpc_before < bootstrap.receipt().qpc_after {
            return None;
        }
        validator.observe(anchor).ok()
    })();
    if valid.is_none() {
        shared.close_at(b, None);
        return Err(Failure {
            fault: super::cuda_setup_trace::Fault::Clock,
            first,
            clock,
            stop,
        });
    }
    let mut setup = match Ledger::new(bootstrap.binding(), shared.clone(), anchor, budget) {
        Ok(v) => v,
        Err(fault) => {
            return Err(Failure {
                fault,
                first,
                clock,
                stop,
            })
        }
    };
    // The observer is bound before the trusted loader enters the first CUDA call.
    // It retains every returned ABI output before the post-clock callback.
    let opened = unsafe {
        CudaBackend::open_observed(
            bootstrap.binding().monitor().device,
            bootstrap.run().plan().shape,
            &mut setup.observer(&mut clock),
        )
    };
    let result = match opened {
        Err(failure) => {
            shared.close();
            Preparation::SetupFailed {
                failure,
                clock,
                stop,
            }
        }
        Ok(driver) => {
            // Successful setup already checked actual context-device equality
            // and the exact supported 7.5 attribute pair before buffer creation.
            let facts = ReadyFacts {
                context: driver.identity(),
                driver_version: driver.driver_version(),
                compute_major: 7,
                compute_minor: 5,
            };
            match Session::prepare(bootstrap, shared, driver, clock, stop, budget) {
                Ok(mut session) => {
                    if let Some(last) = setup.entries().last().and_then(|e| e.after) {
                        let _ = session.inherit_setup(last);
                    }
                    Preparation::Ready { session, facts }
                }
                Err(f) => Preparation::PrepareFailed(f),
            }
        }
    };
    Ok(Prepared {
        first: anchor,
        setup,
        result,
        prestart: super::prestart::Report::default(),
        binding: b,
        _metadata: metadata,
    })
}

impl<C: FnMut() -> Option<Anchor> + 'static, S: FnMut() -> Option<StopWindow> + 'static>
    Prepared<C, S>
{
    /// Detach only retained facts, with model/codec reservations before copies.
    /// No driver, OS query, cleanup or native admission occurs here.
    pub fn setup_artifact(
        &self,
        budget: &Budget,
    ) -> Result<super::worker_setup::Artifact, super::worker_setup::Fault> {
        use super::worker_setup::{Artifact, Facts, File, Preparation as Stage};
        let (stage, cpu_fault, native_fault, backend, failure) = match &self.result {
            Preparation::Ready { session, .. } => (
                Stage::Ready,
                None,
                None,
                Some(session.producer().setup_backend()),
                None,
            ),
            Preparation::SetupFailed { failure, .. } => (
                Stage::CudaFailed,
                None,
                Some(&failure.fault),
                None,
                Some(failure),
            ),
            Preparation::PrepareFailed(f) => {
                use super::producer::Fault;
                let name = match f.fault {
                    Fault::Admission => "admission",
                    Fault::Binding => "binding",
                    Fault::Capacity => "capacity",
                    Fault::Clock => "clock",
                    Fault::Deadline => "deadline",
                    Fault::Protocol => "protocol",
                    Fault::Native(_) => "native",
                    Fault::Incorrect => "incorrect",
                    Fault::Length => "length",
                    Fault::Transport => "transport",
                };
                let native = if let Fault::Native(n) = &f.fault {
                    Some(n)
                } else {
                    None
                };
                (Stage::CpuFailed, Some(name), native, Some(&f.driver), None)
            }
        };
        let file = backend
            .and_then(CudaBackend::runtime_file)
            .or_else(|| failure.and_then(SetupFailure::runtime_file));
        let load_error = failure
            .and_then(SetupFailure::load_error)
            .map(super::runtime_evidence::runtime_load_error)
            .transpose()
            .map_err(|_| super::worker_setup::Fault::Shape)?;
        Artifact::native(
            self.binding,
            Facts {
                first: self.first,
                deadline_raw: self.setup.deadline_raw(),
                preparation: stage,
                cpu_fault,
                native_fault,
                entries: self.setup.entries(),
                pending: self.setup.pending(),
                fault: self.setup.fault(),
                overflow: self.setup.overflow(),
                rejected: self.setup.rejected_return(),
                file_checks: self.setup.file_checks(),
                file_overflow: self.setup.file_overflow(),
                file: file.map(File::Native),
                load_error,
                environment: backend
                    .map(CudaBackend::child_options)
                    .or_else(|| failure.map(SetupFailure::child_options))
                    .unwrap_or_default(),
                driver_version: backend
                    .map(CudaBackend::driver_version)
                    .or_else(|| failure.and_then(SetupFailure::driver_version)),
                module_loading_mode: backend
                    .map(CudaBackend::module_loading_mode)
                    .or_else(|| failure.and_then(SetupFailure::module_loading_mode)),
            },
            budget,
        )
    }
}
