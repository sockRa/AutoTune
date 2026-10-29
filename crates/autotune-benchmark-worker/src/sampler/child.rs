//! Serial child gate shared by the production child and deterministic tests.
use super::{
    manifest::CaptureManifest,
    wire::{Message, WireBinding},
};
use crate::monitor::{clock::Clock, Anchor, Ticket};
use std::sync::atomic::{AtomicU8, Ordering};
/// Linearizes independent stop against native-operation admission. Completion
/// clears only pending, so an in-flight return can never reopen admission.
#[derive(Default)]
pub struct NativeAdmission(AtomicU8);
impl NativeAdmission {
    pub fn begin(&self) -> Result<(), ChildError> {
        self.0
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|state| {
                if state & 1 != 0 {
                    ChildError::Stopped
                } else {
                    ChildError::Pending
                }
            })
    }
    pub fn stop(&self) {
        self.0.fetch_or(1, Ordering::AcqRel);
    }
    pub fn complete(&self) {
        self.0.fetch_and(!2, Ordering::AcqRel);
    }
    pub fn pending(&self) -> bool {
        self.0.load(Ordering::Acquire) & 2 != 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildError {
    NotReady,
    Binding,
    Pending,
    Replay,
    Stopped,
    Protocol,
}
pub struct ChildGate {
    binding: WireBinding,
    bootstrapped: bool,
    pending: bool,
    stopped: bool,
    last_slots: [Option<u64>; 6],
}
impl ChildGate {
    pub fn new(binding: WireBinding) -> Self {
        Self {
            binding,
            bootstrapped: false,
            pending: false,
            stopped: false,
            last_slots: [None; 6],
        }
    }
    pub fn bootstrap(&mut self, message: Message) -> Result<(CaptureManifest, Anchor), ChildError> {
        if self.bootstrapped || self.stopped {
            return Err(ChildError::Protocol);
        }
        let Message::Bootstrap {
            origin,
            raw_uuid,
            raw_pci,
            manifest,
        } = message
        else {
            return Err(ChildError::Protocol);
        };
        let manifest =
            CaptureManifest::parse(manifest.as_bytes()).map_err(|_| ChildError::Binding)?;
        if manifest
            .binding(self.binding.monitor.qpc_frequency)
            .map_err(|_| ChildError::Binding)?
            != self.binding
            || manifest.draft().raw_uuid != raw_uuid
            || manifest.draft().raw_pci != raw_pci
            || Clock::new(self.binding.monitor, origin).is_err()
        {
            return Err(ChildError::Binding);
        }
        self.bootstrapped = true;
        Ok((manifest, origin))
    }
    pub fn begin(&mut self, ticket: Ticket) -> Result<(), ChildError> {
        if self.stopped {
            return Err(ChildError::Stopped);
        }
        if !self.bootstrapped {
            return Err(ChildError::NotReady);
        }
        if self.pending {
            return Err(ChildError::Pending);
        }
        let cadence = autotune_fma_reference::profile::Profile::approved_design()
            .limits
            .cadence_ns;
        if ticket.binding != self.binding.monitor
            || ticket.slot.checked_mul(cadence) != Some(ticket.scheduled_ns)
            || ticket.start_ns < ticket.scheduled_ns
        {
            return Err(ChildError::Binding);
        }
        let previous = &mut self.last_slots[ticket.metric as usize];
        if previous.is_some_and(|slot| ticket.slot <= slot) {
            return Err(ChildError::Replay);
        }
        *previous = Some(ticket.slot);
        self.pending = true;
        Ok(())
    }
    pub fn complete(&mut self) -> Result<(), ChildError> {
        if !self.pending {
            return Err(ChildError::Protocol);
        }
        self.pending = false;
        Ok(())
    }
    pub fn stop(&mut self) {
        self.stopped = true;
    }
}

#[derive(Default)]
pub struct ChildShutdown {
    first: Option<Anchor>,
    finished: bool,
}
impl ChildShutdown {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn first_stop(&self) -> Option<Anchor> {
        self.first
    }
    pub fn stop(&mut self, anchor: Anchor, pending: bool) -> Option<Message> {
        if self.first.is_some() {
            return None;
        }
        self.first = Some(anchor);
        Some(Message::StopAck { anchor, pending })
    }
    pub fn finished(&mut self, anchor: Anchor, pending: bool) -> Result<Message, ChildError> {
        if self.first.is_none() || self.finished {
            return Err(ChildError::Protocol);
        }
        self.finished = true;
        Ok(Message::RoleFinished {
            anchor,
            pending,
            cleanup: "nvml_shutdown_attempted_result_unknown".into(),
        })
    }
}
