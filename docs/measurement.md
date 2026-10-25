# Synthetic telemetry recording — E3.2 foundation

`autotune-measurement` is an independent, pure library for caller-supplied
**synthetic** observations. It has no probe/recovery dependency, hardware/runtime
loader, process execution, clock, I/O or deserialization path. It neither executes
a workload nor establishes physical GPU/workload mapping. E3.2 remains partial.

## API and evidence semantics

Construct `Identity::new(run_id, workload_id, workload_version, workload_seed,
gpu_uuid, pci_bus_id)` and `Policy::new(sample_capacity, maximum_age_ns)`, then
`Recorder::new(identity, policy)`. Both policy values must be positive; there are
no defaults or approved product thresholds. Capacity is a sample count, not an
allocation request: construction does not preallocate it. Each identity string
must be nonblank, at most 256 UTF-8 bytes and contain no control characters.
Identity strings are labels, not validated physical device identifiers.

`append(&binding, metric, reading, observation_ns, receipt_ns)` requires an exact
match of all identity fields, including workload version and seed. Recorder
identity/policy are immutable; `samples()` exposes only an immutable slice in
accepted append order. `Reading` is either an available signed integer or a typed
unavailable reason (unsupported, permission denied, query failed, device lost,
not sampled). Missing values never become zero. Temperature accepts signed `i64`;
clocks and power require nonnegative `i64`; utilization requires 0..=100.
Units derive only from the metric: temperature `degC`, graphics/memory clocks
`MHz`, GPU/memory utilization `%`, power `mW`. These are integer measurements,
not an energy, performance or correctness verdict.

Both timestamps are caller-supplied `u64` nanosecond offsets in **one run-relative
monotonic epoch**. They are not UTC, measured elapsed time or proof of an actual
clock. Observation must not follow receipt; checked subtraction computes age.
Receipt timestamps cannot regress globally (equality allowed); observation
timestamps must strictly increase per metric, including unavailable observations.
Different metrics may share observation times. Age is evaluated only at receipt:
age <= maximum is fresh, otherwise stale. Export never ages or discards samples;
stale values and unavailable reasons retain their original timestamps.

Invalid binding, numeric value, ordering or a full capacity returns a typed
`Error`. No failed append changes retained samples or ordering state, and no
sample is evicted. **Any failed append permanently sets `had_rejections`**, even
if later appends succeed. Constructor failures do not create a recording.
Multiple simultaneous violations have no guaranteed error precedence.

`to_json()` returns deterministic schema-version-1 JSON with simulated-only
provenance, time basis, identity, policy, raw samples and the rejection marker.
It does not claim completeness; absence of rejections does not imply all metrics
were collected. This is an internal synthetic format, not a frozen production
protocol. The policy bounds retained sample count, not serialization memory or a
caller-selected total memory budget.

Exported `u64`/`i64` values require lossless integer consumers. Ordinary JavaScript
`Number` decoding can round integers beyond `2^53 - 1` in magnitude. No JavaScript
interoperability is verified.

## Synthetic energy estimate — narrow E3.3 slice

`energy::Policy::new(start_ns, end_ns, maximum_gap_ns)` requires an explicitly
supplied positive-duration inclusive observation window and positive maximum gap.
Equal/reversed endpoints or a zero gap return typed `PolicyError`s, not unavailable
evidence. There are no defaults or approved product thresholds.
`energy::estimate(&recorder, policy)` returns an immutable owned report.

Exact power observations must exist at both boundaries; there is no endpoint
interpolation or extrapolation. Every in-window power observation must be available
and fresh **at receipt**, with adjacent observation gaps <= the supplied maximum.
Unavailable/stale samples are never filtered out, bridged or treated as zero.
Any sticky recorder rejection makes the entire window unavailable, since the
rejected metric/time is unknown. Non-power samples and unavailable/stale power
outside the window are ignored otherwise. Typed unavailable reasons identify
missing endpoints, unavailable/stale observations, excess gaps or rejected input;
no partial energy quantity is exported. Multiple failures have no promised
precedence. Estimation never modifies the recorder or re-evaluates freshness now.

For valid coverage, piecewise-linear integration uses **observation**, not receipt,
times: `twice_picojoules = sum((left_mW + right_mW) * delta_ns)`.
Operands are widened before addition and all u128 sum/product arithmetic is checked.
One mW-ns is one pJ; the doubled numerator retains half-pJ values without rounding.
Schema-version-1 JSON exports `twice_picojoules` as a **decimal string**, never a
JSON number, plus interval count, algorithm identifier, identity, original recorder
policy, window/gap policy, run-relative time basis and simulated provenance.
Other integer fields still require lossless consumers as above. Unavailable reports
carry only a typed reason in their outcome, not an energy quantity.

