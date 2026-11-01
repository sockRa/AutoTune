# E3 workload manifest and Start binding

Engineering continuation for package 2, E3.1/E3.2/E4.1, under the owner's
continuous software-implementation direction. Read ROADMAP.md and the retained
first-benchmark integration, qualification-profile and live-evidence contracts.
Those contracts and all approved numeric values remain unchanged.

The next integrated source boundary is a separate pilot workload manifest and
worker Start vocabulary. It must not turn the readonly sampler manifest into
workload consent or weaken the existing simulated worker's native rejection.
Actual sampler fixtures, native loads and GPU execution retain their separate
execution approvals; the previously requested fake-only matrix is still pending.

## Initial implementation boundary

- Immutable pilot declaration: exact nonzero session and three independent run
  IDs, original clock token, exact raw NVML UUID/PCI, source commit/inventory,
  lockfile/controller/worker/sampler/PTX/protocol digests, approval reference,
  fixed output root, and explicit digests of starting-settings/environment
  evidence. The latter bind caller-declared artifacts, not their truth or the
  sufficiency of required facts. No launch/admission or baseline verdict follows.
- Serialize exactly the approved three pilot RunPlans, preserving seeds 0/1/2,
  shape/K, warm-up/measured counts and phase/envelope caps. Acceptance repeats
  require a separately qualified pilot and are not admitted by this manifest.
- Reuse the existing canonical codec and digest domains. Pilot profile bytes
  carry their own scope plus the unchanged numeric limits and three RunPlans;
  readonly sampler profile bytes must remain byte-identical.
- Reserve bounded manifest/codec storage before owned model construction;
  reject large strings/capacities and oversized/canonical/schema contradictions.
  Private validated fields prevent caller mutation after freezing.
- A bound run declaration keeps the original manifest/clock/device/artifact
  identity and run origin. It computes phase deadlines through the existing
  approved RunPlan, without rebasing or granting a Start permission. The later
  worker protocol/controller must validate real readiness, safety and consent.

Fixed manifest key order: version, scope, session, runs, clock_token, raw_uuid,
raw_pci, source_commit, source_tree, lockfile, controller, worker, sampler, ptx,
protocol, profile, starting_evidence, environment_evidence, approval_ref,
output_root. RunPlan keys: run, seed, lanes, steps, warmup_dispatches,
measured_dispatches, envelope_ns, phase_caps_ns. Digests bind canonical bytes;
there is no approved=true or native execution token.

## Tasks and evidence

### Worker handshake continuation

The next source slice implements real encoded worker-role frames and the child
handshake state boundary; OS transport/dispatch composition follows. It does
not add a native executable or a native admission token. Bootstrap/Ready/Start
are ordinary-lane messages with explicit controller-to-worker or reverse
direction; Stop/StopAck/RoleFinished use the reserved control lane. Each lane
and direction owns a sequence. Any decode/encode/transport-capacity rejection
poisons that channel; no successor recovery. Encoded frames/decoded message
ownership retain shared RAM charges until dropped/transferred.

Frame keys: version, role, lane, direction, sequence, binding, kind, body.
Binding includes session/run/device, manifest/profile/worker/sampler digests,
original clock epoch/token/frequency, run origin and the frozen full RunPlan.
Bootstrap carries exact manifest bytes; Ready carries observed identity/context,
image digest, driver/capability/prepared-lane assertions and a raw anchor. These
are self-reports, not physical mapping/execution proof. Start carries its issued
anchor and can occur once after a matching Ready. Stop carries its first reason
and trigger/cooperative/exit times capped at original run/session ceilings.
RoleFinished records cleanup attempted/raw return/unknown, never OS exit.

The handshake gate retains original-epoch ordering and first terminal state;
duplicate Start, premature Start, invalid clocks or late Start close admission.
Its Start intent uses a conservative warm-up bound from controller issue time,
including transport delay, capped by the existing cumulative phase boundary.
It never extends any approved phase cap or truncates required counts. Parent
safety/re-entry/required-fact/consent validation remains separately required;
gate acceptance alone is not native execution permission. Dispatch records,
failure-vector transport, independent process supervision and parent controller
composition are still required before a runnable benchmark can be claimed.

- [x] Implement bounded immutable pilot declaration and exact profile/plan
  encoding, with public API tests for source/artifact/device/run binding,
  budget exhaustion, tampering and unchanged readonly profile bytes.
- [x] Add strict reading and original-epoch bound-run phase calculations;
  round-trip exact declared plans and reject scope/plan alterations.
- [ ] Continue to actual worker Start/Ready/Dispatch transport and controller
  composition; keep native admission unavailable until prerequisites exist.
