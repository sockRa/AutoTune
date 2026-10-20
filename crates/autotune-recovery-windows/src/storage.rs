//! Isolated test-store port. This module cannot provision or accept a production root.
//! All namespace opens after pinning are single-component NtCreateFile opens.
//! ReplaceFileW needs paths: outer ancestry denies write/delete sharing; the
//! protected fixture root denies delete sharing and excludes untrusted writers by DACL.
use autotune_recovery_contract::{
    canonical, decode, storage::*, Rejection, ARTIFACT_LIMIT, SNAPSHOT_LIMIT,
};
use std::os::windows::fs::OpenOptionsExt;
use std::{
    ffi::c_void,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf},
    ptr,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{
        Foundation::{
            CompareObjectHandles, DuplicateHandle, GetLastError, LocalFree, GENERIC_READ,
            GENERIC_WRITE, HANDLE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, UNICODE_STRING,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SE_FILE_OBJECT,
            },
            GetAce, GetSecurityDescriptorControl, GetTokenInformation, IsValidSecurityDescriptor,
            IsValidSid, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION,
            OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::*,
        System::{
            Threading::{GetCurrentProcess, OpenProcessToken},
            IO::IO_STATUS_BLOCK,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    ReadOnlyReconciliation,
    IoKind(std::io::ErrorKind),
    Win32(u32),
    Nt(i32),
    Invalid(&'static str),
    Contract(Rejection),
    Poisoned,
    Busy,
    Timeout,
}
pub type Result<T> = std::result::Result<T, StorageError>;
impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        match e.raw_os_error() {
            Some(code) => Self::Win32(code as u32),
            None => Self::IoKind(e.kind()),
        }
    }
}
impl From<Rejection> for StorageError {
    fn from(e: Rejection) -> Self {
        Self::Contract(e)
    }
}
fn last_error() -> StorageError {
    // SAFETY: obtains the current thread's last Win32 error, immediately after a failed call.
    StorageError::Win32(unsafe { GetLastError() })
}
fn wide(s: &std::ffi::OsStr) -> Result<Vec<u16>> {
    let mut v: Vec<u16> = s.encode_wide().collect();
    if v.contains(&0) {
        return Err(StorageError::Invalid("NUL path"));
    }
    v.push(0);
    Ok(v)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn safe_leaf(leaf: &str) -> Result<()> {
    if leaf.is_empty()
        || leaf.len() > 180
        || leaf == "."
        || leaf == ".."
        || !leaf
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        || leaf.ends_with('.')
    {
        return Err(StorageError::Invalid("unsafe leaf"));
    }
    Ok(())
}
struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: these buffers are allocated by the documented LocalAlloc-family APIs.
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn sid_text(sid: *mut c_void) -> Result<String> {
    // SAFETY: only called with SIDs from validated OS-owned token/security buffers.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(StorageError::Invalid("invalid SID"));
    }
    let mut text = ptr::null_mut();
    // SAFETY: OS validates SID and allocates a NUL-terminated Unicode string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last_error());
    }
    let _allocation = LocalAllocation(text.cast());
    let mut length = 0;
    // SAFETY: ConvertSidToStringSidW returns a terminated allocation; bound is defensive.
    unsafe {
        while length < 256 && *text.add(length) != 0 {
            length += 1;
        }
        if length == 256 {
            return Err(StorageError::Invalid("SID string length"));
        }
        String::from_utf16(std::slice::from_raw_parts(text, length))
            .map_err(|_| StorageError::Invalid("SID encoding"))
    }
}
struct Security {
    descriptor: LocalAllocation,
    operator: String,
}
#[derive(Debug, Clone)]
pub struct AncestorSecurity {
    pub owner_sid: String,
    pub dacl_protected: bool,
    pub ace_count: u16,
}
/// Outer scratch/OS ancestry is observed, NOT promoted to a provisioned common root.
/// Its ACL may permit sibling creation. Non-write/non-delete-shared held handles,
/// rather than those permissive grants, protect the existing component chain.
fn observe_ancestor(file: &File) -> Result<AncestorSecurity> {
    let mut owner = ptr::null_mut();
    let mut acl: *mut ACL = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: live read-control directory handle; all returned pointers belong to descriptor.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(StorageError::Win32(status));
    }
    let _allocation = LocalAllocation(descriptor);
    // SAFETY: validate OS-provided descriptor before inspecting the DACL header.
    if unsafe { IsValidSecurityDescriptor(descriptor) } == 0 || acl.is_null() {
        return Err(StorageError::Invalid("invalid ancestor security"));
    }
    let owner_sid = sid_text(owner)?;
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: validated live descriptor and valid output buffers.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
        return Err(last_error());
    }
    // SAFETY: non-null ACL is inside the validated descriptor allocation.
    let ace_count = unsafe { (*acl).AceCount };
    Ok(AncestorSecurity {
        owner_sid,
        dacl_protected: control & SE_DACL_PROTECTED != 0,
        ace_count,
    })
}
impl Security {
    fn current() -> Result<Self> {
        let mut token = ptr::null_mut();
        // SAFETY: process pseudo-handle valid; token is returned as a new owned handle.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error());
        }
        // SAFETY: successful OpenProcessToken transfers one handle to us.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut bytes = 0;
        // SAFETY: sizing query with null buffer and zero length.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut bytes,
            );
        }
        if bytes == 0 || bytes > 65536 {
            return Err(StorageError::Invalid("token size"));
        }
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        // SAFETY: aligned allocation covers bytes; TOKEN_USER remains valid during sid_text.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(last_error());
        }
        // SAFETY: successful token query produced a TOKEN_USER in aligned storage.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let operator = sid_text(user.User.Sid)?;
        let sddl =
            format!("O:{operator}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{operator})");
        let sddl = wide(std::ffi::OsStr::new(&sddl))?;
        let mut descriptor = ptr::null_mut();
        // SAFETY: valid NUL-terminated SDDL; returns a LocalFree-owned descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(Self {
            descriptor: LocalAllocation(descriptor),
            operator,
        })
    }
    fn audit(&self, file: &File) -> Result<()> {
        let mut owner = ptr::null_mut();
        let mut acl: *mut ACL = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: handle is live; OS returns a single descriptor allocation holding owner/DACL.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            return Err(StorageError::Win32(status));
        }
        let _allocation = LocalAllocation(descriptor);
        // SAFETY: OS returned descriptor; validate before inspecting its ACL.
        if unsafe { IsValidSecurityDescriptor(descriptor) } == 0 || acl.is_null() {
            return Err(StorageError::Invalid("null/invalid DACL"));
        }
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: validated descriptor and valid output storage.
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(last_error());
        }
        if control & SE_DACL_PROTECTED == 0 || sid_text(owner)? != self.operator {
            return Err(StorageError::Invalid("owner or unprotected DACL"));
        }
        // Strict allowlist, not a heuristic broad-SID denylist: reject deny, object,
        // callback, inherited, unknown or extra-principal ACEs, even if only read access.
        // SYSTEM/admin/operator are explicitly inside the fixture trust boundary.
        let allowed = ["S-1-5-18", "S-1-5-32-544", self.operator.as_str()];
        let mut seen = [false; 3];
        // SAFETY: ACL is inside validated descriptor, alive through the entire loop.
        let count = unsafe { (*acl).AceCount };
        if count != 3 {
            return Err(StorageError::Invalid("unexpected ACE count"));
        }
        for i in 0..count {
            let mut ace = ptr::null_mut();
            // SAFETY: index is within validated ACL.
            if unsafe { GetAce(acl, i as u32, &mut ace) } == 0 {
                return Err(last_error());
            }
            // SAFETY: GetAce guarantees a header. Check type/size before forming the
            // larger typed reference (an unknown ACE need not be ACCESS_ALLOWED_ACE-sized).
            let header = unsafe { ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
            if header.AceType != 0 || header.AceFlags & !0x03 != 0 || header.AceSize < 20 {
                return Err(StorageError::Invalid("unexpected ACE header"));
            }
            // SAFETY: validated ACL and allow-ACE type/size establish the struct extent.
            let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let sid_length = 8 + 4 * ((ace.SidStart >> 8) & 255) as usize;
            if offset_of!(ACCESS_ALLOWED_ACE, SidStart) + sid_length > header.AceSize as usize
                || ace.Mask != FILE_ALL_ACCESS
            {
                return Err(StorageError::Invalid("unexpected ACE"));
            }
            let sid = sid_text((&ace.SidStart as *const u32).cast_mut().cast())?;
            let index = allowed
                .iter()
                .position(|s| *s == sid)
                .ok_or(StorageError::Invalid("untrusted ACL principal"))?;
            if seen[index] {
                return Err(StorageError::Invalid("duplicate ACE"));
            }
            seen[index] = true;
        }
        if seen != [true; 3] {
            return Err(StorageError::Invalid("missing ACE"));
        }
        Ok(())
    }
}

