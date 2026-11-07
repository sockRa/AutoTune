//! Receipt ownership and local launch-capture matching. No native issuer.
use super::*;
use crate::workload::{
    launch_origin::Origin, parent_context::References, starting_read,
    worker_setup_delivery::ReadyDelivery,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentFault {
    Context,
    Stage,
    Untrusted,
    Decision(Fault),
    Binding,
    Attempt,
    Clock,
    Order,
    Deadline,
    Duplicate,
}
pub struct Decisions {
    pub setup: Record,
    pub workload: Record,
}
pub struct ParentInputs<'a> {
    pub starting: &'a Record,
    pub starting_response: &'a starting_read::Receipt,
    pub setup: &'a mut ReadyDelivery,
    pub now: Anchor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub references: Option<References>,
    pub captures: [Option<CaptureRef>; 3],
    pub now: Anchor,
    /// Starting read, native setup, workload; all outcomes survive rejection.
    pub checks: [Result<(), ParentFault>; 3],
    pub result: Result<(), ParentFault>,
}
pub struct Delivery {
    pub decisions: Decisions,
    pub assessment: Assessment,
}
/// The first submission is retained by ParentSampler, even when rejected.
/// A duplicate returns its original owners instead of overwriting that record.
pub struct Rejected {
    pub fault: ParentFault,
    pub returned: Option<Decisions>,
}
#[derive(Clone, Copy)]
struct View {
    stage: Stage,
    scope: Digest,
    time: Time,
    targets: [Option<LaunchTarget>; 2],
    interactive: bool,
    result: Result<(), Fault>,
    capture: Option<CaptureRef>,
    before: Option<Anchor>,
    receipt: Option<Anchor>,
    used: [bool; 2],
    attempts: [Option<LaunchAttempt>; 2],
    context: Option<WorkloadContext>,
}
impl View {
    fn of(r: &Record) -> Self {
        Self {
            stage: r.request.stage,
            scope: r.request.scope,
            time: r.request.time,
            targets: r.request.targets,
            interactive: r.interactive(),
            result: r.result(),
            capture: r.capture_ref(),
            before: r.facts.before,
            receipt: r.facts.receipt,
            used: r.used,
            attempts: r.attempts,
            context: r.request.workload_context,
        }
    }
}
#[derive(Clone, Copy)]
struct Expected {
    binding: WorkerBinding,
    refs: References,
    peers: [Origin; 3],
    entry: Anchor,
    deadline: u64,
    first: [Anchor; 3],
    latest: [Anchor; 3],
    now: Anchor,
}
fn base(v: View, stage: Stage, e: Expected) -> Result<(), ParentFault> {
    if v.stage != stage {
        return Err(ParentFault::Stage);
    }
    if !v.interactive {
        return Err(ParentFault::Untrusted);
    }
    v.result.map_err(ParentFault::Decision)?;
    if v.scope != e.refs.scope
        || v.time.frequency != e.binding.monitor().qpc_frequency
        || v.time.epoch != e.binding.epoch
    {
        return Err(ParentFault::Binding);
    }
    let capture = v.capture.ok_or(ParentFault::Untrusted)?;
    if Some(capture.receipt) != v.receipt {
        return Err(ParentFault::Binding);
    }
    ordered(
        e.binding,
        [Some(v.time.entry), Some(v.time.issued), v.before, v.receipt],
        v.time.deadline,
    )?;
    Ok(())
}
fn ordered<const N: usize>(
    binding: WorkerBinding,
    anchors: [Option<Anchor>; N],
    deadline: u64,
) -> Result<(), ParentFault> {
    let mut clock = Clock::new(binding.monitor(), binding.epoch).map_err(|_| ParentFault::Clock)?;
    let mut last = binding.epoch;
    for a in anchors {
        let a = a.ok_or(ParentFault::Clock)?;
        if a.qpc_before < last.qpc_after {
            return Err(ParentFault::Order);
        }
        if clock.observe(a).map_err(|_| ParentFault::Clock)? >= deadline {
            return Err(ParentFault::Deadline);
        }
        last = a;
    }
    Ok(())
}
fn launch(
    v: View,
    origin: Origin,
    first: Anchor,
    binding: WorkerBinding,
) -> Result<(), ParentFault> {
    if origin.consent() != v.capture || v.capture.is_none() {
        return Err(ParentFault::Binding);
    }
    let target = LaunchTarget {
        kind: origin.kind(),
        binding: origin.binding(),
        declaration: origin.declaration(),
        image: origin.image(),
    };
    let i = v
        .targets
        .iter()
        .position(|t| *t == Some(target))
        .ok_or(ParentFault::Binding)?;
    let attempt = v.attempts[i].ok_or(ParentFault::Attempt)?;
    if !v.used[i]
        || attempt.result.is_err()
        || attempt.target != target
        || attempt.clock_error.is_some()
        || attempt.observed_clock
            != Some((binding.monitor().qpc_frequency, binding.epoch.clock_token))
    {
        return Err(ParentFault::Attempt);
    }
    ordered(
        binding,
        [v.receipt, attempt.anchor, Some(first)],
        v.time.deadline,
    )
}
fn checks(views: [View; 3], e: Expected) -> [Result<(), ParentFault>; 3] {
    let read = (|| {
        base(views[0], Stage::StartingRead, e)?;
        if views[0].time.deadline
            != autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .preflight_ns
            || views[0].used != [true, false]
        {
            return Err(ParentFault::Binding);
        }
        launch(views[0], e.peers[0], e.first[0], e.binding)
    })();
    let setup = (|| {
        base(views[1], Stage::PilotSetup, e)?;
        if views[1].time.entry != e.entry
            || views[1].time.deadline != e.deadline
            || views[1].used != [true, true]
        {
            return Err(ParentFault::Binding);
        }
        launch(views[1], e.peers[1], e.first[1], e.binding)?;
        launch(views[1], e.peers[2], e.first[2], e.binding)
    })();
    let work = (|| {
        base(views[2], Stage::Workload, e)?;
        if views[2].time.entry != e.entry
            || views[2].time.deadline != e.deadline
            || views[2].used != [false, false]
            || views[2].targets != [None, None]
            || views[2].attempts.iter().any(Option::is_some)
            || views[2].context
                != Some(WorkloadContext {
                    binding: e.binding,
                    refs: e.refs,
                    peers: e.peers,
                })
        {
            return Err(ParentFault::Binding);
        }
        if e.latest
            .iter()
            .any(|a| views[2].time.issued.qpc_before < a.qpc_after)
        {
            return Err(ParentFault::Order);
        }
        ordered(e.binding, [views[2].receipt, Some(e.now)], e.deadline)
    })();
    [read, setup, work]
}
pub(super) fn assess(
    parent: &ParentSampler,
    decisions: &Decisions,
    input: &ParentInputs<'_>,
) -> Assessment {
    let views = [
        View::of(input.starting),
        View::of(&decisions.setup),
        View::of(&decisions.workload),
    ];
    let context = parent.context_check().and_then(|c| c.result.ok());
    let refs = context.map(|c| c.references);
    let expected = (|| {
        if !parent.context_open() {
            return Err(ParentFault::Context);
        }
        let context = context.ok_or(ParentFault::Context)?;
        let peers = parent
            .peer_check()
            .filter(|p| p.result.is_ok() && p.context == refs)
            .ok_or(ParentFault::Context)?;
        if context.references.starting_response != Some(input.starting_response.digest())
            || context.references.worker_setup != input.setup.receipt().digest()
            || input.setup.origin().ok() != Some(peers.peers[2].expected)
        {
            return Err(ParentFault::Binding);
        }
        let (entry, deadline) = parent.consent_window();
        let binding = parent.context_binding();
        let raw = binding
            .epoch
            .interrupt_100ns
            .checked_add(deadline / 100)
            .ok_or(ParentFault::Deadline)?;
        if input.setup.receipt().reported().deadline_raw != raw {
            return Err(ParentFault::Deadline);
        }
        let (before, after) = match parent.ready().and_then(|r| r.message().ok()) {
            Some(super::super::wire::Message::SamplerReady { before, after, .. }) => {
                (*before, *after)
            }
            _ => return Err(ParentFault::Context),
        };
        let worker_ready = match input.setup.ready().message() {
            Ok(super::super::wire::Message::Ready { anchor, .. }) => *anchor,
            _ => return Err(ParentFault::Context),
        };
        if worker_ready != context.worker_ready {
            return Err(ParentFault::Binding);
        }
        let current = parent
            .current()
            .and_then(|r| r.raw().receipt())
            .ok_or(ParentFault::Context)?;
        Ok(Expected {
            binding,
            refs: context.references,
            peers: peers.peers.map(|p| p.expected),
            entry,
            deadline,
            first: [
                input.starting_response.summary().first_entry,
                before,
                input.setup.receipt().reported().first,
            ],
            latest: [after, worker_ready, current],
            now: input.now,
        })
    })();
    let outcomes = expected
        .map(|e| checks(views, e))
        .unwrap_or_else(|f| [Err(f); 3]);
    let result = expected.map(|_| ()).and_then(|()| {
        outcomes
            .iter()
            .copied()
            .find(Result::is_err)
            .unwrap_or(Ok(()))
    });
    Assessment {
        references: refs,
        captures: views.map(|v| v.capture),
        now: input.now,
        checks: outcomes,
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::manifest::{Artifacts, PilotDraft, PilotManifest};
    fn a(n: u64) -> Anchor {
        Anchor {
            clock_token: [1; 16],
            qpc_before: n,
            qpc_after: n,
            interrupt_100ns: n / 100,
            unbiased_100ns: n / 100,
        }
    }
    fn dg(n: u8) -> Digest {
        Digest::of(Domain::RawArtifact, &[n])
    }
    fn fixture() -> (Budget, [View; 3], Expected) {
        let b = Budget::new(8 * 1024 * 1024).unwrap();
        let m = PilotManifest::freeze(
            PilotDraft {
                session: [1; 16],
                runs: [[2; 16], [3; 16], [4; 16]],
                clock_token: [1; 16],
                raw_uuid: "GPU-03030303-0303-0303-0303-030303030303".into(),
                raw_pci: "0000:01:02.0".into(),
                source_commit: "a".repeat(40),
                artifacts: Artifacts {
                    source_tree: dg(1),
                    lockfile: dg(2),
                    controller: dg(3),
                    worker: dg(4),
                    sampler: dg(5),
                    ptx: dg(6),
                    protocol: dg(7),
                },
                starting_evidence: dg(8),
                environment_evidence: dg(9),
                approval_ref: "model only".into(),
                output_root: "D:\\model".into(),
            },
            &b,
        )
        .unwrap();
        let run = m.bind_run(0, 100_000).unwrap();
        let binding = WorkerBinding::new(&run, 1_000_000_000, a(0)).unwrap();
        let refs = References {
            manifest: m.digest(),
            scope: dg(20),
            host: Some(dg(21)),
            starting: Some(dg(22)),
            starting_response: Some(dg(23)),
            worker_setup: dg(24),
            current: Some(dg(25)),
            sampler_ready: Some(dg(26)),
            current_response: Some(dg(27)),
        };
        let captures = [
            CaptureRef {
                request: dg(30),
                receipt: a(1300),
            },
            CaptureRef {
                request: dg(31),
                receipt: a(100400),
            },
            CaptureRef {
                request: dg(32),
                receipt: a(101500),
            },
        ];
        let peers = std::array::from_fn(|i| {
            Origin::new(
                [Kind::Starting, Kind::Sampler, Kind::Worker][i],
                if i == 0 {
                    refs.scope
                } else {
                    binding.transport_digest()
                },
                dg(40 + i as u8),
                if i == 2 {
                    run.worker_digest()
                } else {
                    run.sampler_digest()
                },
                i as u32 + 1,
                100,
            )
            .unwrap()
            .with_consent(captures[usize::from(i != 0)])
        });
        let target = |o: Origin| LaunchTarget {
            kind: o.kind(),
            binding: o.binding(),
            declaration: o.declaration(),
            image: o.image(),
        };
        let time = Time {
            frequency: 1_000_000_000,
            epoch: a(0),
            entry: a(100100),
            issued: a(100200),
            deadline: 10_000_100_100,
        };
        let attempt = |target, at| {
            Some(LaunchAttempt {
                target,
                anchor: Some(a(at)),
                clock_error: None,
                result: Ok(()),
                observed_clock: Some((1_000_000_000, [1; 16])),
            })
        };
        let views = [
            View {
                stage: Stage::StartingRead,
                scope: refs.scope,
                time: Time {
                    entry: a(1000),
                    issued: a(1100),
                    deadline: 10_000_000_000,
                    ..time
                },
                targets: [Some(target(peers[0])), None],
                interactive: true,
                result: Ok(()),
                capture: Some(captures[0]),
                before: Some(a(1200)),
                receipt: Some(a(1300)),
                used: [true, false],
                attempts: [attempt(target(peers[0]), 1400), None],
                context: None,
            },
            View {
                stage: Stage::PilotSetup,
                scope: refs.scope,
                time,
                targets: [Some(target(peers[2])), Some(target(peers[1]))],
                interactive: true,
                result: Ok(()),
                capture: Some(captures[1]),
                before: Some(a(100300)),
                receipt: Some(a(100400)),
                used: [true, true],
                attempts: [
                    attempt(target(peers[2]), 100500),
                    attempt(target(peers[1]), 100600),
                ],
                context: None,
            },
            View {
                stage: Stage::Workload,
                scope: refs.scope,
                time: Time {
                    issued: a(101300),
                    ..time
                },
                targets: [None, None],
                interactive: true,
                result: Ok(()),
                capture: Some(captures[2]),
                before: Some(a(101400)),
                receipt: Some(a(101500)),
                used: [false, false],
                attempts: [None, None],
                context: Some(WorkloadContext {
                    binding,
                    refs,
                    peers,
                }),
            },
        ];
        (
            b,
            views,
            Expected {
                binding,
                refs,
                peers,
                entry: time.entry,
                deadline: time.deadline,
                first: [a(1500), a(100900), a(100800)],
                latest: [a(101100), a(101000), a(101200)],
                now: a(101600),
            },
        )
    }
    #[test]
    fn exact_capture_launch_and_context_models_have_three_independent_successes() {
        let (_b, v, e) = fixture();
        assert_eq!(checks(v, e), [Ok(()); 3]);
    }
    #[test]
    fn another_capture_for_the_same_declaration_cannot_replace_the_used_approval() {
        let (_b, v, mut e) = fixture();
        let old = e.peers[2].consent().unwrap();
        e.peers[2] = e.peers[2].with_consent(CaptureRef {
            receipt: a(100450),
            ..old
        });
        assert_eq!(checks(v, e)[1], Err(ParentFault::Binding));
    }
    #[test]
    fn swapped_or_fixture_decisions_cannot_be_promoted() {
        let (_b, mut v, e) = fixture();
        v.swap(0, 1);
        let out = checks(v, e);
        assert_eq!(out[0], Err(ParentFault::Stage));
        assert_eq!(out[1], Err(ParentFault::Stage));
        let (_b, mut v, e) = fixture();
        v[0].interactive = false;
        assert_eq!(checks(v, e)[0], Err(ParentFault::Untrusted));
    }
    #[test]
    fn later_approval_cannot_explain_an_earlier_launch() {
        let (_b, mut v, mut e) = fixture();
        let capture = CaptureRef {
            request: v[0].capture.unwrap().request,
            receipt: a(1600),
        };
        v[0].capture = Some(capture);
        v[0].receipt = Some(capture.receipt);
        e.peers[0] = e.peers[0].with_consent(capture);
        assert_eq!(checks(v, e)[0], Err(ParentFault::Order));
    }
    #[test]
    fn used_role_requires_its_original_successful_attempt_and_clock_identity() {
        for mode in 0..3 {
            let (_b, mut v, e) = fixture();
            let mut attempt = v[1].attempts[0].unwrap();
            match mode {
                0 => attempt.observed_clock = Some((1, [1; 16])),
                1 => attempt.result = Err(Fault::Clock),
                _ => {
                    attempt.clock_error =
                        Some(super::super::super::starting_export::ClockFault::ZeroToken)
                }
            };
            v[1].attempts[0] = Some(attempt);
            assert_eq!(checks(v, e)[1], Err(ParentFault::Attempt));
        }
    }
    #[test]
    fn workload_must_bind_exact_facts_and_follow_all_readiness_receipts() {
        let (_b, mut v, e) = fixture();
        v[2].time.issued = a(101100);
        assert_eq!(checks(v, e)[2], Err(ParentFault::Order));
        let (_b, mut v, e) = fixture();
        v[2].context.as_mut().unwrap().refs.current = Some(dg(99));
        assert_eq!(checks(v, e)[2], Err(ParentFault::Binding));
    }
    #[test]
    fn original_deadline_equality_and_bad_final_clock_reject_workload() {
        let (_b, v, mut e) = fixture();
        e.now = a(e.deadline);
        assert_eq!(checks(v, e)[2], Err(ParentFault::Deadline));
        let (_b, v, mut e) = fixture();
        e.now.clock_token = [9; 16];
        assert_eq!(checks(v, e)[2], Err(ParentFault::Clock));
    }
    #[test]
    fn setup_entry_deadline_or_unused_role_cannot_be_silently_rebased() {
        for mode in 0..3 {
            let (_b, mut v, e) = fixture();
            match mode {
                0 => v[1].time.entry = a(100000),
                1 => v[1].time.deadline += 100,
                _ => v[1].used[1] = false,
            };
            assert_eq!(checks(v, e)[1], Err(ParentFault::Binding));
        }
    }
    #[test]
    fn all_stage_failures_remain_visible_in_the_same_assessment() {
        let (_b, mut v, e) = fixture();
        v[0].stage = Stage::Workload;
        v[1].scope = dg(99);
        v[2].interactive = false;
        assert_eq!(
            checks(v, e),
            [
                Err(ParentFault::Stage),
                Err(ParentFault::Binding),
                Err(ParentFault::Untrusted)
            ]
        );
    }
    #[test]
    fn comparison_requires_no_new_heap_storage_when_the_root_ledger_is_full() {
        let (b, v, e) = fixture();
        let _pressure = b.reserve(8 * 1024 * 1024 - b.used()).unwrap();
        let used = b.used();
        assert_eq!(checks(v, e), [Ok(()); 3]);
        assert_eq!(b.used(), used);
    }
}
