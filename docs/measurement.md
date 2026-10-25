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

## Remaining integration and decisions

There is no fixed cadence, gap detection, live collection, workload lifecycle
(E3.1), baseline/repeated-run comparison (E3.3), energy integration or optimizer.
Future work needs approved workload/license/execution decisions, reliable device
mapping, live time/collection policy, cadence and missing-data handling, baseline
conditions and objective/uncertainty policy. Synthetic tests do not qualify
hardware, recovery, performance or any product release gate.