This is **estimated synthetic energy**, not directly measured physical energy,
workload correctness or an efficiency claim. No measured-energy producer exists.

Standalone report identity and policy neither uniquely identify nor authenticate
a trace. The report contains no trace digest or embedded samples: retain the exact
original recorder export alongside the report. Identity equality is never proof
of trace integrity.

## Synthetic repeat-run provenance ledger — narrow E3.3 slice

Construct `repeat::Policy::new(run_capacity,
maximum_metadata_entries_per_category)` with two explicit positive counts, then
`repeat::Ledger::new(policy)`. Neither count has a default or triggers
preallocation. `ledger.append(&recorder, context)` snapshots the **exact original
Recorder JSON string**, including raw samples, original recording policy and
sticky recorder rejections, plus its immutable identity and supplied context.
Empty, stale, missing-data and rejected recorders are retainable evidence, not
successful or complete baselines. No energy/performance aggregation is performed.

`repeat::Context` has three required explicit fields: `conditions`, `settings`
and `versions`. Each is `repeat::Metadata::Provided { entries }`, a nonempty
`BTreeMap<String, String>`, or `Unavailable { reason }`. A provided map is only a
**partial caller assertion**: it does not establish completeness, actually applied
settings, readback, licensed execution or physical identity. There is no required
vocabulary or approval of its contents. Keys have exact case-sensitive string
semantics with no normalization or trimming; maps export in sorted key order.
All keys, values and unavailable reasons must be nonblank, at most 256 UTF-8 bytes
and contain no control characters, matching the recorder identity convention.
The entry limit applies independently to each category, not to their sum;
unavailable categories carry only their reason and consume no entries.

Context validation occurs at append. Typed `repeat::Error` variants distinguish
zero policy counts, empty/overfull categories, invalid key/value/reason (with
`Category` and `Field`), duplicate run IDs, full run capacity and serialization
failure. Any failed append sets sticky `had_rejections` without changing accepted
snapshots; later valid appends do not clear it. No eviction or replacement occurs.
Multiple simultaneous violations have no promised error precedence. Invalid
policy construction creates no ledger. Counts bound retained evidence, not a
total memory/serialization budget; recorder snapshot size follows the supplied
recorder's own policy and evidence.

Run IDs must be distinct, even if other identity fields change. IDs are labels,
not authentication or trace-integrity proof. Other workload/version/seed/device
labels may differ freely and are preserved, never silently equated. Timestamps
stay relative to each run independently: equal or differing offsets imply no
cross-run epoch relationship or comparability.

`runs()` exposes an immutable append-ordered slice of `repeat::Run`; its
`identity()`, `context()` and `recorder_json()` accessors expose only immutable
snapshots. Later source/context changes cannot rewrite retained evidence.
`policy()`, `had_rejections()` and `assessment()` expose immutable ledger state.
Deterministic `to_json()` exports schema version 1, simulated provenance,
run-relative time basis, immutable policy, ledger rejection marker and snapshots.
Its assessment is always `not_evaluated` (`repeat::Assessment::NotEvaluated`),
never a successful baseline, comparability or efficiency verdict. Repeated export
does not modify sources or re-age evidence. This internal format does not change
the recorder or energy schemas. Embedded recorder JSON is a string, not a parsed
or rewritten object; consumers must preserve its exact bytes and the lossless
integer requirements described above when decoding it.

## Synthetic direct-worker lifecycle — narrow E3.1 slice

Owner decision (2026-10-25, recommended choices 1A–5A/9A): implement the pure
synthetic lifecycle first, distinguish stop intent from observed termination,
keep late terminal evidence as conflicts, scope evidence to the direct worker,
and use explicit caller phase deadlines with unknown outcomes on expiry.
The four outcomes below are observation labels, not correctness verdicts.
Absolute-offset deadlines and expiry winning ties are internal technical
conventions for this model, not hardware limits or a real workload safety contract.

`lifecycle::Binding::new(identity, direct_worker)` owns the complete existing
identity plus a worker label (nonblank, at most 256 UTF-8 bytes, no control
characters). These are immutable synthetic labels, not process authentication,
license/consent evidence or GPU mapping. Construct
`Deadlines::new(requested_ns, launched_ns, ready_ns, running_ns, stop_requested_ns)`
and `lifecycle::Policy::new(event_capacity, deadlines)`, then
`Lifecycle::new(binding, policy)`. Every deadline and the capacity must be positive;
there are no defaults, ordering requirements or capacity preallocations.
Construction explicitly requests at run-relative synthetic time zero, with no
launch evidence or outcome. Terminated has no deadline.

