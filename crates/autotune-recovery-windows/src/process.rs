//! Measured fixed executable roles, suspended launch and explicit inherited sections.
use autotune_recovery_contract::{
    canonical, decode, digest, process::*, storage::FileIdentity, Hash, Role,
};
use std::{
    ffi::c_void,
    fs::File,
    io::Read,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr,
    sync::Arc,
};
use windows_sys::{
    Wdk::Foundation::{NtQueryObject, ObjectBasicInformation, ObjectTypeInformation},
    Win32::{
        Foundation::*,
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            *,
        },
        Storage::FileSystem::{FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO},
        System::{Console::*, JobObjects::*, Memory::*, Threading::*},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    Win32(u32),
    Nt(i32),
    Policy(&'static str),
    Context(&'static str, Box<ProcessError>),
    /// Authenticated diagnostic rejection, never imported execution authority.
    RemoteRejection(String),
    Contract(autotune_recovery_contract::Rejection),
    Storage(crate::storage::StorageError),
    Io(std::io::ErrorKind),
}
pub type Result<T> = std::result::Result<T, ProcessError>;
const SE_GROUP_LOGON_ID: u32 = 0xc0000000;
impl From<autotune_recovery_contract::Rejection> for ProcessError {
    fn from(e: autotune_recovery_contract::Rejection) -> Self {
        Self::Contract(e)
    }
}
impl From<crate::storage::StorageError> for ProcessError {
    fn from(e: crate::storage::StorageError) -> Self {
        Self::Storage(e)
    }
}
impl From<std::io::Error> for ProcessError {
    fn from(e: std::io::Error) -> Self {
        e.raw_os_error()
            .map_or(Self::Io(e.kind()), |c| Self::Win32(c as u32))
    }
}
pub(crate) fn error() -> ProcessError {
    ProcessError::Win32(unsafe { GetLastError() })
}
pub(crate) fn owned(raw: HANDLE) -> Result<OwnedHandle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        Err(error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}
pub(crate) fn wide(s: &std::ffi::OsStr) -> Result<Vec<u16>> {
    let mut v: Vec<u16> = s.encode_wide().collect();
    if v.contains(&0) {
        return Err(ProcessError::Policy("NUL"));
    }
    v.push(0);
    Ok(v)
}
pub fn boot_ms() -> u64 {
    let mut n = 0;
    unsafe { windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise(&mut n) };
    n / 10_000
}
pub fn current_process_handle() -> BorrowedHandle<'static> {
    unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) }
}
pub fn boot_identity() -> Result<Hash> {
    // User-mode SystemBootEnvironmentInformation: GUID, firmware type, boot flags.
    // No inferred/rebased clock origin or unauthenticated epoch claim.
    let mut data = [0u64; 4];
    let mut size = 0;
    let status = unsafe {
        windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation(
            90,
            data.as_mut_ptr().cast(),
            32,
            &mut size,
        )
    };
    if status < 0 {
        return Err(ProcessError::Nt(status));
    }
    let guid = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), 16) };
    if guid.iter().all(|b| *b == 0) {
        return Err(ProcessError::Policy("unknown boot GUID"));
    }
    Ok(digest(b"windows-boot-v1", guid))
}
pub fn sample_time() -> Result<(u64, u64)> {
    let c = crate::clock().map_err(|_| ProcessError::Policy("clock observation"))?;
    Ok((
        c.interrupt_100ns / 10_000,
        (c.utc_100ns / 10_000)
            .checked_sub(11_644_473_600_000)
            .ok_or(ProcessError::Policy("UTC before epoch"))?,
    ))
}
pub fn alive(handle: BorrowedHandle<'_>) -> bool {
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_TIMEOUT }
}
pub(crate) fn sid(sid: PSID) -> Result<String> {
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(ProcessError::Policy("SID"));
    }
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(error());
    }
    let mut n = 0;
    unsafe {
        while n < 256 && *text.add(n) != 0 {
            n += 1;
        }
    }
    let result = if n < 256 {
        String::from_utf16(unsafe { std::slice::from_raw_parts(text, n) })
            .map_err(|_| ProcessError::Policy("SID UTF16"))
    } else {
        Err(ProcessError::Policy("SID size"))
    };
    unsafe {
        LocalFree(text.cast());
    }
    result
}
fn token_data(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u64>> {
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(token, class, ptr::null_mut(), 0, &mut bytes);
    }
    if bytes == 0 || bytes > 65536 {
        return Err(ProcessError::Policy("token size"));
    }
    let mut data = vec![0u64; (bytes as usize).div_ceil(8)];
    if unsafe { GetTokenInformation(token, class, data.as_mut_ptr().cast(), bytes, &mut bytes) }
        == 0
    {
        return Err(error());
    }
    Ok(data)
}
pub fn measure_process(process: BorrowedHandle<'_>) -> Result<MeasuredProcess> {
    let raw = process.as_raw_handle();
    let pid = unsafe { GetProcessId(raw) };
    if pid == 0 {
        return Err(error());
    }
    let mut times = [FILETIME::default(); 4];
    if unsafe {
        GetProcessTimes(
            raw,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    } == 0
    {
        return Err(error());
    }
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(raw, TOKEN_QUERY, &mut token) } == 0 {
        return Err(error());
    }
    let token = owned(token)?;
    let user = token_data(token.as_raw_handle(), TokenUser)?;
    let user_sid = sid(unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid })?;
    let logon = token_data(token.as_raw_handle(), TokenLogonSid)?;
    let groups = unsafe { &*logon.as_ptr().cast::<TOKEN_GROUPS>() };
    if groups.GroupCount != 1
        || groups.Groups[0].Attributes & SE_GROUP_LOGON_ID != SE_GROUP_LOGON_ID
    {
        return Err(ProcessError::Policy("logon SID"));
    }
    let logon_sid = sid(groups.Groups[0].Sid)?;
    let label = token_data(token.as_raw_handle(), TokenIntegrityLevel)?;
    let s = unsafe { (*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
    if unsafe { IsValidSid(s) } == 0 {
        return Err(ProcessError::Policy("integrity SID"));
    }
    let count = unsafe { *GetSidSubAuthorityCount(s) };
    if count == 0 {
        return Err(ProcessError::Policy("integrity count"));
    }
    let integrity = unsafe { *GetSidSubAuthority(s, (count - 1) as u32) };
    let elevation = token_data(token.as_raw_handle(), TokenElevation)?;
    if unsafe { (*elevation.as_ptr().cast::<TOKEN_ELEVATION>()).TokenIsElevated } != 0
        || integrity >= 0x3000
    {
        return Err(ProcessError::Policy("elevated context unsupported"));
    }
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(raw, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err(error());
    }
    let path = PathBuf::from(
        String::from_utf16(&path[..length as usize])
            .map_err(|_| ProcessError::Policy("image path"))?,
    );
    let (_held, mut file) = crate::storage::pin_image(&path)?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(ProcessError::Policy("image size"));
    }
    let mut info = FILE_ID_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(error());
    }
    Ok(MeasuredProcess {
        process: autotune_recovery_contract::ProcessIdentity {
            pid,
            creation_100ns: ((times[0].dwHighDateTime as u64) << 32)
                | times[0].dwLowDateTime as u64,
            binary: crate::artifact_hash(b"executable-image-v1", &bytes)
                .map_err(|_| ProcessError::Policy("CNG image hash"))?,
            logon_sid,
            integrity,
        },
        user_sid,
        image: FileIdentity {
            volume: info.VolumeSerialNumber,
            id: info.FileId.Identifier,
        },
    })
}
pub fn current_identity() -> Result<MeasuredProcess> {
    measure_process(unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) })
}
pub fn verify_peer(handle: BorrowedHandle<'_>, expected: &MeasuredProcess) -> Result<()> {
    if !alive(handle) || measure_process(handle)? != *expected {
        return Err(ProcessError::Policy(
            "peer process/token/creation/image mismatch",
        ));
    }
    Ok(())
}
pub fn in_any_job(handle: BorrowedHandle<'_>) -> Result<bool> {
    let mut inside = 0;
    if unsafe { IsProcessInJob(handle.as_raw_handle(), ptr::null_mut(), &mut inside) } == 0 {
        return Err(error());
    }
    Ok(inside != 0)
}
pub fn console_state() -> Result<(bool, u64, Vec<u32>)> {
    let window = unsafe { GetConsoleWindow() };
    let mut list = [0u32; 32];
    unsafe {
        SetLastError(0);
    }
    let count = unsafe { GetConsoleProcessList(list.as_mut_ptr(), 32) };
    if count == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_INVALID_HANDLE {
            return Err(ProcessError::Win32(error));
        }
        if !window.is_null() || unsafe { GetConsoleCP() } != 0 {
            return Err(ProcessError::Policy("ambiguous console"));
        }
        return Ok((true, 0, vec![]));
    }
    if count > 32 {
        return Err(ProcessError::Policy("console inventory size"));
    }
    Ok((
        false,
        window as usize as u64,
        list[..count as usize].to_vec(),
    ))
}
pub fn require_independent() -> Result<()> {
    if in_any_job(unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) })?
        || !console_state()?.0
    {
        return Err(ProcessError::Policy(
            "guardian/worker console or job membership",
        ));
    }
    Ok(())
}
pub fn current_job_limits() -> Result<Option<u32>> {
    if !in_any_job(current_process_handle())? {
        return Ok(None);
    }
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    if unsafe {
        QueryInformationJobObject(
            ptr::null_mut(),
            JobObjectExtendedLimitInformation,
            (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(error());
    }
    Ok(Some(info.BasicLimitInformation.LimitFlags))
}

pub(crate) struct SecurityDescriptor(pub *mut c_void);
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
pub(crate) fn pipe_security(logon: &str, owner: &str) -> Result<SecurityDescriptor> {
    let text = wide(std::ffi::OsStr::new(&format!(
        "O:{owner}D:P(A;;0x00120183;;;{logon})"
    )))?;
    let mut p = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut p,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(error());
    }
    Ok(SecurityDescriptor(p))
}
pub(crate) fn duplicate(
    handle: BorrowedHandle<'_>,
    access: u32,
    inherit: bool,
) -> Result<OwnedHandle> {
    let mut h = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            GetCurrentProcess(),
            &mut h,
            access,
            inherit as i32,
            0,
        )
    } == 0
    {
        return Err(error());
    }
    owned(h)
}
#[repr(C, align(8))]
#[derive(Default)]
struct ObjectBasic {
    attributes: u32,
    access: u32,
    handles: u32,
    pointers: u32,
    reserved: [u32; 10],
}
pub fn validate_handle(handle: BorrowedHandle<'_>, kind: &str, access: u32) -> Result<()> {
    let mut basic = ObjectBasic::default();
    let mut n = 0;
    let status = unsafe {
        NtQueryObject(
            handle.as_raw_handle(),
            ObjectBasicInformation,
            (&mut basic as *mut ObjectBasic).cast(),
            size_of::<ObjectBasic>() as u32,
            &mut n,
        )
    };
    if status < 0 {
        return Err(ProcessError::Nt(status));
    }
    if basic.access != access {
        return Err(ProcessError::Policy("inherited rights mismatch"));
    }
    let mut buffer = vec![0u64; 8192];
    let status = unsafe {
        NtQueryObject(
            handle.as_raw_handle(),
            ObjectTypeInformation,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut n,
        )
    };
    if status < 0 {
        return Err(ProcessError::Nt(status));
    }
    let name = unsafe { &*buffer.as_ptr().cast::<UNICODE_STRING>() };
    let start = name.Buffer as usize;
    let base = buffer.as_ptr() as usize;
    if !name.Length.is_multiple_of(2)
        || start < base
        || start
            .checked_add(name.Length as usize)
            .is_none_or(|end| end > base + buffer.len() * 8)
    {
        return Err(ProcessError::Policy("object type bounds"));
    }
    if String::from_utf16(unsafe {
        std::slice::from_raw_parts(name.Buffer, name.Length as usize / 2)
    })
    .map_err(|_| ProcessError::Policy("object type"))?
        != kind
    {
        return Err(ProcessError::Policy("inherited object type"));
    }
    Ok(())
}
pub struct Executable {
    pub path: PathBuf,
    pub hash: Hash,
    _pins: Vec<File>,
    _file: File,
}
impl Executable {
    pub fn isolated_current_copy() -> Result<Arc<Self>> {
        let current = std::env::current_exe()?;
        let (_pins, mut file) = crate::storage::pin_image(&current)?;
        let (path, pins, mut file) = crate::storage::copy_role_image(&mut file)?;
        let mut data = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut data)?;
        Ok(Arc::new(Self {
            path,
            hash: crate::artifact_hash(b"executable-image-v1", &data)
                .map_err(|_| ProcessError::Policy("CNG image hash"))?,
            _pins: pins,
            _file: file,
        }))
    }
    pub fn current_role_image() -> Result<Arc<Self>> {
        let path = std::env::current_exe()?;
        let root = path
            .parent()
            .ok_or(ProcessError::Policy("role image parent"))?;
        if path.file_name() != Some(std::ffi::OsStr::new("role.exe")) {
            return Err(ProcessError::Policy("not fixed measured role image"));
        }
        let _root = crate::storage::TestRoot::open(root)?;
        let (pins, mut file) = crate::storage::pin_image(&path)?;
        let mut data = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut data)?;
        Ok(Arc::new(Self {
            path,
            hash: crate::artifact_hash(b"executable-image-v1", &data)
                .map_err(|_| ProcessError::Policy("CNG image hash"))?,
            _pins: pins,
            _file: file,
        }))
    }
}
pub struct OwnedJob {
    handle: OwnedHandle,
}
impl OwnedJob {
    pub fn new(allow_breakaway: bool) -> Result<Self> {
        let handle = owned(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | if allow_breakaway {
                JOB_OBJECT_LIMIT_BREAKAWAY_OK
            } else {
                0
            };
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(error());
        }
        Ok(Self { handle })
    }
    pub fn assign(&self, process: BorrowedHandle<'_>) -> Result<()> {
        if unsafe { AssignProcessToJobObject(self.handle.as_raw_handle(), process.as_raw_handle()) }
            == 0
        {
            return Err(error());
        }
        Ok(())
    }
    pub fn terminate(&self) -> Result<()> {
        if unsafe { TerminateJobObject(self.handle.as_raw_handle(), 2) } == 0 {
            return Err(error());
        }
        Ok(())
    }
    pub fn contains(&self, process: BorrowedHandle<'_>) -> Result<bool> {
        let mut yes = 0;
        if unsafe {
            IsProcessInJob(
                process.as_raw_handle(),
                self.handle.as_raw_handle(),
                &mut yes,
            )
        } == 0
        {
            return Err(error());
        }
        Ok(yes != 0)
    }
}
pub struct OwnedProcess {
    pub identity: MeasuredProcess,
    handle: OwnedHandle,
    thread: OwnedHandle,
    _image: Arc<Executable>,
}
pub fn close_owned_console(process: &OwnedProcess, status: &RoleStatus) -> Result<()> {
    if status.process != process.identity
        || !matches!(status.role, Role::Coordinator | Role::Verifier)
        || status.console_window == 0
        || status.console_processes != vec![process.identity.process.pid]
    {
        return Err(ProcessError::Policy(
            "console is not exclusively owned coordinator console",
        ));
    }
    if unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            status.console_window as usize as _,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            0,
            0,
        )
    } == 0
    {
        return Err(error());
    }
    if !process.wait(5000)? {
        return Err(ProcessError::Policy(
            "owned console host did not close coordinator",
        ));
    }
    Ok(())
}
impl OwnedProcess {
    pub fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
    pub fn alive(&self) -> bool {
        alive(self.handle.as_handle())
    }
    pub fn terminate_owned(&self) -> Result<()> {
        if unsafe { TerminateProcess(self.handle.as_raw_handle(), 2) } == 0 {
            return Err(error());
        }
        Ok(())
    }
    pub fn wait(&self, ms: u32) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), ms) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(error()),
        }
    }
    pub fn suspend_owned(&self) -> Result<()> {
        if unsafe { SuspendThread(self.thread.as_raw_handle()) } == u32::MAX {
            return Err(error());
        }
        Ok(())
    }
    pub fn resume_owned(&self) -> Result<()> {
        if unsafe { ResumeThread(self.thread.as_raw_handle()) } == u32::MAX {
            return Err(error());
        }
        Ok(())
    }
}
struct AttributeList {
    storage: Vec<usize>,
    initialized: bool,
}
impl AttributeList {
    fn new(handles: &mut [HANDLE]) -> Result<Self> {
        let mut n = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut n);
        }
        if n == 0 || n > 65536 {
            return Err(ProcessError::Policy("attribute list size"));
        }
        let mut a = Self {
            storage: vec![0; n.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(a.storage.as_mut_ptr().cast(), 1, 0, &mut n) }
            == 0
        {
            return Err(error());
        }
        a.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                a.storage.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(error());
        }
        Ok(a)
    }
}
impl Drop for AttributeList {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.storage.as_mut_ptr().cast());
            }
        }
    }
}
struct Mapping {
    handle: OwnedHandle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}
