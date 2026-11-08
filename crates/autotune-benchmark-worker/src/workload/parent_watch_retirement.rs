//! Normal retirement of host containment, not GPU quiescence or run acceptance.
use super::{
    gate::StopWindow, launch_origin::Origin, parent_watch::Window, starting_export::ClockFault,
    stop_requests::Observation,
};
use crate::{
    monitor::{clock::Clock, Anchor},
    sampler::supervisor::ProcessObservation,
};
#[cfg(any(windows, test))]
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Incomplete,
    Context,
    Binding,
    Clock,
    Deadline,
    Missing,
    Os,
    Pending,
    Exit,
    Termination,
    Untrusted,
    Duplicate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capture {
    pub anchor: Option<Anchor>,
    pub error: Option<ClockFault>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read {
    pub origin: Origin,
    pub before: Capture,
    pub requests_before: [Observation; 2],
    pub raw: Option<ProcessObservation>,
    pub after: Capture,
    pub requests_after: [Observation; 2],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    window: Window,
    stop: StopWindow,
    expected: [Origin; 2],
    before: Capture,
    after: Capture,
    reads: [Option<Read>; 2],
    clock_identity: (u64, [u8; 16]),
    trusted: bool,
    result: Result<(), Fault>,
}
impl Frame {
    pub fn window(&self) -> Window {
        self.window
    }
    pub fn stop(&self) -> StopWindow {
        self.stop
    }
    pub fn expected(&self) -> [Origin; 2] {
        self.expected
    }
    pub fn before(&self) -> Capture {
        self.before
    }
    pub fn after(&self) -> Capture {
        self.after
    }
    pub fn reads(&self) -> &[Option<Read>; 2] {
        &self.reads
    }
    pub fn result(&self) -> Result<(), Fault> {
        self.result
    }
    pub fn clock_identity(&self) -> (u64, [u8; 16]) {
        self.clock_identity
    }
    /// Detached fixtures cannot disarm real owners or retire a controller.
    pub fn fixture(
        window: Window,
        stop: StopWindow,
        expected: [Origin; 2],
        before: Capture,
        after: Capture,
        reads: [Option<Read>; 2],
    ) -> Self {
        let mut f = Self {
            window,
            stop,
            expected,
            before,
            after,
            reads,
            clock_identity: (
                window.binding.monitor().qpc_frequency,
                window.binding.monitor().clock_token,
            ),
            trusted: false,
            result: Err(Fault::Untrusted),
        };
        f.result = assess(&f);
        f
    }
    #[cfg(any(windows, test))]
    pub(crate) fn certifies(&self, origin: Origin) -> bool {
        self.trusted && self.result.is_ok() && self.expected.contains(&origin)
    }
    pub(crate) fn elapsed_end(&self) -> Result<u64, Fault> {
        self.result?;
        self.window
            .binding
            .elapsed(self.after.anchor.ok_or(Fault::Clock)?)
            .map_err(|_| Fault::Clock)
    }
}
fn process(read: Read, expected: Origin) -> Result<(), Fault> {
    if read.origin != expected {
        return Err(Fault::Binding);
    }
    let p = read.raw.ok_or(Fault::Missing)?;
    if p.wait_error.is_some() || p.exit_error.is_some() || p.accounting_error.is_some() {
        return Err(Fault::Os);
    }
    if read.requests_before[1].requested
        || read.requests_after[1].requested
        || p.termination.is_some()
    {
        return Err(Fault::Termination);
    }
    for r in read
        .requests_before
        .iter()
        .chain(read.requests_after.iter())
    {
        if !r.requested && r.returned.is_some() {
            return Err(Fault::Binding);
        }
        if r.requested {
            match r.returned {
                Some(Err(_)) => return Err(Fault::Os),
                None => return Err(Fault::Pending),
                _ => (),
            }
        }
    }
    if p.creation != expected.creation() {
        return Err(Fault::Binding);
    }
    if p.wait_status == 258 {
        return Err(Fault::Pending);
    }
    if p.wait_status != 0 || p.exit_code != Some(0) || p.active_processes != Some(0) {
        return Err(Fault::Exit);
    }
    Ok(())
}
fn assess(f: &Frame) -> Result<(), Fault> {
    if !f.trusted {
        return Err(Fault::Untrusted);
    }
    let binding = f.window.binding;
    if f.expected[0].pid() == f.expected[1].pid()
        || f.expected[0].kind() != super::launch_origin::Kind::Sampler
        || f.expected[1].kind() != super::launch_origin::Kind::Worker
        || f.clock_identity != (binding.monitor().qpc_frequency, binding.epoch.clock_token)
    {
        return Err(Fault::Binding);
    }
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    if f.stop.cooperative_ns
        != f.stop
            .trigger_ns
            .saturating_add(p.cooperative_stop_ns)
            .min(binding.run_end())
        || f.stop.exit_ns
            != f.stop
                .trigger_ns
                .saturating_add(p.exit_observation_ns)
                .min(binding.run_end())
    {
        return Err(Fault::Binding);
    }
    let mut clock = Clock::new(binding.monitor(), binding.epoch).map_err(|_| Fault::Clock)?;
    let mut last = binding.epoch;
    let mut observe = |c: Capture| -> Result<(), Fault> {
        if c.error.is_some() {
            return Err(Fault::Clock);
        }
        let a = c.anchor.ok_or(Fault::Clock)?;
        if a.qpc_before < last.qpc_after {
            return Err(Fault::Clock);
        }
        let now = clock.observe(a).map_err(|_| Fault::Clock)?;
        last = a;
        if now < f.stop.trigger_ns {
            return Err(Fault::Binding);
        }
        if now >= f.stop.cooperative_ns.min(binding.run_end()) {
            return Err(Fault::Deadline);
        }
        Ok(())
    };
    observe(f.before)?;
    for (i, r) in f.reads.iter().enumerate() {
        let r = r.ok_or(Fault::Missing)?;
        observe(r.before)?;
        // A returned OS error is retained and remains primary over a bad
        // post-call capture. Neither output is dropped to repair the other.
        process(r, f.expected[i])?;
        observe(r.after)?;
    }
    observe(f.after)
}
/// The same atomic decision arbitrates force issuance and retirement. A force
/// request already published can never be cancelled by later exit evidence.
#[cfg(any(windows, test))]
pub(crate) struct Arbitration(AtomicU8);
#[cfg(any(windows, test))]
impl Arbitration {
    pub(crate) fn new() -> Self {
        Self(AtomicU8::new(0))
    }
    pub(crate) fn force(&self) -> bool {
        let _ = self.0.try_update(Ordering::AcqRel, Ordering::Acquire, |v| {
            if v == 0 || v == 3 {
                Some(1)
            } else {
                None
            }
        });
        self.0.load(Ordering::Acquire) == 1
    }
    pub(crate) fn retire(&self) -> Result<(), Fault> {
        self.0
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|v| {
                if v == 2 {
                    Fault::Duplicate
                } else if v == 3 {
                    Fault::Context
                } else {
                    Fault::Termination
                }
            })
    }
    pub(crate) fn retired(&self) -> bool {
        self.0.load(Ordering::Acquire) == 2
    }
    pub(crate) fn active(&self) -> bool {
        self.0.load(Ordering::Acquire) == 0
    }
    pub(crate) fn stop(&self) {
        let _ = self
            .0
            .compare_exchange(0, 3, Ordering::AcqRel, Ordering::Acquire);
    }
    /// Immediate abort reserves force before any signal gate is published.
    /// Otherwise a concurrent retirement could win between the signal check
    /// and force arbitration and leave a newly requested signal after retirement.
    pub(crate) fn dispatch(&self, stop: bool, force: bool) -> (bool, bool) {
        let terminate = force && self.force();
        (stop && !self.retired(), terminate)
    }
}

