//! Read-only Windows 10+ clock capture, not native workload admission.
//!
//! Raw 100 ns units are not an accuracy claim. The existing controller still
//! decides bracket width, agreement, regression and suspend policy. Capture
//! failure returns no anchor: callers must not substitute or reuse an old one.
//!
//! ABI: windows-sys 0.61.2 `System::Performance` (QPC/frequency) and
//! `System::WindowsProgramming` (precise interrupt APIs), checked against:
//! <https://learn.microsoft.com/en-us/windows/win32/api/profileapi/nf-profileapi-queryperformancecounter>
//! <https://learn.microsoft.com/en-us/windows/win32/api/profileapi/nf-profileapi-queryperformancefrequency>
//! <https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-queryinterrupttimeprecise>
//! <https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-queryunbiasedinterrupttimeprecise>

use super::Anchor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CounterReading {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureError {
    ZeroToken,
    FrequencyFailed(u32),
    InvalidFrequency(i64),
    CounterFailed { at: CounterReading, code: u32 },
    InvalidCounter { at: CounterReading, value: i64 },
    AnchorOrder { before: u64, after: u64 },
}

/// One caller-established epoch. Neither token nor exact OS frequency can change.
///
/// Use `clock_token()` and `qpc_frequency()` unchanged in the monitor `Binding`.
/// The token is supplied by the caller, not generated as proof of a boot/session.
pub struct WindowsClock {
    clock_token: [u8; 16],
    qpc_frequency: u64,
}

// Private injection only: tests exercise the same acquisition/validation order
// as the OS implementation, without exposing a configurable native backend.
trait Source {
    fn frequency(&mut self) -> Result<i64, u32>;
    fn counter(&mut self) -> Result<i64, u32>;
    fn interrupt(&mut self) -> u64;
    fn unbiased(&mut self) -> u64;
}

impl WindowsClock {
    #[cfg(windows)]
    pub fn new(clock_token: [u8; 16]) -> Result<Self, CaptureError> {
        Self::initialize(clock_token, &mut OsClock)
    }

    pub fn clock_token(&self) -> [u8; 16] {
        self.clock_token
    }

    pub fn qpc_frequency(&self) -> u64 {
        self.qpc_frequency
    }

    /// QPC-before, precise interrupt, precise unbiased interrupt, QPC-after.
    /// Does not round, rebase, retry, enforce policy limits or change OS settings.
    #[cfg(windows)]
    pub fn capture(&self) -> Result<Anchor, CaptureError> {
        self.capture_from(&mut OsClock)
    }

    fn initialize(clock_token: [u8; 16], source: &mut impl Source) -> Result<Self, CaptureError> {
        if clock_token == [0; 16] {
            return Err(CaptureError::ZeroToken);
        }
        let frequency = source.frequency().map_err(CaptureError::FrequencyFailed)?;
        if frequency <= 0 {
            return Err(CaptureError::InvalidFrequency(frequency));
        }
        Ok(Self {
            clock_token,
            qpc_frequency: frequency as u64,
        })
    }

    fn capture_from(&self, source: &mut impl Source) -> Result<Anchor, CaptureError> {
        let qpc_before = counter(source, CounterReading::Before)?;
        let interrupt_100ns = source.interrupt();
        let unbiased_100ns = source.unbiased();
        let qpc_after = counter(source, CounterReading::After)?;
        if qpc_after < qpc_before {
            return Err(CaptureError::AnchorOrder {
                before: qpc_before,
                after: qpc_after,
            });
        }
        Ok(Anchor {
            clock_token: self.clock_token,
            qpc_before,
            qpc_after,
            interrupt_100ns,
            unbiased_100ns,
        })
    }
}

fn counter(source: &mut impl Source, at: CounterReading) -> Result<u64, CaptureError> {
    let value = source
        .counter()
        .map_err(|code| CaptureError::CounterFailed { at, code })?;
    u64::try_from(value).map_err(|_| CaptureError::InvalidCounter { at, value })
}

#[cfg(windows)]
struct OsClock;

#[cfg(windows)]
impl Source for OsClock {
    fn frequency(&mut self) -> Result<i64, u32> {
        use windows_sys::Win32::{
            Foundation::GetLastError, System::Performance::QueryPerformanceFrequency,
        };
        let mut value = 0i64;
        // SAFETY: writable, aligned i64 matches the pinned LARGE_INTEGER output.
        // No pointer is retained. Read the output only after a nonzero BOOL.
        if unsafe { QueryPerformanceFrequency(&mut value) } == 0 {
            // SAFETY: no preconditions; read immediately after the failed call.
            return Err(unsafe { GetLastError() });
        }
        Ok(value)
    }

    fn counter(&mut self) -> Result<i64, u32> {
        use windows_sys::Win32::{
            Foundation::GetLastError, System::Performance::QueryPerformanceCounter,
        };
        let mut value = 0i64;
        // SAFETY: writable, aligned i64 matches the pinned LARGE_INTEGER output.
        // No pointer is retained. Read the output only after a nonzero BOOL.
        if unsafe { QueryPerformanceCounter(&mut value) } == 0 {
            // SAFETY: no preconditions; read immediately after the failed call.
            return Err(unsafe { GetLastError() });
        }
        Ok(value)
    }

    fn interrupt(&mut self) -> u64 {
        use windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise;
        let mut value = 0u64;
        // SAFETY: writable, aligned ULONGLONG output; the void API always writes
        // it and does not retain the pointer. No success status exists.
        unsafe { QueryInterruptTimePrecise(&mut value) };
        value
    }

    fn unbiased(&mut self) -> u64 {
        use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTimePrecise;
        let mut value = 0u64;
        // SAFETY: writable, aligned ULONGLONG output; the void API always writes
        // it and does not retain the pointer. No success status exists.
        unsafe { QueryUnbiasedInterruptTimePrecise(&mut value) };
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Read {
        Frequency(Result<i64, u32>),
        Counter(Result<i64, u32>),
        Interrupt(u64),
        Unbiased(u64),
    }

    struct Script(VecDeque<Read>);

    impl Script {
        fn new(reads: impl IntoIterator<Item = Read>) -> Self {
            Self(reads.into_iter().collect())
        }

        fn done(&self) {
            assert!(self.0.is_empty(), "unconsumed calls: {:?}", self.0);
        }
    }

    impl Source for Script {
        fn frequency(&mut self) -> Result<i64, u32> {
            match self.0.pop_front().expect("unexpected frequency call") {
                Read::Frequency(value) => value,
                other => panic!("frequency called out of order: {other:?}"),
            }
        }

        fn counter(&mut self) -> Result<i64, u32> {
            match self.0.pop_front().expect("unexpected counter call") {
                Read::Counter(value) => value,
                other => panic!("counter called out of order: {other:?}"),
            }
        }

        fn interrupt(&mut self) -> u64 {
            match self.0.pop_front().expect("unexpected interrupt call") {
                Read::Interrupt(value) => value,
                other => panic!("interrupt called out of order: {other:?}"),
            }
        }

        fn unbiased(&mut self) -> u64 {
            match self.0.pop_front().expect("unexpected unbiased call") {
                Read::Unbiased(value) => value,
                other => panic!("unbiased called out of order: {other:?}"),
            }
        }
    }

    #[test]
    fn exact_raw_mapping_and_immutable_epoch_across_captures() {
        use Read::*;
        let mut source = Script::new([
            Frequency(Ok(i64::MAX)),
            Counter(Ok(0)),
            Interrupt(u64::MAX),
            Unbiased(17),
            Counter(Ok(i64::MAX)),
            Counter(Ok(81)),
            Interrupt(0),
            Unbiased(u64::MAX),
            Counter(Ok(81)),
        ]);
        let clock = WindowsClock::initialize([7; 16], &mut source).unwrap();
        assert_eq!(clock.qpc_frequency(), i64::MAX as u64);
        assert_eq!(clock.clock_token(), [7; 16]);
        assert_eq!(
            clock.capture_from(&mut source),
            Ok(Anchor {
                clock_token: [7; 16],
                qpc_before: 0,
                qpc_after: i64::MAX as u64,
                interrupt_100ns: u64::MAX,
                unbiased_100ns: 17,
            })
        );
        assert_eq!(
            clock.capture_from(&mut source),
            Ok(Anchor {
                clock_token: [7; 16],
                qpc_before: 81,
                qpc_after: 81,
                interrupt_100ns: 0,
                unbiased_100ns: u64::MAX,
            })
        );
        source.done();
    }

    #[test]
    fn zero_token_rejects_before_any_os_call() {
        let mut source = Script::new([]);
        assert!(matches!(
            WindowsClock::initialize([0; 16], &mut source),
            Err(CaptureError::ZeroToken)
        ));
        source.done();
    }

    #[test]
    fn frequency_failures_never_fabricate_a_frequency() {
        for (raw, expected) in [
            (Err(123), CaptureError::FrequencyFailed(123)),
            (Ok(0), CaptureError::InvalidFrequency(0)),
            (Ok(-1), CaptureError::InvalidFrequency(-1)),
            (Ok(i64::MIN), CaptureError::InvalidFrequency(i64::MIN)),
        ] {
            let mut source = Script::new([Read::Frequency(raw)]);
            assert!(matches!(
                WindowsClock::initialize([1; 16], &mut source),
                Err(error) if error == expected
            ));
            source.done();
        }
    }

    #[test]
    fn counter_failures_stop_at_the_failed_reading() {
        use Read::*;
        for at in [CounterReading::Before, CounterReading::After] {
            for raw in [Err(0), Err(456), Ok(-1), Ok(i64::MIN)] {
                let mut reads = vec![Frequency(Ok(10_000_000))];
                if at == CounterReading::After {
                    reads.extend([Counter(Ok(10)), Interrupt(20), Unbiased(30)]);
                }
                reads.push(Counter(raw));
                let mut source = Script::new(reads);
                let clock = WindowsClock::initialize([1; 16], &mut source).unwrap();
                let expected = match raw {
                    Err(code) => CaptureError::CounterFailed { at, code },
                    Ok(value) => CaptureError::InvalidCounter { at, value },
                };
                assert_eq!(clock.capture_from(&mut source), Err(expected));
                source.done();
            }
        }
    }

    #[test]
    fn reversed_bracket_returns_no_anchor() {
        use Read::*;
        let mut source = Script::new([
            Frequency(Ok(3)),
            Counter(Ok(11)),
            Interrupt(20),
            Unbiased(30),
            Counter(Ok(10)),
        ]);
        let clock = WindowsClock::initialize([1; 16], &mut source).unwrap();
        assert_eq!(
            clock.capture_from(&mut source),
            Err(CaptureError::AnchorOrder {
                before: 11,
                after: 10,
            })
        );
        source.done();
    }

    #[test]
    fn mapped_binding_and_anchors_reach_existing_controller_without_admission() {
        use crate::{
            monitor::{Binding, Controller, Session},
            worker::Identity,
        };
        use Read::*;
        let mut source = Script::new([
            Frequency(Ok(10_000_000)),
            Counter(Ok(100)),
            Interrupt(200),
            Unbiased(150),
            Counter(Ok(102)),
            Counter(Ok(200)),
            Interrupt(300),
            Unbiased(250),
            Counter(Ok(202)),
        ]);
        let clock = WindowsClock::initialize([1; 16], &mut source).unwrap();
        let binding = Binding {
            session: [2; 16],
            run: [3; 16],
            device: Identity {
                uuid: [4; 16],
                domain: 0,
                bus: 1,
                device: 2,
            },
            clock_token: clock.clock_token(),
            qpc_frequency: clock.qpc_frequency(),
        };
        let origin = clock.capture_from(&mut source).unwrap();
        let later = clock.capture_from(&mut source).unwrap();
        let mut controller = Controller::new(binding, origin, Session::Pilot).unwrap();
        controller.poll(later);
        assert_eq!(controller.report().binding(), binding);
        assert_eq!(controller.report().now_ns(), 10_000);
        assert!(controller.report().current_clock_valid());
        assert_eq!(controller.report().events().next().unwrap().anchor, origin);
        assert_eq!(controller.report().events().nth(1).unwrap().anchor, later);
        assert!(!controller.report().native_admission_available());
        assert!(!controller.report().fixture_compute_eligible());
        source.done();
    }
}
