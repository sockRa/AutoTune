mod bound_setup_tests {
    use super::*;
    use crate::{
        evidence::Digest,
        monitor::Anchor,
        sampler::budget::Budget,
        workload::{
            control::{Shared, State as Control},
            cuda_setup_trace::{Fault as TraceFault, Ledger},
            manifest::{Artifacts, PilotDraft, PilotManifest},
            wire::WorkerBinding,
        },
    };
    use std::sync::Arc;
    // Nonzero shared epoch tests that deadlines are never independently rebased.
    fn a(ns: u64) -> Anchor {
        let raw = ns + 1_000_000_000;
        Anchor {
            clock_token: [5; 16],
            qpc_before: raw,
            qpc_after: raw,
            interrupt_100ns: raw / 100,
            unbiased_100ns: raw / 100,
        }
    }
    fn binding(budget: &Budget) -> WorkerBinding {
        let d = |n| Digest::parse(&format!("{n:02x}").repeat(32)).unwrap();
        let manifest = PilotManifest::freeze(
            PilotDraft {
                session: [1; 16],
                runs: [[2; 16], [3; 16], [4; 16]],
                clock_token: [5; 16],
                raw_uuid: "GPU-01010101-0101-0101-0101-010101010101".into(),
                raw_pci: "0000:00:00.0".into(),
                source_commit: "a".repeat(40),
                artifacts: Artifacts {
                    source_tree: d(6),
                    lockfile: d(7),
                    controller: d(8),
                    worker: d(9),
                    sampler: d(10),
                    ptx: d(11),
                    protocol: d(12),
                },
                starting_evidence: d(13),
                environment_evidence: d(14),
                approval_ref: "fixture, no execution authority".into(),
                output_root: "D:\\capture\\pilot".into(),
            },
            budget,
        )
        .unwrap();
        WorkerBinding::new(&manifest.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap()
    }
    fn setup() -> (Budget, WorkerBinding, Arc<Shared>, Ledger) {
        let budget = Budget::new(16 * 1024 * 1024).unwrap();
        let binding = binding(&budget);
        let shared = Arc::new(Shared::default());
        let _state = Control::new(binding, a(0), shared.clone(), &budget).unwrap();
        shared.configure_setup(binding, 1_000_000_000).unwrap();
        let ledger = Ledger::new(binding, shared.clone(), a(0), &budget).unwrap();
        (budget, binding, shared, ledger)
    }
    #[test]
    fn actual_setup_engine_uses_bound_clock_control_ledger_without_loading() {
        let _lock = serial();
        let (_budget, binding, shared, mut ledger) = setup();
        let mut backend = setup_fixture();
        let mut n = 0;
        backend
            .setup_observed(&mut ledger.observer(|| {
                n += 1000;
                Some(a(n))
            }))
            .unwrap();
        assert_eq!(ledger.entries().len(), 20);
        assert!(ledger
            .entries()
            .iter()
            .all(|e| e.attempted && e.raw.is_some() && e.before.is_some() && e.after.is_some()));
        assert_eq!(ledger.fault(), None);
        assert!(!ledger.pending());
        assert!(!shared.admission().pending());
        assert!(!shared.admission().stopped());
        assert_eq!(shared.setup_observations().1, None);
        shared.complete_setup(binding, a(n + 1000)).unwrap();
        assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 20);
        drop(backend);
        assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
    }
    unsafe extern "system" fn unique_uuid(out: *mut Uuid, device: i32) -> i32 {
        *out = Uuid {
            bytes: if device == 0 { [1; 16] } else { [2; 16] },
        };
        setup_status()
    }
    #[test]
    fn maximum_device_enumeration_fits_prepaid_ledger() {
        let _lock = serial();
        let (_budget, _binding, _shared, mut ledger) = setup();
        let mut backend = setup_fixture();
        SETUP_COUNT.store(64, Ordering::SeqCst);
        backend.api = Api::fake(|name| {
            if name == "cuDeviceGetUuid_v2" {
                Some(Box::new(unique_uuid as DeviceUuid) as Box<dyn std::any::Any>)
            } else {
                setup_symbols(name)
            }
        })
        .unwrap();
        let mut n = 0;
        backend
            .setup_observed(&mut ledger.observer(|| {
                n += 1000;
                Some(a(n))
            }))
            .unwrap();
        assert_eq!(ledger.entries().len(), 146);
        assert_eq!(ledger.fault(), None);
        assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 146);
        assert!(ledger.overflow().is_none());
    }
    #[test]
    fn missing_clock_or_control_stop_at_every_boundary_prevents_successor() {
        let _lock = serial();
        for boundary in 0..40 {
            for clock_lost in [false, true] {
                let (_budget, _binding, shared, mut ledger) = setup();
                let mut backend = setup_fixture();
                let mut i = 0;
                let result = backend.setup_observed(&mut ledger.observer(|| {
                    let at = i;
                    i += 1;
                    if at == boundary {
                        if clock_lost {
                            return None;
                        }
                        shared.close();
                    }
                    Some(a(i * 1000))
                }));
                assert!(result.is_err());
                assert_eq!(
                    ledger.fault(),
                    Some(if clock_lost {
                        TraceFault::Clock
                    } else {
                        TraceFault::Stopped
                    })
                );
                assert_eq!(
                    SETUP_NATIVE.load(Ordering::SeqCst),
                    (boundary as usize).div_ceil(2)
                );
                assert!(!ledger.pending());
                assert!(shared.admission().stopped());
                assert!(!shared.admission().pending());
                if boundary % 2 == 1 {
                    assert!(ledger.entries().last().unwrap().raw.is_some());
                }
                drop(backend);
                assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
            }
        }
    }
    #[test]
    fn late_return_uses_original_setup_cap_before_control_thread_ticks() {
        let _lock = serial();
        for n in [1_000_000_000, 1_500_000_000] {
            let (_budget, _binding, shared, mut ledger) = setup();
            let mut backend = setup_fixture();
            let mut count = 0;
            assert!(backend
                .setup_observed(&mut ledger.observer(|| {
                    count += 1;
                    Some(a(if count == 1 { 1000 } else { n }))
                }))
                .is_err());
            assert_eq!(ledger.fault(), Some(TraceFault::Deadline));
            assert_eq!(ledger.entries()[0].raw.unwrap().status, 0);
            assert_eq!(shared.cooperative_raw(), a(2_000_000_000).interrupt_100ns);
            assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 1);
        }
    }
    #[test]
    fn raw_native_errors_and_return_survive_post_clock_panic() {
        let _lock = serial();
        for failed in 0..20 {
            let (_budget, _binding, shared, mut ledger) = setup();
            let mut backend = setup_fixture();
            SETUP_FAIL_AT.store(failed, Ordering::SeqCst);
            let mut n = 0;
            assert!(backend
                .setup_observed(&mut ledger.observer(|| {
                    n += 1000;
                    Some(a(n))
                }))
                .is_err());
            assert_eq!(ledger.fault(), Some(TraceFault::Native(-71)));
            assert_eq!(ledger.entries().last().unwrap().raw.unwrap().status, -71);
            assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), failed + 1);
            assert!(shared.admission().stopped());
        }
        let (_budget, _binding, shared, mut ledger) = setup();
        let mut backend = setup_fixture();
        let mut i = 0;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = backend.setup_observed(&mut ledger.observer(|| {
                i += 1;
                if i == 2 {
                    panic!("fixture post-clock panic");
                }
                Some(a(1000))
            }));
        }));
        assert!(result.is_err());
        assert_eq!(ledger.entries()[0].raw.unwrap().status, 0);
        assert!(!shared.admission().pending());
        assert_eq!(SETUP_NATIVE.load(Ordering::SeqCst), 1);
        assert!(shared.admission().stopped());
        assert_eq!(shared.cooperative_raw(), 0);
        drop(backend);
        assert_eq!(SETUP_CLEANUP.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn mismatched_return_keeps_original_pending_and_closes_admission() {
        let (_budget, _binding, shared, mut ledger) = setup();
        let raw = SetupReturn {
            status: 0,
            output: SetupOutput::Integer(12),
        };
        let mut n = 0;
        let mut observer = ledger.observer(|| {
            n += 1000;
            Some(a(n))
        });
        observer.before(SetupOperation::Initialize).unwrap();
        assert!(observer.returned(SetupOperation::DeviceCount, raw).is_err());
        drop(observer);
        assert!(ledger.pending());
        assert!(shared.admission().pending());
        assert!(shared.admission().stopped());
        assert_eq!(ledger.rejected_return().unwrap().1, raw);
        assert_eq!(ledger.entries()[0].raw, None);
    }
    #[test]
    fn capacity_clock_regression_and_completed_setup_close_successors() {
        let (_budget, binding, shared, mut ledger) = setup();
        let raw = SetupReturn {
            status: 0,
            output: SetupOutput::None,
        };
        let mut n = 0;
        for _ in 0..146 {
            let mut o = ledger.observer(|| {
                n += 1000;
                Some(a(n))
            });
            o.before(SetupOperation::Initialize).unwrap();
            o.returned(SetupOperation::Initialize, raw).unwrap();
        }
        assert!(ledger
            .observer(|| Some(a(n + 1000)))
            .before(SetupOperation::Initialize)
            .is_err());
        assert_eq!(ledger.entries().len(), 146);
        assert!(ledger.overflow().is_some());
        assert_eq!(ledger.fault(), Some(TraceFault::Capacity));
        assert!(shared.admission().stopped());
        let (_budget, _, shared, mut ledger) = setup();
        let mut o = ledger.observer(|| Some(a(1000)));
        o.before(SetupOperation::Initialize).unwrap();
        o.returned(SetupOperation::Initialize, raw).unwrap();
        drop(o);
        assert!(ledger
            .observer(|| Some(a(0)))
            .before(SetupOperation::Initialize)
            .is_err());
        assert_eq!(ledger.fault(), Some(TraceFault::Clock));
        assert!(shared.admission().stopped());
        let (_budget, _, shared, mut ledger) = setup();
        shared.complete_setup(binding, a(1000)).unwrap();
        assert!(ledger
            .observer(|| Some(a(2000)))
            .before(SetupOperation::Initialize)
            .is_err());
        assert_eq!(ledger.fault(), Some(TraceFault::Protocol));
    }
    #[test]
    fn constructor_rejects_missing_binding_expired_deadline_and_budget() {
        let (budget, binding, _, _ledger) = setup();
        let shared = Arc::new(Shared::default());
        assert!(matches!(
            Ledger::new(binding, shared.clone(), a(0), &budget),
            Err(TraceFault::Binding)
        ));
        assert!(shared.admission().stopped());
        let (_, binding, shared, _ledger) = setup();
        assert!(matches!(
            Ledger::new(binding, shared.clone(), a(1_000_000_000), &budget),
            Err(TraceFault::Deadline)
        ));
        assert_eq!(shared.cooperative_raw(), a(2_000_000_000).interrupt_100ns);
        let (_, binding, shared, _ledger) = setup();
        let tiny = Budget::new(1).unwrap();
        assert!(matches!(
            Ledger::new(binding, shared.clone(), a(1000), &tiny),
            Err(TraceFault::Capacity)
        ));
        assert!(shared.admission().stopped());
    }
}
