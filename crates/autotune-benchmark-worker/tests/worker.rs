use autotune_benchmark_worker::worker::*;
use autotune_fma_reference::input::{DispatchIndex, Recipe, Shape};
use sha2::{Digest, Sha256};
use std::{cell::Cell, rc::Rc};

const ID: Identity = Identity {
    uuid: [1; 16],
    domain: 0,
    bus: 1,
    device: 0,
};
#[derive(Clone, Copy)]
enum Mode {
    Good,
    Corrupt,
    Partial,
    Nonfinite,
    Stale,
    Length,
    Fault,
    SubmitFault,
    QueryFault,
    Identity,
    Native,
}
struct Fake {
    mode: Mode,
    values: Vec<f32>,
    polls: usize,
    submits: Rc<Cell<usize>>,
}
impl Backend for Fake {
    fn provenance(&self) -> Provenance {
        if matches!(self.mode, Mode::Native) {
            Provenance::Native
        } else {
            Provenance::SimulatedFixture
        }
    }
    fn identity(&mut self) -> Result<Identity, Fault> {
        assert!(
            !matches!(self.mode, Mode::Native),
            "native identity must not be called"
        );
        if matches!(self.mode, Mode::Identity) && self.submits.get() > 0 {
            Ok(Identity { bus: 2, ..ID })
        } else {
            Ok(ID)
        }
    }
    fn submit(
        &mut self,
        a: &[f32],
        b: &[f32],
        x: &[f32],
        k: u32,
        sentinel: &[f32],
    ) -> Result<(), Fault> {
        assert!(sentinel.iter().all(|v| v.is_nan()));
        self.submits.set(self.submits.get() + 1);
        if matches!(self.mode, Mode::SubmitFault) {
            return Err(Fault {
                operation: "submit",
                raw_status: 12345,
            });
        }
        self.values = a
            .iter()
            .zip(b)
            .zip(x)
            .map(|((a, b), x)| x + a * b * k as f32)
            .collect();
        match self.mode {
            Mode::Corrupt => self.values[0] += 1.0,
            Mode::Partial | Mode::Nonfinite => self.values[0] = sentinel[0],
            Mode::Stale => {
                for v in &mut self.values {
                    *v -= 1.0;
                }
            }
            _ => (),
        }
        Ok(())
    }
    fn query(&mut self) -> Result<Query, Fault> {
        if matches!(self.mode, Mode::QueryFault) {
            return Err(Fault {
                operation: "query",
                raw_status: 700,
            });
        }
        self.polls += 1;
        Ok(if self.polls == 1 {
            Query::Pending
        } else {
            Query::Complete
        })
    }
    fn readback(&mut self, output: &mut [f32]) -> Result<usize, Fault> {
        if matches!(self.mode, Mode::Fault) {
            return Err(Fault {
                operation: "readback",
                raw_status: -999,
            });
        }
        output.copy_from_slice(&self.values);
        Ok(output.len() - usize::from(matches!(self.mode, Mode::Length)))
    }
}

