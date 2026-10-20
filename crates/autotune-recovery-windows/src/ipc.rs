//! One-use local named-pipe channels. Run blocking I/O on startup/worker or RPC
//! executor threads, never on the guardian actor. Cancellation drains OVERLAPPED
//! storage on that thread before freeing it; timeout is not native quiescence.
use crate::process::Result;
use crate::process::*;
use autotune_recovery_contract::{process::*, *};
use std::{
    mem::size_of,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{
        Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT},
        GetAce, GetSecurityDescriptorControl, IsValidSecurityDescriptor, ACCESS_ALLOWED_ACE,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES,
        SE_DACL_PROTECTED,
    },
    Storage::FileSystem::*,
    System::{Pipes::*, Threading::*, IO::*},
};

pub struct Server {
    pub name: String,
    pipe: OwnedHandle,
}
fn event() -> Result<OwnedHandle> {
    owned(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) })
}
fn finish(
    file: BorrowedHandle<'_>,
    overlapped: &mut OVERLAPPED,
    peer: Option<BorrowedHandle<'_>>,
    deadline: Option<u64>,
) -> Result<u32> {
    let mut handles = vec![overlapped.hEvent];
    if let Some(peer) = peer {
        handles.push(peer.as_raw_handle());
    }
    let timeout = deadline.map_or(INFINITE, |d| {
        d.saturating_sub(boot_ms()).min(u32::MAX as u64 - 1) as u32
    });
    let wait =
        unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) };
    let mut bytes = 0;
    if wait != WAIT_OBJECT_0 || deadline.is_some_and(|d| boot_ms() >= d) {
        // SAFETY: cancel only THIS OVERLAPPED. Drain before its buffers/event leave
        // scope. This can block and is deliberately confined to the I/O executor.
        unsafe {
            CancelIoEx(file.as_raw_handle(), overlapped);
            GetOverlappedResult(file.as_raw_handle(), overlapped, &mut bytes, 1);
        }
        return Err(ProcessError::Policy(if wait == WAIT_TIMEOUT {
            "IPC deadline/uncertain I/O"
        } else {
            "IPC peer lost/uncertain I/O"
        }));
    }
    if unsafe { GetOverlappedResult(file.as_raw_handle(), overlapped, &mut bytes, 0) } == 0 {
        return Err(error());
    }
    Ok(bytes)
}
fn transfer(
    file: BorrowedHandle<'_>,
    bytes: &mut [u8],
    write: bool,
    peer: BorrowedHandle<'_>,
    deadline: Option<u64>,
) -> Result<usize> {
    let event = event()?;
    let mut o = OVERLAPPED {
        hEvent: event.as_raw_handle(),
        ..Default::default()
    };
    let mut count = 0;
    let ok = unsafe {
        if write {
            WriteFile(
                file.as_raw_handle(),
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut count,
                &mut o,
            )
        } else {
            ReadFile(
                file.as_raw_handle(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                &mut count,
                &mut o,
            )
        }
    };
    if ok == 0 {
        if unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(error());
        }
        count = finish(file, &mut o, Some(peer), deadline)?;
    }
    if deadline.is_some_and(|d| boot_ms() >= d) {
        return Err(ProcessError::Policy("IPC deadline/uncertain I/O"));
    }
    if count == 0 {
        return Err(ProcessError::Policy("IPC closed/uncertain I/O"));
    }
    Ok(count as usize)
}
impl Server {
    pub fn audit_security(&self, parent: &MeasuredProcess) -> Result<()> {
        let mut descriptor = ptr::null_mut();
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let error_code = unsafe {
            GetSecurityInfo(
                self.pipe.as_raw_handle(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error_code != 0 {
            return Err(ProcessError::Win32(error_code));
        }
        let _allocation = SecurityDescriptor(descriptor);
        if descriptor.is_null()
            || acl.is_null()
            || unsafe { IsValidSecurityDescriptor(descriptor) } == 0
        {
            return Err(ProcessError::Policy("invalid pipe security"));
        }
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(error());
        }
        if control & SE_DACL_PROTECTED == 0
            || sid(owner)? != parent.user_sid
            || unsafe { (*acl).AceCount } != 1
        {
            return Err(ProcessError::Policy("pipe DACL/owner"));
        }
        let mut raw = ptr::null_mut();
        if unsafe { GetAce(acl, 0, &mut raw) } == 0 {
            return Err(error());
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        if ace.Header.AceType != 0
            || ace.Header.AceFlags != 0
            || ace.Header.AceSize < 20
            || ace.Mask != 0x120183
            || sid((&ace.SidStart as *const u32).cast_mut().cast())? != parent.process.logon_sid
        {
            return Err(ProcessError::Policy(
                "pipe grants not minimal logon-SID rights",
            ));
        }
        Ok(())
    }
    pub fn new(parent: &MeasuredProcess) -> Result<Self> {
        let random = crate::random().map_err(|_| ProcessError::Policy("CNG RNG"))?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let name = format!(r"\\.\pipe\AutoTune-fake-v1-{suffix}");
        Self::with_name(name, parent)
    }
    /// Same explicit name is useful to demonstrate first-instance squatter rejection.
    pub fn with_name(name: String, parent: &MeasuredProcess) -> Result<Self> {
        if !name.starts_with(r"\\.\pipe\AutoTune-fake-v1-") {
            return Err(ProcessError::Policy("local pipe name"));
        }
        let security = pipe_security(&parent.process.logon_sid, &parent.user_sid)?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.0,
            bInheritHandle: 0,
        };
        let w = wide(std::ffi::OsStr::new(&name))?;
        let pipe = owned(unsafe {
            CreateNamedPipeW(
                w.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                FRAME_LIMIT as u32,
                FRAME_LIMIT as u32,
                0,
                &sa,
            )
        })?;
        let server = Self { name, pipe };
        server.audit_security(parent)?;
        Ok(server)
    }
    pub fn accept(
        self,
        bootstrap: &ProcessBootstrap,
        child: BorrowedHandle<'_>,
        deadline: u64,
    ) -> Result<Channel> {
        let event = event()?;
        let mut o = OVERLAPPED {
            hEvent: event.as_raw_handle(),
            ..Default::default()
        };
        if unsafe { ConnectNamedPipe(self.pipe.as_raw_handle(), &mut o) } == 0 {
            match unsafe { GetLastError() } {
                ERROR_PIPE_CONNECTED => (),
                ERROR_IO_PENDING => {
                    finish(self.pipe.as_handle(), &mut o, Some(child), Some(deadline))?;
                }
                _ => return Err(error()),
            }
        }
        let mut pid = 0;
        if unsafe { GetNamedPipeClientProcessId(self.pipe.as_raw_handle(), &mut pid) } == 0 {
            return Err(error());
        }
        if pid != bootstrap.context.child.process.pid {
            return Err(ProcessError::Policy("wrong pipe client PID"));
        }
        verify_peer(child, &bootstrap.context.child)?;
        let peer = duplicate(child, SUPERVISOR_ACCESS, false)?;
        let mut channel = Channel::new(self.pipe, peer, bootstrap, true)?;
        channel.parent_handshake(&bootstrap.context, deadline)?;
        Ok(channel)
    }
}
pub struct Channel {
    pipe: OwnedHandle,
    peer: OwnedHandle,
    key: Hash,
    session: Session,
    from: Role,
    to: Role,
    digest: Hash,
    next: u64,
    receiver: Receiver,
    poisoned: bool,
}
impl Channel {
    /// Fixed verifier-only hostile-wire seam. Unavailable on authority channels;
    /// it cannot grant permissions or choose an executable/backend.
    pub fn send_invalid_fixture(&mut self, fault: WireFault, deadline: u64) -> Result<()> {
        if self.from != Role::Verifier || self.to != Role::Launcher {
            return Err(ProcessError::Policy("fault seam is verifier-only"));
        }
        let mut frame = Frame {
            version: VERSION,
            from: self.from,
            to: self.to,
            session: self.session,
            digest: self.digest,
            generation: 0,
            sequence: self.next,
            operation_id: 0,
            message: Message::Heartbeat,
        };
        match fault {
            WireFault::Replay => frame.sequence = self.next.saturating_sub(1),
            WireFault::SequenceGap => frame.sequence += 1,
            WireFault::WrongRole => frame.from = Role::Worker,
            WireFault::WrongDigest => frame.digest[0] ^= 1,
            _ => (),
        }
        let mut bytes = encode_frame(&frame, &self.key)?;
        match fault {
            WireFault::BadMac => {
                let end = bytes.len() - 1;
                bytes[end] ^= 1;
            }
            WireFault::Oversize => bytes = ((FRAME_LIMIT + 1) as u32).to_le_bytes().to_vec(),
            WireFault::Truncated => {
                bytes.truncate(12);
            }
            WireFault::UnknownField => {
                let mut payload = canonical(&frame)?;
                payload.pop();
                payload.extend(b",\"alien\":0}");
                bytes = ((payload.len() + 32) as u32).to_le_bytes().to_vec();
                bytes.extend(payload);
                let tag =
                    crate::hmac(&self.key, &bytes).map_err(|_| ProcessError::Policy("CNG HMAC"))?;
                bytes.extend(tag);
            }
            _ => (),
        }
        let mut offset = 0;
        while offset < bytes.len() {
            offset += transfer(
                self.pipe.as_handle(),
                &mut bytes[offset..],
                true,
                self.peer.as_handle(),
                Some(deadline),
            )?;
        }
        self.next += 1;
        Ok(())
    }
    fn new(
        pipe: OwnedHandle,
        peer: OwnedHandle,
        b: &ProcessBootstrap,
        parent: bool,
    ) -> Result<Self> {
        let (from, to) = if parent {
            (b.context.supervisor_role, b.context.child_role)
        } else {
            (b.context.child_role, b.context.supervisor_role)
        };
        let digest = b.context.digest()?;
        Ok(Self {
            pipe,
            peer,
            key: b.key,
            session: b.context.session,
            from,
            to,
            digest,
            next: 1,
            receiver: Receiver::new(to, from, b.context.session, digest),
            poisoned: false,
        })
    }
    pub fn connect(b: &ChildBootstrap, deadline: u64) -> Result<Self> {
        let name = wide(std::ffi::OsStr::new(&b.envelope.pipe))?;
        // No GENERIC_WRITE: it grants FILE_CREATE_PIPE_INSTANCE on named pipes.
        let rights = FILE_READ_DATA
            | FILE_WRITE_DATA
            | FILE_READ_ATTRIBUTES
            | FILE_WRITE_ATTRIBUTES
            | READ_CONTROL
            | SYNCHRONIZE;
        let pipe = owned(unsafe {
            CreateFileW(
                name.as_ptr(),
                rights,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        })?;
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } == 0 {
            return Err(error());
        }
        if pid != b.envelope.context.supervisor.process.pid {
            return Err(ProcessError::Policy("wrong pipe server PID"));
        }
        verify_peer(b.supervisor.as_handle(), &b.envelope.context.supervisor)?;
        let peer = duplicate(b.supervisor.as_handle(), SUPERVISOR_ACCESS, false)?;
        let mut c = Self::new(pipe, peer, &b.envelope, false)?;
        c.child_handshake(&b.envelope.context, deadline)?;
        Ok(c)
    }
    pub fn bind_digest(&mut self, digest: Hash) {
        self.digest = digest;
        self.receiver.bind_digest(digest);
    }
    pub fn peer_alive(&self) -> bool {
        alive(self.peer.as_handle())
    }
    pub fn send(
        &mut self,
        message: Message,
        generation: u64,
        operation_id: u64,
        deadline: u64,
    ) -> Result<()> {
        if self.poisoned {
            return Err(ProcessError::Policy("channel permanently closed"));
        }
        let frame = Frame {
            version: VERSION,
            from: self.from,
            to: self.to,
            session: self.session,
            digest: self.digest,
            generation,
            sequence: self.next,
            operation_id,
            message,
        };
        self.next = self
            .next
            .checked_add(1)
            .ok_or(ProcessError::Policy("sequence overflow"))?;
        let mut bytes = encode_frame(&frame, &self.key)?;
        let end = bytes.len() - 32;
        let tag =
            crate::hmac(&self.key, &bytes[..end]).map_err(|_| ProcessError::Policy("CNG HMAC"))?;
        bytes[end..].copy_from_slice(&tag);
        let result = (|| {
            let mut sent = 0;
            while sent < bytes.len() {
                sent += transfer(
                    self.pipe.as_handle(),
                    &mut bytes[sent..],
                    true,
                    self.peer.as_handle(),
                    Some(deadline),
                )?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn receive(&mut self, deadline: Option<u64>) -> Result<Frame> {
        if self.poisoned {
            return Err(ProcessError::Policy("channel permanently closed"));
        }
        let result = (|| {
            let mut prefix = [0u8; 4];
            let mut used = 0;
            while used < 4 {
                used += transfer(
                    self.pipe.as_handle(),
                    &mut prefix[used..],
                    false,
                    self.peer.as_handle(),
                    deadline,
                )?;
            }
            let length = u32::from_le_bytes(prefix) as usize;
            if length + 4 > FRAME_LIMIT || length < 32 {
                return Err(ProcessError::Policy("frame length"));
            }
            let mut bytes = vec![0u8; length + 4];
            bytes[..4].copy_from_slice(&prefix);
            used = 4;
            let body_deadline = deadline.unwrap_or_else(|| boot_ms().saturating_add(WATCHDOG_MS));
            while used < bytes.len() {
                used += transfer(
                    self.pipe.as_handle(),
                    &mut bytes[used..],
                    false,
                    self.peer.as_handle(),
                    Some(body_deadline),
                )?;
            }
            let end = bytes.len() - 32;
            let tag: Hash = bytes[end..]
                .try_into()
                .map_err(|_| ProcessError::Policy("MAC size"))?;
            let expected = crate::hmac(&self.key, &bytes[..end])
                .map_err(|_| ProcessError::Policy("CNG HMAC"))?;
            if !constant_time_equal(&tag, &expected) {
                return Err(ProcessError::Policy("CNG MAC rejection"));
            }
            Ok(self.receiver.receive(&bytes, &self.key)?)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn parent_handshake(&mut self, context: &ChannelContext, deadline: u64) -> Result<()> {
        let p = crate::random().map_err(|_| ProcessError::Policy("CNG RNG"))?;
        let frame = challenge(context, self.from, self.next, p, [0; 32])?;
        self.send(frame.message, 0, 0, deadline)?;
        let reply = self.receive(Some(deadline))?;
        let c = match reply.message {
            Message::Challenge { local, .. } => local,
            _ => return Err(ProcessError::Policy("challenge response")),
        };
        verify_challenge(&reply, context, self.to, c, p)?;
        let frame = challenge(context, self.from, self.next, p, c)?;
        self.send(frame.message, 0, 0, deadline)?;
        Ok(())
    }
    fn child_handshake(&mut self, context: &ChannelContext, deadline: u64) -> Result<()> {
        let first = self.receive(Some(deadline))?;
        let p = match first.message {
            Message::Challenge { local, .. } => local,
            _ => return Err(ProcessError::Policy("initial challenge")),
        };
        verify_challenge(&first, context, self.to, p, [0; 32])?;
        let c = crate::random().map_err(|_| ProcessError::Policy("CNG RNG"))?;
        let frame = challenge(context, self.from, self.next, c, p)?;
        self.send(frame.message, 0, 0, deadline)?;
        let last = self.receive(Some(deadline))?;
        verify_challenge(&last, context, self.to, p, c)?;
        Ok(())
    }
}

/// Exactly one in-flight RPC. Actor polls replies; I/O and MAC/strict decoding run
/// off-loop. A timed-out/lost operation is the guardian reducer's pending permit,
/// never a reason to create a replacement worker or retransmit a command.
enum RpcRequest {
    Frame(Message, u64, u64),
    Bind(Hash, std::sync::mpsc::SyncSender<Result<()>>),
}
pub struct RpcLane {
    requests: Option<std::sync::mpsc::SyncSender<RpcRequest>>,
    replies: std::sync::mpsc::Receiver<Result<Frame>>,
    pending: bool,
    failed: bool,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl RpcLane {
    pub fn start(mut channel: Channel) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<RpcRequest>(1);
        let (reply, replies) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("autotune-ipc".into())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    let outcome = match request {
                        RpcRequest::Frame(m, g, id) => channel
                            .send(m, g, id, boot_ms().saturating_add(WATCHDOG_MS))
                            .and_then(|_| channel.receive(None)),
                        RpcRequest::Bind(digest, ack) => {
                            channel.bind_digest(digest);
                            let _ = ack.send(Ok(()));
                            continue;
                        }
                    };
                    let failure = outcome.is_err();
                    if reply.send(outcome).is_err() || failure {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: Some(tx),
            replies,
            pending: false,
            failed: false,
            thread: Some(thread),
        })
    }
    /// Seal changes the authenticated frame digest. The lane must be idle and is
    /// synchronously updated before any new command is accepted.
    pub fn bind_digest(&mut self, digest: Hash) -> Result<()> {
        if self.failed || self.pending {
            return Err(ProcessError::Policy("bind requires an idle lane"));
        }
        let (ack, done) = std::sync::mpsc::sync_channel(1);
        self.requests
            .as_ref()
            .ok_or(ProcessError::Policy("RPC closed"))?
            .try_send(RpcRequest::Bind(digest, ack))
            .map_err(|_| ProcessError::Policy("RPC bind queue full"))?;
        done.recv_timeout(std::time::Duration::from_millis(1000))
            .map_err(|_| ProcessError::Policy("RPC bind acknowledgement timeout"))?
    }
    pub fn submit(&mut self, message: Message, generation: u64, id: u64) -> Result<()> {
        if self.failed || self.pending {
            return Err(ProcessError::Policy("one pending RPC/no reconnect"));
        }
        self.requests
            .as_ref()
            .ok_or(ProcessError::Policy("RPC closed"))?
            .try_send(RpcRequest::Frame(message, generation, id))
            .map_err(|_| ProcessError::Policy("RPC queue full"))?;
        self.pending = true;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Result<Frame>> {
        match self.replies.try_recv() {
            Ok(reply) => {
                self.pending = false;
                if reply.is_err() {
                    self.failed = true;
                }
                Some(reply)
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) if self.pending => {
                self.failed = true;
                Some(Err(ProcessError::Policy("RPC executor lost")))
            }
            _ => None,
        }
    }
    /// Only after every operation has retired and the peer can return. This may
    /// block; unresolved native calls must retain this lane and ownership instead.
    pub fn shutdown(mut self) -> Result<()> {
        self.requests.take();
        if let Some(t) = self.thread.take() {
            t.join()
                .map_err(|_| ProcessError::Policy("RPC thread panic"))?;
        }
        Ok(())
    }
}