- [ ] Independent review and source-bound CPU/compile checks at a coherent
  implementation milestone; update ROADMAP.md and this log without claiming
  process execution, real correctness or useful-baseline acceptance.

## Execution log

- Final actual-adapter tracing verification binds baseline `ae44192` plus source
  with identical before/after inventories: **261 CPU-only tests** (204 integration,
  19 probe ABI fakes, 13 feature-library fakes, 25 reference/prepared), scoped
  all-feature warning-denied Clippy, workspace formatting and full-workspace/
  all-feature compile/link pass. Eight portable trace tests and three new typed
  CUDA ABI-fake tests cover raw status/clock/control/fault retention, original
  binding, trace charges, actual callback order and poisoned adapter successors.
  Manifest `target/workload-verification/trace-reviewed-manifest.json`, SHA-256
  `1f69ce90c5e7081329eae7cb412d5c8245779fe3a03c4ba416edaa98cb4c2d94`.
  Earlier trace-final/pre-review checks remain historical; this manifest covers
  the reviewed binding correction. Initial test enum-shadowing/array-comparison
  compile failures and Clippy large-error/array-chunks/unused-import findings
  were fixed without weakening lint. Both pinned original images still match.
  No DLL loading/native/process/GPU tests. Assessment `e53c7e2` passed both hosted
  platforms in run 41339063933; this source has its own later publication scope.

- Independent tracing review: Standards found zero hard violations/material
  smells. Spec found a material attribution defect: the first draft could bind
  collected A calls to a B summary/channel. Fixed by retaining exact original
  WorkerBinding/phase/j, validating summary/pending assertions, replacing plain
  Message exposure with borrowed record inspection and `Channel::encode_traced`
  enforcing the pinned identity. Final Spec review closed P2 with no remaining
  material defect. The regression covers changed manifest with same clock,
  seed/j/phase, foreign channel poison and preserved raw owners. Continuous
  cross-trace clocks and frozen phase entry remain producer obligations.

- Actual-adapter tracing continuation: `CudaBackend` now has observed
  submit/query/readback methods; existing Backend methods delegate through a
  no-op observer and preserve existing behavior. Every upload, default-stream
  upload-completion query, launch, kernel query and readback is observed before
  entry/after raw return. An observer failure keeps the adapter poisoned and
  prevents successors. Default-stream NOT_READY still fails submit without
  launch/retry; only the kernel query may remain pending.
- `workload::trace::CallTrace` owns reserved raw calls, injected clock/control
  captures and sticky first faults. It retains raw status before post-clock
  capture, including native failure plus missing post-clock. It observes the
  unchanged original phase/stream/cycle limits where entries are known; the
  future producer must keep one frozen phase entry across dispatches, supply
  Input/Check/Hash host stages and preserve actual summary/output evidence.
  Partial traces alone are not whole-cycle or native qualification.
- A successful bind transfers the trace and charge into immutable TracedRecord,
  borrowed directly by the actual worker encoder. Failed nonempty-summary binds
  retain both raw owners; a pre-reserved boxed error avoids copying a large
  stack error. Fault and first unmatched return remain separate owned supplemental
  evidence, not silently serialized as an ordinary matched call. Future exporter
  must retain them. No new binary, native admission or executed native path.
- Integration reconciliation: existing CUDA submit performs a required
  default-stream upload-completion observation before launch. Added its explicit
  Operation kind and made parent audit require it; prior aggregate trace fixtures
  omitted this real call. Successful order is now Input/uploads/sentinel/upload
  completion/launch/kernel queries/readback/check/hash. Numeric policy and old
  handshake/sampler bytes remain unchanged. Full native producer/host buffers,
  retained summaries/failure export, admission and Windows composition remain
  Task 3; this milestone closes the real-adapter per-call tracing seam only.

- Parent report-assessment milestone reviewed and source-bound verified:
  independent Standards axis found zero hard violations/material smells; Spec
  axis found no material defect. The count-completion dimension is separate from
  later fault and all runtime/qualification obligations. Eight new tests pass;
  final suite totals 250 CPU-only tests (196 integration, 19 probe ABI fakes,
  10 feature-library fakes, 25 reference/prepared), plus formatting, scoped
  all-feature Clippy and full-workspace/all-feature compile/link. Unchanged source
  inventories bind baseline `64e5c00` plus sources in
  `target/workload-verification/assessment-final-manifest.json`, SHA-256
  `862f6ac8311d33af79d922c6f0b744164b60859f0fb7e7f884b0bdb98002e713`.
  Original pinned sampler image hashes still match; no process/native/GPU paths
  ran. Dispatch `8fb5de3` passed Linux and Windows/MSVC in hosted run 41338199629.
  Native producer/parent admission/Windows composition remain Task 3, not a
  substitute benchmark-only product or completed E3/Gate B.

