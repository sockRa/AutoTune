//! Parent-side handoff for one already-launched starting process: one-shot
//! bootstrap frame, then the bounded session driver. It creates no process and
//! issues no native admission; the caller keeps the process and its consent.
use super::{
    launch::CancelQueue,
    starting::Scope,
    starting_bootstrap::{self, Frame},
    starting_receive::Fault as SessionFault,
    starting_session::{Session, Step},
    transport::SendFailure,
    windows_process::{Process, ProcessQueue},
};
use crate::{monitor::Anchor, sampler::budget::Budget};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Session(SessionFault),
    Frame(starting_bootstrap::Error),
    Sender(SendFailure),
}
/// One bootstrap attempt and one bounded reception over a caller-owned starting
/// process. A failed send keeps the exact unsent frame and the first cause; no
/// successor frame is attempted.
pub struct Handoff<'a> {
    sender:
        starting_bootstrap::Sender<CancelQueue<ProcessQueue<'a, starting_bootstrap::Declaration>>>,
    session: Session,
    frame: Option<Frame>,
    send_fault: Option<SendFailure>,
}
/// Owned terminal parent-side starting evidence after its process borrow ends.
pub struct ClosedHandoff {
    pub session: Session,
    /// Exact bootstrap frame remains owned if its one-shot send failed.
    pub unsent_frame: Option<Frame>,
    pub send_fault: Option<SendFailure>,
}
impl<'a> Handoff<'a> {
    /// Bind the session and freeze the one-shot frame before any bytes move.
    pub fn open(
        process: &'a Process<starting_bootstrap::Declaration>,
        scope: &Scope,
        budget: &Budget,
    ) -> Result<Self, Fault> {
        let session = Session::new(process.declaration(), scope, budget).map_err(Fault::Session)?;
        let frame = Frame::encode(process.declaration(), scope, budget).map_err(Fault::Frame)?;
        let sender = process.starting_sender(budget).map_err(Fault::Sender)?;
        Ok(Self {
            sender,
            session,
            frame: Some(frame),
            send_fault: None,
        })
    }
    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }
    pub fn send_fault(&self) -> Option<SendFailure> {
        self.send_fault
    }
    /// Send the single bootstrap frame. A second attempt is rejected and the
    /// exact frame of a failed attempt remains owned for the caller's evidence.
    pub fn send(&mut self) -> Result<(), SendFailure> {
        let Some(frame) = self.frame.take() else {
            let fault = SendFailure::Closed;
            self.send_fault.get_or_insert(fault);
            return Err(fault);
        };
        match self.sender.send(frame) {
            Ok(()) => Ok(()),
            Err(frame) => {
                let fault = self.sender.fault().unwrap_or(SendFailure::Closed);
                self.frame = Some(frame);
                self.send_fault = Some(fault);
                Err(fault)
            }
        }
    }
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }
    /// Close the process-borrowing sender and retain the complete reception
    /// state plus any unsent frame/failure for later session finalization.
    pub fn close(self) -> ClosedHandoff {
        let Self {
            sender,
            session,
            frame,
            send_fault,
        } = self;
        drop(sender);
        ClosedHandoff {
            session,
            unsent_frame: frame,
            send_fault,
        }
    }
    /// Drive one nonblocking poll of the caller-owned process into the session.
    pub fn poll(
        &mut self,
        process: &Process<starting_bootstrap::Declaration>,
        clock: impl FnMut() -> Option<Anchor>,
    ) -> Step {
        self.session.poll_process(process, clock)
    }
}