#[test]
fn native_and_invalid_request_admission_is_unavailable() {
    let request = Request {
        run: [1; 16],
        identity: ID,
        recipe: Recipe::new(0, Shape::new(1, 4096).unwrap()),
        dispatches: 1,
        start_ns: 10,
        deadline_ns: 20,
    };
    let make = |mode| Fake {
        mode,
        values: vec![],
        polls: 0,
        submits: Rc::new(Cell::new(0)),
    };
    assert_eq!(
        Worker::new(make(Mode::Native), request).err(),
        Some(Outcome::LiveAdmissionUnavailable)
    );
    for request in [
        Request {
            run: [0; 16],
            ..request
        },
        Request {
            dispatches: 0,
            ..request
        },
        Request {
            dispatches: 4097,
            ..request
        },
        Request {
            deadline_ns: 10,
            ..request
        },
        Request {
            identity: Identity { bus: 9, ..ID },
            ..request
        },
    ] {
        assert!(Worker::new(make(Mode::Good), request).is_err());
    }
    let mut w = Worker::new(make(Mode::Good), request).ok().unwrap();
    w.advance(&mut || 20, false);
    assert_eq!(w.report().outcome(), Some(Outcome::DeadlineUnknown));
    assert!(w.report().records().is_empty());
}
fn worker(mode: Mode) -> (Worker<Fake>, Rc<Cell<usize>>) {
    let submits = Rc::new(Cell::new(0));
    let backend = Fake {
        mode,
        values: vec![],
        polls: 0,
        submits: submits.clone(),
    };
    (
        Worker::new(
            backend,
            Request {
                run: [1; 16],
                identity: ID,
                recipe: Recipe::new(0, Shape::new(32, 4096).unwrap()),
                dispatches: 2,
                start_ns: 0,
                deadline_ns: 1_000_000_000,
            },
        )
        .ok()
        .unwrap(),
        submits,
    )
}
#[test]
fn serial_full_checks_and_immutable_fixture_report() {
    let (mut w, submits) = worker(Mode::Good);
    for time in 0..8 {
        w.advance(&mut || time, false);
    }
    assert_eq!(w.report().outcome(), Some(Outcome::CheckedFixture));
    assert_eq!(submits.get(), 2);
    assert_eq!(w.report().records().len(), 2);
    assert_eq!(w.report().records()[1].check.checked_lanes(), 32);
    assert_ne!(
        w.report().records()[0].output_digest,
        w.report().records()[1].output_digest
    );
    let snapshot = w.report().clone();
    w.advance(&mut || u64::MAX, true);
    assert_eq!(snapshot.outcome(), w.report().outcome());
    assert_eq!(w.report().provenance(), Provenance::SimulatedFixture);
    assert!(!w.report().native_quiescence_known());
}
#[test]
fn report_evidence_binds_independent_input_digest_length_and_last_check() {
    let (mut w, _) = worker(Mode::Good);
    for time in 0..3 {
        w.advance(&mut || time, false);
    }
    assert_eq!(w.report().records().len(), 1);
    // Independent digest over this dispatch's a/b/x bits, in the documented
    // slice order (all lanes of a, then b, then x), never the implementation's
    // own digest helper.
    let dispatch = DispatchIndex::new(0).unwrap();
    let recipe = Recipe::new(0, Shape::new(32, 4096).unwrap());
    let lanes: Vec<[f32; 3]> = recipe.lanes().map(|lane| lane.inputs(dispatch)).collect();
    let mut hash = Sha256::new();
    for column in 0..3 {
        for lane in &lanes {
            hash.update(lane[column].to_bits().to_le_bytes());
        }
    }
    let expected: [u8; 32] = hash.finalize().into();
    assert_eq!(w.report().records()[0].input_digest, expected);
    assert_eq!(w.report().readback_length(), Some(32));
    assert_eq!(w.report().last_check().unwrap().checked_lanes(), 32);
}
#[test]
fn post_completion_cycle_cap_and_repeated_stop_keep_first_observation() {
    // The 250 ms cap applies only after completion is observed: 200 ms is
    // inside it, while 250 ms is the deadline boundary and records nothing.
    for (post_check_ns, outcome) in [
        (200_000_000, None),
        (250_000_000, Some(Outcome::DeadlineUnknown)),
    ] {
        let (mut w, submits) = worker(Mode::Good);
        w.advance(&mut || 0, false);
        w.advance(&mut || 1, false);
        let mut times = [2, 2, 2, 2, post_check_ns].into_iter();
        w.advance(&mut || times.next().unwrap(), false);
        assert_eq!(submits.get(), 1);
        assert_eq!(w.report().outcome(), outcome);
        assert_eq!(w.report().records().len(), usize::from(outcome.is_none()));
    }
    let (mut w, _) = worker(Mode::Good);
    w.advance(&mut || 0, false);
    w.advance(&mut || 5, true);
    assert_eq!(w.report().stop_ns(), Some(5));
    w.advance(&mut || 7, true);
    assert_eq!(w.report().stop_ns(), Some(5));
    assert_eq!(w.report().outcome(), Some(Outcome::Stopped));
}
#[test]
fn all_bad_outputs_and_raw_faults_close_admission() {
    for mode in [
        Mode::Corrupt,
        Mode::Partial,
        Mode::Nonfinite,
        Mode::Stale,
        Mode::Length,
        Mode::Fault,
        Mode::SubmitFault,
        Mode::QueryFault,
        Mode::Identity,
    ] {
        let (mut w, submits) = worker(mode);
        for time in 0..10 {
            w.advance(&mut || time, false);
        }
        assert_eq!(submits.get(), 1);
        assert_ne!(w.report().outcome(), None);
        assert_ne!(w.report().outcome(), Some(Outcome::CheckedFixture));
        if matches!(mode, Mode::Fault) {
            assert_eq!(
                w.report().outcome(),
                Some(Outcome::BackendFault(Fault {
                    operation: "readback",
                    raw_status: -999
                }))
            );
        }
        if matches!(
            mode,
            Mode::Corrupt | Mode::Partial | Mode::Nonfinite | Mode::Stale
        ) {
            assert_eq!(w.report().failure_output().unwrap().len(), 32);
        }
    }
}
#[test]
fn stop_is_not_completion_and_deadline_equality_wins() {
    let (mut w, submits) = worker(Mode::Good);
    w.advance(&mut || 0, false);
    w.advance(&mut || 1, true);
    assert_eq!(w.report().outcome(), None);
    assert_eq!(w.report().stop_ns(), Some(1));
    w.advance(&mut || 2, true);
    assert_eq!(w.report().outcome(), Some(Outcome::Stopped));
    assert_eq!(submits.get(), 1);
    let (mut w, submits) = worker(Mode::Good);
    w.advance(&mut || 0, false);
    w.advance(&mut || 100_000_000, true);
    w.advance(&mut || 100_000_001, false);
    assert_eq!(w.report().outcome(), Some(Outcome::DeadlineUnknown));
    assert_eq!(w.report().records().len(), 0);
    assert_eq!(submits.get(), 1);
    let (mut w, submits) = worker(Mode::Good);
    w.advance(&mut || 0, true);
    assert_eq!(w.report().outcome(), Some(Outcome::Stopped));
    assert_eq!(submits.get(), 0);
    let (mut w, _) = worker(Mode::Good);
    w.advance(&mut || 10, false);
    w.advance(&mut || 9, false);
    assert_eq!(w.report().outcome(), Some(Outcome::ClockFault));
}

