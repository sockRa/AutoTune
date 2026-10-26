# Pure FMA design and CPU reference — E3 foundation

`autotune-fma-reference` is an independent pure Rust library. It validates the
approved **design** profile, generates versioned mathematical input data and
checks **caller-supplied** FP32 vectors against an independent integer oracle.
It contains no GPU workload, PTX, CUDA/NVML binding, process, clock, I/O,
deserialization, execution-consent or live-evidence producer. Unsafe Rust is
forbidden. Nothing in a passing reference check proves that a GPU ran.

## Frozen profile, not runtime enforcement

`profile::Profile::approved_design()` explicitly selects the owner-approved
`e3-fma-qualification-v0.1` values. There is no `Default`. Public draft fields
allow inspecting/testing candidate data; `ValidatedProfile::new(profile)` rejects
unsupported versions, checked arithmetic/coherence failures and **any** mismatch
from the frozen version. It owns the accepted snapshot and exposes only shared
access plus computed reservation totals. Mutating a separate draft cannot modify
it. Error precedence for simultaneous defects is not promised.

The profile contains the three fixed pilot shapes/seeds, seven-run acceptance
plan, counts/phase caps, session/stop/time budgets, temperature/power limits,
RAM/application-VRAM/IPC/evidence ceilings, collection/clock policies and
repeatability/loading limits. Basis-point fields are integer hundredths of a
percent: 200 = 2%, 300 = 3%, 500 = 5%. It computes pilot reservation 163 s and
acceptance reservation 553 s within 180/600 s design ceilings; ordinary six-metric
250 ms slots across 600 s total 14400. These are arithmetic facts, not evidence
of actual scheduling, warm-up sufficiency, sensor accuracy or hardware safety.

### Checked-in frozen numeric design reference

The owner's Q4 acceptance applies to these values for this profile version;
it grants no execution permission. This table/semantics and the named fields in
`profile.rs` are readable in a fresh clone without ignored BMAD planning files.

| Design dimension | Frozen values and meaning |
| --- | --- |
| Pilot stages | `(lanes,K,seed)` = `(65536,4096,0)`, `(262144,16384,1)`, `(262144,65536,2)`; 128 warm-up +256 measured dispatches each; 45 s envelope |
| Acceptance | 7 independent P3-shape runs with seed0; 1024 warm-up +2048 measured; 75 s envelope; 4096 maximum dispatches/run, never retry-to-green |
| Phase caps | Pilot re-entry/setup/warm-up/measured/exit =10/10/5/10/3 s; acceptance15/10/15/30/3 s; cumulative boundaries10/20/25/35/38 s and15/25/40/70/73 s |
| Session reserves | Pilot180 s, acceptance600 s; each reserves preflight10 s, finalization10 s, shutdown8 s. Unused reserve is not extra work |
| Safety/entry | Stop at temperature **>=75 degC** or GPU power **>=200000 mW**; enter only after four valid250 ms observations with temperature<=55 degC and GPU utilization<=10%. No control changes to meet cutoffs |
| Stop windows | Controller target10 ms; dispatch submit-to-completion100 ms, complete cycle250 ms; cooperative1 s and exit observation3 s from **first trigger**, capped by earlier run/session deadlines. Not hard real-time/native stop guarantees |
| RAM/CPU/jobs | Worker/sampler/controller commit1 GiB/256 MiB/512 MiB; separate worker/sampler jobs, one process each. CPU worker45 s/run, sampler30 s/session, controller60 s/session |
| GPU buffers | Application16 MiB cap,512 MiB free headroom after planned allocation; P3 four buffers4 MiB. Excludes opaque driver/context memory and does not reserve physical VRAM |
| IPC/evidence | Frame64 KiB; ordinary/control queues64/4 frames; samples32768, other events65536; metadata4 MiB; evidence RAM32 MiB with writer8 MiB/failure4 MiB inside; total serialized128 MiB includes logs16 MiB |
| Collection/clock | Six metrics250 ms; query bracket<=50 ms, transport age<=100 ms, observation gap<=750 ms. Safety gap reaching750 ms stops. Clock bracket<=1 ms, agreement<=5 ms plus retained bracket uncertainty, suspend divergence>5 ms stops; sensor internal age unknown |
| Required samples | Temperature, power, graphics clock, memory clock and GPU utilization required for baseline; memory utilization optional. Temperature/power required before compute and missing/stale safety data requests stop. Measured window>=8 fresh observations per required metric with no bad included records/gaps |
| Descriptive screens | Seven valid independent rates: unscaled relative MAD<=2%, first/last-three median drift<=3%, **every** run within5% of median. Undefined/zero denominator or insufficient evidence is inconclusive, never a pass |
| Transition/loading | Last-quarter warm-up vs first-quarter measured medians: temperature difference<=2 degC, graphics/memory-clock difference<=5%; >=4 samples per compared quarter. Median GPU utilization>=80% and submit-to-complete fraction>=70% are loading proxies, not optimizer-sensitivity proof |

