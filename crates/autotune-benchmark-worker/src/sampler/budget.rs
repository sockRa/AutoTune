//! Shared reservations acquired BEFORE allocating transport/codec/evidence data.
//! This bounds accounted application allocations, not OS/allocator/driver RAM.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetError {
    InvalidLimit,
    Exhausted,
}
struct Inner {
    limit: usize,
    used: AtomicUsize,
    _parent: Option<Reservation>,
}
#[derive(Clone)]
pub struct Budget(Arc<Inner>);
pub struct Reservation {
    budget: Budget,
    bytes: usize,
}
impl Budget {
    pub fn new(limit: usize) -> Result<Self, BudgetError> {
        if limit == 0
            || limit
                > autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .evidence_ram_bytes as usize
        {
            return Err(BudgetError::InvalidLimit);
        }
        Ok(Self(Arc::new(Inner {
            limit,
            used: AtomicUsize::new(0),
            _parent: None,
        })))
    }
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }
    /// Reserve a disjoint pool in the parent. Its charge survives every clone,
    /// detached I/O worker and outstanding frame until the entire pool drops.
    pub fn partition(&self, bytes: usize) -> Result<Self, BudgetError> {
        if bytes == 0 {
            return Err(BudgetError::InvalidLimit);
        }
        let parent = self.reserve(bytes)?;
        Ok(Self(Arc::new(Inner {
            limit: bytes,
            used: AtomicUsize::new(0),
            _parent: Some(parent),
        })))
    }
    pub fn reserve(&self, bytes: usize) -> Result<Reservation, BudgetError> {
        self.0
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|n| *n <= self.0.limit)
            })
            .map_err(|_| BudgetError::Exhausted)?;
        Ok(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
impl Reservation {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn is_within(&self, root: &Budget) -> bool {
        let mut current = &self.budget.0;
        loop {
            if Arc::ptr_eq(current, &root.0) {
                return true;
            }
            match &current._parent {
                Some(parent) => current = &parent.budget.0,
                None => return false,
            }
        }
    }
    pub fn belongs_to(&self, budget: &Budget) -> bool {
        Arc::ptr_eq(&self.budget.0, &budget.0)
    }
    pub fn shrink_to(&mut self, bytes: usize) -> Result<(), BudgetError> {
        if bytes > self.bytes {
            return Err(BudgetError::InvalidLimit);
        }
        self.budget
            .0
            .used
            .fetch_sub(self.bytes - bytes, Ordering::AcqRel);
        self.bytes = bytes;
        Ok(())
    }
}