pub use autotune_recovery_contract::storage::FileIdentity;
fn information<T: Default>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS) -> Result<T> {
    let mut info = T::default();
    // SAFETY: private callers pair each POD Windows struct with its documented information class.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            class,
            (&mut info as *mut T).cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(info)
}
fn identity(file: &File) -> Result<FileIdentity> {
    let id: FILE_ID_INFO = information(file, FileIdInfo)?;
    Ok(FileIdentity {
        volume: id.VolumeSerialNumber,
        id: id.FileId.Identifier,
    })
}
fn audit_kind(file: &File, directory: bool, volume: Option<u64>) -> Result<FileIdentity> {
    let tags: FILE_ATTRIBUTE_TAG_INFO = information(file, FileAttributeTagInfo)?;
    let standard: FILE_STANDARD_INFO = information(file, FileStandardInfo)?;
    let id = identity(file)?;
    if tags.FileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DEVICE) != 0
        || standard.Directory != directory
        || standard.DeletePending
        || (!directory && standard.NumberOfLinks != 1)
        || volume.is_some_and(|v| v != id.volume)
    {
        return Err(StorageError::Invalid(
            "reparse/type/hardlink/volume/identity",
        ));
    }
    Ok(id)
}
fn audit_streams(file: &File, directory: bool) -> Result<()> {
    let mut buffer = vec![0u64; 8192];
    // SAFETY: aligned 64-KiB metadata buffer and live file/directory handle. Named
    // streams are checked by handle; rejecting ':' in supplied names is not enough.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStreamInfo,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
        )
    };
    if ok == 0 {
        // SAFETY: read error immediately. A directory without data streams can report EOF.
        let error = unsafe { GetLastError() };
        if directory && error == 38 {
            return Ok(());
        }
        return Err(StorageError::Win32(error));
    }
    // SAFETY: output allocation is aligned and larger than FILE_STREAM_INFO.
    let entry = unsafe { ptr::read_unaligned(buffer.as_ptr().cast::<FILE_STREAM_INFO>()) };
    if directory && entry.NextEntryOffset == 0 && entry.StreamNameLength == 0 {
        return Ok(());
    }
    if entry.NextEntryOffset != 0 || entry.StreamNameLength != 14 {
        return Err(StorageError::Invalid("unexpected alternate streams"));
    }
    // SAFETY: exactly seven UTF-16 code units fit in the metadata buffer after its header.
    let name = unsafe {
        std::slice::from_raw_parts(
            buffer
                .as_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_STREAM_INFO, StreamName))
                .cast::<u16>(),
            7,
        )
    };
    if name != [58, 58, 36, 68, 65, 84, 65] {
        return Err(StorageError::Invalid("unexpected alternate streams"));
    }
    Ok(())
}
fn nt_open(
    parent: Option<&File>,
    name: &std::ffi::OsStr,
    directory: bool,
    create: bool,
    exclusive: bool,
    security: Option<&Security>,
) -> Result<File> {
    let wide: Vec<u16> = name.encode_wide().collect();
    if wide.is_empty() || wide.len() > 16000 || wide.contains(&0) {
        return Err(StorageError::Invalid("NT name"));
    }
    let mut name = UNICODE_STRING {
        Length: (wide.len() * 2) as u16,
        MaximumLength: (wide.len() * 2) as u16,
        Buffer: wide.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.map_or(ptr::null_mut(), AsRawHandle::as_raw_handle),
        ObjectName: &mut name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security.map_or(ptr::null_mut(), |s| s.descriptor.0.cast()),
        SecurityQualityOfService: ptr::null_mut(),
    };
    let mut handle = ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    let access = SYNCHRONIZE
        | GENERIC_READ
        | READ_CONTROL
        | if !directory && (create || exclusive || security.is_some()) {
            GENERIC_WRITE
        } else {
            0
        };
    let sharing = if exclusive {
        0
    } else if directory {
        // Protected fixture root must permit ReplaceFileW's parent-directory access.
        // Untrusted outer ancestors remain non-write/non-delete shared.
        FILE_SHARE_READ
            | if security.is_some() {
                FILE_SHARE_WRITE
            } else {
                0
            }
    } else {
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    };
    // SAFETY: borrowed root handle and name/SD buffers live through synchronous NtCreateFile;
    // options force type, disable reparse following and use a synchronous non-alertable handle.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            ptr::null(),
            if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            },
            sharing,
            if create { FILE_CREATE } else { FILE_OPEN },
            FILE_SYNCHRONOUS_IO_NONALERT
                | FILE_OPEN_REPARSE_POINT
                | if directory {
                    FILE_DIRECTORY_FILE
                } else {
                    FILE_NON_DIRECTORY_FILE
                },
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        return Err(StorageError::Nt(result));
    }
    // SAFETY: successful FILE_OPEN/CREATE transfers ownership of this synchronous file handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Test policy: root must be a direct recovery-storage-* child of the preapproved
