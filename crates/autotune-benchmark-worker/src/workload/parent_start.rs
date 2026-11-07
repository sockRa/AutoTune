//! One retained parent Start handoff. Queue acceptance is not compute authority.
use super::{
    parent_sampler, process_state, starting_export::ClockFault, transport::SendFailure,
    wire::WorkerBinding,
};
#[cfg(any(windows, test))]
use crate::monitor::clock::Clock;
use crate::{
    monitor::{Anchor, Record},
    sampler::wire::WireError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Duplicate,
    CachedProcess,
    Clock,
    Deadline,
    Context,
    Telemetry,
    Process(process_state::Fault),
    Codec(WireError),
    Send(SendFailure),
}
/// The exact local state at one boundary, including data rejected by policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub anchor: Option<Anchor>,
    pub capture_error: Option<ClockFault>,
    pub clock_identity: (u64, [u8; 16]),
    pub parent_fault: Option<parent_sampler::Fault>,
    pub context: bool,
    pub reported_screen: bool,
    pub safety: [Option<Record>; 2],
    pub reentry: Option<[Record; 2]>,
    pub consecutive: u32,
}
/// Boundaries are entry, post-collection/issued, pre-queue, and post-queue.
/// Neither this record nor a successful result can mint NativeAdmission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub binding: WorkerBinding,
    pub deadline_ns: u64,
    pub observations: [Option<Observation>; 4],
    pub checks: [Option<Result<(), Fault>>; 4],
    pub process_result: Option<Result<(), process_state::Fault>>,
    pub codec_error: Option<WireError>,
    pub send_error: Option<SendFailure>,
    pub queued: bool,
    pub result: Result<(), Fault>,
}
impl Attempt {
    #[cfg(any(windows, test))]
    pub(crate) fn new(binding: WorkerBinding, deadline_ns: u64) -> Self {
        Self {
            binding,
            deadline_ns,
            observations: [None; 4],
            checks: [None; 4],
            process_result: None,
            codec_error: None,
            send_error: None,
            queued: false,
            result: Err(Fault::Context),
        }
    }
    #[cfg(any(windows, test))]
    pub(crate) fn observe(&mut self, index: usize, raw: Observation) -> Result<(), Fault> {
        // The concrete caller supplies exactly these four fixed boundaries.
        // Reject reordered or replaced observations without losing the first.
        if index >= 4
            || self.observations[index].is_some()
            || self.observations[..index].iter().any(Option::is_none)
        {
            return Err(Fault::Duplicate);
        }
        self.observations[index] = Some(raw);
        let result = (|| {
            if raw.capture_error.is_some()
                || raw.clock_identity
                    != (
                        self.binding.monitor().qpc_frequency,
                        self.binding.epoch.clock_token,
                    )
            {
                return Err(Fault::Clock);
            }
            let mut clock =
                Clock::new(self.binding.monitor(), self.binding.epoch).map_err(|_| Fault::Clock)?;
            let mut last = self.binding.epoch;
            for observation in self.observations[..=index].iter().flatten() {
                let a = observation.anchor.ok_or(Fault::Clock)?;
                if a.qpc_before < last.qpc_after {
                    return Err(Fault::Clock);
                }
                if clock.observe(a).map_err(|_| Fault::Clock)? >= self.deadline_ns {
                    return Err(Fault::Deadline);
                }
                last = a;
            }
            if raw.parent_fault.is_some() || !raw.context {
                return Err(Fault::Context);
            }
            if !raw.reported_screen {
                return Err(Fault::Telemetry);
            }
            Ok(())
        })();
        self.checks[index] = Some(result);
        result
    }
}

