# E3 independent sampler transport and supervision

Date: 2026-10-29. Owner-approved implementation design for roadmap delivery
package 2, E3.1/E3.2, building on the delivered selective-getter bridge. The owner
approved continuous implementation and delegated routine engineering choices;
the paired plan is in progress. No process, DLL or GPU execution is authorized
by that software-only approval.

## Intent and existing contracts

Make the six-metric NVML sampler independently killable while controller ticks,
stop handling and exit observation remain responsive to a blocked getter.
Deliver the actual transport/process integration source, not another substitute
for live integration. Successful compilation and injected tests cannot establish
Windows containment, observed native readings or baseline qualification.

Read together with `ROADMAP.md`, `docs/benchmark-worker.md`,
`docs/nvidia-feasibility.md` and the local first-benchmark contracts:
`_bmad-output/specs/spec-e3-first-real-benchmark/{integration-contract,
qualification-profile,live-evidence-schema}.md`. Their approved numeric profile
and safety boundaries remain authoritative. Missing local contracts block the
dependent implementation; this proposal cannot reconstruct their authority.

The archived OMO G003 integration draft located by the roadmap was inspected as
prior engineering analysis. It is not approval or an implemented interface.

## Approaches

1. **Separate sampler process and nonblocking supervisor (recommended).** Fixed
   child binary, private inherited channels, independent stop event, private Job
   Object and retained process handle. Reuses the existing probe, monitor, clock,
   converter and codec; requires real Windows verification later.
2. **In-process sampler thread.** Less Windows transport code, but cannot provide
   independently killable containment when a getter blocks. Does not satisfy the
   existing integration contract and is excluded.

The first approach follows the selected roadmap architecture. The details below
are engineering proposals, not previously approved owner API preferences.

## Scope and deliverables

- Preserve existing public converter imports while moving `sampler.rs` into
  `sampler/mod.rs` if needed. Introduce separate wire, supervisor, child, Windows
  transport/containment and fixture modules; keep their responsibilities distinct.
- Add fixed `autotune-nvml-sampler` and `autotune-readonly-controller` binaries,
  plus a separately featured fixed fake sampler. No arbitrary executable, DLL,
  kernel, endpoint or shell override. Compiling binaries does not run them.
- Extend `NvmlSampler` with a read-only description of the matched raw UUID/PCI
  and existing runtime evidence. Retain the matched strings during selection;
  reuse the trusted loader/session and current report types. No additional
  control APIs or altered getter semantics.
- Expose the existing clock validator for separate child-observation validation,
  without changing numeric policy, original epoch or sticky clock rejection.
- Integrate a bounded raw trace and an honest sampler-only envelope producer.
  Leave CUDA/benchmark worker launch and native compute admission disabled.
- Use existing locked Rust dependencies offline. Continue in the current checkout
  under the recorded dirty-main preference; preserve existing unrelated changes.
  No automatic staging, commit, push, installation or recovery expansion.

## Binding and protocol

An immutable `CaptureManifest` binds schema/version, nonzero session/run/clock
IDs, selected parsed device identity, exact raw NVML UUID/PCI strings,
source/tree/lockfile and controller/sampler artifact digests, frozen profile and
protocol digests, approval reference, scope `readonly_sampler`, and fixed output
leaves. An approval reference or digest is never authenticated consent. Runtime
clock frequency and original anchor accompany Bootstrap; neither revises consent.
Actual launch remains blocked without separately retained exact approval.

Use four inherited anonymous pipe lanes (ordinary and control in each direction)
and an independent manual-reset stop event. Only allowlisted child endpoints and
the event are inherited through STARTUPINFOEX; process/job/controller endpoints
are not inherited. Each direction/lane has its own checked sequence starting at
zero; no reconnect, replay or wrap. Binding includes version, session, run,
role=sampler, lane, sequence, manifest digest and clock token on every message.

Frames have a little-endian u32 total byte length including prefix and canonical
UTF-8 JSON using the existing codec. Validate length before allocation, exact
ordered keys, canonical decimal strings, types, binding and sequence before
dispatch. Reject unknown/duplicate keys, truncated/oversize input, sequence gaps,
wrong identity/version/role and uncertain sends. Never drain an attacker-declared
oversized body. Retain bounded rejected prefix/bytes and the rejection reason.