/// scratch directory. All ancestors are opened component-wise and held without
/// write/delete sharing. Their ACLs are NOT claimed to provision a production trust
/// boundary; the unique protected fixture anchor and all its objects are audited.
pub struct TestRoot {
    path: PathBuf,
    ancestors: Vec<(File, FileIdentity)>,
    directory: File,
    id: FileIdentity,
    security: Security,
}
// Security descriptor is immutable LocalAlloc memory, exclusively owned by TestRoot.
// SAFETY: the pointed-to descriptor has no thread affinity and is freed exactly once.
unsafe impl Send for Security {}
impl TestRoot {
    pub fn approved_parent() -> Result<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or(StorageError::Invalid("missing approved scratch path"))?;
        Ok(PathBuf::from(local).join("Temp").join("opencode"))
    }
    fn ancestry(path: &Path) -> Result<Vec<(File, FileIdentity)>> {
        let components: Vec<_> = path.components().collect();
        let drive = match components.first() {
            Some(Component::Prefix(p)) => match p.kind() {
                std::path::Prefix::Disk(d) if d.is_ascii_alphabetic() => d as char,
                _ => return Err(StorageError::Invalid("non-local drive")),
            },
            _ => return Err(StorageError::Invalid("absolute local path required")),
        };
        if !matches!(components.get(1), Some(Component::RootDir)) {
            return Err(StorageError::Invalid("rooted drive required"));
        }
        let drive_path = format!("{drive}:\\");
        let drive_wide = wide(std::ffi::OsStr::new(&drive_path))?;
        // SAFETY: NUL-terminated local drive root, read-only query.
        if unsafe { GetDriveTypeW(drive_wide.as_ptr()) } != 3 {
            return Err(StorageError::Invalid("non-fixed drive"));
        }
        let root = nt_open(
            None,
            std::ffi::OsStr::new(&format!("\\??\\{drive}:\\")),
            true,
            false,
            false,
            None,
        )?;
        let id = audit_kind(&root, true, None)?;
        observe_ancestor(&root)?;
        let mut fs = [0u16; 32];
        // SAFETY: live directory handle and appropriately sized filesystem-name buffer.
        if unsafe {
            GetVolumeInformationByHandleW(
                root.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } == 0
        {
            return Err(last_error());
        }
        if fs[..5] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0] {
            return Err(StorageError::Invalid("NTFS required"));
        }
        let mut held = vec![(root, id)];
        for part in &components[2..] {
            let Component::Normal(leaf) = part else {
                return Err(StorageError::Invalid("path component"));
            };
            let text = leaf
                .to_str()
                .ok_or(StorageError::Invalid("path encoding"))?;
            if text.contains([':', '\\', '/']) || text.ends_with(['.', ' ']) {
                return Err(StorageError::Invalid("path component"));
            }
            let child = nt_open(
                Some(&held.last().unwrap().0),
                leaf,
                true,
                false,
                false,
                None,
            )?;
            let child_id = audit_kind(&child, true, Some(id.volume))?;
            observe_ancestor(&child)?;
            held.push((child, child_id));
        }
        Ok(held)
    }
    pub fn provision_fixture() -> Result<PathBuf> {
        let parent = Self::approved_parent()?;
        let held = Self::ancestry(&parent)?;
        let security = Security::current()?;
        let name = format!(
            "recovery-storage-{}",
            hex(&super::random().map_err(|_| StorageError::Invalid("CNG RNG"))?)
        );
        let directory = nt_open(
            Some(&held.last().unwrap().0),
            std::ffi::OsStr::new(&name),
            true,
            true,
            false,
            Some(&security),
        )?;
        let id = audit_kind(&directory, true, Some(held[0].1.volume))?;
        audit_streams(&directory, true)?;
        security.audit(&directory)?;
        let root = Self {
            path: parent.join(name),
            ancestors: held,
            directory,
            id,
            security,
        };
        root.create("owner.lock")?.sync_all()?;
        let anchor = JournalEnvelope::new(JournalPayload::Anchor {
            version: STORAGE_VERSION,
            store: super::random().map_err(|_| StorageError::Invalid("CNG RNG"))?,
        })?;
        let mut journal = root.create("journal.json")?;
        journal.write_all(&canonical(&anchor)?)?;
        journal.sync_all()?;
        let path = root.path.clone();
        drop(journal);
        drop(root);
        Ok(path)
    }
    pub fn open(path: &Path) -> Result<Self> {
        let parent = Self::approved_parent()?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(StorageError::Invalid("fixture leaf"))?;
        safe_leaf(name)?;
        if path.parent() != Some(parent.as_path())
            || !name.starts_with("recovery-storage-")
            || name.len() != "recovery-storage-".len() + 64
        {
            return Err(StorageError::Invalid("not approved isolated fixture"));
        }
        let ancestors = Self::ancestry(&parent)?;
        let security = Security::current()?;
        let directory = nt_open(
            Some(
                &ancestors
                    .last()
                    .ok_or(StorageError::Invalid("missing anchor"))?
                    .0,
            ),
            std::ffi::OsStr::new(name),
            true,
            false,
            false,
            Some(&security),
        )?;
        let id = audit_kind(&directory, true, Some(ancestors[0].1.volume))?;
        audit_streams(&directory, true)?;
        security.audit(&directory)?;
        Ok(Self {
            path: path.into(),
            ancestors,
            directory,
            id,
            security,
        })
    }
    fn revalidate(&self) -> Result<()> {
        for (file, id) in &self.ancestors {
            if audit_kind(file, true, Some(self.id.volume))? != *id {
                return Err(StorageError::Invalid("ancestor identity drift"));
            }
        }
        if audit_kind(&self.directory, true, Some(self.id.volume))? != self.id {
            return Err(StorageError::Invalid("root identity drift"));
        }
        audit_streams(&self.directory, true)?;
        self.security.audit(&self.directory)
    }
    pub fn ancestry_security(&self) -> Result<Vec<AncestorSecurity>> {
        self.ancestors
            .iter()
            .map(|(f, _)| observe_ancestor(f))
            .collect()
    }
    fn leaf(&self, name: &str, create: bool, exclusive: bool) -> Result<File> {
        safe_leaf(name)?;
        self.revalidate()?;
        let file = nt_open(
            Some(&self.directory),
            std::ffi::OsStr::new(name),
            false,
            create,
            exclusive,
            create.then_some(&self.security),
        )?;
        audit_kind(&file, false, Some(self.id.volume))?;
        audit_streams(&file, false)?;
        self.security.audit(&file)?;
        Ok(file)
    }
    fn create(&self, name: &str) -> Result<File> {
        self.leaf(name, true, false)
    }
    fn writable_leaf(&self, name: &str) -> Result<File> {
        safe_leaf(name)?;
        self.revalidate()?;
        let file = nt_open(
            Some(&self.directory),
            std::ffi::OsStr::new(name),
            false,
            false,
            false,
            Some(&self.security),
        )?;
        audit_kind(&file, false, Some(self.id.volume))?;
        audit_streams(&file, false)?;
        self.security.audit(&file)?;
        Ok(file)
    }
    fn read(&self, name: &str) -> Result<(FileIdentity, Vec<u8>)> {
        let mut file = self.leaf(name, false, false)?;
        let id = identity(&file)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((SNAPSHOT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > SNAPSHOT_LIMIT || identity(&file)? != id {
            return Err(StorageError::Invalid("read limit/identity"));
        }
        Ok((id, bytes))
    }
    fn entries(&self) -> Result<Vec<String>> {
        self.revalidate()?;
        let mut names = Vec::new();
        let mut restart = true;
        loop {
            let mut buffer = vec![0u64; 8192];
            // SAFETY: aligned 64-KiB output buffer and live synchronous directory handle.
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    self.directory.as_raw_handle(),
                    if restart {
                        FileIdBothDirectoryRestartInfo
                    } else {
                        FileIdBothDirectoryInfo
                    },
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                )
            };
            if ok == 0 {
                // SAFETY: immediately obtains the failing call's error; 18 means end of scan.
                let error = unsafe { GetLastError() };
                if error == 18 {
                    break;
                }
                return Err(StorageError::Win32(error));
            }
            restart = false;
            let size = buffer.len() * 8;
            let mut offset = 0;
            loop {
                let header_size = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > size {
                    return Err(StorageError::Invalid("directory header"));
                }
                // SAFETY: bounds checked, unaligned read avoids relying on entry alignment.
                let entry = unsafe {
                    ptr::read_unaligned(
                        buffer
                            .as_ptr()
                            .cast::<u8>()
                            .add(offset)
                            .cast::<FILE_ID_BOTH_DIR_INFO>(),
                    )
                };
                let length = entry.FileNameLength as usize;
                let span = if entry.NextEntryOffset == 0 {
                    size - offset
                } else {
                    entry.NextEntryOffset as usize
                };
                if !length.is_multiple_of(2)
                    || header_size + length > span
                    || offset + span > size
                    || span < header_size + 2
                    || (entry.NextEntryOffset != 0 && !span.is_multiple_of(8))
                {
                    return Err(StorageError::Invalid("directory entry"));
                }
                // SAFETY: checked name bounds, buffer and offsets are at least u16-aligned.
                let text = unsafe {
                    std::slice::from_raw_parts(
                        buffer
                            .as_ptr()
                            .cast::<u8>()
                            .add(offset + header_size)
                            .cast::<u16>(),
                        length / 2,
                    )
                };
                let name = String::from_utf16(text)
                    .map_err(|_| StorageError::Invalid("directory encoding"))?;
                if name != "." && name != ".." {
                    safe_leaf(&name)?;
                    names.push(name);
                }
                if names.len() > ARTIFACT_LIMIT {
                    return Err(StorageError::Invalid("artifact capacity"));
                }
                if entry.NextEntryOffset == 0 {
                    break;
                }
                offset += span;
            }
        }
        names.sort();
        if names.windows(2).any(|w| w[0].eq_ignore_ascii_case(&w[1])) {
            return Err(StorageError::Invalid("alias entries"));
        }
        Ok(names)
    }
}