impl Mapping {
    fn new() -> Result<Self> {
        let handle = owned(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                BOOTSTRAP_BYTES as u32,
                ptr::null(),
            )
        })?;
        let view = unsafe {
            MapViewOfFile(
                handle.as_raw_handle(),
                FILE_MAP_WRITE,
                0,
                0,
                BOOTSTRAP_BYTES,
            )
        };
        if view.Value.is_null() {
            return Err(error());
        }
        Ok(Self { handle, view })
    }
    fn write(&mut self, bootstrap: &ProcessBootstrap) -> Result<()> {
        bootstrap.validate()?;
        let bytes = canonical(bootstrap)?;
        unsafe {
            ptr::write_unaligned(self.view.Value.cast::<u32>(), (bytes.len() as u32).to_le());
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.view.Value.cast::<u8>().add(4),
                bytes.len(),
            );
        }
        Ok(())
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}

pub struct Launch {
    pub process: OwnedProcess,
    pub bootstrap: ProcessBootstrap,
    pub server: crate::ipc::Server,
}
pub struct LaunchRequest<'a> {
    pub parent_role: Role,
    pub role: Role,
    pub session: Hash,
    pub profile: FakeProfile,
    pub root: Option<&'a Path>,
    pub custody: Option<&'a crate::storage::Custody>,
    pub job: Option<&'a OwnedJob>,
}
pub fn launch(image: Arc<Executable>, request: LaunchRequest<'_>) -> Result<Launch> {
    launch_impl(image, request, false)
}
/// Explicit console-closure diagnostic only. Guardian remains detached; its
/// bootstrap requests an owned console solely for the diagnostic coordinator.
pub fn launch_console_test(image: Arc<Executable>, request: LaunchRequest<'_>) -> Result<Launch> {
    launch_impl(image, request, true)
}
pub fn launch_verifier(
    image: Arc<Executable>,
    session: Hash,
    profile: FakeProfile,
    console: bool,
    job: Option<&OwnedJob>,
) -> Result<Launch> {
    launch_impl(
        image,
        LaunchRequest {
            parent_role: Role::Launcher,
            role: Role::Verifier,
            session,
            profile,
            root: None,
            custody: None,
            job,
        },
        console,
    )
}
fn launch_impl(
    image: Arc<Executable>,
    request: LaunchRequest<'_>,
    console_test: bool,
) -> Result<Launch> {
    let LaunchRequest {
        parent_role,
        role,
        session,
        profile,
        root,
        custody,
        job,
    } = request;
    if parent_role == Role::Guardian {
        require_independent()?;
    } else if parent_role != Role::Launcher || !matches!(role, Role::Guardian | Role::Verifier) {
        return Err(ProcessError::Policy("fixed role parent"));
    }
    let parent = current_identity()?;
    if parent.process.binary != image.hash {
        return Err(ProcessError::Policy("launch image changed"));
    }
    let server = crate::ipc::Server::new(&parent)?;
    let mut map = Mapping::new()?;
    let boot = duplicate(map.handle.as_handle(), BOOTSTRAP_ACCESS, true)?;
    let supervisor = duplicate(
        unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) },
        SUPERVISOR_ACCESS,
        true,
    )?;
    let lock = if let Some(c) = custody {
        Some(duplicate(c.as_handle(), CUSTODY_ACCESS, true)?)
    } else {
        None
    };
    let mut handles = vec![boot.as_raw_handle(), supervisor.as_raw_handle()];
    if let Some(lock) = &lock {
        handles.push(lock.as_raw_handle());
    }
    let mut attributes = AttributeList::new(&mut handles)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = boot.as_raw_handle();
    startup.lpAttributeList = attributes.storage.as_mut_ptr().cast();
    let name = match role {
        Role::Guardian => "guardian",
        Role::Coordinator => "coordinator",
        Role::Worker => "worker",
        Role::Verifier => "verifier",
        _ => return Err(ProcessError::Policy("child role")),
    };
    let application = wide(image.path.as_os_str())?;
    let mut command = wide(std::ffi::OsStr::new(&format!(
        "\"{}\" --internal-role {name}",
        image.path.display()
    )))?;
    let mut process = PROCESS_INFORMATION::default();
    let console_flags = console_creation_flags(role, console_test)?;
    let flags = CREATE_SUSPENDED
        | EXTENDED_STARTUPINFO_PRESENT
        | console_flags
        | if matches!(role, Role::Guardian | Role::Worker)
            && in_any_job(unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) })?
        {
            CREATE_BREAKAWAY_FROM_JOB
        } else {
            0
        };
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            flags,
            ptr::null(),
            ptr::null(),
            &startup.StartupInfo,
            &mut process,
        )
    } == 0
    {
        return Err(error());
    }
    let handle = owned(process.hProcess)?;
    let thread = owned(process.hThread)?;
    // Any pre-resume failure must terminate ONLY this suspended harness-owned process.
    let setup = (|| {
        if let Some(job) = job {
            job.assign(handle.as_handle())?;
        }
        if matches!(role, Role::Guardian | Role::Worker) && in_any_job(handle.as_handle())? {
            return Err(ProcessError::Policy("remaining nested job"));
        }
        let child = measure_process(handle.as_handle())?;
        let bootstrap = ProcessBootstrap {
            context: ChannelContext {
                version: PROCESS_VERSION,
                session,
                supervisor_role: parent_role,
                supervisor: parent,
                child_role: role,
                child: child.clone(),
            },
            handles: InheritedHandles {
                bootstrap_map: boot.as_raw_handle() as usize as u64,
                supervisor_query: supervisor.as_raw_handle() as usize as u64,
                custody: lock.as_ref().map(|h| CustodyHandle {
                    handle: h.as_raw_handle() as usize as u64,
                    identity: custody.unwrap().identity,
                }),
            },
            pipe: server.name.clone(),
            key: crate::random().map_err(|_| ProcessError::Policy("CNG RNG"))?,
            fixture_root: root.map(|p| p.to_string_lossy().into_owned()),
            console_test,
            profile,
        };
        map.write(&bootstrap)?;
        Ok((child, bootstrap))
    })();
    let (identity, bootstrap) = match setup {
        Ok(v) => v,
        Err(e) => {
            unsafe {
                TerminateProcess(handle.as_raw_handle(), 2);
                WaitForSingleObject(handle.as_raw_handle(), 5000);
            }
            return Err(e);
        }
    };
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        unsafe {
            TerminateProcess(handle.as_raw_handle(), 2);
        }
        return Err(error());
    }
    Ok(Launch {
        process: OwnedProcess {
            identity,
            handle,
            thread,
            _image: image,
        },
        bootstrap,
        server,
    })
}
fn console_creation_flags(role: Role, console_test: bool) -> Result<u32> {
    match (role, console_test) {
        (Role::Guardian | Role::Worker | Role::Verifier | Role::Coordinator, false)
        | (Role::Guardian, true) => Ok(DETACHED_PROCESS),
        (Role::Coordinator | Role::Verifier, true) => Ok(CREATE_NEW_CONSOLE),
        _ => Err(ProcessError::Policy("console launch role")),
    }
}

