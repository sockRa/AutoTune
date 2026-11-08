//! Private retained collector machinery. No public configurable native callback.
use super::parent_watch_retirement;
use crate::sampler::budget::{Budget, Reservation};
use std::{
    any::Any,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Capacity,
    Spawn,
    Panicked,
    Operation(parent_watch_retirement::Fault),
}
pub(crate) trait Cancel {
    fn cancel(&self);
}
struct Inner<T> {
    owned: Option<T>,
    operation: Option<Result<(), Fault>>,
    panic: Option<Box<dyn Any + Send>>,
}
struct Cell<T> {
    inner: Mutex<Inner<T>>,
    allocation: Arc<Reservation>,
}
impl<T> Cell<T> {
    fn execute(
        &self,
        operation: impl FnOnce(&mut T) -> Result<(), parent_watch_retirement::Fault>,
    ) -> bool {
        // Only this collector holds its owner mutex. Controller/cancellation
        // have independent references and never acquire it.
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let result = catch_unwind(AssertUnwindSafe(|| {
            operation(inner.owned.as_mut().expect("retained collector input"))
        }));
        match result {
            Ok(result) => inner.operation = Some(result.map_err(Fault::Operation)),
            Err(payload) => {
                inner.panic = Some(payload);
                inner.operation = Some(Err(Fault::Panicked));
            }
        }
        inner.operation.is_some_and(|r| r.is_err())
    }
    fn take(&self, joined: thread::Result<()>) -> Outcome<T> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let join_panic = joined.err();
        let result = if join_panic.is_some() {
            Err(Fault::Panicked)
        } else {
            inner.operation.unwrap_or(Err(Fault::Panicked))
        };
        Outcome {
            owned: inner
                .owned
                .take()
                .expect("single collector ownership transfer"),
            result,
            operation: inner.operation,
            panics: [inner.panic.take(), join_panic],
            allocation: self.allocation.clone(),
        }
    }
}
pub(crate) struct Outcome<T> {
    pub owned: T,
    pub result: Result<(), Fault>,
    pub operation: Option<Result<(), Fault>>,
    pub panics: [Option<Box<dyn Any + Send>>; 2],
    pub allocation: Arc<Reservation>,
}
pub(crate) struct Failure<T> {
    pub fault: Fault,
    pub error: Option<std::io::Error>,
    pub owned: T,
}
pub(crate) struct Task<T, C: Cancel> {
    cell: Arc<Cell<T>>,
    thread: Option<JoinHandle<()>>,
    cancel: C,
}
impl<T: Send + 'static, C: Cancel + Clone + Send + 'static> Task<T, C> {
    #[allow(clippy::result_large_err)] // Never allocate to return original owners on failure.
    pub(crate) fn start(
        owned: T,
        cancel: C,
        budget: &Budget,
        operation: fn(&mut T) -> Result<(), parent_watch_retirement::Fault>,
    ) -> Result<Self, Failure<T>> {
        let bytes = std::mem::size_of::<Cell<T>>() + std::mem::size_of::<Self>() + 4096;
        let charge = if bytes
            <= autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .metadata_bytes as usize
        {
            budget.reserve(bytes).ok()
        } else {
            None
        };
        let Some(charge) = charge else {
            cancel.cancel();
            return Err(Failure {
                fault: Fault::Capacity,
                error: None,
                owned,
            });
        };
        let cell = Arc::new(Cell {
            inner: Mutex::new(Inner {
                owned: Some(owned),
                operation: None,
                panic: None,
            }),
            allocation: Arc::new(charge),
        });
        let worker = cell.clone();
        let signal = cancel.clone();
        match thread::Builder::new()
            .name("pilot-retirement-collector".into())
            .spawn(move || {
                if worker.execute(operation) {
                    signal.cancel();
                }
            }) {
            Ok(thread) => Ok(Self {
                cell,
                thread: Some(thread),
                cancel,
            }),
            Err(error) => {
                // A failed spawn drops its closure before returning; it never
                // takes input from the cell. Recover the original owner here.
                cancel.cancel();
                let mut inner = cell.inner.lock().unwrap_or_else(|e| e.into_inner());
                Err(Failure {
                    fault: Fault::Spawn,
                    error: Some(error),
                    owned: inner.owned.take().expect("unstarted collector input"),
                })
            }
        }
    }
    pub(crate) fn finished(&self) -> bool {
        self.thread.as_ref().is_some_and(JoinHandle::is_finished)
    }
    pub(crate) fn poll(&mut self) -> Option<Outcome<T>> {
        if !self.finished() {
            return None;
        }
        let joined = self
            .thread
            .take()
            .expect("observed collector thread")
            .join();
        // Even an outer thread panic cannot discard the retained input cell.
        Some(self.cell.take(joined))
    }
}
impl<T, C: Cancel> Drop for Task<T, C> {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Owner {
        drops: Arc<AtomicUsize>,
        partial: usize,
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[derive(Clone)]
    struct Signal(Arc<AtomicUsize>);
    impl Cancel for Signal {
        fn cancel(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn cell(budget: &Budget, drops: &Arc<AtomicUsize>) -> Cell<Owner> {
        Cell {
            inner: Mutex::new(Inner {
                owned: Some(Owner {
                    drops: drops.clone(),
                    partial: 0,
                }),
                operation: None,
                panic: None,
            }),
            allocation: Arc::new(budget.reserve(256).unwrap()),
        }
    }
    #[test]
    fn owner_and_charge_survive_collection_and_transfer() {
        let budget = Budget::new(4096).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = cell(&budget, &drops);
        assert!(!cell.execute(|o| {
            o.partial = 7;
            Ok(())
        }));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let outcome = cell.take(Ok(()));
        drop(cell);
        assert_eq!(outcome.owned.partial, 7);
        assert_eq!(outcome.result, Ok(()));
        assert_eq!(outcome.operation, Some(Ok(())));
        assert!(outcome.panics.iter().all(Option::is_none));
        assert_eq!(outcome.allocation.bytes(), 256);
        assert_eq!(budget.used(), 256);
        drop(outcome);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn unwind_preserves_partial_owner_and_original_payload() {
        let budget = Budget::new(4096).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = cell(&budget, &drops);
        assert!(cell.execute(|o| {
            o.partial = 11;
            std::panic::panic_any(123u32)
        }));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let outcome = cell.take(Ok(()));
        assert_eq!(outcome.owned.partial, 11);
        assert_eq!(outcome.result, Err(Fault::Panicked));
        assert_eq!(
            outcome.panics[0].as_ref().unwrap().downcast_ref::<u32>(),
            Some(&123)
        );
        assert!(outcome.panics[1].is_none());
    }
    #[test]
    fn outer_unwind_retains_operation_failure_separately() {
        let budget = Budget::new(4096).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = cell(&budget, &drops);
        assert!(cell.execute(|o| {
            o.partial = 9;
            Err(parent_watch_retirement::Fault::Os)
        }));
        let outcome = cell.take(Err(Box::new(456u32)));
        assert_eq!(
            outcome.operation,
            Some(Err(Fault::Operation(parent_watch_retirement::Fault::Os)))
        );
        assert_eq!(outcome.result, Err(Fault::Panicked));
        assert_eq!(outcome.owned.partial, 9);
        assert_eq!(
            outcome.panics[1].as_ref().unwrap().downcast_ref::<u32>(),
            Some(&456)
        );
    }
    #[test]
    fn both_unwind_payloads_survive_without_replacing_partial_evidence() {
        let budget = Budget::new(4096).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = cell(&budget, &drops);
        assert!(cell.execute(|o| {
            o.partial = 17;
            std::panic::panic_any(111u32)
        }));
        let outcome = cell.take(Err(Box::new(222u32)));
        assert_eq!(outcome.owned.partial, 17);
        assert_eq!(
            outcome.panics[0].as_ref().unwrap().downcast_ref::<u32>(),
            Some(&111)
        );
        assert_eq!(
            outcome.panics[1].as_ref().unwrap().downcast_ref::<u32>(),
            Some(&222)
        );
    }
    #[test]
    fn missing_operation_cannot_become_success() {
        let budget = Budget::new(4096).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = cell(&budget, &drops);
        let outcome = cell.take(Ok(()));
        assert_eq!(outcome.result, Err(Fault::Panicked));
        assert_eq!(outcome.operation, None);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn cancellation_does_not_acquire_collector_owner_lock() {
        let budget = Budget::new(4096).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = cell(&budget, &drops);
        let signal = Signal(Arc::new(AtomicUsize::new(0)));
        assert!(!cell.execute(|_| {
            assert!(cell.inner.try_lock().is_err());
            signal.cancel();
            assert_eq!(signal.0.load(Ordering::SeqCst), 1);
            Ok(())
        }));
    }
    #[test]
    fn capacity_failure_returns_owner_without_spawning_or_invoking() {
        let budget = Budget::new(1).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let signal = Signal(Arc::new(AtomicUsize::new(0)));
        let owner = Owner {
            drops: drops.clone(),
            partial: 13,
        };
        let failure = match Task::start(owner, signal.clone(), &budget, |_| panic!("must not run"))
        {
            Err(e) => e,
            Ok(_) => panic!("must not spawn"),
        };
        assert_eq!(failure.fault, Fault::Capacity);
        assert!(failure.error.is_none());
        assert_eq!(failure.owned.partial, 13);
        assert_eq!(signal.0.load(Ordering::SeqCst), 1);
        assert_eq!(budget.used(), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(failure);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn unstarted_poll_is_nonblocking_and_does_not_take_owners() {
        let budget = Budget::new(4096).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = Arc::new(cell(&budget, &drops));
        let signal = Signal(Arc::new(AtomicUsize::new(0)));
        let mut task = Task {
            cell: cell.clone(),
            thread: None,
            cancel: signal.clone(),
        };
        let guard = cell.inner.lock().unwrap();
        assert!(!task.finished());
        assert!(task.poll().is_none());
        drop(task);
        assert!(guard.owned.is_some());
        assert_eq!(signal.0.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
}
