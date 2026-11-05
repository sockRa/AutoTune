//! Host-only Win32 reads, never driver loading, tuning, or consent issuance.
//! API review: Microsoft Learn RtlGetVersion, IsWow64Process2,
//! GetTokenInformation(TokenElevation), GetSystemPowerStatus,
//! PowerGetActiveScheme and LocalFree. See docs/benchmark-worker.md.
use super::{
    host_environment::{BoolReturn, Conditions, Fault, Operation, Raw, Reader, Snapshot},
    starting::Scope,
};
use crate::{
    monitor::windows_clock::{CaptureError, WindowsClock},
    sampler::budget::Budget,
};
use std::{
    mem::{size_of, zeroed},
    ptr::null_mut,
};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::{
            Power::{GetSystemPowerStatus, PowerGetActiveScheme, SYSTEM_POWER_STATUS},
            SystemInformation::OSVERSIONINFOW,
            Threading::{GetCurrentProcess, IsWow64Process2, OpenProcessToken},
        },
    },
};
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut OSVERSIONINFOW) -> i32;
}

/// Failed OS cleanup remains owned here, with no implicit native Drop or retry.
/// The capture result must be retained alongside the snapshot on every branch.
pub struct WindowsReader {
    token: HANDLE,
    scheme: *mut GUID,
}
impl Default for WindowsReader {
    fn default() -> Self {
        Self {
            token: null_mut(),
            scheme: null_mut(),
        }
    }
}
impl WindowsReader {
    pub fn retained_token(&self) -> bool {
        !self.token.is_null()
    }
    pub fn retained_scheme(&self) -> bool {
        !self.scheme.is_null()
    }
}
fn returned(r: i32) -> BoolReturn {
    BoolReturn {
        returned: r,
        error: (r == 0).then(|| unsafe { GetLastError() }),
    }
}
impl Reader for WindowsReader {
    fn read(&mut self, operation: Operation) -> Raw {
        unsafe {
            match operation {
                Operation::Version => {
                    let mut v: OSVERSIONINFOW = zeroed();
                    v.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
                    let status = RtlGetVersion(&mut v);
                    Raw::Version {
                        status,
                        major: v.dwMajorVersion,
                        minor: v.dwMinorVersion,
                        build: v.dwBuildNumber,
                        platform: v.dwPlatformId,
                    }
                }
                Operation::Architecture => {
                    let mut process_machine = 0;
                    let mut native = 0;
                    let call = returned(IsWow64Process2(
                        GetCurrentProcess(),
                        &mut process_machine,
                        &mut native,
                    ));
                    Raw::Architecture {
                        call,
                        native,
                        process_machine,
                        process_x64: cfg!(target_arch = "x86_64"),
                    }
                }
                Operation::Elevation => {
                    if !self.token.is_null() {
                        return Raw::Elevation {
                            open: BoolReturn {
                                returned: 0,
                                error: None,
                            },
                            query: None,
                            returned_bytes: 0,
                            elevated: 0,
                            close: None,
                        };
                    }
                    let open = returned(OpenProcessToken(
                        GetCurrentProcess(),
                        TOKEN_QUERY,
                        &mut self.token,
                    ));
                    let mut elevation: TOKEN_ELEVATION = zeroed();
                    let mut bytes = 0;
                    let query = if open.success() && !self.token.is_null() {
                        Some(returned(GetTokenInformation(
                            self.token,
                            TokenElevation,
                            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                            size_of::<TOKEN_ELEVATION>() as u32,
                            &mut bytes,
                        )))
                    } else {
                        None
                    };
                    let close = if query.is_some() {
                        let r = returned(CloseHandle(self.token));
                        if r.success() {
                            self.token = null_mut();
                        }
                        Some(r)
                    } else {
                        None
                    };
                    Raw::Elevation {
                        open,
                        query,
                        returned_bytes: bytes,
                        elevated: elevation.TokenIsElevated,
                        close,
                    }
                }
                Operation::PowerStatus => {
                    let mut v: SYSTEM_POWER_STATUS = zeroed();
                    let call = returned(GetSystemPowerStatus(&mut v));
                    Raw::PowerStatus {
                        call,
                        ac: v.ACLineStatus,
                        battery_flags: v.BatteryFlag,
                        battery_percent: v.BatteryLifePercent,
                        saver: v.SystemStatusFlag,
                        lifetime: v.BatteryLifeTime,
                        full_lifetime: v.BatteryFullLifeTime,
                    }
                }
                Operation::PowerScheme => {
                    if !self.scheme.is_null() {
                        return Raw::PowerScheme {
                            status: u32::MAX,
                            pointer_present: true,
                            guid: None,
                            released: None,
                            release_error: None,
                        };
                    }
                    let status = PowerGetActiveScheme(null_mut(), &mut self.scheme);
                    let pointer_present = !self.scheme.is_null();
                    let mut guid = None;
                    let mut released = None;
                    let mut release_error = None;
                    if status == 0 && pointer_present {
                        let g = *self.scheme;
                        // Canonical GUID field order, not host-endian struct bytes.
                        let mut bytes = [0; 16];
                        bytes[..4].copy_from_slice(&g.data1.to_be_bytes());
                        bytes[4..6].copy_from_slice(&g.data2.to_be_bytes());
                        bytes[6..8].copy_from_slice(&g.data3.to_be_bytes());
                        bytes[8..].copy_from_slice(&g.data4);
                        guid = Some(bytes);
                        let failed = LocalFree(self.scheme.cast());
                        released = Some(failed.is_null());
                        if failed.is_null() {
                            self.scheme = null_mut();
                        } else {
                            release_error = Some(GetLastError());
                        }
                    }
                    Raw::PowerScheme {
                        status,
                        pointer_present,
                        guid,
                        released,
                        release_error,
                    }
                }
            }
        }
    }
}
pub struct Captured {
    pub snapshot: Result<Snapshot, Fault>,
    pub reader: WindowsReader,
    pub clock_error: Option<CaptureError>,
}
/// Caller runs this bounded preflight off its deadline/control thread. This
/// observes only the current parent process/host; child effective options and
/// native runtime/peer facts remain separately required. No settings are changed.
pub fn capture<S: FnMut() -> bool>(
    scope: &Scope,
    conditions: Conditions,
    clock: &WindowsClock,
    stopped: S,
    budget: &Budget,
) -> Captured {
    let mut reader = WindowsReader::default();
    let mut clock_error = None;
    let d = scope.draft();
    let snapshot = if clock.clock_token() != d.clock_token || clock.qpc_frequency() != d.frequency {
        Err(Fault::Binding)
    } else {
        Snapshot::collect_windows(
            scope,
            conditions,
            &mut reader,
            || match clock.capture() {
                Ok(a) => Some(a),
                Err(e) => {
                    clock_error.get_or_insert(e);
                    None
                }
            },
            stopped,
            budget,
        )
    };
    Captured {
        snapshot,
        reader,
        clock_error,
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn host_capture_is_linked_without_os_api_invocation() {
        type Stop = fn() -> bool;
        type Entry = fn(
            &super::Scope,
            super::Conditions,
            &super::WindowsClock,
            Stop,
            &super::Budget,
        ) -> super::Captured;
        let pointer: Entry = super::capture::<Stop>;
        std::hint::black_box(pointer);
        assert_eq!(std::mem::size_of::<super::TOKEN_ELEVATION>(), 4);
        assert_eq!(std::mem::size_of::<super::OSVERSIONINFOW>(), 276);
        assert_eq!(std::mem::size_of::<super::SYSTEM_POWER_STATUS>(), 12);
    }
}
