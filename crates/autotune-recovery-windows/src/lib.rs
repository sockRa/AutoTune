//! Windows diagnostics and isolated test-root ownership/publication ports.
//! Not an armed transport or production root. Missing integration rejects arming.
use autotune_recovery_contract::Hash;

#[cfg(windows)]
pub mod ipc;
#[cfg(windows)]
pub mod process;
#[cfg(windows)]
pub mod storage;
#[cfg(windows)]
pub mod storage_executor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    Unsupported,
    Os(u32),
    Crypto(i32),
    Isolation,
    Blocked(&'static str),
}
pub type Result<T> = std::result::Result<T, PlatformError>;

#[derive(Debug, Clone, Copy)]
pub struct ClockObservation {
    pub interrupt_100ns: u64,
    pub unbiased_100ns: u64,
    pub utc_100ns: u64,
    pub bracket_100ns: u64,
}

/// There is deliberately no fall-through to std::process or string-only path checks.
pub fn require_arm_support() -> Result<()> {
    if !cfg!(windows) {
        return Err(PlatformError::Unsupported);
    }
    Err(PlatformError::Blocked("storage and read-only process/IPC diagnostics are not arm authority: complete reducer/worker/journal integration, external fake witness, console-closure evidence and mandatory failure cases remain incomplete; production and hardware gates stay closed"))
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::c_void, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, FILETIME},
        Security::Cryptography::{
            BCryptCloseAlgorithmProvider, BCryptGenRandom, BCryptHash, BCryptOpenAlgorithmProvider,
            BCRYPT_ALG_HANDLE_HMAC_FLAG, BCRYPT_SHA256_ALGORITHM, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        },
        System::{
            Console::GetConsoleCP,
            JobObjects::IsProcessInJob,
            SystemInformation::GetSystemTimePreciseAsFileTime,
            Threading::GetCurrentProcess,
            WindowsProgramming::{QueryInterruptTimePrecise, QueryUnbiasedInterruptTimePrecise},
        },
    };

    pub fn random() -> Result<Hash> {
        let mut key = [0u8; 32];
        // SAFETY: writable 32-byte buffer, system RNG requires null provider.
        let status = unsafe {
            BCryptGenRandom(
                ptr::null_mut(),
                key.as_mut_ptr(),
                32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status < 0 {
            Err(PlatformError::Crypto(status))
        } else {
            Ok(key)
        }
    }

    pub fn hmac(key: &Hash, data: &[u8]) -> Result<Hash> {
        let length = u32::try_from(data.len())
            .map_err(|_| PlatformError::Blocked("HMAC input too large"))?;
        let mut algorithm = ptr::null_mut();
        // SAFETY: valid output pointer, static NUL-terminated algorithm identifier.
        let status = unsafe {
            BCryptOpenAlgorithmProvider(
                &mut algorithm,
                BCRYPT_SHA256_ALGORITHM,
                ptr::null(),
                BCRYPT_ALG_HANDLE_HMAC_FLAG,
            )
        };
        if status < 0 {
            return Err(PlatformError::Crypto(status));
        }
        let mut output = [0u8; 32];
        // SAFETY: provider remains open; key/input/output slices cover advertised lengths.
        let status = unsafe {
            BCryptHash(
                algorithm,
                key.as_ptr(),
                32,
                data.as_ptr(),
                length,
                output.as_mut_ptr(),
                32,
            )
        };
        // SAFETY: owned provider is closed once, after its last use.
        let close = unsafe { BCryptCloseAlgorithmProvider(algorithm, 0) };
        if status < 0 {
            Err(PlatformError::Crypto(status))
        } else if close < 0 {
            Err(PlatformError::Crypto(close))
        } else {
            Ok(output)
        }
    }
    pub fn artifact_hash(domain: &[u8], data: &[u8]) -> Result<Hash> {
        let mut transcript = autotune_recovery_contract::digest_prefix(domain);
        transcript.extend(data);
        let length = u32::try_from(transcript.len())
            .map_err(|_| PlatformError::Blocked("hash input size"))?;
        let mut algorithm = ptr::null_mut();
        // SAFETY: SHA256 provider, bounded live input/output buffers, no key for unkeyed hash.
        let opened = unsafe {
            BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_SHA256_ALGORITHM, ptr::null(), 0)
        };
        if opened < 0 {
            return Err(PlatformError::Crypto(opened));
        }
        let mut hash = [0u8; 32];
        let status = unsafe {
            BCryptHash(
                algorithm,
                ptr::null(),
                0,
                transcript.as_ptr(),
                length,
                hash.as_mut_ptr(),
                32,
            )
        };
        let close = unsafe { BCryptCloseAlgorithmProvider(algorithm, 0) };
        if status < 0 {
            Err(PlatformError::Crypto(status))
        } else if close < 0 {
            Err(PlatformError::Crypto(close))
        } else {
            Ok(hash)
        }
    }

    pub fn clock() -> Result<ClockObservation> {
        let mut before = 0;
        let mut after = 0;
        let mut unbiased = 0;
        let mut utc = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        // SAFETY: all API outputs reference valid writable initialized storage.
        unsafe {
            QueryInterruptTimePrecise(&mut before);
            QueryUnbiasedInterruptTimePrecise(&mut unbiased);
            GetSystemTimePreciseAsFileTime(&mut utc);
            QueryInterruptTimePrecise(&mut after);
        }
        let bracket = after
            .checked_sub(before)
            .ok_or(PlatformError::Blocked("clock regression"))?;
        if bracket > 20_000 {
            return Err(PlatformError::Blocked(
                "clock sampling bracket exceeds 2 ms",
            ));
        }
        Ok(ClockObservation {
            interrupt_100ns: before,
            unbiased_100ns: unbiased,
            utc_100ns: ((utc.dwHighDateTime as u64) << 32) | utc.dwLowDateTime as u64,
            bracket_100ns: bracket,
        })
    }

    pub fn verify_isolation() -> Result<()> {
        let mut in_job = 0;
        // SAFETY: pseudo process handle is valid and never closed; null job queries ANY job.
        if unsafe { IsProcessInJob(GetCurrentProcess(), ptr::null_mut::<c_void>(), &mut in_job) }
            == 0
        {
            // SAFETY: reads thread-local error immediately after failing API.
            return Err(PlatformError::Os(unsafe { GetLastError() }));
        }
        // SAFETY: diagnostic only. A zero code page indicates no associated console.
        if in_job != 0 || unsafe { GetConsoleCP() } != 0 {
            return Err(PlatformError::Isolation);
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use native::{artifact_hash, clock, hmac, random, verify_isolation};
#[cfg(not(windows))]
pub fn random() -> Result<Hash> {
    Err(PlatformError::Unsupported)
}
#[cfg(not(windows))]
pub fn hmac(_: &Hash, _: &[u8]) -> Result<Hash> {
    Err(PlatformError::Unsupported)
}
#[cfg(not(windows))]
pub fn clock() -> Result<ClockObservation> {
    Err(PlatformError::Unsupported)
}
#[cfg(not(windows))]
pub fn verify_isolation() -> Result<()> {
    Err(PlatformError::Unsupported)
}
