//! Shared one-shot request observations. A request is never proof of exit.
#[cfg(any(windows, test))]
use crate::sampler::budget::{Budget, BudgetError, Reservation};
#[cfg(any(windows, test))]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub requested: bool,
    /// None after requested means pending/unknown, including callback unwind.
    pub returned: Option<Result<(), u32>>,
}
#[derive(Default)]
#[cfg(any(windows, test))]
struct Operation {
    requested: AtomicBool,
    returned: OnceLock<Result<(), u32>>,
}
#[cfg(any(windows, test))]
impl Operation {
    fn observe(&self) -> Observation {
        // Read the return first: a concurrent completion cannot yield an
        // impossible returned-but-never-requested observation.
        let returned = self.returned.get().copied();
        Observation {
            requested: self.requested.load(Ordering::Acquire),
            returned,
        }
    }
    #[cfg(any(windows, test))]
    fn invoke(&self, call: impl FnOnce() -> Result<(), u32>) -> Observation {
        if self
            .requested
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let returned = call();
            let _ = self.returned.set(returned);
        }
        self.observe()
    }
}
#[cfg(any(windows, test))]
pub(crate) struct Requests {
    cancelled: Arc<AtomicBool>,
    stop: Operation,
    terminate: Operation,
    _allocation: Reservation,
}
#[cfg(any(windows, test))]
impl Requests {
    #[cfg(any(windows, test))]
    pub(crate) fn new(
        cancelled: Arc<AtomicBool>,
        budget: &Budget,
    ) -> Result<Arc<Self>, BudgetError> {
        let allocation =
            budget.reserve(std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>())?;
        Ok(Arc::new(Self {
            cancelled,
            stop: Operation::default(),
            terminate: Operation::default(),
            _allocation: allocation,
        }))
    }
    #[cfg(any(windows, test))]
    pub(crate) fn signal(&self, call: impl FnOnce() -> Result<(), u32>) -> Observation {
        self.cancelled.store(true, Ordering::Release);
        self.stop.invoke(call)
    }
    #[cfg(any(windows, test))]
    pub(crate) fn terminate(&self, call: impl FnOnce() -> Result<(), u32>) -> Observation {
        self.cancelled.store(true, Ordering::Release);
        self.terminate.invoke(call)
    }
    pub(crate) fn stop(&self) -> Observation {
        self.stop.observe()
    }
    pub(crate) fn termination(&self) -> Observation {
        self.terminate.observe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn requests() -> (Arc<AtomicBool>, Arc<Requests>, Budget) {
        let flag = Arc::new(AtomicBool::new(false));
        let budget = Budget::new(4096).unwrap();
        let requests = Requests::new(flag.clone(), &budget).unwrap();
        (flag, requests, budget)
    }
    #[test]
    fn cancellation_is_published_before_either_actual_request_and_cannot_reopen() {
        let (flag, r, _) = requests();
        assert_eq!(r.stop(), Observation::default());
        assert_eq!(r.termination(), Observation::default());
        assert_eq!(
            r.signal(|| {
                assert!(flag.load(Ordering::Acquire));
                Ok(())
            })
            .returned,
            Some(Ok(()))
        );
        assert_eq!(
            r.terminate(|| {
                assert!(flag.load(Ordering::Acquire));
                Err(5)
            })
            .returned,
            Some(Err(5))
        );
        assert!(flag.load(Ordering::Acquire));
    }
    #[test]
    fn duplicate_and_concurrent_requests_never_call_a_replacement_operation() {
        let (_, r, _) = requests();
        r.signal(|| {
            assert_eq!(
                r.signal(|| panic!("duplicate stop")),
                Observation {
                    requested: true,
                    returned: None
                }
            );
            Err(87)
        });
        assert_eq!(
            r.signal(|| panic!("retry stop")),
            Observation {
                requested: true,
                returned: Some(Err(87))
            }
        );
        r.terminate(|| {
            assert_eq!(
                r.terminate(|| panic!("duplicate termination")),
                Observation {
                    requested: true,
                    returned: None
                }
            );
            Ok(())
        });
        assert_eq!(
            r.terminate(|| panic!("retry termination")).returned,
            Some(Ok(()))
        );
    }
    #[test]
    fn a_blocked_stop_operation_does_not_own_the_termination_lane() {
        let (_, r, _) = requests();
        r.signal(|| {
            assert_eq!(
                r.stop(),
                Observation {
                    requested: true,
                    returned: None
                }
            );
            assert_eq!(r.terminate(|| Ok(())).returned, Some(Ok(())));
            assert_eq!(r.stop().returned, None);
            Err(6)
        });
        assert_eq!(r.stop().returned, Some(Err(6)));
        assert_eq!(r.termination().returned, Some(Ok(())));
    }
    #[test]
    fn unwound_request_stays_pending_and_cannot_be_silently_retried() {
        let (flag, r, _) = requests();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            r.terminate(|| panic!("unreturned operation"))
        }));
        assert!(outcome.is_err());
        assert!(flag.load(Ordering::Acquire));
        assert_eq!(
            r.termination(),
            Observation {
                requested: true,
                returned: None
            }
        );
        assert_eq!(
            r.terminate(|| panic!("retry after unwind")),
            r.termination()
        );
    }
    #[test]
    fn shared_request_ledger_keeps_its_charge_after_original_owner_is_gone() {
        let (_, r, b) = requests();
        let charged = b.used();
        assert!(charged >= std::mem::size_of::<Requests>());
        let independent = r.clone();
        drop(r);
        assert_eq!(b.used(), charged);
        independent.terminate(|| Err(5));
        assert_eq!(independent.termination().returned, Some(Err(5)));
        drop(independent);
        assert_eq!(b.used(), 0);
    }
    #[test]
    fn capacity_failure_precedes_any_request_or_cancellation() {
        let flag = Arc::new(AtomicBool::new(false));
        let b = Budget::new(1).unwrap();
        assert!(matches!(
            Requests::new(flag.clone(), &b),
            Err(BudgetError::Exhausted)
        ));
        assert!(!flag.load(Ordering::Acquire));
        assert_eq!(b.used(), 0);
    }
}