#[cfg(windows)]
pub(crate) fn collect_into(
    window: Window,
    stop: StopWindow,
    expected: [Origin; 2],
    processes: [&super::windows_process::Process; 2],
    clock: &crate::monitor::windows_clock::WindowsClock,
    context: bool,
    output: &mut Option<Frame>,
) {
    fn capture(clock: &crate::monitor::windows_clock::WindowsClock) -> Capture {
        match clock.capture() {
            Ok(a) => Capture {
                anchor: Some(a),
                error: None,
            },
            Err(e) => Capture {
                anchor: None,
                error: Some(e.into()),
            },
        }
    }
    *output = Some(Frame {
        window,
        stop,
        expected,
        before: Capture {
            anchor: None,
            error: None,
        },
        after: Capture {
            anchor: None,
            error: None,
        },
        reads: [None; 2],
        clock_identity: (clock.qpc_frequency(), clock.clock_token()),
        trusted: true,
        result: Err(Fault::Incomplete),
    });
    let f = output.as_mut().expect("initialized retirement evidence");
    f.before = capture(clock);
    let mut validator = Clock::new(window.binding.monitor(), window.binding.epoch).ok();
    let mut last = window.binding.epoch;
    let mut observe = |c: Capture| -> bool {
        let Some(a) = c.anchor else { return false };
        if c.error.is_some() || a.qpc_before < last.qpc_after {
            return false;
        }
        let now = validator.as_mut().and_then(|v| v.observe(a).ok());
        last = a;
        now.is_some_and(|n| {
            n >= stop.trigger_ns && n < stop.cooperative_ns.min(window.binding.run_end())
        })
    };
    let eligible = context
        && f.before.error.is_none()
        && f.clock_identity
            == (
                window.binding.monitor().qpc_frequency,
                window.binding.epoch.clock_token,
            )
        && observe(f.before);
    if eligible {
        for i in 0..2 {
            let p = processes[i];
            let before = capture(clock);
            let requests_before = p.stop_requests();
            let mut read = Read {
                origin: p.origin(),
                before,
                requests_before,
                raw: None,
                after: Capture {
                    anchor: None,
                    error: None,
                },
                requests_after: requests_before,
            };
            // Preserve pre-call facts even if the original-handle query blocks
            // or unwinds before returning. No raw return is invented.
            f.reads[i] = Some(read);
            if observe(before) {
                read.raw = Some(p.observe());
            }
            // Store native returns before any post-call capture.
            f.reads[i] = Some(read);
            read.after = capture(clock);
            read.requests_after = p.stop_requests();
            f.reads[i] = Some(read);
            if process(read, expected[i]).is_err() || !observe(read.after) {
                break;
            }
        }
    }
    f.after = capture(clock);
    f.result = if context {
        assess(f)
    } else {
        Err(Fault::Context)
    };
}