- Parent report-assessment continuation scope: add `workload::assessment::Audit`
  over retained worker messages with bounded fixed metadata and no backend/OS
  calls. It checks original Start, exact j order and full bound counts; distinct
  Input/upload-A/B/X/sentinel/launch/query/readback/check/hash call order/statuses,
  original raw clock brackets/order and separate source/receipt/controller-tick
  streams, pending/readback/full-check assertions and exact reported work. The
  auditor borrows messages: its caller must first retain every owned raw frame,
  receipt and rejection; assessment is no archive or live truth claim.
- Measurement entry now has a worker-role `PhaseEntered` frame, emitted before
  measured work. Warm-up uses the existing conservative issued-Start bound;
  measurement freezes its relative cap at the reported actual entry, capped at
  the original cumulative boundary. Before entry is known only that immutable
  cumulative deadline can be assessed. Native producer must send entry before
  work and stop on uncertain send; independent controller supervision remains
  mandatory. Queued evidence never rebases time; a newer controller tick is
  considered when assessing an older entry/receipt. Controller ticks and delayed
  receive observations use independent original-epoch clocks, preventing a valid
  queued receipt from spuriously regressing the tick stream.
- Per-cycle enforcement includes all input/submit/readback/check/hash work at
  250 ms and first stream upload through returned completion at 100 ms, both
  under earlier frozen phase ceilings with equality losing. Missing, malformed,
  failed, skipped or duplicate evidence closes assessment, retaining counts and
  first fault; late evidence cannot resume it. All-reports-complete is a narrow
  dimension, never consent, coverage, physical identity, replayed successful GPU
  vectors, teardown/exit or useful baseline acceptance. Safety/re-entry/exact
  consent admission and native/Windows/controller composition remain Task 3.
- Initial assessment checks: eight CPU tests pass, including actual encoded full
  128+256 plan, explicit phase entry, retained message ownership, delayed source
  and receipt streams, query NOT_READY before completion, malformed calls/clocks,
  fixed metadata charges, silent-worker ticks and exact deadline equality. An
  upload-start budget regression was observed red before correction (the first
  draft counted from launch); final focused tests and scoped Clippy now pass.
  Early compile failure used the wrong existing Phase variant name and is fixed.

- Dispatch-transport milestone delivered: nine new CPU tests cover raw stage
  clocks/statuses/unknowns, exact bound phase/count/work, strict nested rejection,
  independent body golden, ownership/control compatibility, gaps/duplicates/hash
  uncertainty and full P3 1 MiB vectors through actual encoded worker frames.
  Independent Standards review found zero hard violations/material smells;
  Spec review found no material defect, including the final explicit upload,
  sentinel/launch kinds and boundary-test delta. No native/process/GPU paths ran.
- Final dispatch verification binds baseline `ca6efa1` plus unchanged before/after
  source inventories: 242 CPU-only tests (188 integration, 19 probe ABI fakes,
  10 feature-library fakes, 25 reference/prepared), formatting, scoped all-feature
  Clippy and full-workspace/all-feature compile/link pass. Manifest
  `target/workload-verification/dispatch-final-manifest.json`, SHA-256
  `d9d9386129d900bf0e7e25aa9f5ae1c13ffad3576a23d56488b5069ff3e5d83d`.
  Both original pinned matrix images remain unchanged. Handshake `4c4fdfb`
  now passed Linux and Windows/MSVC in hosted run 41337285143. Task 3 stays open
  for parent adjudication/admission, native production and Windows composition;
  decode/hash completion alone never closes correctness, process or Gate B.

- Dispatch continuation scope (November 1): extend the existing worker channel
  with bound DispatchRecord and FailureChunk bodies, keeping handshake/sampler
  schema bytes unchanged. Records retain raw optional pre/post anchors and signed
  statuses for input/submit/query/readback/check/hash, bound plan/index/phase,
  lengths/full-check diagnostics, exact verified work and optional raw SHA-256
  input/output digests. Missing/contradictory raw clocks remain observations for
  parent assessment, not an admission/result verdict. Passed vectors are not
  archived; the first full failing vector travels as ordered bounded chunks.
  Receiver reserves its full storage before allocation, retains partial bytes on
  gap/foreign/hash/transport failure and never claims OS/native completion.
  Per-message structural bounds (64 raw calls, 2048 words per chunk, 4096 JSON
  structures) exist inside the unchanged 64 KiB frame/RAM limits. Overflow must
  stop/preserve caller-owned trace; never truncate or merge query calls silently.
  These are representation bounds, not new polling/operating budgets. Sampler
  complexity remains 256. Parent sequence/lifecycle/clock/coverage adjudication,
  native producer, Windows adapter and exact consent remain Task 3 obligations.