pub struct ChildBootstrap {
    pub envelope: ProcessBootstrap,
    pub supervisor: OwnedHandle,
    pub custody: Option<OwnedHandle>,
}
pub fn inherited_bootstrap(role: Role) -> Result<ChildBootstrap> {
    let raw = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let section = owned(raw)?;
    validate_handle(section.as_handle(), "Section", BOOTSTRAP_ACCESS)?;
    let view = unsafe {
        MapViewOfFile(
            section.as_raw_handle(),
            FILE_MAP_READ,
            0,
            0,
            BOOTSTRAP_BYTES,
        )
    };
    if view.Value.is_null() {
        return Err(error());
    }
    let n = unsafe { u32::from_le(ptr::read_unaligned(view.Value.cast::<u32>())) as usize };
    let result = if n > BOOTSTRAP_LIMIT {
        Err(ProcessError::Policy("bootstrap length"))
    } else {
        decode::<ProcessBootstrap>(
            unsafe { std::slice::from_raw_parts(view.Value.cast::<u8>().add(4), n) },
            BOOTSTRAP_LIMIT,
        )
        .map_err(ProcessError::from)
    };
    unsafe {
        UnmapViewOfFile(view);
    }
    let envelope = result?;
    envelope.validate()?;
    if envelope.context.child_role != role
        || envelope.handles.bootstrap_map != raw as usize as u64
        || current_identity()? != envelope.context.child
    {
        return Err(ProcessError::Policy("bootstrap self identity"));
    }
    let supervisor = owned(envelope.handles.supervisor_query as usize as HANDLE)?;
    validate_handle(supervisor.as_handle(), "Process", SUPERVISOR_ACCESS)?;
    verify_peer(supervisor.as_handle(), &envelope.context.supervisor)?;
    let custody = if let Some(c) = &envelope.handles.custody {
        let h = owned(c.handle as usize as HANDLE)?;
        validate_handle(h.as_handle(), "File", CUSTODY_ACCESS)?;
        crate::storage::attest_custody(h.as_handle(), c.identity)?;
        Some(h)
    } else {
        None
    };
    for raw in [
        Some(supervisor.as_raw_handle()),
        custody.as_ref().map(AsRawHandle::as_raw_handle),
    ]
    .into_iter()
    .flatten()
    {
        if unsafe { SetHandleInformation(raw, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(error());
        }
    }
    if matches!(role, Role::Guardian | Role::Worker) {
        require_independent()?;
    }
    Ok(ChildBootstrap {
        envelope,
        supervisor,
        custody,
    })
}

#[cfg(test)]
mod quiet_launch_tests {
    use super::*;

    #[test]
    fn ordinary_roles_never_create_a_console_and_independent_roles_stay_detached() {
        for role in [
            Role::Guardian,
            Role::Worker,
            Role::Coordinator,
            Role::Verifier,
        ] {
            let flags = console_creation_flags(role, false).unwrap();
            assert_eq!(flags & CREATE_NEW_CONSOLE, 0);
            assert_eq!(flags, DETACHED_PROCESS);
        }
    }

    #[test]
    fn only_explicit_diagnostic_roles_may_get_a_console() {
        assert_eq!(
            console_creation_flags(Role::Guardian, true).unwrap(),
            DETACHED_PROCESS
        );
        for role in [Role::Coordinator, Role::Verifier] {
            assert_eq!(
                console_creation_flags(role, true).unwrap(),
                CREATE_NEW_CONSOLE
            );
        }
        assert!(console_creation_flags(Role::Worker, true).is_err());
        assert!(console_creation_flags(Role::Launcher, true).is_err());
    }
}
