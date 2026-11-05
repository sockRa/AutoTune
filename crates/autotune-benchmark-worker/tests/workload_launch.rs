use autotune_benchmark_worker::{
    evidence::Digest,
    monitor::Anchor,
    sampler::budget::Budget,
    workload::{
        launch::{Arguments, Declaration, Role},
        manifest::{Artifacts, PilotDraft, PilotManifest},
    },
};
include!("support/worker_setup.rs");
fn digest(n: u8) -> Digest {
    Digest::parse(&format!("{n:02x}").repeat(32)).unwrap()
}
fn manifest(b: &Budget) -> PilotManifest {
    PilotManifest::freeze(
        PilotDraft {
            session: [1; 16],
            runs: [[2; 16], [3; 16], [4; 16]],
            clock_token: [5; 16],
            raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
            raw_pci: "0000:01:02.0".into(),
            source_commit: "a".repeat(40),
            artifacts: Artifacts {
                source_tree: digest(6),
                lockfile: digest(7),
                controller: digest(8),
                worker: digest(9),
                sampler: digest(10),
                ptx: digest(11),
                protocol: digest(12),
            },
            starting_evidence: digest(13),
            environment_evidence: digest(14),
            approval_ref: "not execution authority".into(),
            output_root: "D:\\capture\\pilot".into(),
        },
        b,
    )
    .unwrap()
}
fn a(ns: u64) -> Anchor {
    Anchor {
        clock_token: [5; 16],
        qpc_before: ns,
        qpc_after: ns,
        interrupt_100ns: ns / 100,
        unbiased_100ns: ns / 100,
    }
}