pub struct Custody {
    handle: OwnedHandle,
    pub identity: FileIdentity,
}
impl Custody {
    pub fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}
pub struct RemoteCustody {
    pub handle: usize,
    pub identity: FileIdentity,
}
/// Identity/ACL attestation of a TRUSTED duplicated handle. File ID alone is not
/// bootstrap authentication; the later process port must bind the actual handoff.
pub fn attest_custody(handle: BorrowedHandle<'_>, expected: FileIdentity) -> Result<()> {
    let file = File::from(handle.try_clone_to_owned()?);
    if audit_kind(&file, false, Some(expected.volume))? != expected {
        return Err(StorageError::Invalid("custody identity"));
    }
    Security::current()?.audit(&file)
}
fn duplicate(source: &File, target: HANDLE) -> Result<HANDLE> {
    let mut result = ptr::null_mut();
    // SAFETY: source is live, target is a borrowed process handle with DUP_HANDLE rights;
    // deliberately non-inheritable and restricted to query/synchronize rights, no lock I/O.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source.as_raw_handle(),
            target,
            &mut result,
            FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            0,
            0,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(result)
}
#[derive(Debug, Clone)]
pub struct Scan {
    pub active: JournalEnvelope,
    pub active_identity: FileIdentity,
    pub artifacts: usize,
    pub read_only_required: bool,
}
pub struct Store {
    root: TestRoot,
    lock: File,
    current: Scan,
    poisoned: bool,
    reconciliation_only: bool,
}
impl Store {
    pub(crate) fn pin_ancestry(&self) -> Result<Vec<File>> {
        self.root
            .ancestors
            .iter()
            .map(|(f, _)| f.try_clone().map_err(StorageError::from))
            .chain(std::iter::once(
                self.root.directory.try_clone().map_err(StorageError::from),
            ))
            .collect()
    }
    pub fn ancestry_security(&self) -> Result<Vec<AncestorSecurity>> {
        self.root.ancestry_security()
    }
    /// Startup never initializes, repairs, removes, replays or chooses a convenient backup.
    pub fn open(path: &Path) -> Result<Self> {
        let root = TestRoot::open(path)?;
        let lock = root.leaf("owner.lock", false, true)?;
        let current = scan(&root)?;
        // A cold scanner cannot prove the previous actor received an on-time terminal
        // acknowledgement. Even a structurally clean terminal may be a late write from
        // a timed-out publisher. ALL recovered history is read-only, not restart authority.
        // Clean session rollover is supported only within this original ownership lifetime.
        let reconciliation_only = matches!(current.active.payload, JournalPayload::Snapshot(_));
        Ok(Self {
            root,
            lock,
            current,
            poisoned: false,
            reconciliation_only,
        })
    }
    pub fn scan(&self) -> &Scan {
        &self.current
    }
    pub fn reconciliation_only(&self) -> bool {
        self.reconciliation_only
    }
    pub fn duplicate_custody(&self) -> Result<Custody> {
        // SAFETY: current-process pseudo handle is valid and requires no closing.
        let handle = duplicate(&self.lock, unsafe { GetCurrentProcess() })?;
        // SAFETY: DuplicateHandle created one owned handle in THIS process.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(Custody {
            handle,
            identity: identity(&self.lock)?,
        })
    }
    pub fn duplicate_custody_into(&self, process: BorrowedHandle<'_>) -> Result<RemoteCustody> {
        let handle = duplicate(&self.lock, process.as_raw_handle())?;
        // The target process owns this value; never CloseHandle it in the source process.
        Ok(RemoteCustody {
            handle: handle as usize,
            identity: identity(&self.lock)?,
        })
    }
    pub fn same_lock_object(&self, custody: &Custody) -> bool {
        // SAFETY: both handles are live in this process; CompareObjectHandles compares objects, not IDs.
        unsafe {
            CompareObjectHandles(self.lock.as_raw_handle(), custody.handle.as_raw_handle()) != 0
        }
    }
    pub fn publish(&mut self, next: JournalEnvelope) -> Result<Scan> {
        self.publish_with(next, |_| Ok(()))
    }
    pub fn publish_with(
        &mut self,
        next: JournalEnvelope,
        mut barrier: impl FnMut(PublicationStep) -> Result<()>,
    ) -> Result<Scan> {
        if self.reconciliation_only {
            return Err(StorageError::ReadOnlyReconciliation);
        }
        if self.poisoned {
            return Err(StorageError::Poisoned);
        }
        next.follows(&self.current.active)?;
        // Reserve room for stage + retained backup BEFORE touching the namespace.
        if self
            .current
            .artifacts
            .checked_add(2)
            .is_none_or(|n| n > ARTIFACT_LIMIT)
        {
            return Err(StorageError::Invalid("artifact capacity"));
        }
        let outcome = self.publication(next, &mut barrier);
        if outcome.is_err() {
            self.poisoned = true;
        }
        if let Ok(scan) = &outcome {
            self.current = scan.clone();
        }
        outcome
    }
    fn active_matches(&self) -> Result<()> {
        self.root.revalidate()?;
        let (id, bytes) = self.root.read("journal.json")?;
        let active: JournalEnvelope = decode(&bytes, SNAPSHOT_LIMIT)?;
        if id != self.current.active_identity || active != self.current.active {
            return Err(StorageError::Invalid("unexpected active replacement"));
        }
        Ok(())
    }
    fn publication(
        &mut self,
        next: JournalEnvelope,
        barrier: &mut impl FnMut(PublicationStep) -> Result<()>,
    ) -> Result<Scan> {
        self.root.revalidate()?;
        // In-session validation uses the already validated active chain. A cold
        // open still scans every artifact; this avoids O(history) work per call
        // while the original live owner holds the lock and ancestry pins.
        self.active_matches()?;
        let stage = format!(
            "stage-{:020}-{}.json",
            next.sequence(),
            hex(&super::random().map_err(|_| StorageError::Invalid("CNG RNG"))?)
        );
        let backup = backup_name(&self.current.active);
        let bytes = canonical(&next)?;
        let mut file = self.root.create(&stage)?;
        barrier(PublicationStep::StageCreated)?;
        // A separate midpoint enables actual torn-file/crash tests, not only labels.
        let midpoint = bytes.len() / 2;
        file.write_all(&bytes[..midpoint])?;
        barrier(PublicationStep::PartialWrite)?;
        file.write_all(&bytes[midpoint..])?;
        barrier(PublicationStep::Written)?;
        file.sync_all()?;
        let stage_id = audit_kind(&file, false, Some(self.root.id.volume))?;
        self.root.security.audit(&file)?;
        barrier(PublicationStep::FirstFlush)?;
        drop(file); // close stage and ALL leaf handles before ReplaceFileW
        let (measured, stage_bytes) = self.root.read(&stage)?;
        if measured != stage_id || stage_bytes != bytes {
            return Err(StorageError::Invalid("stage identity/bytes"));
        }
        self.root.revalidate()?;
        barrier(PublicationStep::BeforeReplace)?;
        let active_path = wide(self.root.path.join("journal.json").as_os_str())?;
        let stage_path = wide(self.root.path.join(&stage).as_os_str())?;
        let backup_path = wide(self.root.path.join(&backup).as_os_str())?;
        // SAFETY: non-delete-shared ancestry pins names; outer ancestors also deny write
        // sharing and the write-shared fixture root has a strict protected DACL. Leaves
        // are internal safe names, stage/active IDs are measured. Flags=0:
        // no unsupported WRITE_THROUGH or ignored ACL/merge errors; backup is retained.
        if unsafe {
            ReplaceFileW(
                active_path.as_ptr(),
                stage_path.as_ptr(),
                backup_path.as_ptr(),
                0,
                ptr::null(),
                ptr::null(),
            )
        } == 0
        {
            return Err(last_error());
        }
        barrier(PublicationStep::Replaced)?;
        let mut active = self.root.writable_leaf("journal.json")?;
        if identity(&active)? != stage_id {
            return Err(StorageError::Invalid("published file identity"));
        }
        barrier(PublicationStep::Reopened)?;
        active.sync_all()?;
        barrier(PublicationStep::SecondFlush)?;
        active.seek(SeekFrom::Start(0))?;
        let mut readback = Vec::new();
        Read::by_ref(&mut active)
            .take((SNAPSHOT_LIMIT + 1) as u64)
            .read_to_end(&mut readback)?;
        if readback != bytes {
            return Err(StorageError::Invalid("publication readback"));
        }
        let (backup_id, backup_bytes) = self.root.read(&backup)?;
        if backup_id != self.current.active_identity
            || backup_bytes != canonical(&self.current.active)?
        {
            return Err(StorageError::Invalid("backup lineage"));
        }
        barrier(PublicationStep::Readback)?;
        drop(active);
        barrier(PublicationStep::Closed)?;
        // Bounded namespace check: exact expected names, no orphan stage, and the
        // new active bytes/identity. Historical artifacts remain covered by the
        // cold-start full scan and the in-memory validated chain.
        let names = self.root.entries()?;
        let expected_backup = backup_name(&self.current.active);
        if names.len() != self.current.artifacts + 1
            || names.iter().any(|name| name.starts_with("stage-"))
            || !names.iter().any(|name| name == "journal.json")
            || !names.iter().any(|name| name == "owner.lock")
            || !names.iter().any(|name| name == &expected_backup)
        {
            return Err(StorageError::Invalid("post-publication namespace"));
        }
        let (verified_id, verified_bytes) = self.root.read("journal.json")?;
        let verified: JournalEnvelope = decode(&verified_bytes, SNAPSHOT_LIMIT)?;
        if verified_id != stage_id || verified != next {
            return Err(StorageError::Invalid("post-publication active"));
        }
        Ok(Scan {
            active: verified,
            active_identity: verified_id,
            artifacts: self.current.artifacts + 1,
            read_only_required: next.read_only_required(),
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationStep {
    StageCreated,
    PartialWrite,
    Written,
    FirstFlush,
    BeforeReplace,
    Replaced,
    Reopened,
    SecondFlush,
    Readback,
    Closed,
}
fn backup_name(e: &JournalEnvelope) -> String {
    format!("backup-{:020}-{}.json", e.sequence(), hex(&e.checksum))
}
fn scan(root: &TestRoot) -> Result<Scan> {
    let names = root.entries()?;
    if !names.iter().any(|s| s == "owner.lock") || !names.iter().any(|s| s == "journal.json") {
        return Err(StorageError::Invalid("missing anchor/lock"));
    }
    let (id, bytes) = root.read("journal.json")?;
    let active: JournalEnvelope = decode(&bytes, SNAPSHOT_LIMIT)?;
    active.validate()?;
    let mut previous: Option<JournalEnvelope> = None;
    let mut backup_count = 0u64;
    for name in &names {
        if name == "owner.lock" || name == "journal.json" {
            continue;
        }
        if !name.starts_with("backup-") {
            return Err(StorageError::Invalid("unexplained stage/artifact"));
        }
        let (_, bytes) = root.read(name)?;
        let record: JournalEnvelope = decode(&bytes, SNAPSHOT_LIMIT)?;
        record.validate()?;
        if *name != backup_name(&record) || record.store() != active.store() {
            return Err(StorageError::Invalid("backup name/store"));
        }
        if record.sequence() != backup_count {
            return Err(StorageError::Invalid("missing/conflicting history"));
        }
        if let Some(old) = &previous {
            record.follows(old)?;
        } else if !matches!(record.payload, JournalPayload::Anchor { .. }) {
            return Err(StorageError::Invalid("missing initial anchor"));
        }
        previous = Some(record);
        backup_count += 1;
    }
    if backup_count != active.sequence() {
        return Err(StorageError::Invalid("missing/conflicting history"));
    }
    if let Some(previous) = previous {
        active.follows(&previous)?;
    } else if !matches!(active.payload, JournalPayload::Anchor { .. }) {
        return Err(StorageError::Invalid("snapshot without anchor"));
    }
    let read_only_required = active.read_only_required();
    Ok(Scan {
        active,
        active_identity: id,
        artifacts: names.len(),
        read_only_required,
    })
}

/// Trusted build-source measurement only. A launch copy below uses the strict
/// protected fixture policy and a new single-link file. Source Cargo hardlinks
/// are pinned against writes/deletion; they are not production image qualification.
pub(crate) fn pin_image(path: &Path) -> Result<(Vec<File>, File)> {
    let parent = path.parent().ok_or(StorageError::Invalid("image parent"))?;
    let held = TestRoot::ancestry(parent)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let tags: FILE_ATTRIBUTE_TAG_INFO = information(&file, FileAttributeTagInfo)?;
    let standard: FILE_STANDARD_INFO = information(&file, FileStandardInfo)?;
    if tags.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || standard.Directory
        || standard.DeletePending
    {
        return Err(StorageError::Invalid("image type/reparse"));
    }
    audit_streams(&file, false)?;
    Ok((held.into_iter().map(|(f, _)| f).collect(), file))
}
pub(crate) fn copy_role_image(source: &mut File) -> Result<(PathBuf, Vec<File>, File)> {
    let path = TestRoot::provision_fixture()?;
    let root = TestRoot::open(&path)?;
    let mut output = root.create("role.exe")?;
    source.seek(SeekFrom::Start(0))?;
    std::io::copy(&mut source.take(64 * 1024 * 1024 + 1), &mut output)?;
    if output.metadata()?.len() > 64 * 1024 * 1024 {
        return Err(StorageError::Invalid("image size"));
    }
    output.sync_all()?;
    drop(output);
    let role = path.join("role.exe");
    let (held, file) = pin_image(&role)?;
    audit_kind(&file, false, Some(root.id.volume))?;
    root.security.audit(&file)?;
    Ok((role, held, file))
}