- November 1 handshake milestone: canonical worker frames and child gate now
  implement the scoped continuation above. Nine CPU tests cover actual bytes,
  independent Start golden, binding/sequence/canonical rejection, sticky lanes,
  ownership/control reservations, raw statuses, clocks and terminal/deadline
  boundaries. Standards review found zero hard violations/material smells.
  Spec review found two timing defects: setup consumed unused re-entry allowance
  and future Stop triggers were accepted. Setup failure was reproduced before
  correction; setup deadline is now frozen at Prepare and checked at Ready/Start,
  and future triggers fail. Genuine late triggers keep expired original ceilings;
  the reviewer withdrew its earlier suggestion to reject all late triggers.
  Final independent static review found no remaining material defect.
- Final source-bound handshake verification: baseline `de18d39` plus sources,
  unchanged before/after inventories, 233 CPU-only tests (179 integration,
  19 probe ABI fakes, 10 library fakes, 25 reference/prepared), formatting,
  scoped all-feature Clippy and full-workspace/all-feature compile/link pass.
  Manifest `target/workload-verification/handshake-reviewed-manifest.json`,
  SHA-256 `4c512b76ac0a3bd0755b3dd6f55dab78046bda96d1de2dd2dbb887c2fdd6f0bc`.
  The earlier eight-test pre-review manifest remains historical. No fake process,
  native capture or GPU execution occurred. Pinned matrix images still match.
- Declaration milestone `1bfcac3` passed Linux and Windows/MSVC in hosted run
  40952952692. Handshake publication is a further software milestone, not
  completion of Task 3: Dispatch/failure-vector transport, parent admission,
  independent Windows supervision and exact consent still remain.

- Initial declaration slice delivered: immutable scoped pilot manifest, exact
  three-stage plans/profile bytes, strict canonical reading and bound-run
  calculations. Six workload tests plus one readonly compatibility regression
  pass. The initial missing-module failure was observed before implementation.
  Readonly profile digest was captured from the old implementation before
  extracting `limits_value`; the placeholder assertion failure was baseline
  capture, not a production defect or an independent digest derivation.
- Independent review: Spec axis found no material correctness/spec issue in
  this declaration-only boundary. Standards axis found zero hard violations
  and one optional low duplication smell in internal canonical/hex helpers;
  keeping manifest errors local avoids a broader codec refactor in this slice.
  No native/process/fixture commands were run by reviewers. Actual Start
  protocol/controller/native admission remain unfinished, as task 3 states.
- Source-bound final checks: baseline `3ed68d2` plus the declaration sources;
  unchanged before/after inventory, 224 CPU-only tests (170 integration, 19 probe
  ABI fakes, 10 feature-library fakes, 25 FMA reference/prepared), scoped
  all-target/all-feature warning-denied Clippy, workspace formatting and
  all-workspace/all-feature compile/link pass. Manifest
  `target/workload-verification/workload-final-manifest.json`, SHA-256
  `c2cb6f00268b6e5a26e854eb480ae92c0265f2046d0f2aa3070add8b528fd61f`.
  The first Clippy pass rejected manual_contains; corrected before final checks.
- Build isolation correction: early CPU checks reused the sampler build target
  and replaced its root fixture image. Recovered the exact matrix-pinned child
  from the existing Cargo cache (verified SHA-256), restored it without
  modifying its cached hardlinks, and copied both pinned images to
  `target/sampler-verification/pinned-matrix-2f932f31/`. Final checks use
  `target/workload-verification/build` exclusively. Both original matrix image
  hashes still match; no fixture was executed. This source now differs from
  the old matrix revision, so an approved old-matrix execution must first
  re-establish that exact reviewed source snapshot reversibly, preserving all
  newer/user-owned work, rather than skipping its guard or changing its digest.
- Start: clean `3ed68d2` checkout; previous turn delivered concrete preparation
  and publication progress. Exact fake-only execution matrix remains pending,
  not an authorization to run its branch. Implementation proceeds inline,
  preserving source/artifacts needed for that existing request; if approval
  arrives while this work is dirty, isolate only these agent-owned source
  changes reversibly before revalidating the already pinned fixture revision.
