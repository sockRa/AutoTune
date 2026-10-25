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

## Remaining integration and decisions

There is no fixed cadence, live gap detection, live collection, workload lifecycle
(E3.1), baseline/repeated-run comparison (E3.3), live energy integration or optimizer.
Future work needs approved workload/license/execution decisions, reliable device
mapping, live time/collection policy, cadence and missing-data handling, baseline
conditions and objective/uncertainty policy. Synthetic tests do not qualify
hardware, recovery, performance or any product release gate.