#[test]
fn fixed_arguments_roundtrip_role_manifest_run_origin_and_epoch() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    for (i, role) in [(0, Role::Worker), (1, Role::Sampler), (2, Role::Worker)] {
        let run = m.bind_run(i, 10_000_000_000).unwrap();
        let d = Declaration::new(&run, 1_000_000_000, a(100), role, &b).unwrap();
        let args = d.arguments([100, 200, 300, 400, 500]).unwrap();
        assert_eq!(args.len(), 16);
        assert_eq!(
            args[0],
            if role == Role::Worker {
                "--pilot-worker-v1"
            } else {
                "--pilot-sampler-v1"
            }
        );
        assert_eq!(args[6], m.digest().to_hex());
        assert_eq!(args[8], "1000000000");
        assert_eq!(args[10], "100");
        assert_eq!(args[14], "10000000000");
        let parsed = Arguments::parse(&args).unwrap();
        assert_eq!(parsed.handles(), [100, 200, 300, 400, 500]);
        assert_eq!(parsed.role(), role);
        assert_eq!(parsed.epoch(), a(100));
        assert_eq!(parsed.frequency(), 1_000_000_000);
        let bound = parsed.bind(&m, &b).unwrap();
        assert_eq!(bound.run(), run.run());
        assert_eq!(bound.plan(), run.plan());
        assert_eq!(bound.origin_ns(), run.origin_ns());
        assert_eq!(
            d.image_digest(),
            if role == Role::Worker {
                digest(9)
            } else {
                digest(10)
            }
        );
    }
}
#[test]
fn tampering_cannot_change_role_scope_or_original_clock() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    let args = d.arguments([100, 200, 300, 400, 500]).unwrap();
    for (index, value) in [
        (0, "--pilot-sampler-v1".into()),
        (6, "ab".repeat(32)),
        (7, "03".repeat(16)),
        (8, "999999999".into()),
        (9, "06".repeat(16)),
        (10, "1".into()),
        (11, "1".into()),
        (12, "1".into()),
        (13, "1".into()),
        (14, "100".into()),
        (15, "00".repeat(32)),
    ] {
        let mut changed = args.clone();
        changed[index] = value;
        assert!(
            Arguments::parse(&changed)
                .and_then(|p| p.bind(&m, &b))
                .is_err(),
            "changed token {index}"
        );
    }
}
#[test]
fn malformed_or_aliased_handles_never_reach_os_adoption() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    for handles in [[0, 2, 3, 4, 5], [1, 1, 3, 4, 5], [1, 2, 3, 4, usize::MAX]] {
        assert!(d.arguments(handles).is_err());
    }
    let args = d.arguments([1, 2, 3, 4, 5]).unwrap();
    for value in [
        "0",
        "01",
        "+1",
        "-1",
        " 1",
        "1 ",
        "18446744073709551616",
        "",
    ] {
        let mut changed = args.clone();
        changed[1] = value.into();
        assert!(Arguments::parse(&changed).is_err());
    }
    let mut changed = args.clone();
    changed.push("unexpected".into());
    assert!(Arguments::parse(&changed).is_err());
    let mut changed = args.clone();
    changed[15] = "a".repeat(129);
    assert!(Arguments::parse(&changed).is_err());
    let mut changed = args;
    changed[0] = "--sampler-v1".into();
    assert!(Arguments::parse(&changed).is_err());
}
#[test]
fn foreign_manifest_and_capacity_failures_preserve_budget() {
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let baseline = b.used();
    {
        let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Sampler, &b).unwrap();
        let parsed = Arguments::parse(&d.arguments([1, 2, 3, 4, 5]).unwrap()).unwrap();
        let other = PilotManifest::parse(
            m.canonical()
                .replace(&"a".repeat(40), &"b".repeat(40))
                .as_bytes(),
            &b,
        )
        .unwrap();
        assert!(parsed.bind(&other, &b).is_err());
        let small = Budget::new(100).unwrap();
        assert!(parsed.bind(&m, &small).is_err());
        assert_eq!(small.used(), 0);
    }
    assert_eq!(b.used(), baseline);
}
#[test]
fn fixed_images_limits_and_child_environment_follow_role() {
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    assert_eq!(Role::Worker.image_name(), "autotune-pilot-worker.exe");
    assert_eq!(Role::Sampler.image_name(), "autotune-pilot-sampler.exe");
    assert_eq!(
        Role::Worker.limits(),
        (p.worker_commit_bytes, p.worker_cpu_ns_per_run)
    );
    assert_eq!(
        Role::Sampler.limits(),
        (p.sampler_commit_bytes, p.sampler_cpu_ns_per_session)
    );
    assert_eq!(
        Role::Worker.environment(),
        "CUDA_CACHE_DISABLE=1\0CUDA_MODULE_LOADING=EAGER\0\0"
            .encode_utf16()
            .collect::<Vec<_>>()
    );
    assert_eq!(Role::Sampler.environment(), vec![0, 0]);
    assert!(!String::from_utf16(&Role::Worker.environment())
        .unwrap()
        .contains("PATH"));
}

