//! Actual inherited Windows pipe bridge. No launch, native loading or admission.
use super::{
    transport::{Queue, RawReceived, SendFailure, Sender},
    wire::{Direction, WorkerBinding},
};
use crate::{
    monitor::Anchor,
    sampler::{
        budget::Budget,
        runtime::IoObservation,
        windows_io::{ControlPort, Transport},
        wire::Lane,
    },
};
use std::{
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::GetHandleInformation,
    Storage::FileSystem::{GetFileType, FILE_TYPE_PIPE},
};

pub struct InheritedTransport {
    transport: Transport,
    sender_taken: AtomicBool,
    control_taken: bool,
}
pub struct WindowsQueue<'a> {
    transport: &'a Transport,
    ordinary_only: bool,
}
impl Queue for WindowsQueue<'_> {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        if self.ordinary_only && lane == Lane::Control {
            return Err(SendFailure::Binding);
        }
        self.transport.send_borrowed(lane, bytes)
    }
}
pub enum Receive {
    Packet(RawReceived),
    Idle {
        anchor: Option<Anchor>,
    },
    Fault {
        code: Option<i32>,
        anchor: Option<Anchor>,
    },
}
impl InheritedTransport {
    pub(crate) fn queue_copy(&self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        self.transport.send_borrowed(lane, bytes)
    }
    /// Adopt exactly four unique live pipe endpoints supplied by the fixed
    /// launcher. Job/process/peer/image/consent checks remain the launcher's job.
    pub fn new(
        read: [OwnedHandle; 2],
        write: [OwnedHandle; 2],
        budget: Budget,
    ) -> io::Result<Self> {
        let handles = [
            read[0].as_raw_handle(),
            read[1].as_raw_handle(),
            write[0].as_raw_handle(),
            write[1].as_raw_handle(),
        ];
        for (i, handle) in handles.iter().enumerate() {
            let mut flags = 0;
            if handles[..i].contains(handle) {
                return Err(io::Error::other("duplicate workload pipe"));
            }
            // SAFETY: borrowed handles remain owned by the input arrays. All
            // endpoints are validated before any I/O thread adopts them.
            unsafe {
                if GetHandleInformation(*handle, &mut flags) == 0 {
                    return Err(io::Error::last_os_error());
                }
                if GetFileType(*handle) != FILE_TYPE_PIPE {
                    return Err(io::Error::other("workload endpoint is not a pipe"));
                }
            }
        }
        Ok(Self {
            transport: Transport::new(read, write, budget)?,
            sender_taken: AtomicBool::new(false),
            control_taken: false,
        })
    }
    /// Split BEFORE creating a sender. The returned owner can move to a
    /// dedicated control thread while CUDA/NVML blocks on the producer thread.
    /// The ordinary receiver can no longer consume control frames or faults.
    pub fn split_control(&mut self) -> Result<ControlTransport, SendFailure> {
        if self.control_taken || self.sender_taken.load(Ordering::Acquire) {
            return Err(SendFailure::Closed);
        }
        self.control_taken = true;
        Ok(ControlTransport {
            port: self.transport.take_control(),
            sender_taken: AtomicBool::new(false),
        })
    }
    /// Worker/parent sends and Producer::FrameSink use this same bounded path.
    /// Preserve the returned sender for the whole session; failures are sticky.
    pub fn sender(
        &self,
        binding: WorkerBinding,
        direction: Direction,
        budget: &Budget,
    ) -> Result<Sender<WindowsQueue<'_>>, SendFailure> {
        if self.sender_taken.swap(true, Ordering::AcqRel) {
            return Err(SendFailure::Closed);
        }
        Sender::new(
            WindowsQueue {
                transport: &self.transport,
                ordinary_only: self.control_taken,
            },
            binding,
            direction,
            budget,
        )
    }
    /// Nonblocking queue poll; the supplied clock is captured AFTER receive.
    /// Clock failure never replaces an already received packet with an error.
    pub fn receive(&self, mut clock: impl FnMut() -> Option<Anchor>) -> Receive {
        let result = if self.control_taken {
            self.transport.receive_ordinary()
        } else {
            self.transport.receive()
        };
        let anchor = clock();
        match result {
            Ok(Some((lane, frame))) => {
                Receive::Packet(RawReceived::new(lane, frame.into_packet(), anchor))
            }
            Ok(None) => Receive::Idle { anchor },
            Err(e) => Receive::Fault {
                code: e.raw_os_error(),
                anchor,
            },
        }
    }
    pub fn output_drained(&self) -> bool {
        self.transport.output_drained()
    }
    /// Check this BEFORE a receive that returns Idle, preserving drain ordering.
    pub fn inputs_finished(&self) -> bool {
        self.transport.inputs_finished()
    }
    /// Requests cancellation without waiting/joining; results retain unresolved
    /// thread completion separately. No process exit or GPU quiescence follows.
    pub fn shutdown(&self) -> [IoObservation; 4] {
        self.transport.shutdown()
    }
}
pub struct ControlTransport {
    port: ControlPort,
    sender_taken: AtomicBool,
}
pub struct WindowsControlQueue<'a> {
    port: &'a ControlPort,
}
impl Queue for WindowsControlQueue<'_> {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        if lane != Lane::Control {
            return Err(SendFailure::Binding);
        }
        self.port.send_borrowed(bytes)
    }
}
impl ControlTransport {
    /// Encode/decode as well as queue copies use the reserved control pool.
    pub fn budget(&self) -> Budget {
        self.port.budget()
    }
    pub fn sender(
        &self,
        binding: WorkerBinding,
        direction: Direction,
        budget: &Budget,
    ) -> Result<Sender<WindowsControlQueue<'_>>, SendFailure> {
        if self.sender_taken.swap(true, Ordering::AcqRel) {
            return Err(SendFailure::Closed);
        }
        Sender::new(
            WindowsControlQueue { port: &self.port },
            binding,
            direction,
            budget,
        )
    }
    pub fn receive(&self, mut clock: impl FnMut() -> Option<Anchor>) -> Receive {
        let result = self.port.receive();
        let anchor = clock();
        match result {
            Ok(Some(frame)) => {
                Receive::Packet(RawReceived::new(Lane::Control, frame.into_packet(), anchor))
            }
            Ok(None) => Receive::Idle { anchor },
            Err(e) => Receive::Fault {
                code: e.raw_os_error(),
                anchor,
            },
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn independent_control_owner_is_send_without_native_calls() {
        fn require_send<T: Send>() {}
        require_send::<super::ControlTransport>();
        require_send::<super::InheritedTransport>();
    }
}
