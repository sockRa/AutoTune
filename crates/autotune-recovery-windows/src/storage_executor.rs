//! One guardian-owned storage thread, one request, no replacement publisher.
//! Timeout quarantines the thread; it does not cancel kernel I/O or prove durability.
use crate::storage::*;
use autotune_recovery_contract::{storage::JournalEnvelope, PUBLICATION_MS};
use std::{
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread::{self, JoinHandle},
};

fn now_ms() -> u64 {
    let mut ticks = 0;
    // SAFETY: valid output pointer; required Windows precise boot-clock API.
    unsafe {
        windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise(&mut ticks);
    }
    ticks / 10_000
}
pub struct StorageExecutor {
    sender: Option<SyncSender<JournalEnvelope>>,
    replies: Receiver<Result<Scan>>,
    thread: Option<JoinHandle<()>>,
    custody: Custody,
    _ancestry: Vec<std::fs::File>,
    pending_deadline: Option<u64>,
    quarantined: bool,
    late_completion_observed: bool,
    late_outcome: Option<Result<Scan>>,
}
impl StorageExecutor {
    pub fn start(store: Store) -> Result<Self> {
        Self::start_with(store, |_| Ok(()))
    }
    /// Barrier is solely a test fault seam; never a second publisher or a device lane.
    pub fn start_with(
        mut store: Store,
        mut barrier: impl FnMut(PublicationStep) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        if store.reconciliation_only() {
            return Err(StorageError::ReadOnlyReconciliation);
        }
        let custody = store.duplicate_custody()?;
        let ancestry = store.pin_ancestry()?;
        let (sender, requests) = mpsc::sync_channel(1);
        let (reply, replies) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("autotune-storage".into())
            .spawn(move || {
                while let Ok(snapshot) = requests.recv() {
                    let result = store.publish_with(snapshot, &mut barrier);
                    let failed = result.is_err();
                    if reply.send(result).is_err() || failed {
                        break;
                    }
                }
                // Store, ancestry, leaf handles and publisher ownership die on THIS thread.
            })?;
        Ok(Self {
            sender: Some(sender),
            replies,
            thread: Some(handle),
            custody,
            _ancestry: ancestry,
            pending_deadline: None,
            quarantined: false,
            late_completion_observed: false,
            late_outcome: None,
        })
    }
    pub fn custody(&self) -> &Custody {
        &self.custody
    }
    pub fn quarantined(&self) -> bool {
        self.quarantined
    }
    pub fn late_completion_observed(&self) -> bool {
        self.late_completion_observed
    }
    /// Diagnostic evidence only; never returned as a timely acknowledgement by poll().
    pub fn late_outcome(&self) -> Option<&Result<Scan>> {
        self.late_outcome.as_ref()
    }
    /// The guardian supplies only an already frozen cleanup deadline, never a duration.
    pub fn submit(
        &mut self,
        envelope: JournalEnvelope,
        cleanup_deadline_ms: Option<u64>,
    ) -> Result<()> {
        if self.quarantined {
            return Err(StorageError::Poisoned);
        }
        if self.pending_deadline.is_some() {
            return Err(StorageError::Busy);
        }
        // Canonical encoding, full validation and history scans belong to the storage
        // thread, not this actor-facing submission path. The bounded lane owns one value.
        let now = now_ms();
        let deadline = now
            .checked_add(PUBLICATION_MS)
            .ok_or(StorageError::Invalid("deadline overflow"))?;
        let deadline = cleanup_deadline_ms.map_or(deadline, |d| d.min(deadline));
        if now >= deadline {
            return Err(StorageError::Timeout);
        }
        self.sender
            .as_ref()
            .ok_or(StorageError::Poisoned)?
            .try_send(envelope)
            .map_err(|_| StorageError::Busy)?;
        self.pending_deadline = Some(deadline);
        Ok(())
    }
    /// Nonblocking actor input. Deadline wins ties, including already queued replies.
    pub fn poll(&mut self) -> Result<Option<Scan>> {
        if self.quarantined {
            if let Ok(result) = self.replies.try_recv() {
                self.late_completion_observed = true;
                self.late_outcome = Some(result);
            }
            return Err(StorageError::Poisoned);
        }
        let Some(deadline) = self.pending_deadline else {
            return Ok(None);
        };
        if now_ms() >= deadline {
            self.quarantined = true;
            return Err(StorageError::Timeout);
        }
        match self.replies.try_recv() {
            Ok(result) => {
                self.pending_deadline = None;
                if result.is_err() {
                    self.quarantined = true;
                }
                result.map(Some)
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.quarantined = true;
                Err(StorageError::Poisoned)
            }
        }
    }
    /// Orderly shutdown waits for the sole writer. Never invoke this on the actor loop
    /// while I/O is stalled. Process exit also kills this thread, not an independent helper.
    pub fn shutdown(mut self) -> Result<()> {
        self.join()
    }
    fn join(&mut self) -> Result<()> {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| StorageError::Invalid("storage thread panic"))?;
        }
        Ok(())
    }
}
impl Drop for StorageExecutor {
    fn drop(&mut self) {
        // Safety invariant over responsiveness at orderly shutdown: never detach a live
        // publisher and release its ancestry/guardian ownership underneath it.
        let _ = self.join();
    }
}
