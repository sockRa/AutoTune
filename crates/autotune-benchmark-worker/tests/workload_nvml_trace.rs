use autotune_benchmark_worker::{
    monitor::Anchor,
    sampler::{budget::Budget, child::NativeAdmission},
    workload::nvml_trace::{Fault, Ledger},
};
use autotune_probe::nvml::{SamplerCall, SamplerObserver, SamplerOutput, SamplerReturn};

const FREQUENCY: u64 = 1_000_000_000;
const SURFACE: usize = 1 + 1 + 1024 * 3 + 3;

struct FileBytes {
    position: usize,
}
impl autotune_probe::runtime_file::Source for FileBytes {
    fn identity(
        &mut self,
    ) -> autotune_probe::runtime_file::Io<autotune_probe::runtime_file::Identity> {
        Ok(autotune_probe::runtime_file::Identity {
            volume_serial: 1,
            index: 2,
            bytes: 3,
            last_write: 4,
            attributes: 0x20,
            file_type: 1,
            file_type_error: None,
        })
    }
    fn read(&mut self, buffer: &mut [u8]) -> autotune_probe::runtime_file::Io<usize> {
        let n = (3 - self.position).min(buffer.len());
        buffer[..n].copy_from_slice(&b"abc"[self.position..self.position + n]);
        self.position += n;
        Ok(n)
    }
}
#[test]
fn file_checks_and_native_returns_share_original_clock_without_clearing_outer_pending() {
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let used = b.used();
    let _pressure = b.reserve(32 * 1024 * 1024 - used).unwrap();
    let baseline = b.used();
    gate.begin().unwrap();
    let mut n = 1000;
    let mut observer = l.observer(&gate, || {
        n += 1000;
        Some(a(n))
    });
    let mut report = autotune_probe::runtime_file::Report::default();
    report.measure(&mut FileBytes { position: 0 }, |s| observer.file_check(s));
    assert!(report.complete());
    assert!(gate.pending());
    assert!(observer.before(SamplerCall::Initialize));
    assert!(observer.returned(SamplerCall::Initialize, ok(SamplerOutput::None)));
    drop(observer);
    assert_eq!(l.file_checks().len(), 8);
    assert_eq!(l.entries()[0].before, Some(a(10_000)));
    assert_eq!(l.entries()[0].after, Some(a(11_000)));
    assert_eq!(l.fault(), None);
    assert_eq!(b.used(), baseline);
    assert!(!gate.stopped());
    assert!(gate.pending());
    gate.complete();
    assert!(!gate.pending());
}
#[test]
fn bad_file_clock_deadline_stop_and_unreturned_abi_forbid_successors() {
    use autotune_probe::runtime_file::Stage;
    for mode in 0..6 {
        let b = budget();
        let gate = NativeAdmission::default();
        let mut l = ledger(10_000_000_000, &b);
        assert!(l
            .observer(&gate, || Some(a(2000)))
            .file_check(Stage::MetadataBefore));
        if mode == 4 {
            gate.stop();
        }
        if mode == 5 {
            assert!(l
                .observer(&gate, || Some(a(2500)))
                .before(SamplerCall::Count));
        }
        let clock = match mode {
            0 => None,
            1 => Some(a(10_000_000_000)),
            2 => Some(a(0)),
            3 => {
                let mut v = a(3000);
                v.clock_token = [9; 16];
                Some(v)
            }
            _ => Some(a(3000)),
        };
        let mut o = l.observer(&gate, || clock);
        assert!(!o.file_check(Stage::MetadataAfter));
        assert!(!o.before(SamplerCall::Initialize));
        drop(o);
        assert_eq!(l.file_checks().last().unwrap().anchor, clock);
        assert!(gate.stopped());
        assert_eq!(
            l.fault(),
            Some(match mode {
                0 | 2 | 3 => Fault::Clock,
                1 => Fault::Deadline,
                4 => Fault::Stopped,
                _ => Fault::Protocol,
            })
        );
        assert!(l.entries().last().unwrap().raw.is_none());
    }
}
#[test]
fn file_history_is_prepaid_keeps_first_overflow_and_never_grows() {
    use autotune_probe::runtime_file::{Stage, MAX_CHECKS};
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let used = b.used();
    let mut n = 1000;
    let mut o = l.observer(&gate, || {
        n += 100;
        Some(a(n))
    });
    for index in 0..MAX_CHECKS {
        assert!(o.file_check(Stage::ReadBefore(index as u32)));
    }
    assert!(!o.file_check(Stage::FinalMetadataAfter));
    drop(o);
    let overflow = l.file_overflow();
    assert!(!l
        .observer(&gate, || Some(a(1_000_000)))
        .file_check(Stage::LibraryLoadBefore));
    assert_eq!(l.file_checks().len(), MAX_CHECKS);
    assert_eq!(l.file_checks()[0].anchor, Some(a(1100)));
    assert_eq!(l.file_overflow(), overflow);
    assert_eq!(b.used(), used);
    assert_eq!(l.fault(), Some(Fault::Capacity));
    assert!(gate.stopped());
}
#[test]
fn file_post_read_unwind_keeps_raw_progress_and_closes_unknown_outer_call() {
    let b = budget();
    let gate = NativeAdmission::default();
    gate.begin().unwrap();
    let mut l = ledger(10_000_000_000, &b);
    let mut report = autotune_probe::runtime_file::Report::default();
    let mut n = 0;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut o = l.observer(&gate, || {
            let i = n;
            n += 1;
            if i == 3 {
                panic!("post-read clock");
            }
            Some(a((n * 1000 + 1000) as u64))
        });
        report.measure(&mut FileBytes { position: 0 }, |s| o.file_check(s));
    }));
    assert!(result.is_err());
    assert_eq!(report.last_read, Some(Ok(3)));
    assert_eq!(report.sha256, None);
    assert!(gate.stopped());
    assert!(gate.pending());
    assert_eq!(l.fault(), Some(Fault::Protocol));
    assert!(l.entries().is_empty());
}
#[test]
fn failed_native_return_is_first_fault_even_when_post_clock_unwinds() {
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut n = 0;
    let raw = SamplerReturn {
        status: -7,
        output: SamplerOutput::Count(u32::MAX),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut o = l.observer(&gate, || {
            n += 1;
            if n == 2 {
                panic!("post-native clock");
            }
            Some(a(2000))
        });
        assert!(o.before(SamplerCall::Count));
        o.returned(SamplerCall::Count, raw);
    }));
    assert!(result.is_err());
    assert_eq!(l.entries()[0].raw, Some(raw));
    assert_eq!(l.entries()[0].after, None);
    assert_eq!(l.fault(), Some(Fault::Native(-7)));
    assert!(gate.stopped());
}

fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [3; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}
fn budget() -> Budget {
    Budget::new(32 * 1024 * 1024).unwrap()
}
fn ok(output: SamplerOutput) -> SamplerReturn {
    SamplerReturn { status: 0, output }
}
fn ledger(deadline_ns: u64, b: &Budget) -> Ledger {
    Ledger::new(FREQUENCY, a(0), a(1000), deadline_ns, b)
        .ok()
        .unwrap()
}

#[test]
fn raw_before_and_after_are_retained_for_each_attempted_call() {
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut anchors = [a(2000), a(3000), a(4000), a(5000)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(o.before(SamplerCall::Initialize));
        assert!(o.returned(SamplerCall::Initialize, ok(SamplerOutput::None)));
        assert!(o.before(SamplerCall::Count));
        assert!(o.returned(SamplerCall::Count, ok(SamplerOutput::Count(1))));
    }
    assert_eq!(l.fault(), None);
    assert!(!gate.stopped());
    let entries = l.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].call, SamplerCall::Initialize);
    assert_eq!(entries[0].before, Some(a(2000)));
    assert_eq!(entries[0].after, Some(a(3000)));
    assert!(entries[0].attempted);
    assert_eq!(entries[0].raw, Some(ok(SamplerOutput::None)));
    assert_eq!(entries[1].call, SamplerCall::Count);
    assert_eq!(entries[1].before, Some(a(4000)));
    assert_eq!(entries[1].after, Some(a(5000)));
    assert!(entries[1].attempted);
    assert_eq!(entries[1].raw, Some(ok(SamplerOutput::Count(1))));
}

#[test]
fn stopped_gate_rejects_before_any_attempt_and_is_not_renewed() {
    let b = budget();
    let gate = NativeAdmission::default();
    gate.stop();
    let mut l = ledger(10_000_000_000, &b);
    let mut anchors = [a(2000)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(!o.before(SamplerCall::Count));
    }
    assert_eq!(l.fault(), Some(Fault::Stopped));
    assert!(gate.stopped());
    assert_eq!(l.entries().len(), 1);
    assert_eq!(l.entries()[0].before, Some(a(2000)));
    assert!(!l.entries()[0].attempted);
    assert_eq!(l.entries()[0].raw, None);
}

#[test]
fn deadline_boundary_rejects_before_and_after_with_raw_retained() {
    let b = budget();
    // Equality with the deadline loses, both before the call and at return.
    let gate = NativeAdmission::default();
    let mut l = ledger(5000, &b);
    let mut anchors = [a(5000)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(!o.before(SamplerCall::Count));
    }
    assert_eq!(l.fault(), Some(Fault::Deadline));
    assert!(gate.stopped());
    assert_eq!(l.entries().len(), 1);
    assert!(!l.entries()[0].attempted);

    let gate = NativeAdmission::default();
    let mut l = ledger(5000, &b);
    let mut anchors = [a(4000), a(5000)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(o.before(SamplerCall::Count));
        assert!(!o.returned(SamplerCall::Count, ok(SamplerOutput::Count(1))));
    }
    assert_eq!(l.fault(), Some(Fault::Deadline));
    assert!(gate.stopped());
    assert_eq!(l.entries()[0].after, Some(a(5000)));
    assert_eq!(l.entries()[0].raw, Some(ok(SamplerOutput::Count(1))));
}

