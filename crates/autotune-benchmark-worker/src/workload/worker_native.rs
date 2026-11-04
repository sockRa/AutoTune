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
    _metadata: Reservation,
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
        _metadata: metadata,
    })
}
