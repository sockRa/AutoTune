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
