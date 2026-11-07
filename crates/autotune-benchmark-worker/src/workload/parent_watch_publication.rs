//! Private parent-to-controller publication policy. No fixture, ready flag or
//! successful publication can mint native execution authority.
use super::parent_watch::Fault;
#[cfg(any(windows, test))]
use super::{
    gate::StopWindow,
    launch_origin::Origin,
    parent_watch::{Update, Window},
};
#[cfg(any(windows, test))]
use crate::monitor::Anchor;
use crate::monitor::Record;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub setup_sent: bool,
    pub last_safety: Option<[Option<Record>; 2]>,
    pub first_fault: Option<Fault>,
}
#[cfg(any(windows, test))]
#[derive(Clone, Copy)]
pub(crate) struct Facts {
    pub window: Window,
    pub anchor: Option<Anchor>,
    pub open: bool,
    pub stop: Option<StopWindow>,
    pub start_ok: Option<bool>,
    pub peers: Option<[Origin; 2]>,
    pub safety: [Option<Record>; 2],
}
#[cfg(any(windows, test))]
pub(crate) trait Endpoint {
    fn ready(&self) -> bool;
    fn closed(&self) -> bool;
    fn send(&mut self, update: Update) -> Result<(), Fault>;
    fn stop(&mut self, window: StopWindow);
    fn abort(&mut self, fault: Fault);
}
#[cfg(any(windows, test))]
pub(crate) struct Publication {
    window: Window,
    origins: [Origin; 2],
    status: Status,
}
#[cfg(any(windows, test))]
impl Publication {
    pub(crate) fn new(window: Window, origins: [Origin; 2]) -> Self {
        Self {
            window,
            origins,
            status: Status::default(),
        }
    }
    pub(crate) fn status(&self) -> Status {
        self.status
    }
    pub(crate) fn matches(&self, window: Window, peers: Option<[Origin; 2]>) -> bool {
        self.window == window && peers == Some(self.origins)
    }
    pub(crate) fn ready(
        &self,
        window: Window,
        peers: Option<[Origin; 2]>,
        endpoint: &impl Endpoint,
    ) -> bool {
        self.status.first_fault.is_none()
            && self.matches(window, peers)
            && endpoint.ready()
            && !endpoint.closed()
    }
    pub(crate) fn publish(
        &mut self,
        facts: Facts,
        endpoint: &mut impl Endpoint,
    ) -> Result<(), Fault> {
        let result = (|| {
            if let Some(f) = self.status.first_fault {
                return Err(f);
            }
            if facts.window != self.window {
                return Err(Fault::Binding);
            }
            // Forward the parent's original stop window even when control is
            // already closed; no new timestamp or fresh grace is invented.
            if let Some(window) = facts.stop {
                endpoint.stop(window);
                return Ok(());
            }
            if endpoint.closed() {
                return Err(Fault::Requested);
            }
            let Some(start_ok) = facts.start_ok else {
                return Ok(());
            };
            if !start_ok || !facts.open || facts.peers != Some(self.origins) {
                return Err(Fault::Binding);
            }
            if !endpoint.ready() {
                return Err(Fault::Thread);
            }
            if self.status.setup_sent && self.status.last_safety == Some(facts.safety) {
                return Ok(());
            }
            let anchor = facts.anchor.ok_or(Fault::Clock)?;
            let update = Update {
                anchor,
                setup: !self.status.setup_sent,
                safety: facts.safety,
            };
            endpoint.send(update)?;
            // Commit only the actual queue acceptance, never a failed attempt.
            self.status.setup_sent = true;
            self.status.last_safety = Some(facts.safety);
            Ok(())
        })();
        if let Err(f) = result {
            self.status.first_fault.get_or_insert(f);
            endpoint.abort(f);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        evidence::{Digest, Domain},
        monitor::{Metric, Status as SensorStatus, Ticket},
        sampler::budget::Budget,
        workload::{
            launch_origin::Kind,
            manifest::{Artifacts, PilotDraft, PilotManifest},
            wire::WorkerBinding,
        },
    };
    fn a(ns: u64) -> Anchor {
        Anchor {
            clock_token: [5; 16],
            qpc_before: ns,
            qpc_after: ns,
            interrupt_100ns: ns / 100,
            unbiased_100ns: ns / 100,
        }
    }
    struct Fake {
        ready: bool,
        closed: bool,
        fail: bool,
        sent: Vec<Update>,
        stops: Vec<StopWindow>,
        aborts: Vec<Fault>,
    }
    impl Endpoint for Fake {
        fn ready(&self) -> bool {
            self.ready
        }
        fn closed(&self) -> bool {
            self.closed
        }
        fn send(&mut self, u: Update) -> Result<(), Fault> {
            if self.fail {
                Err(Fault::Transport)
            } else {
                self.sent.push(u);
                Ok(())
            }
        }
        fn stop(&mut self, w: StopWindow) {
            self.closed = true;
            self.stops.push(w);
        }
        fn abort(&mut self, f: Fault) {
            self.closed = true;
            self.aborts.push(f);
        }
    }
    fn fixture() -> (Publication, Facts, Fake) {
        let b = Budget::new(4 * 1024 * 1024).unwrap();
        let d = Digest::of(Domain::RawArtifact, b"CPU publication model");
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
                approval_ref: "no model execution authority".into(),
                output_root: "D:\\capture\\pilot".into(),
            },
            &b,
        )
        .unwrap();
        let binding = WorkerBinding::new(&m.bind_run(0, 0).unwrap(), 1_000_000_000, a(0)).unwrap();
        let window = Window {
            binding,
            setup_deadline_ns: 10_000_000_000,
        };
        let origins = std::array::from_fn(|i| {
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
        let safety = std::array::from_fn(|i| {
            let metric = [Metric::Temperature, Metric::Power][i];
            Some(Record {
                ticket: Ticket {
                    binding: binding.monitor(),
                    metric,
                    slot: 0,
                    scheduled_ns: 1000,
                    start_ns: 1000,
                },
                end_ns: 1000,
                receipt_ns: 1000,
                unit: metric.unit(),
                status: SensorStatus::Available,
                value: Some(if i == 0 { 50 } else { 100_000 }),
            })
        });
        (
            Publication::new(window, origins),
            Facts {
                window,
                anchor: Some(a(1200)),
                open: true,
                stop: None,
                start_ok: Some(true),
                peers: Some(origins),
                safety,
            },
            Fake {
                ready: true,
                closed: false,
                fail: false,
                sent: Vec::new(),
                stops: Vec::new(),
                aborts: Vec::new(),
            },
        )
    }
    #[test]
    fn preparing_parent_waits_without_publishing_setup_or_granting_permission() {
        let (mut p, mut f, mut e) = fixture();
        f.start_ok = None;
        e.ready = false;
        assert_eq!(p.publish(f, &mut e), Ok(()));
        assert_eq!(p.status(), Status::default());
        assert!(e.sent.is_empty());
        assert!(!p.ready(f.window, f.peers, &e));
        e.ready = true;
        assert!(p.ready(f.window, f.peers, &e));
        assert!(!p.status().setup_sent);
    }
    #[test]
    fn successful_start_publishes_once_then_only_changed_safety_without_reissued_setup() {
        let (mut p, mut f, mut e) = fixture();
        p.publish(f, &mut e).unwrap();
        p.publish(f, &mut e).unwrap();
        assert_eq!(e.sent.len(), 1);
        assert!(e.sent[0].setup);
        f.anchor = Some(a(1300));
        f.safety[0].as_mut().unwrap().value = Some(51);
        p.publish(f, &mut e).unwrap();
        assert_eq!(e.sent.len(), 2);
        assert!(!e.sent[1].setup);
        assert_eq!(e.sent[1].anchor, a(1300));
        assert_eq!(p.status().last_safety, Some(f.safety));
    }
    #[test]
    fn first_rejected_handoff_cannot_commit_or_retry_even_if_a_signal_is_reset() {
        let (mut p, f, mut e) = fixture();
        e.fail = true;
        assert_eq!(p.publish(f, &mut e), Err(Fault::Transport));
        assert!(!p.status().setup_sent);
        assert_eq!(p.status().last_safety, None);
        e.fail = false;
        e.closed = false;
        assert_eq!(p.publish(f, &mut e), Err(Fault::Transport));
        assert!(e.sent.is_empty());
        assert_eq!(p.status().first_fault, Some(Fault::Transport));
        assert!(!p.ready(f.window, f.peers, &e));
    }
    #[test]
    fn rejected_later_sample_preserves_the_last_accepted_pair_and_closes_control() {
        let (mut p, mut f, mut e) = fixture();
        p.publish(f, &mut e).unwrap();
        let accepted = p.status().last_safety;
        f.safety[1].as_mut().unwrap().value = Some(100_001);
        e.fail = true;
        assert_eq!(p.publish(f, &mut e), Err(Fault::Transport));
        assert!(e.closed);
        assert_eq!(p.status().last_safety, accepted);
        assert_eq!(e.sent.len(), 1);
    }
    #[test]
    fn changed_window_peers_failed_start_and_closed_parent_cannot_publish() {
        for mode in 0..5 {
            let (mut p, mut f, mut e) = fixture();
            match mode {
                0 => f.window.setup_deadline_ns -= 1,
                1 => f.peers = None,
                2 => f.peers.as_mut().unwrap().swap(0, 1),
                3 => f.start_ok = Some(false),
                _ => f.open = false,
            }
            assert_eq!(p.publish(f, &mut e), Err(Fault::Binding));
            assert!(e.sent.is_empty());
            assert!(e.closed);
        }
    }
    #[test]
    fn original_parent_stop_is_forwarded_even_after_controller_closes() {
        let (mut p, mut f, mut e) = fixture();
        e.closed = true;
        e.ready = false;
        let w = StopWindow {
            trigger_ns: 1000,
            cooperative_ns: 1_000_001_000,
            exit_ns: 3_000_001_000,
        };
        f.stop = Some(w);
        assert_eq!(p.publish(f, &mut e), Ok(()));
        assert_eq!(e.stops, [w]);
        assert!(e.aborts.is_empty());
        assert!(e.sent.is_empty());
    }
    #[test]
    fn missing_current_clock_is_not_replaced_with_the_previous_publication_anchor() {
        let (mut p, mut f, mut e) = fixture();
        p.publish(f, &mut e).unwrap();
        f.anchor = None;
        f.safety[0].as_mut().unwrap().value = Some(51);
        assert_eq!(p.publish(f, &mut e), Err(Fault::Clock));
        assert_eq!(e.sent.len(), 1);
        assert!(e.closed);
    }
    #[test]
    fn start_handoff_needs_live_ready_control_and_matching_original_owners() {
        let (mut p, f, mut e) = fixture();
        e.ready = false;
        assert_eq!(p.publish(f, &mut e), Err(Fault::Thread));
        assert!(e.sent.is_empty());
        let (mut p, f, mut e) = fixture();
        e.closed = true;
        assert_eq!(p.publish(f, &mut e), Err(Fault::Requested));
        assert!(!p.ready(f.window, f.peers, &e));
        let (p, mut f, e) = fixture();
        f.peers.as_mut().unwrap().swap(0, 1);
        assert!(!p.ready(f.window, f.peers, &e));
    }
}