#[test]
fn missing_or_regressing_clock_anchor_is_a_fault() {
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut anchors = [None].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next().flatten());
        assert!(!o.before(SamplerCall::Count));
    }
    assert_eq!(l.fault(), Some(Fault::Clock));
    assert!(gate.stopped());
    assert_eq!(l.entries()[0].before, None);
    assert!(!l.entries()[0].attempted);

    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut anchors = [a(500)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(!o.before(SamplerCall::Count));
    }
    assert_eq!(l.fault(), Some(Fault::Clock));
    assert!(gate.stopped());
    assert_eq!(l.entries()[0].before, Some(a(500)));
    assert!(!l.entries()[0].attempted);
}

#[test]
fn unpaired_or_mismatched_return_is_a_protocol_fault() {
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut anchors = [a(2000)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(!o.returned(SamplerCall::Count, ok(SamplerOutput::Count(1))));
    }
    assert_eq!(l.fault(), Some(Fault::Protocol));
    assert!(gate.stopped());
    assert!(l.entries().is_empty());
    assert_eq!(
        l.rejected_return(),
        Some((
            SamplerCall::Count,
            ok(SamplerOutput::Count(1)),
            Some(a(2000))
        ))
    );

    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut anchors = [a(2000), a(3000)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(o.before(SamplerCall::Initialize));
        assert!(!o.returned(SamplerCall::Count, ok(SamplerOutput::Count(1))));
    }
    assert_eq!(l.fault(), Some(Fault::Protocol));
    assert!(gate.stopped());
    assert_eq!(l.entries()[0].raw, None);
    assert_eq!(
        l.rejected_return(),
        Some((
            SamplerCall::Count,
            ok(SamplerOutput::Count(1)),
            Some(a(3000))
        ))
    );
}

#[test]
fn native_status_closes_the_lane_before_successors() {
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut anchors = [a(2000), a(3000), a(4000)].into_iter();
    {
        let mut o = l.observer(&gate, || anchors.next());
        assert!(o.before(SamplerCall::Initialize));
        assert!(!o.returned(
            SamplerCall::Initialize,
            SamplerReturn {
                status: 7,
                output: SamplerOutput::None,
            }
        ));
        assert!(!o.before(SamplerCall::Count));
    }
    assert_eq!(l.fault(), Some(Fault::Native(7)));
    assert!(gate.stopped());
    assert_eq!(l.entries().len(), 2);
    assert_eq!(l.entries()[0].raw.unwrap().status, 7);
    assert_eq!(l.entries()[0].after, Some(a(3000)));
    assert!(!l.entries()[1].attempted);
    assert_eq!(l.entries()[1].raw, None);
}

#[test]
fn capacity_is_fixed_and_retains_the_earliest_entries() {
    let b = budget();
    let gate = NativeAdmission::default();
    let mut l = ledger(10_000_000_000, &b);
    let mut ns = 1000;
    {
        let mut o = l.observer(&gate, || {
            ns += 1000;
            Some(a(ns))
        });
        let mut attempted = 0;
        for _ in 0..SURFACE + 16 {
            if !o.before(SamplerCall::Count) {
                break;
            }
            assert!(o.returned(SamplerCall::Count, ok(SamplerOutput::Count(1))));
            attempted += 1;
        }
        assert_eq!(attempted, SURFACE);
    }
    assert_eq!(l.fault(), Some(Fault::Capacity));
    assert!(gate.stopped());
    assert_eq!(l.entries().len(), SURFACE);
    // No eviction: the first observation is still present.
    assert_eq!(l.entries()[0].before, Some(a(2000)));
    assert_eq!(l.entries()[SURFACE - 1].call, SamplerCall::Count);
    assert_eq!(
        l.overflow().map(|(call, anchor)| (call, anchor.is_some())),
        Some((SamplerCall::Count, true))
    );
}

#[test]
fn construction_rejects_deadline_clock_and_capacity_before_any_call() {
    let b = budget();
    assert_eq!(
        Ledger::new(FREQUENCY, a(0), a(1000), 1000, &b).err(),
        Some(Fault::Deadline)
    );
    assert_eq!(
        Ledger::new(0, a(0), a(1000), 10_000_000_000, &b).err(),
        Some(Fault::Clock)
    );
    assert_eq!(
        Ledger::new(FREQUENCY, a(1000), a(500), 10_000_000_000, &b).err(),
        Some(Fault::Clock)
    );
    let tiny = Budget::new(1).unwrap();
    assert_eq!(
        Ledger::new(FREQUENCY, a(0), a(1000), 10_000_000_000, &tiny).err(),
        Some(Fault::Capacity)
    );
}