| Message | Required evidence and semantics |
| --- | --- |
| Bootstrap | Exact manifest, original anchor/frequency and inherited-object inventory; first ordinary controller frame, once only. |
| Ready | Fresh matched identity/runtime, measured sampler digest, frequency and readiness anchor; first ordinary child reply. Not sampling or admission. |
| Query | Exact outstanding monitor Ticket, only after Ready; controller owns the schedules. |
| MetricRecord | Echoed Ticket, actual before/after getter anchors and complete bounded raw Field: unit, integer or unavailable reason, vendor status, symbol state and detail. |
| Stop | First trigger/reason and frozen cooperative/exit deadlines; control lane plus independent event. Repetition cannot extend deadlines. |
| StopAck / RoleFinished | Anchor, pending getter and cleanup attempted/result/unknown. Never process-exit proof. |

The implementation plan must freeze each typed body key order and golden wire
vectors before parser implementation. Numeric failure/status codes retain exact
signedness. Raw detail lengths are checked against remaining frame/metadata
budgets before building owned strings; no silent truncation or synthesized Field.

## Timing, sampling and stop

One child execution lane owns initialization, description and all getters.
One Ticket can be pending. Control reception/event signaling is independent of
that lane; stop closes admission even while the getter remains blocked. Never
open a replacement sampler/session or dispatch another getter after uncertain
completion. A StopAck may report a pending getter; it cannot prove cancellation.

Keep the controller-issued Ticket start and actual child getter start distinct.
Require issue <= actual start <= end <= controller-captured receipt, using the
shared original epoch/frequency and retained bracket uncertainty. Ambiguous order
rejects rather than inventing precision. Validate child anchors sequentially on a
separate clock track; controller receipt is never supplied by the child. Existing
monitor Record validation uses the conservative issue-to-end bracket; exported
query-start remains the actual child observation. A transport delay may therefore
reject a short physical getter; do not relax the monitor to make it pass.

Use `map_field` once; retain its raw result and request sticky stop on conversion
error, including GPU loss on optional metrics. Use existing Controller begin,
poll, complete and request_stop; no safety refresh from rejected/late evidence.
Missed slots come from the controller, not manufactured child observations.

All safety/resource/time values come from `Profile::approved_design().limits`:
250 ms schedules, 50 ms query bracket, 100 ms transport, 750 ms safety gap,
10 ms controller polling target, 1 ms anchor bracket, existing 5 ms clock limits,
75 degC/200000 mW stops, 1 s cooperative and 3 s exit observation. Equality wins
for deadlines. Startup consumes the original safety gap; Ready cannot reset it.

Sampler-only capture requests orderly stop at original origin + the existing
10 s pilot preflight limit, possibly earlier for any fault or safety gap. This
is a proposed bounded capture scope, not execution approval or a workload pilot.
The 180/600 s session ceilings and 8 s finalization reserve remain unchanged.

Supervisor phases are Requested, Launched, Ready, Sampling, Stopping,
ExitObserved, Finalized. Any active phase can stop. Finalization may occur with
unknown exit. Preserve the first terminal reason/time and pending uncertainty;
late evidence is supplemental and cannot upgrade the frozen outcome.

Clock capture failure closes admission without a fabricated anchor. A separately
established sleep-including shutdown watchdog must still request termination
within the frozen grace/observation bounds; if it cannot establish trustworthy
remaining time, request termination immediately and record timing/exit unknown.
It must not wait for another successful original-clock capture to escalate.

## Windows containment and bounded I/O

Measure and hold the fixed absolute child binary against replacement through
launch; launch suspended and quietly. Configure/read back job commit/CPU,
one-process and kill-on-close limits, assign the private job, then resume.
Reject nested-job/limit/assignment failures. Controller stays outside the job
and holds its sole noninherited job handle plus original process handle and
creation identity. Keep the existing fake-worker launcher fake-only.

Separate bounded pipe/writer workers isolate synchronous operations. The
supervision path performs no synchronous pipe read/write, native query, disk
write or unbounded join. Cancellation of pending I/O and unresolved thread
observations must be recorded. A full ordinary queue cannot consume reserved
control capacity or block the stop event. Queue/control overflow still fails.

At stop, signal event and enqueue Stop independently. At grace expiry request
job termination. Observe retained process handle wait status, exit-code result,
termination result and job active-process query independently, with errors and
timestamps. EOF, child done, PID disappearance, termination-call success and
ack are insufficient. Direct exit plus active-count zero proves only OS scope;
it cannot clear unknown NVML/GPU completion. Controller loss relies on
kill-on-close and leaves an unresolved capture; no automatic replay/continuation.

