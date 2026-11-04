//! Single owned receive step, shared by CPU fixtures and the real pipe adapter.
use super::{Driver, Fault, Session};
use crate::{
    monitor::Anchor,
    workload::{
        gate::StopWindow,
        producer::Fault as ProducerFault,
        transport::{RawReceived, WorkerReceived},
    },
};
impl<
        B: Driver,
        C: FnMut() -> Option<Anchor> + 'static,
        S: FnMut() -> Option<StopWindow> + 'static,
    > Session<B, C, S>
{
    pub fn transport_fault(&self) -> Option<(Option<i32>, Option<Anchor>)> {
        self.transport_fault
    }
    pub fn input_finished(&self) -> bool {
        self.input_finished
    }
    fn observe_receive(&mut self, anchor: Option<Anchor>) -> Result<(), Fault> {
        if let Some(f) = self.fault {
            return Err(f);
        }
        let now = self.producer.observe_poll(anchor);
        self.observe_history(anchor)?;
        let now = now.map_err(|e| self.fail(Fault::Producer(e)))?;
        if !self.started && now >= self.deadline {
            return Err(self.fail(Fault::Producer(ProducerFault::Deadline)));
        }
        if self.shared.admission().stopped()
            && (!self.started || !matches!(self.shared.stop_window(self.binding), Ok(Some(_))))
        {
            return Err(self.fail(Fault::Control));
        }
        Ok(())
    }
    /// Idle receive advances the same worker clock and original setup ceiling.
    pub fn idle(&mut self, anchor: Option<Anchor>) -> Result<(), Fault> {
        self.observe_receive(anchor)
    }
    /// Retain the OS result before checking its possibly missing/invalid clock.
    pub fn receive_failed(&mut self, code: Option<i32>, anchor: Option<Anchor>) {
        self.transport_fault.get_or_insert((code, anchor));
        let _ = self.observe_receive(anchor);
        self.fail(Fault::Control);
    }
    pub fn receive_finished(&mut self) {
        self.input_finished = true;
        self.fail(Fault::Protocol);
    }
    #[allow(clippy::result_large_err)] // Return the exact surplus/rejected original owner.
    pub fn raw_packet(&mut self, raw: RawReceived) -> Result<(), WorkerReceived> {
        let _ = self.observe_receive(raw.receipt());
        let raw = raw.decode_worker(&mut self.receiver);
        self.packet(raw)
    }
    /// One nonblocking ordinary receive. The independent control owner must be
    /// split before use; this method does not launch, load or issue admission.
    #[cfg(windows)]
    #[allow(clippy::result_large_err)]
    pub fn poll_native(
        &mut self,
        port: &crate::workload::windows_transport::InheritedTransport,
    ) -> Result<(), WorkerReceived> {
        if self.fault.is_some() {
            return Ok(());
        }
        match port.receive(|| self.producer.poll_clock()) {
            crate::workload::windows_transport::Receive::Packet(raw) => self.raw_packet(raw),
            crate::workload::windows_transport::Receive::Idle { anchor } => {
                let _ = self.idle(anchor);
                if port.inputs_finished() {
                    self.receive_finished();
                }
                Ok(())
            }
            crate::workload::windows_transport::Receive::Fault { code, anchor } => {
                self.receive_failed(code, anchor);
                Ok(())
            }
        }
    }
}