#[test]
fn cancellation_closes_an_existing_sender_without_losing_frame_or_control() {
    use autotune_benchmark_worker::{
        evidence::Reason,
        sampler::wire::Lane,
        workload::{
            launch::CancelQueue,
            transport::{Queue, QueuedBytes, SendFailure, Sender},
            wire::{Channel, Direction, Message},
        },
    };
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    struct Memory {
        b: Budget,
        frames: Rc<RefCell<Vec<(Lane, QueuedBytes)>>>,
    }
    impl Queue for Memory {
        fn try_send(&mut self, lane: Lane, bytes: &[u8]) -> Result<(), SendFailure> {
            self.frames
                .borrow_mut()
                .push((lane, QueuedBytes::copy(bytes, &self.b)?));
            Ok(())
        }
    }
    let b = Budget::new(8 * 1024 * 1024).unwrap();
    let m = manifest(&b);
    let run = m.bind_run(0, 0).unwrap();
    let d = Declaration::new(&run, 1_000_000_000, a(0), Role::Worker, &b).unwrap();
    let frames = Rc::new(RefCell::new(Vec::new()));
    let cancelled = Arc::new(AtomicBool::new(false));
    let queue = CancelQueue::new(
        Memory {
            b: b.clone(),
            frames: frames.clone(),
        },
        cancelled.clone(),
    );
    let mut sender = Sender::new(queue, d.binding(), Direction::ToWorker, &b).unwrap();
    let mut ordinary =
        Channel::new(d.binding(), Lane::Ordinary, Direction::ToWorker, b.clone()).unwrap();
    let frame = ordinary
        .encode(&Message::Start { issued: a(1000) })
        .unwrap();
    let pointer = frame.as_bytes().as_ptr();
    cancelled.store(true, Ordering::Release);
    let returned = sender.send_worker(frame).err().unwrap();
    assert_eq!(returned.as_bytes().as_ptr(), pointer);
    assert_eq!(sender.fault(Lane::Ordinary), Some(SendFailure::Closed));
    assert!(frames.borrow().is_empty());
    let mut control =
        Channel::new(d.binding(), Lane::Control, Direction::ToWorker, b.clone()).unwrap();
    let frame = control
        .encode(&Message::Stop {
            reason: Reason::new("fixture-stop").unwrap(),
            trigger_ns: 1000,
            cooperative_ns: 1_000_001_000,
            exit_ns: 3_000_001_000,
        })
        .unwrap();
    assert!(sender.send_worker(frame).is_ok());
    assert_eq!(frames.borrow()[0].0, Lane::Control);
    cancelled.store(false, Ordering::Release); // even an erroneous signal reset cannot reopen this sender
    assert!(sender.send_worker(returned).is_err());
    assert_eq!(frames.borrow().len(), 1);
}

#[test]
fn child_options_declared_block_and_readback_values_match_without_mutating_environment() {
    use autotune_benchmark_worker::workload::child_options::{Variable, ENVIRONMENT};
    let entries: Vec<_> = ENVIRONMENT.trim_end_matches('\0').split('\0').collect();
    assert_eq!(entries.len(), 2);
    for (entry, v) in entries.iter().zip(Variable::ALL) {
        assert_eq!(*entry, format!("{}={}", v.name(), v.expected()));
    }
}
#[test]
fn child_option_readback_rejects_failed_oversized_empty_and_nonexact_values() {
    use autotune_benchmark_worker::workload::child_options::{Read, Variable};
    for variable in Variable::ALL {
        let mut raw = Read {
            length: variable.expected().len() as u32,
            error: None,
            units: [0; 16],
        };
        for (slot, unit) in raw.units.iter_mut().zip(variable.expected().encode_utf16()) {
            *slot = unit;
        }
        assert!(raw.matches(variable));
        for length in [0, 16, u32::MAX] {
            assert!(!Read { length, ..raw }.matches(variable));
        }
        assert!(!Read {
            error: Some(203),
            ..raw
        }
        .matches(variable));
        let mut other = raw;
        other.units[raw.length as usize] = 1;
        assert!(!other.matches(variable));
        other = raw;
        other.units[0] = 0xd800;
        assert!(!other.matches(variable));
    }
}
#[test]
fn child_option_unknown_read_stops_without_retry_and_keeps_raw_failure() {
    use autotune_benchmark_worker::workload::child_options::{Facts, Read};
    for raw in [
        Read {
            length: 0,
            error: Some(203),
            units: [0; 16],
        },
        Read {
            length: u32::MAX,
            error: None,
            units: [0xd800; 16],
        },
    ] {
        let mut facts = Facts::default();
        let mut calls = 0;
        assert_eq!(
            facts.observe::<()>(|_, slot| {
                calls += 1;
                *slot = Some(raw);
                Ok(())
            }),
            Ok(false)
        );
        assert_eq!(calls, 1);
        assert_eq!(facts.cache_disable, Some(raw));
        assert_eq!(facts.module_loading, None);
    }
}