Enforce existing sampler 256 MiB/30 s CPU and controller 512 MiB/60 s CPU bounds
where supported. Controller resource observations/allocation accounting are not
an OS hard-cap claim. The plan must identify enforceable limits and fail closed
when required support is absent, without changing enclosing host job policy.

The aggregate allocation ledger includes all queues, raw metadata, codec model
clones, encoded strings, prefix buffers and finalization work before allocation.
Use the approved 64 KiB frame, 64 ordinary/4 reserved control frames per lane,
32 MiB retained evidence RAM including 8 MiB writer and 4 MiB failure reserves,
32768 samples, 65536 events, 4 MiB metadata, 128 MiB aggregate output and 16 MiB
diagnostics caps. No eviction, unchecked count-based allocation or hidden
temporary budget. An inability to retain mandatory evidence stops the capture.

## Output and honest results

Write only fixed run-owned leaves: manifest.json, profile.json,
raw-events.ndjson, session.json and capture-index.json. The raw trace preserves
issued and actual query brackets, controller receipts, rejected/missing/pending
data, process observations and supplementary late evidence. Index exact lengths
and ordinary artifact SHA-256 alongside the existing domain-separated digests.
Define index coverage without a self-hash cycle. Failures leave an explicit
incomplete capture; no durable-recovery guarantee or fabricated complete index.

Use unchanged envelope semantics. Telemetry observed count is distinct
(metric, slot) pairs; expected schedules run from original zero through admission
closure with scheduled boundary slots included. No padding or overwriting pairs.
If a late event cannot fit primary lifecycle ordering, preserve it only as raw
supplemental evidence. Preflight codec/temporary capacity before model creation.

Sampler-only result keeps worker/CUDA/context/PTX identity unavailable, physical
worker mapping unknown, dispatches empty, admission blocked, correctness not
checked, native completion unknown, coverage incomplete and verdict blocked
(invalid for faults). Scope process/job observations to sampler explicitly.
Use not_applicable for kernel/generator/oracle text where the schema requires it;
retain sampler_only, workload_disabled, sensor_age_unknown and
native_completion_unknown limitations. ABI fakes remain simulated_fixture;
live_observed requires separately authorized actual observations. No baseline,
energy, efficiency or tuning verdict follows this slice.

## Acceptance and next boundary

Implementation exit: integrated source for transport, supervisor, fixed child,
Windows containment and raw/envelope production; independent review; source-bound
offline CPU tests, compile/link checks and formatting/Clippy. Tests exercise
real shared typed messages and existing controller/converter composition, with
no sleeps or test-only duplicate policy.

Cover six metrics; malformed/truncated/oversize/wrong-binding/sequence input;
frequency/identity mismatch; blocked getter with continued ticks/missed slots;
stop before Ready/during query/at exact deadlines; clock failure escalation;
queue/writer/reserve exhaustion; EOF/done/terminate without observed exit;
unknown job count; controller loss model; late evidence without outcome upgrade;
canonical export and bound capacity accounting. Preserve adjacent converter,
monitor, probe no-setter, worker rejection and envelope regressions.

Do not run bare workspace tests that include CLI/native/clock/process execution.
Explicitly selected deterministic CPU tests may run offline; Windows production
and fake-process paths compile only. Fake-process tests are ignored and require
a separately approved source/artifact-bound execution matrix. Real read-only
capture requires a separate exact manifest with device/runtime, binary hashes,
duration, quotas/output root and human consent. GPU workload execution is a later
separate boundary after worker/device/manifest/live-evidence integration.

This design is not another product-status ledger. Update ROADMAP.md handoff and
the paired implementation checklist as work proceeds. E3 stays partial and Gate B
unmet until their actual acceptance criteria have evidence.

## Builder review — 2026-10-29

Independent read-only reviewer `/root/sampler_design_check` read this proposal
and its referenced contracts/source. Result: nothing important remains unanswered
before implementation planning; remaining choices can reasonably be settled by
the builder. This is static design review, not test/process/native evidence.
The owner approved this design and explicitly delegated ongoing implementation
without further routine questions on 2026-10-29 (`ja sluta fråga mig ... non-stop
... slutföra roadmappen`). The paired implementation plan is
`docs/superpowers/plans/2026-10-29-e3-independent-sampler.md`. Exact native,
process-fixture and hardware execution consent remains separate.
