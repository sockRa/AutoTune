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
    controls: std::sync::mpsc::SyncSender<Message>,
    oracle_active: std::sync::Arc<std::sync::Mutex<bool>>,
    control_acks: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    oracle_release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requests: Option<std::sync::mpsc::SyncSender<RpcRequest>>,
    replies: std::sync::mpsc::Receiver<Result<Frame>>,
    pending: bool,
    failed: bool,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl RpcLane {
    pub fn start(channel: Channel) -> Result<Self> {
        Self::start_with_oracle(channel, |_, _, _| {
            Err(ProcessError::Policy("no fake oracle configured"))
        })
    }
    /// Fake-only external oracle off the I/O thread. A FakeCall is intermediate
    /// evidence, never an idle reply. Exactly one such invocation is allowed for
    /// the outstanding Dispatch, with the exact original permit and frame IDs.
    pub fn start_with_oracle(
        mut channel: Channel,
        oracle: impl FnMut(
                &autotune_recovery_contract::process::DispatchProof,
                autotune_recovery_contract::process::FakeInvocation,
                &std::sync::atomic::AtomicBool,
            ) -> Result<autotune_recovery_contract::RawResult>
            + Send
            + 'static,
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<RpcRequest>(1);
        let (reply, replies) = std::sync::mpsc::sync_channel(1);
        let oracle_release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release = oracle_release.clone();
        let oracle = std::sync::Arc::new(std::sync::Mutex::new(oracle));
        let (controls, control_rx) = std::sync::mpsc::sync_channel::<Message>(2);
        let oracle_active = std::sync::Arc::new(std::sync::Mutex::new(false));
        let active = oracle_active.clone();
        let control_acks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let acknowledged = control_acks.clone();
        let thread = std::thread::Builder::new()
            .name("autotune-ipc".into())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    let outcome = match request {
                        RpcRequest::Frame(m, g, id) => (|| {
                            let dispatch = match &m {
                                Message::Dispatch(proof) => Some((**proof).clone()),
                                _ => None,
                            };
                            channel.send(m, g, id, boot_ms().saturating_add(WATCHDOG_MS))?;
                            let frame = channel.receive(None)?;
                            if let Message::FakeCall(invocation) = &frame.message {
                                let proof = dispatch
                                    .ok_or(ProcessError::Policy("unsolicited fake call"))?;
                                if invocation.permit != proof.permit
                                    || frame.generation != g
                                    || frame.operation_id != id
                                {
                                    return Err(ProcessError::Policy("fake call permit mismatch"));
                                }
                                // The oracle may block, but must not own the
                                // authenticated control lane. Still one device
                                // operation, with no replacement worker.
                                let oracle = oracle.clone();
                                let invocation = (**invocation).clone();
                                let release = release.clone();
                                let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
                                *active.lock().map_err(|_| ProcessError::Policy("oracle control state poisoned"))? = true;
                                let call = std::thread::Builder::new()
                                    .name("autotune-fake-call".into())
                                    .spawn(move || {
                                        let result = oracle.lock()
                                            .map_err(|_| ProcessError::Policy("fake oracle poisoned"))
                                            .and_then(|mut oracle| oracle(&proof, invocation, &release));
                                        let _ = result_tx.send(result);
                                    })?;
                                let mut completed = None;
                                let result = loop {
                                    // Control acks cannot retire Dispatch or be
                                    // mistaken for ReturnAndIdle.
                                    while let Ok(control) = control_rx.try_recv() {
                                        channel.send(control, 0, 0, boot_ms().saturating_add(WATCHDOG_MS))?;
                                        let ack = channel.receive(Some(boot_ms().saturating_add(WATCHDOG_MS)))?;
                                        if ack.generation != 0 || ack.operation_id != 0
                                            || !matches!(ack.message, Message::Heartbeat)
                                        {
                                            return Err(ProcessError::Policy("in-call control acknowledgement"));
                                        }
                                        acknowledged.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    if let Some(result) = completed.take() {
                                        break result;
                                    }
                                    match result_rx.try_recv() {
                                        Ok(result) => {
                                            // Stop new controls atomically, then
                                            // drain already accepted ones before
                                            // FakeResult. None may outlive Dispatch.
                                            *active.lock().map_err(|_| ProcessError::Policy("oracle control state poisoned"))? = false;
                                            completed = Some(result?);
                                        }
                                        Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(1)),
                                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return Err(ProcessError::Policy("fake oracle lost")),
                                    }
                                };
                                call.join().map_err(|_| ProcessError::Policy("fake oracle panic"))?;
                                channel.send(
                                    Message::FakeResult {
                                        operation_id: id,
                                        result,
                                    },
                                    g,
                                    id,
                                    boot_ms().saturating_add(WATCHDOG_MS),
                                )?;
                                let completion = channel.receive(None)?;
                                if !matches!(completion.message, Message::ReturnAndIdle { .. }) {
                                    return Err(ProcessError::Policy(
                                        "oracle result is not worker idle",
                                    ));
                                }
                                Ok(completion)
                            } else {
                                if dispatch.is_some()
                                    && matches!(frame.message, Message::ReturnAndIdle {
                                        outcome: ref o, ..
                                    } if matches!(**o, autotune_recovery_contract::Completion::Returned { .. }))
                                {
                                    return Err(ProcessError::Policy("returned call has no external oracle entry"));
                                }
                                Ok(frame)
                            }
                        })(),
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
            controls,
            oracle_active,
            control_acks,
            oracle_release,
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
        self.oracle_release
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.requests
            .as_ref()
            .ok_or(ProcessError::Policy("RPC closed"))?
            .try_send(RpcRequest::Frame(message, generation, id))
            .map_err(|_| ProcessError::Policy("RPC queue full"))?;
        self.pending = true;
        Ok(())
    }
    /// Test-oracle effect barrier only, never permission or pending retirement.
    pub fn release_fake_barrier(&self) {
        self.oracle_release
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// Only revocation/storage notifications may accompany a pending Dispatch.
    /// Queue exhaustion is a failure, never dropped authority or a retry.
    pub fn notify_pending(
        &self,
        generation: u64,
        window: autotune_recovery_contract::storage::RecoveryWindow,
        storage_failed: bool,
    ) -> Result<usize> {
        if !self.pending || self.failed {
            return Err(ProcessError::Policy(
                "in-call notification requires pending lane",
            ));
        }
        let active = self
            .oracle_active
            .lock()
            .map_err(|_| ProcessError::Policy("oracle control state poisoned"))?;
        if !*active {
            return Ok(0);
        }
        self.controls
            .try_send(Message::Revoke {
                generation,
                trigger_ms: window.trigger_ms,
                deadline_ms: window.deadline_ms,
            })
            .map_err(|_| ProcessError::Policy("in-call control queue full/closed"))?;
        if storage_failed {
            self.controls
                .try_send(Message::StorageFailed(window))
                .map_err(|_| ProcessError::Policy("in-call control queue full/closed"))?;
        }
        Ok(if storage_failed { 2 } else { 1 })
    }
    pub fn control_acknowledgements(&self) -> usize {
        self.control_acks.load(std::sync::atomic::Ordering::SeqCst)
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

#[cfg(test)]
mod tests {
    use super::*;
    use autotune_recovery_contract::storage::RecoveryWindow;

    // Real isolated local pipe, with the normal codec/MAC/sequence path. Peer
    // bootstrap authentication is covered by the separate process tests.
    fn pair() -> (Channel, Channel) {
        let identity = current_identity().unwrap();
        let server = Server::new(&identity).unwrap();
        let name = wide(std::ffi::OsStr::new(&server.name)).unwrap();
        let client = owned(unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_READ_DATA | FILE_WRITE_DATA,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        })
        .unwrap();
        let event = event().unwrap();
        let mut o = OVERLAPPED {
            hEvent: event.as_raw_handle(),
            ..Default::default()
        };
        let connected = unsafe { ConnectNamedPipe(server.pipe.as_raw_handle(), &mut o) };
        assert!(connected != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED);
        let make = |pipe, from, to| Channel {
            pipe,
            peer: duplicate(current_process_handle(), SUPERVISOR_ACCESS, false).unwrap(),
            key: [9; 32],
            session: [8; 32],
            from,
            to,
            digest: [7; 32],
            next: 1,
            receiver: Receiver::new(to, from, [8; 32], [7; 32]),
            poisoned: false,
        };
        (
            make(server.pipe, Role::Launcher, Role::Guardian),
            make(client, Role::Guardian, Role::Launcher),
        )
    }

    #[test]
    fn authenticated_in_call_controls_do_not_retire_dispatch_or_admit_a_successor() {
        let (guardian, mut worker) = pair();
        let now = boot_ms();
        let permit = Permission {
            session: [8; 32],
            boot: [6; 32],
            digest: [7; 32],
            generation: 1,
            operation_id: 1,
            operation: Operation::CandidateOnce,
            dispatched_ms: now,
            watchdog_ms: now + WATCHDOG_MS,
            expiry_ms: now + WATCHDOG_MS,
            recovery_trigger_ms: None,
        };
        let proof = DispatchProof {
            permit: permit.clone(),
            intent_sequence: 1,
            intent_utc_ms: 1,
            publication: Some([5; 32]),
        };
        let window = RecoveryWindow {
            trigger_ms: now,
            deadline_ms: now + CLEANUP_MS,
        };
        let expected_window = window.clone();
        let expected = permit.clone();
        let peer = std::thread::spawn(move || {
            let frame = worker.receive(Some(boot_ms() + 5000)).unwrap();
            assert!(matches!(frame.message, Message::Dispatch(_)));
            worker
                .send(
                    Message::FakeCall(Box::new(FakeInvocation {
                        permit: expected.clone(),
                        entry_ms: now,
                        write_mw: Some(242_123),
                        device: Device {
                            uuid: "SYNTHETIC-test".into(),
                            pci: "0000:01:00.0".into(),
                            vendor: 1,
                            device: 2,
                            subsystem: 3,
                            model: "fake".into(),
                        },
                    })),
                    1,
                    1,
                    boot_ms() + 5000,
                )
                .unwrap();
            let revoke = worker.receive(Some(boot_ms() + 5000)).unwrap();
            assert_eq!((revoke.generation, revoke.operation_id), (0, 0));
            assert_eq!(
                revoke.message,
                Message::Revoke {
                    generation: 2,
                    trigger_ms: expected_window.trigger_ms,
                    deadline_ms: expected_window.deadline_ms
                }
            );
            worker
                .send(Message::Heartbeat, 0, 0, boot_ms() + 5000)
                .unwrap();
            let failed = worker.receive(Some(boot_ms() + 5000)).unwrap();
            assert_eq!(failed.message, Message::StorageFailed(expected_window));
            worker
                .send(Message::Heartbeat, 0, 0, boot_ms() + 5000)
                .unwrap();
            let result = worker.receive(Some(boot_ms() + 5000)).unwrap();
            let Message::FakeResult {
                result,
                operation_id: 1,
            } = result.message
            else {
                panic!()
            };
            worker
                .send(
                    Message::ReturnAndIdle {
                        permit: expected,
                        outcome: Box::new(Completion::Returned {
                            entry_ms: now,
                            return_ms: boot_ms(),
                            result,
                        }),
                    },
                    1,
                    1,
                    boot_ms() + 5000,
                )
                .unwrap();
        });
        let mut lane = RpcLane::start_with_oracle(guardian, |_, _, release| {
            let deadline = boot_ms() + 5000;
            while !release.load(std::sync::atomic::Ordering::SeqCst) {
                if boot_ms() >= deadline {
                    return Err(ProcessError::Policy("test barrier expired"));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(RawResult::Api { status: 0 })
        })
        .unwrap();
        assert!(lane.notify_pending(2, window.clone(), true).is_err());
        lane.submit(Message::Dispatch(Box::new(proof)), 1, 1)
            .unwrap();
        let deadline = boot_ms() + 5000;
        loop {
            if lane.notify_pending(2, window.clone(), true).unwrap() == 2 {
                break;
            }
            assert!(boot_ms() < deadline);
            std::thread::yield_now();
        }
        while lane.control_acknowledgements() != 2 {
            assert!(lane.poll().is_none(), "a control ack is not ReturnAndIdle");
            assert!(boot_ms() < deadline);
            std::thread::yield_now();
        }
        assert!(lane.poll().is_none());
        assert!(lane
            .submit(Message::Control(Control::Observe), 0, 0)
            .is_err());
        lane.release_fake_barrier();
        let frame = loop {
            if let Some(frame) = lane.poll() {
                break frame.unwrap();
            }
            assert!(boot_ms() < deadline);
            std::thread::yield_now();
        };
        assert!(
            matches!(frame.message, Message::ReturnAndIdle { permit: ref p, .. } if *p == permit)
        );
        assert!(lane.notify_pending(2, window, true).is_err());
        lane.shutdown().unwrap();
        peer.join().unwrap();
    }

    #[test]
    fn delayed_reader_retains_client_final_frame_after_writer_handle_close() {
        let (mut reader, mut writer) = pair();
        let expected = Message::Rejected {
            reason: "synthetic-report".repeat(2000),
        };
        let message = expected.clone();
        std::thread::spawn(move || writer.send(message, 0, 0, boot_ms() + 2000))
            .join()
            .unwrap()
            .unwrap();
        let frame = reader
            .receive(Some(boot_ms() + 2000))
            .expect("final client report must not disappear after successful send");
        assert_eq!(frame.message, expected);
    }

    #[test]
    fn delayed_reader_retains_server_final_frame_after_writer_handle_close() {
        let (mut writer, mut reader) = pair();
        let expected = Message::Rejected {
            reason: "synthetic-report".repeat(2000),
        };
        let message = expected.clone();
        std::thread::spawn(move || writer.send(message, 0, 0, boot_ms() + 2000))
            .join()
            .unwrap()
            .unwrap();
        let frame = reader
            .receive(Some(boot_ms() + 2000))
            .expect("final server report must not disappear after successful send");
        assert_eq!(frame.message, expected);
    }
}