`append(&binding, event, at_ns)` requires exact identity (including seed/version)
and worker binding, nonregressing `u64` synthetic time (equality allowed), and
remaining event capacity. This is one supplied effective event time, not separate
occurrence/receipt times or a qualified observation clock. Nothing advances time
spontaneously; export does not age evidence. There is no real clock or suspend
guarantee. Deadlines are **absolute offsets from request time zero**, not durations
rebased on phase entry or repeated stop requests.

Normal order is explicit `LaunchedObserved` -> `ReadyObserved` ->
`RunningObserved`, from requested -> launched -> ready -> running. None can skip a
phase or resume after stop. `StopRequested` branches from any active phase; repeats
are no-ops and establish intent only. It never proves termination.
`TerminatedObserved { reason }` is the only event that marks phase terminated:

- `Completed` requires previously observed running, including completion after
  a stop request; outcome is `completed_observed`, not correctness or success.
- `Stopped` requires stop-requested phase and previously observed launch;
  outcome is `stop_observed`, without a causality claim.
- `Crashed` requires previously observed launch, including early launched/ready
  failure; outcome is `outcome_unknown`.

`NeverLaunchedObserved` explicitly asserts launch was never dispatched; it requires
no observed launch and requested/stop-requested phase, sets `never_launched`, and
does not fabricate termination. Missing launch alone proves nothing. `WorkerLost`
requires observed launch; `ControllerLost` is allowed in any active phase. Both
close `outcome_unknown` while retaining the last observed phase. `AdvanceTime`
supplies time without a requested phase transition.

After binding/time/capacity validation, the active deadline is checked **before
event semantics**. At equality or excess, even an otherwise invalid-phase event
is retained as `BoundExceeded`, closes `outcome_unknown`, and does not apply its
requested transition. Expiry therefore precedes competing completion. A valid
transition into an active destination also checks that destination's deadline at
the same time: its observed new phase is retained, but an already-expired bound
closes unknown. No expiry infers termination, cleanup or physical runtime bounds.

Outcome terminalization is separate from phase. Once any outcome is set, it and
the phase are immutable. Subsequent binding/time/capacity-valid events are retained
as explicit `Conflict` with sticky `had_conflicts`; only `AdvanceTime` is a retained
no-op. Late termination cannot overwrite loss/expiry, and late stop cannot overwrite
completion. Invalid phase/order, binding, time or capacity returns typed
`lifecycle::Error`; failed appends change only sticky `had_rejections`, not accepted
events, time, phase or outcome. Multiple validation failures have no promised
precedence. Full capacity never evicts evidence or infers progress, even at expiry.

`events()` exposes immutable append-ordered records containing the raw event/time,
decision and resulting phase/outcome. `binding()`, `policy()`, `phase()`, `outcome()`,
`had_rejections()` and `had_conflicts()` expose immutable state. `to_json()` exports
deterministic schema-version-1 JSON with simulated provenance, run-relative time
basis, request/last accepted times, binding, policy, all accepted records and sticky
markers. Descendants, orphans and resource release are explicitly `unknown`;
correctness is `not_evaluated`, even after completion. This additive internal schema
does not change recorder, energy, repeat or recovery formats. Integers require
lossless consumers as above. Count/string limits bound retained evidence, not total
serialization memory, real execution or resources.

## Remaining integration and decisions

There is no fixed cadence, live gap detection, live collection, real workload
lifecycle integration (E3.1), baseline/repeated-run comparison (E3.3), live energy
integration or optimizer.
Future work needs approved workload/license/execution decisions, reliable device
mapping, live time/collection policy, cadence and missing-data handling, required
baseline metadata vocabulary/completeness, warm-up, variation/drift, seed-matching,
conditions and objective/uncertainty policy. The ledger does not evaluate these
policies. Full E3.3 and Gate B remain open. Synthetic tests do not qualify
hardware, recovery, performance or any product release gate.

The approved E3.1 slice resolves synthetic lifecycle semantics only, not full E3.1
or Gate B. Fixed licensed backend selection, verified GPU mapping, process isolation,
real readiness/termination evidence, cancellation/escalation, descendants/orphans,
resource bounds and clock qualification remain unfinished integration work.
Recovery fake-worker contracts are not workload policy. No workload/process or
hardware execution is implemented or authorized; process exit is not E4 correctness.
E3.3 metadata evaluation, warm-up and descriptive statistics are separate work.