// Only the concrete parent adapter implements this outside CPU tests. No
// configurable native source or public fixture-to-permission path is exposed.
#[cfg(any(windows, test))]
pub(super) trait Source {
    fn observe(&mut self) -> Observation;
    fn cached_process(&self) -> bool;
    fn collect(&mut self) -> Result<(), process_state::Fault>;
    fn encode(&mut self, issued: Anchor) -> Result<(), WireError>;
    fn send(&mut self) -> Result<(), SendFailure>;
}
#[cfg(any(windows, test))]
pub(super) fn run(binding: WorkerBinding, deadline_ns: u64, source: &mut impl Source) -> Attempt {
    let mut attempt = Attempt::new(binding, deadline_ns);
    attempt.result = (|| {
        attempt.observe(0, source.observe())?;
        if source.cached_process() {
            return Err(Fault::CachedProcess);
        }
        let processes = source.collect();
        attempt.process_result = Some(processes);
        // Preserve the first returned failure and also the post-call state.
        let observed = attempt.observe(1, source.observe());
        processes.map_err(Fault::Process)?;
        observed?;
        let issued = attempt.observations[1]
            .expect("retained boundary")
            .anchor
            .expect("checked clock");
        if let Err(e) = source.encode(issued) {
            attempt.codec_error = Some(e);
            return Err(Fault::Codec(e));
        }
        attempt.observe(2, source.observe())?;
        let sent = source.send();
        attempt.queued = sent.is_ok();
        attempt.send_error = sent.err();
        let observed = attempt.observe(3, source.observe());
        sent.map_err(Fault::Send)?;
        observed
    })();
    attempt
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{Artifacts, PilotDraft, PilotManifest};
    use super::*;
    use crate::{
        evidence::{Digest, Domain},
        sampler::budget::Budget,
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
    fn attempt() -> Attempt {
        let b = Budget::new(4 * 1024 * 1024).unwrap();
        let d = Digest::of(Domain::RawArtifact, b"CPU-only Start model");
        let manifest = PilotManifest::freeze(
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
                approval_ref: "not execution authority".into(),
                output_root: "D:\\capture\\pilot".into(),
            },
            &b,
        )
        .unwrap();
        let run = manifest.bind_run(0, 0).unwrap();
        Attempt::new(
            WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap(),
            10_000,
        )
    }
    fn raw(ns: u64) -> Observation {
        Observation {
            anchor: Some(a(ns)),
            capture_error: None,
            clock_identity: (1_000_000_000, [5; 16]),
            parent_fault: None,
            context: true,
            reported_screen: true,
            safety: [None; 2],
            reentry: None,
            consecutive: 4,
        }
    }
    #[test]
    fn four_boundaries_share_original_clock_and_deadline_without_granting_authority() {
        let mut x = attempt();
        for i in 0..4 {
            assert_eq!(x.observe(i, raw((i as u64 + 1) * 100)), Ok(()));
        }
        assert_eq!(x.deadline_ns, 10_000);
        // This model observes no actual queue/process handoff or native permit.
        assert!(!x.queued);
        assert_eq!(x.process_result, None);
        assert_eq!(x.result, Err(Fault::Context));
    }
    #[test]
    fn expiry_at_each_boundary_is_retained_and_cannot_renew_setup() {
        for boundary in 0..4 {
            let mut x = attempt();
            for i in 0..boundary {
                x.observe(i, raw((i as u64 + 1) * 100)).unwrap();
            }
            let expired = raw(x.deadline_ns);
            assert_eq!(x.observe(boundary, expired), Err(Fault::Deadline));
            assert_eq!(x.observations[boundary], Some(expired));
            assert_eq!(x.checks[boundary], Some(Err(Fault::Deadline)));
            assert_eq!(x.deadline_ns, 10_000);
        }
    }
    #[test]
    fn failed_capture_preserves_actual_error_and_does_not_substitute_last_anchor() {
        let mut x = attempt();
        x.observe(0, raw(100)).unwrap();
        let failed = Observation {
            anchor: None,
            capture_error: Some(ClockFault::CounterFailed {
                before: true,
                code: 87,
            }),
            ..raw(200)
        };
        assert_eq!(x.observe(1, failed), Err(Fault::Clock));
        assert_eq!(x.observations[1], Some(failed));
        assert_eq!(x.observations[0].unwrap().anchor, Some(a(100)));
    }
    #[test]
    fn identity_regression_suspend_and_wide_bracket_close_the_same_attempt() {
        for mode in 0..5 {
            let mut x = attempt();
            x.observe(0, raw(1_000)).unwrap();
            let mut next = raw(2_000);
            match mode {
                0 => next.clock_identity.0 += 1,
                1 => next.clock_identity.1 = [6; 16],
                2 => next.anchor = Some(a(900)),
                3 => {
                    let mut a = a(2_000);
                    a.interrupt_100ns += 100_000;
                    next.anchor = Some(a);
                }
                _ => {
                    let mut a = a(2_000);
                    a.qpc_after += 1_000_001;
                    next.anchor = Some(a);
                }
            }
            assert_eq!(x.observe(1, next), Err(Fault::Clock));
            assert_eq!(x.observations[1], Some(next));
        }
    }
    #[test]
    fn fresh_time_does_not_replace_missing_context_or_current_telemetry() {
        for mode in 0..3 {
            let mut x = attempt();
            x.observe(0, raw(100)).unwrap();
            let mut next = raw(200);
            match mode {
                0 => next.context = false,
                1 => next.parent_fault = Some(parent_sampler::Fault::Stopped),
                _ => next.reported_screen = false,
            }
            assert_eq!(
                x.observe(1, next),
                Err(if mode == 2 {
                    Fault::Telemetry
                } else {
                    Fault::Context
                })
            );
            assert_eq!(x.observations[1], Some(next));
        }
    }
    #[test]
    fn replaced_or_skipped_boundary_cannot_erase_original_observation() {
        let mut x = attempt();
        assert_eq!(x.observe(1, raw(200)), Err(Fault::Duplicate));
        assert!(x.observations.iter().all(Option::is_none));
        let first = raw(100);
        x.observe(0, first).unwrap();
        assert_eq!(x.observe(0, raw(300)), Err(Fault::Duplicate));
        assert_eq!(x.observations[0], Some(first));
        assert_eq!(x.observe(4, raw(300)), Err(Fault::Duplicate));
    }

    struct Fake {
        observations: [Observation; 4],
        next: usize,
        cached: bool,
        process: Result<(), process_state::Fault>,
        codec: Result<(), WireError>,
        send: Result<(), SendFailure>,
        issued: Option<Anchor>,
        events: Vec<&'static str>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                observations: std::array::from_fn(|i| raw((i as u64 + 1) * 100)),
                next: 0,
                cached: false,
                process: Ok(()),
                codec: Ok(()),
                send: Ok(()),
                issued: None,
                events: Vec::new(),
            }
        }
        fn run(&mut self) -> Attempt {
            let x = attempt();
            run(x.binding, x.deadline_ns, self)
        }
    }
    impl Source for Fake {
        fn observe(&mut self) -> Observation {
            self.events.push("observe");
            let result = self.observations[self.next];
            self.next += 1;
            result
        }
        fn cached_process(&self) -> bool {
            self.cached
        }
        fn collect(&mut self) -> Result<(), process_state::Fault> {
            self.events.push("collect");
            self.process
        }
        fn encode(&mut self, issued: Anchor) -> Result<(), WireError> {
            self.events.push("encode");
            self.issued = Some(issued);
            self.codec
        }
        fn send(&mut self) -> Result<(), SendFailure> {
            self.events.push("send");
            self.send
        }
    }
    #[test]
    fn complete_engine_collects_inside_attempt_and_brackets_encoded_queue_handoff() {
        let mut fake = Fake::new();
        let result = fake.run();
        assert_eq!(result.result, Ok(()));
        assert!(result.queued);
        assert_eq!(result.process_result, Some(Ok(())));
        assert_eq!(fake.issued, Some(a(200)));
        assert_eq!(
            fake.events,
            ["observe", "collect", "observe", "encode", "observe", "send", "observe"]
        );
    }
    #[test]
    fn each_expired_boundary_prevents_successors_but_preserves_accepted_queue_truth() {
        for boundary in 0..4 {
            let mut fake = Fake::new();
            fake.observations[boundary] = raw(10_000);
            let result = fake.run();
            assert_eq!(result.result, Err(Fault::Deadline));
            assert_eq!(fake.next, boundary + 1);
            assert_eq!(fake.events.contains(&"collect"), boundary > 0);
            assert_eq!(fake.events.contains(&"encode"), boundary > 1);
            assert_eq!(fake.events.contains(&"send"), boundary > 2);
            assert_eq!(result.queued, boundary == 3);
        }
    }
    #[test]
    fn cached_process_frame_cannot_be_promoted_into_start_even_with_good_telemetry() {
        let mut fake = Fake::new();
        fake.cached = true;
        let result = fake.run();
        assert_eq!(result.result, Err(Fault::CachedProcess));
        assert_eq!(fake.events, ["observe"]);
        assert_eq!(result.process_result, None);
    }
    #[test]
    fn process_failure_retains_later_clock_loss_without_encoding_or_queueing() {
        let mut fake = Fake::new();
        fake.process = Err(process_state::Fault::Missing);
        fake.observations[1].anchor = None;
        let result = fake.run();
        assert_eq!(
            result.result,
            Err(Fault::Process(process_state::Fault::Missing))
        );
        assert_eq!(result.checks[1], Some(Err(Fault::Clock)));
        assert_eq!(fake.events, ["observe", "collect", "observe"]);
    }
    #[test]
    fn safety_loss_during_collection_or_encoding_prevents_start_queueing() {
        for boundary in [1, 2] {
            let mut fake = Fake::new();
            fake.observations[boundary].reported_screen = false;
            let result = fake.run();
            assert_eq!(result.result, Err(Fault::Telemetry));
            assert!(!result.queued);
            assert!(!fake.events.contains(&"send"));
            assert_eq!(fake.events.contains(&"encode"), boundary == 2);
        }
    }
    #[test]
    fn codec_failure_retains_cause_and_never_enters_queue() {
        let mut fake = Fake::new();
        fake.codec = Err(WireError::Capacity);
        let result = fake.run();
        assert_eq!(result.result, Err(Fault::Codec(WireError::Capacity)));
        assert_eq!(result.codec_error, Some(WireError::Capacity));
        assert_eq!(fake.next, 2);
        assert!(!fake.events.contains(&"send"));
    }
    #[test]
    fn queue_failure_has_priority_over_postqueue_expiry_and_both_remain_visible() {
        let mut fake = Fake::new();
        fake.send = Err(SendFailure::Os(232));
        fake.observations[3] = raw(10_000);
        let result = fake.run();
        assert_eq!(result.result, Err(Fault::Send(SendFailure::Os(232))));
        assert_eq!(result.send_error, Some(SendFailure::Os(232)));
        assert_eq!(result.checks[3], Some(Err(Fault::Deadline)));
        assert!(!result.queued);
        assert_eq!(fake.next, 4);
    }
}
