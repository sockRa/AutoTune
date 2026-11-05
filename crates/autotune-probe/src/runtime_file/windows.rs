//! Trusted system file observation only; never loads a DLL or invokes its exports.
//! File IDs/path reports describe the observed files, not mapped image bytes.
use super::{Identity, Io, Report, Source, Stage};
use std::{
    ffi::{c_void, OsString},
    fs::{File, OpenOptions},
    io::Read,
    mem::zeroed,
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::GetLastError,
    Storage::FileSystem::{
        GetFileInformationByHandle, GetFileType, BY_HANDLE_FILE_INFORMATION,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    },
    System::{LibraryLoader::GetModuleFileNameW, SystemInformation::GetSystemDirectoryW},
};
/// Fixed path/UTF-16 scratch allowance, included in the worker's prepaid 512 KiB.
pub const PATH_UNITS: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cuda,
    Nvml,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Cuda => "nvcuda.dll",
            Self::Nvml => "nvml.dll",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Guard,
    Directory,
    Open,
    Measurement,
    ModulePath,
    LoadedOpen,
    LoadedMetadata,
    Identity,
    Repeated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathReturn {
    pub length: u32,
    pub error: Option<u32>,
}
pub struct Image {
    requested: Option<PathBuf>,
    loaded: Option<PathBuf>,
    file: Option<File>,
    pub directory: Option<PathReturn>,
    pub open: Option<Io<()>>,
    pub report: Report,
    pub module_path: Option<PathReturn>,
    pub loaded_open: Option<Io<()>>,
    pub loaded_identity: Option<Io<Identity>>,
    pub fault: Option<Fault>,
    bound: bool,
}
fn metadata(file: &File) -> Io<Identity> {
    let mut v: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut v) };
    if ok == 0 {
        return Err(Some(unsafe { GetLastError() } as i32));
    }
    let file_type = unsafe { GetFileType(file.as_raw_handle()) };
    let file_type_error = (file_type == 0).then(|| unsafe { GetLastError() });
    Ok(Identity {
        volume_serial: v.dwVolumeSerialNumber,
        index: (u64::from(v.nFileIndexHigh) << 32) | u64::from(v.nFileIndexLow),
        bytes: (u64::from(v.nFileSizeHigh) << 32) | u64::from(v.nFileSizeLow),
        last_write: (u64::from(v.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(v.ftLastWriteTime.dwLowDateTime),
        attributes: v.dwFileAttributes,
        file_type,
        file_type_error,
    })
}
struct Disk<'a>(&'a mut File);
impl Source for Disk<'_> {
    fn identity(&mut self) -> Io<Identity> {
        metadata(self.0)
    }
    fn read(&mut self, b: &mut [u8]) -> Io<usize> {
        self.0.read(b).map_err(|e| e.raw_os_error())
    }
}
fn open(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}
impl Image {
    pub fn requested(&self) -> Option<&Path> {
        self.requested.as_deref()
    }
    pub fn loaded(&self) -> Option<&Path> {
        self.loaded.as_deref()
    }
    pub fn held(&self) -> bool {
        self.file.is_some()
    }
    pub fn measured(&self) -> bool {
        self.fault.is_none() && self.report.complete() && self.file.is_some()
    }
    pub fn bound(&self) -> bool {
        self.bound && self.fault.is_none()
    }
    pub fn prepare(kind: Kind, mut guard: impl FnMut(Stage) -> bool) -> Self {
        let mut out = Self {
            requested: None,
            loaded: None,
            file: None,
            directory: None,
            open: None,
            report: Report::default(),
            module_path: None,
            loaded_open: None,
            loaded_identity: None,
            fault: None,
            bound: false,
        };
        let result = (|| -> Result<(), Fault> {
            if !guard(Stage::DirectoryBefore) {
                return Err(Fault::Guard);
            }
            let mut buffer = [0u16; PATH_UNITS];
            let n = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
            out.directory = Some(PathReturn {
                length: n,
                error: (n == 0).then(|| unsafe { GetLastError() }),
            });
            if n > 0 && (n as usize) < buffer.len() {
                out.requested = Some(
                    PathBuf::from(OsString::from_wide(&buffer[..n as usize])).join(kind.name()),
                );
            }
            if !guard(Stage::DirectoryAfter) {
                return Err(Fault::Guard);
            }
            if n == 0 || n as usize >= buffer.len() {
                return Err(Fault::Directory);
            }
            let path = out
                .requested
                .as_ref()
                .expect("retained successful path")
                .clone();
            if !path.is_absolute() {
                return Err(Fault::Directory);
            }
            if !guard(Stage::OpenBefore) {
                return Err(Fault::Guard);
            }
            match open(&path) {
                Ok(f) => {
                    out.file = Some(f);
                    out.open = Some(Ok(()));
                }
                Err(e) => out.open = Some(Err(e.raw_os_error())),
            }
            if !guard(Stage::OpenAfter) {
                return Err(Fault::Guard);
            }
            let file = out.file.as_mut().ok_or(Fault::Open)?;
            out.report.measure(&mut Disk(file), &mut guard);
            if !out.report.complete() {
                return Err(Fault::Measurement);
            }
            Ok(())
        })();
        if let Err(f) = result {
            out.fault = Some(f);
        }
        out
    }
    /// # Safety
    /// `module` is the live module retained by the caller. No module is loaded,
    /// freed or dereferenced here; only the OS-reported path is observed.
    /// Repeated calls cannot replace original evidence or renew a deadline.
    pub unsafe fn bind_module(
        &mut self,
        module: *mut c_void,
        mut guard: impl FnMut(Stage) -> bool,
    ) {
        if self.module_path.is_some() || self.bound {
            self.fault.get_or_insert(Fault::Repeated);
            return;
        }
        if !self.measured() {
            return;
        }
        let result = (|| -> Result<(), Fault> {
            if !guard(Stage::ModulePathBefore) {
                return Err(Fault::Guard);
            }
            let mut buffer = [0u16; PATH_UNITS];
            let n = GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32);
            self.module_path = Some(PathReturn {
                length: n,
                error: (n == 0 || n as usize >= buffer.len()).then(|| GetLastError()),
            });
            if n > 0 && (n as usize) < buffer.len() {
                self.loaded = Some(PathBuf::from(OsString::from_wide(&buffer[..n as usize])));
            }
            if !guard(Stage::ModulePathAfter) {
                return Err(Fault::Guard);
            }
            if n == 0 || n as usize >= buffer.len() {
                return Err(Fault::ModulePath);
            }
            let path = self
                .loaded
                .as_ref()
                .expect("retained successful path")
                .clone();
            if !path.is_absolute() {
                return Err(Fault::ModulePath);
            }
            if !guard(Stage::LoadedOpenBefore) {
                return Err(Fault::Guard);
            }
            let file = match open(&path) {
                Ok(f) => {
                    self.loaded_open = Some(Ok(()));
                    Some(f)
                }
                Err(e) => {
                    self.loaded_open = Some(Err(e.raw_os_error()));
                    None
                }
            };
            if !guard(Stage::LoadedOpenAfter) {
                return Err(Fault::Guard);
            }
            let file = file.ok_or(Fault::LoadedOpen)?;
            if !guard(Stage::LoadedMetadataBefore) {
                return Err(Fault::Guard);
            }
            self.loaded_identity = Some(metadata(&file));
            if !guard(Stage::LoadedMetadataAfter) {
                return Err(Fault::Guard);
            }
            let observed = self
                .loaded_identity
                .expect("retained identity")
                .map_err(|_| Fault::LoadedMetadata)?;
            if !self.report.matches_identity(observed) {
                return Err(Fault::Identity);
            }
            self.bound = true;
            Ok(())
        })();
        if let Err(f) = result {
            self.fault = Some(f);
        }
    }
}