First/last-three drift windows are chronological nonoverlapping **runs**, not
samples. Quarter selection and attribution of boundary query brackets must be
specified by the future baseline assessor before implementing the transition
screen; this numeric-only library does not decide time-versus-cycle quarter
partitioning or invent coverage. No statistical assessment is delivered here.
Descriptive criteria allow equality; safety stops use the operators above.
Missing/failed runs remain visible and prevent acceptance; no outlier deletion,
automatic relaxation, warm-up shortening or repeated seed reset within a run.
Power/temperature values are approved design cutoffs, not manufacturer safe
limits. All settings stay unchanged and observed, not assumed factory/known-good.

`RunPlan::phase_deadline_ns(phase, run_origin_ns, first_entry_ns)` is a pure,
checked offset calculation: earlier of original cumulative phase boundary and
first entry plus phase cap. Use a validated plan; the calculator does not retain
first-entry state or enforce one-shot entry, monotonic time, session ceilings,
deadline tie rules or termination. Its caller must implement those lifecycle
requirements. Both absolute candidates must fit u64; otherwise the calculator
returns `ArithmeticOverflow`, even if the other candidate would fit. No clock is
sampled. All numeric design values remain subject to
qualification; modifications require a new approved profile revision, not a
relaxed constructor. No baseline/statistical assessment is implemented here.

## Inputs and integer oracle

`input::Shape::new(lanes, steps)` checks 1..=262144 lanes and a positive K
multiple of 4096 up to 65536. This mathematical domain permits small test vectors;
it is not approval to execute arbitrary shapes. Profile validation additionally
requires the exact frozen stage/acceptance shapes. `DispatchIndex::new(j)` permits
0..=4095; j spans warm-up **and** measurement and must not reset between phases.
These constructors allocate no lane/output arrays.

`Recipe::new(seed, shape)` retains immutable seed/shape. `block_digest` hashes:

```text
UTF-8("AutoTune/FMA/input/v1") || one zero byte || seed:u64LE || block:u64LE
```

Each digest supplies eight u32LE words. Low/next nibble select signed nonzero
integers a/b with magnitudes 1..8; bits 8..15 are signed-eight-bit base. High bits
are unused. Lane `i` uses block `i/8`, word `i%8`. `recipe.lane(i)` is indexed;
`recipe.lanes()` streams in constant space, hashing each block once. Neither
materializes a run or precomputes every dispatch. `Lane::inputs(j)` returns exact
FP32 representations of a, b and `x0 = 4096*base+j`.

`reference::expected_integer(lane, steps, j)` evaluates the independent CPU
closed form `4096*base + K*a*b + j`, not repeated host/GPU FMA. Validated immutable
types bound every intermediate within i32 and the FP32 exact-integer domain
below 2^24. For the maximum design, the conservative magnitude bound is
4722687 < 16777216. Expected mathematical modulo 4096 equals j, including for
negative outputs: stale same-run dispatch data cannot match a different tag.
Identical recipes across different runs can legitimately match; tags and hashes
do not authenticate device, run or process identity.

## Checking supplied outputs

`reference::check(recipe, dispatch, output)` checks length **before** scanning.
Length error retains expected/actual counts. Otherwise it visits every lane even
after a failure, allocating no report/vector buffers, and returns an immutable
owned report bound to the exact recipe and dispatch. The report exposes checked
lane count, all mismatches (including nonfinite), nonfinite subset and the first
failure's index, expected integer, exact original FP32 bits and typed kind.

Only exact numeric agreement counts; +0/-0 agree for an expected zero. NaN,
infinity, wrong, partial or stale-tag data fail. The source is never changed.
`matches_reference()` concerns **only supplied data**, not observed computation,
performance, correctness of all hardware behavior or an accepted baseline.
The report does not retain the complete output trace or its digest; future
evidence producers must retain their own bound raw data/digests/retention scope.
This library does not claim archival or verification of GPU output bytes.
This simple checker regenerates lane/reference data while scanning and is not a
prepared-reference timed-worker API. A future native worker must prepare bounded
lane/expected bases before its measured window as required by the profile; no
claim of that integration/performance qualification is made by this delivery.

## Verification boundary

Deterministic tests pin all published numeric fields, profile coherence failures,
input domain/endian/sign/seed/block boundaries, independent documentation vectors,
integer/FP32 exactness, complete tail scanning, original nonfinite bits and source
immutability. SHA-256 golden values were independently calculated with .NET;
the implementation uses the existing locked sha2 0.10.9 dependency, not recovery
or any new vendor runtime. The checker never executes the prospective workload.

Existing measurement schemas and simulated provenance stay unchanged. CI adds
this pure library to Windows deterministic tests; hosted execution is not claimed
for these changes until separately observed. See [CI scope](ci.md) and the
[canonical roadmap](../ROADMAP.md) for dated evidence and remaining gates.