#[test]
fn late_backend_returns_cannot_renew_deadlines_or_hide_raw_faults() {
    let (mut w, submits) = worker(Mode::Good);
    let mut times = [0, 0, 0, 100_000_000].into_iter();
    w.advance(&mut || times.next().unwrap_or(100_000_000), false);
    assert_eq!(w.report().outcome(), Some(Outcome::DeadlineUnknown));
    assert_eq!(submits.get(), 1);
    let (mut w, _) = worker(Mode::SubmitFault);
    let mut times = [0, 0, 0, 100_000_000].into_iter();
    w.advance(&mut || times.next().unwrap_or(100_000_000), false);
    assert_eq!(w.report().outcome(), Some(Outcome::DeadlineUnknown));
    assert_eq!(w.report().raw_fault().unwrap().raw_status, 12345);
}

#[test]
fn full_identity_normalization_and_rejections() {
    let uuid = "GPU-01234567-89ab-cdef-0123-456789abcdef";
    let a = DiscoveryBinding::parse(uuid, "00000000:0A:1f.7").unwrap();
    let b = DiscoveryBinding::parse(&uuid.to_uppercase(), "0:a:1F.7").unwrap();
    assert_eq!(a.identity, b.identity);
    assert_eq!(a.pci_function, 7);
    assert_ne!(a.raw_pci, b.raw_pci);
    for pci in [
        "0:a:20.0",
        "10000:0:0.0",
        "0:00:0.8",
        "0:00:0",
        "0::0.0",
        "0:gg:0.0",
        "0:0:0.0:1",
        "0.1.2.3",
        "0:1:2:3",
    ] {
        assert!(DiscoveryBinding::parse(uuid, pci).is_err());
    }
    for uuid in [
        "MIG-01234567-89ab-cdef-0123-456789abcdef",
        "GPU-00000000-0000-0000-0000-000000000000",
        "GPU-0123456789abcdef0123456789abcdef",
    ] {
        assert!(DiscoveryBinding::parse(uuid, "0:0:0.0").is_err());
    }
}