#[cfg(test)]
pub(crate) fn model(window: Window, stop: StopWindow) -> Frame {
    use super::launch_origin::Kind;
    use crate::evidence::{Digest, Domain};
    let d = Digest::of(Domain::RawArtifact, b"private CPU exit model");
    let expected = std::array::from_fn(|i| {
        Origin::new(
            [Kind::Sampler, Kind::Worker][i],
            d,
            d,
            d,
            i as u32 + 1,
            i as u64 + 1,
        )
        .unwrap()
    });
    let cap = |ns| {
        let ns = stop.trigger_ns + ns;
        Capture {
            anchor: Some(Anchor {
                clock_token: window.binding.epoch.clock_token,
                qpc_before: ns,
                qpc_after: ns,
                interrupt_100ns: ns / 100,
                unbiased_100ns: ns / 100,
            }),
            error: None,
        }
    };
    let reads = std::array::from_fn(|i| {
        Some(Read {
            origin: expected[i],
            before: cap(200 + i as u64 * 200),
            after: cap(300 + i as u64 * 200),
            requests_before: [Observation::default(); 2],
            requests_after: [Observation::default(); 2],
            raw: Some(ProcessObservation {
                creation: expected[i].creation(),
                wait_status: 0,
                wait_error: None,
                exit_code: Some(0),
                exit_error: None,
                termination: None,
                active_processes: Some(0),
                accounting_error: None,
            }),
        })
    });
    let mut frame = Frame {
        window,
        stop,
        expected,
        before: cap(100),
        after: cap(600),
        reads,
        clock_identity: (
            window.binding.monitor().qpc_frequency,
            window.binding.epoch.clock_token,
        ),
        trusted: true,
        result: Err(Fault::Context),
    };
    frame.result = assess(&frame);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        evidence::{Digest, Domain},
        sampler::budget::Budget,
        workload::{
            manifest::{Artifacts, PilotDraft, PilotManifest},
            wire::WorkerBinding,
        },
    };
    fn frame() -> Frame {
        let b = Budget::new(4 * 1024 * 1024).unwrap();
        let d = Digest::of(Domain::RawArtifact, b"CPU retirement binding");
        let m = PilotManifest::freeze(
            PilotDraft {
                session: [1; 16],
                runs: [[2; 16], [3; 16], [4; 16]],
                clock_token: [5; 16],
                raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
                raw_pci: "0000:01:02.0".into(),
                source_commit: "a".repeat(40),
                artifacts: Artifacts {
                    source_tree: d,
                    lockfile: d,
                    controller: d,
                    worker: d,
                    sampler: d,
                    ptx: d,
                    protocol: d,
                },
                starting_evidence: d,
                environment_evidence: d,
                approval_ref: "CPU model is no execution approval".into(),
                output_root: "D:\\capture\\pilot".into(),
            },
            &b,
        )
        .unwrap();
        let epoch = Anchor {
            clock_token: [5; 16],
            qpc_before: 0,
            qpc_after: 0,
            interrupt_100ns: 0,
            unbiased_100ns: 0,
        };
        let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, epoch).unwrap();
        model(
            Window {
                binding,
                setup_deadline_ns: 10_000_000_000,
            },
            StopWindow {
                trigger_ns: 1_000_000_000,
                cooperative_ns: 2_000_000_000,
                exit_ns: 4_000_000_000,
            },
        )
    }
    #[test]
    fn both_original_normal_exits_are_required_and_public_fixtures_stay_untrusted() {
        let f = frame();
        assert_eq!(assess(&f), Ok(()));
        assert!(f.certifies(f.expected[0]));
        assert!(f.certifies(f.expected[1]));
        let detached = Frame::fixture(f.window, f.stop, f.expected, f.before, f.after, f.reads);
        assert_eq!(detached.result(), Err(Fault::Untrusted));
        assert!(!detached.certifies(f.expected[0]));
        for i in 0..2 {
            let mut incomplete = f;
            incomplete.reads[i] = None;
            assert_eq!(assess(&incomplete), Err(Fault::Missing));
        }
    }
    #[test]
    fn abnormal_pending_missing_or_replaced_process_facts_cannot_retire() {
        for role in 0..2 {
            for mode in 0..9 {
                let mut f = frame();
                let r = f.reads[role].as_mut().unwrap();
                let p = r.raw.as_mut().unwrap();
                let expected = match mode {
                    0 => {
                        p.wait_status = 258;
                        Fault::Pending
                    }
                    1 => {
                        p.exit_code = Some(93);
                        Fault::Exit
                    }
                    2 => {
                        p.active_processes = Some(1);
                        Fault::Exit
                    }
                    3 => {
                        p.exit_code = None;
                        Fault::Exit
                    }
                    4 => {
                        p.creation += 1;
                        Fault::Binding
                    }
                    5 => {
                        p.termination = Some(Ok(()));
                        Fault::Termination
                    }
                    6 => {
                        r.requests_after[1] = Observation {
                            requested: true,
                            returned: None,
                        };
                        Fault::Termination
                    }
                    7 => {
                        r.requests_before[0] = Observation {
                            requested: true,
                            returned: None,
                        };
                        Fault::Pending
                    }
                    _ => {
                        r.requests_after[0] = Observation {
                            requested: true,
                            returned: Some(Err(5)),
                        };
                        Fault::Os
                    }
                };
                assert_eq!(assess(&f), Err(expected));
            }
        }
    }
    #[test]
    fn returned_os_errors_survive_a_later_failed_capture() {
        for mode in 0..3 {
            let mut f = frame();
            let r = f.reads[0].as_mut().unwrap();
            let p = r.raw.as_mut().unwrap();
            match mode {
                0 => p.wait_error = Some(6),
                1 => p.exit_error = Some(87),
                _ => p.accounting_error = Some(5),
            }
            r.after = Capture {
                anchor: None,
                error: Some(ClockFault::CounterFailed {
                    before: false,
                    code: 87,
                }),
            };
            assert_eq!(assess(&f), Err(Fault::Os));
            assert!(f.reads[0].unwrap().after.error.is_some());
        }
    }
    #[test]
    fn missing_regressing_wrong_token_and_expired_clocks_never_establish_retirement() {
        for mode in 0..5 {
            let mut f = frame();
            match mode {
                0 => f.before.anchor = None,
                1 => f.reads[0].as_mut().unwrap().after.anchor = None,
                2 => f.after.anchor.as_mut().unwrap().clock_token = [6; 16],
                3 => f.reads[1].as_mut().unwrap().before = f.before,
                _ => {
                    let n = f.stop.cooperative_ns;
                    let a = f.after.anchor.as_mut().unwrap();
                    a.qpc_before = n;
                    a.qpc_after = n;
                    a.interrupt_100ns = n / 100;
                    a.unbiased_100ns = n / 100;
                }
            }
            assert_eq!(
                assess(&f),
                Err(if mode == 4 {
                    Fault::Deadline
                } else {
                    Fault::Clock
                })
            );
        }
    }
    #[test]
    fn forced_arbitration_cannot_be_reopened_by_later_normal_exit() {
        let a = Arbitration::new();
        assert!(a.active());
        assert!(a.force());
        let a = Arbitration::new();
        assert_eq!(a.dispatch(true, true), (true, true));
        assert_eq!(a.retire(), Err(Fault::Termination));
        assert_eq!(a.retire(), Err(Fault::Termination));
        assert!(!a.retired());
        assert!(a.force());
    }
    #[test]
    fn retirement_wins_once_and_withdraws_future_force_requests() {
        let a = Arbitration::new();
        a.retire().unwrap();
        assert!(a.retired());
        assert_eq!(a.dispatch(true, true), (false, false));
        assert!(!a.active());
        assert!(!a.force());
        assert_eq!(a.retire(), Err(Fault::Duplicate));
        assert!(a.retired());
    }
    #[test]
    fn unplanned_stop_after_planned_finish_vetoes_retirement_without_renewing_force() {
        let a = Arbitration::new();
        a.stop();
        assert!(!a.active());
        assert_eq!(a.retire(), Err(Fault::Context));
        assert!(a.force());
        assert_eq!(a.retire(), Err(Fault::Termination));
        let b = Arbitration::new();
        b.retire().unwrap();
        b.stop();
        assert!(b.retired());
        assert!(!b.force());
    }
}
