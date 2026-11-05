use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::{budget::Budget, wire::Lane},
    workload::{
        manifest::Artifacts,
        starting::{Scope, ScopeDraft},
        starting_bootstrap::{Arguments, Bound, Declaration, Error, Frame, Sender},
        transport::{Queue, QueuedBytes, SendFailure},
    },
};
fn d(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn budget() -> Budget {
    Budget::new(32 * 1024 * 1024).unwrap()
}
#[test]
fn cancellation_after_sender_creation_closes_original_handoff() {
    use autotune_benchmark_worker::workload::launch::CancelQueue;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let b = budget();
    let s = scope(&b);
    let d = declaration(&s, &b);
    let cancelled = Arc::new(AtomicBool::new(false));
    let queue = CancelQueue::new(
        Sink {
            budget: b.clone(),
            bytes: None,
            failure: None,
        },
        cancelled.clone(),
    );
    let mut sender = Sender::new(queue, &d, &b).ok().unwrap();
    cancelled.store(true, Ordering::Release);
    let frame = Frame::encode(&d, &s, &b).unwrap();
    let ptr = frame.as_bytes().as_ptr();
    let returned = sender.send(frame).err().unwrap();
    assert_eq!(returned.as_bytes().as_ptr(), ptr);
    assert_eq!(sender.fault(), Some(SendFailure::Closed));
    cancelled.store(false, Ordering::Release);
    assert!(sender.send(returned).is_err());
}
#[cfg(windows)]
#[test]
fn fixed_windows_starting_process_api_links_without_launching() {
    use autotune_benchmark_worker::workload::windows_process::{Launch, Process};
    // Take the actual monomorphized functions, never invoke them. Compilation
    // checks the retained-process launch/observe APIs for the separate scope.
    let _start: fn(
        Declaration,
        Budget,
        &mut autotune_benchmark_worker::workload::consent::Record,
        &autotune_benchmark_worker::monitor::windows_clock::WindowsClock,
    ) -> std::io::Result<Launch<Declaration>> = Launch::<Declaration>::start;
    let _poll = Launch::<Declaration>::poll;
    let _sender = Process::<Declaration>::starting_sender;
    type ReceiveFn = fn(
        &Process<Declaration>,
        fn() -> Option<Anchor>,
    ) -> autotune_benchmark_worker::workload::windows_transport::Receive;
    let _receive: ReceiveFn = Process::<Declaration>::receive;
    let _observe = Process::<Declaration>::observe;
    let _resources = Process::<Declaration>::resources;
    let _terminate = Process::<Declaration>::request_termination;
}
#[cfg(windows)]
#[test]
fn starting_child_context_has_separate_typed_entry_without_adopting_any_handles() {
    use autotune_benchmark_worker::workload::child_windows::{Context, Failure};
    type StartingContext = Context<Bound>;
    type Entry = unsafe fn(&Arguments, Budget) -> Result<StartingContext, Failure>;
    let _entry: Entry = StartingContext::from_starting_inherited;
    let _scope = StartingContext::bootstrap;
    let _raw = StartingContext::raw;
    let _image = StartingContext::image;
    let _parts = StartingContext::parts;
}
#[test]
fn child_wait_before_first_packet_uses_original_preflight_deadline() {
    let b = budget();
    let s = scope(&b);
    let d = declaration(&s, &b);
    let args = arguments(&d);
    assert_eq!(args.preflight_end_100ns(a(1000)).unwrap(), 100_000_000);
    assert_eq!(
        args.preflight_end_100ns(a(9_999_999_900)).unwrap(),
        100_000_000
    );
    assert_eq!(
        args.preflight_end_100ns(a(10_000_000_000)),
        Err(Error::Deadline)
    );
    assert_eq!(
        args.preflight_end_100ns(a(11_000_000_000)),
        Err(Error::Deadline)
    );
    let mut bad = a(1000);
    bad.clock_token = [99; 16];
    assert_eq!(args.preflight_end_100ns(bad), Err(Error::Clock));
    let mut bad = a(1000);
    bad.qpc_after = 0;
    assert_eq!(args.preflight_end_100ns(bad), Err(Error::Clock));
}
#[test]
fn original_nonzero_epoch_and_absolute_prebootstrap_overflow_are_checked() {
    let b = budget();
    let s = scope(&b);
    let mut draft = s.draft().clone();
    draft.epoch = a(7_000_000_000);
    let s = Scope::freeze(draft, &b).unwrap();
    let d = Declaration::new(&s, a(7_000_001_000), &b).unwrap();
    let args = arguments(&d);
    assert_eq!(
        args.preflight_end_100ns(a(16_999_999_900)).unwrap(),
        170_000_000
    );
    assert_eq!(
        args.preflight_end_100ns(a(17_000_000_000)),
        Err(Error::Deadline)
    );
    assert_eq!(
        args.preflight_end_100ns(a(6_999_999_900)),
        Err(Error::Clock)
    );
    let mut tokens = d.arguments([1, 2, 3, 4, 5]).unwrap();
    tokens[11] = (u64::MAX - 1).to_string();
    tokens[12] = tokens[11].clone();
    let args = Arguments::parse(&tokens).unwrap();
    let mut first = a(7_000_000_000);
    first.interrupt_100ns = u64::MAX - 1;
    first.unbiased_100ns = u64::MAX - 1;
    assert_eq!(args.preflight_end_100ns(first), Err(Error::Deadline));
}
fn declaration(s: &Scope, b: &Budget) -> Declaration {
    Declaration::new(s, a(1000), b).unwrap()
}
fn arguments(d: &Declaration) -> Arguments {
    Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap()
}
#[test]
fn distinct_pre_manifest_roundtrip_preserves_original_epoch_and_scope() {
    let b = budget();
    let s = scope(&b);
    let d = declaration(&s, &b);
    assert_eq!(d.image_name(), "autotune-pilot-sampler.exe");
    assert_eq!(d.image_digest(), s.draft().artifacts.sampler);
    let args = arguments(&d);
    assert_eq!(args.handles(), [1, 2, 3, 4, 5]);
    assert_eq!(args.epoch(), a(0));
    assert_eq!(args.frequency(), 1_000_000_000);
    let frame = Frame::encode(&d, &s, &b).unwrap();
    let bound = Bound::parse(&args, frame.as_bytes(), a(2000), a(3000), &b)
        .ok()
        .unwrap();
    assert_eq!(bound.scope().canonical(), s.canonical());
    assert_eq!(bound.declaration().entry(), a(1000));
    assert_eq!(bound.first_entry(), a(2000));
    assert_eq!(bound.receipt(), a(3000));
    assert_eq!(bound.deadline_ns(), 10_000_000_000);
    let text = std::str::from_utf8(&frame.as_bytes()[4..]).unwrap();
    assert!(!text.contains("run_id"));
    assert!(!text.contains("pilot_manifest"));
    assert!(
        autotune_benchmark_worker::workload::launch::Arguments::parse(
            &d.arguments([1, 2, 3, 4, 5]).unwrap()
        )
        .is_err()
    );
}
#[test]
fn arguments_reject_aliased_handles_noncanonical_numbers_and_foreign_modes() {
    let b = budget();
    let s = scope(&b);
    let d = declaration(&s, &b);
    for handles in [[0, 2, 3, 4, 5], [1, 1, 3, 4, 5], [1, 2, 3, 4, usize::MAX]] {
        assert!(d.arguments(handles).is_err());
    }
    let original = d.arguments([1, 2, 3, 4, 5]).unwrap();
    for (i, value) in [
        (0, "--pilot-sampler-v1"),
        (1, "01"),
        (1, "-1"),
        (2, "1"),
        (7, "0"),
        (8, "BAD"),
        (13, "xyz"),
    ] {
        let mut args = original.clone();
        args[i] = value.into();
        assert!(Arguments::parse(&args).is_err());
    }
    let mut extra = original.clone();
    extra.push("x".into());
    assert!(Arguments::parse(&extra).is_err());
    let mut long = original;
    long[0] = "x".repeat(129);
    assert!(Arguments::parse(&long).is_err());
}
#[test]
fn original_preflight_deadline_cannot_be_renewed_by_late_launch_or_receipt() {
    let b = budget();
    let s = scope(&b);
    assert!(matches!(
        Declaration::new(&s, a(10_000_000_000), &b),
        Err(Error::Deadline)
    ));
    let d = Declaration::new(&s, a(9_000_000_000), &b).unwrap();
    let args = arguments(&d);
    let frame = Frame::encode(&d, &s, &b).unwrap();
    assert!(Bound::parse(
        &args,
        frame.as_bytes(),
        a(9_000_001_000),
        a(9_999_999_900),
        &b
    )
    .is_ok());
    assert!(matches!(
        Bound::parse(
            &args,
            frame.as_bytes(),
            a(9_000_001_000),
            a(10_000_000_000),
            &b
        ),
        Err(Error::Deadline)
    ));
    assert!(matches!(
        Bound::parse(
            &args,
            frame.as_bytes(),
            a(8_000_000_000),
            a(9_000_000_000),
            &b
        ),
        Err(Error::Clock)
    ));
    let mut suspended = a(9_000_002_000);
    suspended.unbiased_100ns -= 100_000;
    assert!(matches!(
        Bound::parse(&args, frame.as_bytes(), a(9_000_001_000), suspended, &b),
        Err(Error::Clock)
    ));
}
fn framed(text: &str) -> Vec<u8> {
    let mut bytes = ((text.len() + 4) as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}
#[test]
fn rejects_cross_scope_cross_entry_and_malformed_frame_without_placeholder_binding() {
    let b = budget();
    let s = scope(&b);
    let d = declaration(&s, &b);
    let args = arguments(&d);
    let mut foreign = s.draft().clone();
    foreign.read_approval_ref = "different read declaration".into();
    let foreign = Scope::freeze(foreign, &b).unwrap();
    assert!(matches!(
        Frame::encode(&d, &foreign, &b),
        Err(Error::Binding)
    ));
    let foreign_d = declaration(&foreign, &b);
    let foreign_frame = Frame::encode(&foreign_d, &foreign, &b).unwrap();
    assert!(Bound::parse(&args, foreign_frame.as_bytes(), a(2000), a(3000), &b).is_err());
    let later = Declaration::new(&s, a(1500), &b).unwrap();
    let later = Frame::encode(&later, &s, &b).unwrap();
    assert!(Bound::parse(&args, later.as_bytes(), a(2000), a(3000), &b).is_err());
    let frame = Frame::encode(&d, &s, &b).unwrap();
    let text = std::str::from_utf8(&frame.as_bytes()[4..]).unwrap();
    for bytes in [
        vec![],
        vec![1, 2, 3],
        frame.as_bytes()[..frame.as_bytes().len() - 1].to_vec(),
        framed(&format!(" {text}")),
        framed(&text.replace("pilot_starting_bootstrap", "pilot_bootstrap")),
    ] {
        assert!(Bound::parse(&args, &bytes, a(2000), a(3000), &b).is_err());
    }
}
struct Sink {
    budget: Budget,
    bytes: Option<QueuedBytes>,
    failure: Option<SendFailure>,
}
impl Queue for Sink {
    fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
        assert_eq!(lane, Lane::Ordinary);
        if let Some(f) = self.failure {
            return Err(f);
        }
        assert!(self.bytes.is_none());
        self.bytes = Some(QueuedBytes::copy(bytes, &self.budget)?);
        Ok(())
    }
}
#[test]
fn handoff_is_one_shot_and_retains_exact_unsent_owner_and_first_error() {
    let b = budget();
    let s = scope(&b);
    let d = declaration(&s, &b);
    let frame = Frame::encode(&d, &s, &b).unwrap();
    let ptr = frame.as_bytes().as_ptr();
    let mut sender = Sender::new(
        Sink {
            budget: b.clone(),
            bytes: None,
            failure: Some(SendFailure::Os(123)),
        },
        &d,
        &b,
    )
    .ok()
    .unwrap();
    let frame = sender.send(frame).err().unwrap();
    assert_eq!(frame.as_bytes().as_ptr(), ptr);
    assert_eq!(sender.fault(), Some(SendFailure::Os(123)));
    let frame = sender.send(frame).err().unwrap();
    assert_eq!(frame.as_bytes().as_ptr(), ptr);
    assert_eq!(sender.fault(), Some(SendFailure::Os(123)));
    let mut sender = Sender::new(
        Sink {
            budget: b.clone(),
            bytes: None,
            failure: None,
        },
        &d,
        &b,
    )
    .ok()
    .unwrap();
    assert!(sender.send(frame).is_ok());
    assert!(sender.send(Frame::encode(&d, &s, &b).unwrap()).is_err());
}
#[test]
fn foreign_frame_poison_and_capacity_failure_prevent_retry() {
    let b = budget();
    let s = scope(&b);
    let d = declaration(&s, &b);
    let foreign = Declaration::new(&s, a(1500), &b).unwrap();
    let mut sender = Sender::new(
        Sink {
            budget: b.clone(),
            bytes: None,
            failure: None,
        },
        &d,
        &b,
    )
    .ok()
    .unwrap();
    assert!(sender
        .send(Frame::encode(&foreign, &s, &b).unwrap())
        .is_err());
    assert_eq!(sender.fault(), Some(SendFailure::Binding));
    assert!(sender.send(Frame::encode(&d, &s, &b).unwrap()).is_err());
    let tiny = Budget::new(1).unwrap();
    assert!(matches!(Frame::encode(&d, &s, &tiny), Err(Error::Capacity)));
    assert!(matches!(
        Bound::parse(
            &arguments(&d),
            Frame::encode(&d, &s, &b).unwrap().as_bytes(),
            a(2000),
            a(3000),
            &tiny
        ),
        Err(Error::Capacity)
    ));
    let mut sender = Sender::new(
        Sink {
            budget: tiny,
            bytes: None,
            failure: None,
        },
        &d,
        &b,
    )
    .ok()
    .unwrap();
    assert!(sender.send(Frame::encode(&d, &s, &b).unwrap()).is_err());
    assert_eq!(sender.fault(), Some(SendFailure::Capacity));
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
fn scope(b: &Budget) -> Scope {
    Scope::freeze(
        ScopeDraft {
            session: [1; 16],
            capture_nonce: [2; 16],
            clock_token: [3; 16],
            frequency: 1_000_000_000,
            epoch: a(0),
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: d(4),
                lockfile: d(5),
                controller: d(6),
                worker: d(7),
                sampler: d(8),
                ptx: d(9),
                protocol: d(10),
            },
            read_approval_ref: "read declaration; not compute consent".into(),
        },
        b,
    )
    .unwrap()
}
