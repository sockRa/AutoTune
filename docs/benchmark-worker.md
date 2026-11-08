# E3.1 bounded benchmark-worker foundation

`autotune-benchmark-worker` is an independent package. Its portable worker admits
**simulated fixtures only**; native/live admission is unavailable. It moves the
automated benchmark/search/validation product toward an isolated checked workload,
but does not supply live monitoring, baseline acceptance, search or any release gate.
No existing recovery, probe or synthetic measurement implementation is changed.

The crate also hosts the portable `evidence` module: canonical value encoding,
decimal integer/ratio accessors, domain-separated digests and a strict bounded
decoder. It is a byte/digest layer only, with no Windows, clock, process or CUDA
dependency, and is documented in [live-evidence.md](live-evidence.md). It creates
no live evidence and closes no gate.

## Prepared reference and injected worker

`autotune_fma_reference::reference::PreparedReference` prepares exactly one
validated recipe's FP32 a/b arrays, integer x bases and integer expected bases.
`fill_x` adds the validated dispatch tag; `check` compares every lane without
allocating, regenerating SHA inputs or mutating output. The original oracle API
is preserved. There is no cache of all dispatch outputs. See the frozen numeric
and exactness contract in [fma-reference.md](fma-reference.md).

The worker binds a nonzero run ID, UUID plus PCI domain/bus/device, validated
recipe, 1..4096 dispatches and one immutable absolute deadline. Full discovery
UUID/PCI parsing preserves raw strings and the discovery PCI function separately;
CUDA never claims to query that function. PCI function 0..7 is accepted and
retained as discovery metadata only; it is not part of CUDA identity matching.
The injected backend has one submit,
bounded query progression and one exact-size readback buffer. Every dispatch
begins with a nonfinite sentinel; every returned lane is checked before another
dispatch. Inputs and checked outputs have SHA256 digests over their exact
little-endian FP32 bits. These are internal fixture records, **not** the proposed
canonical live-session JSON/digest schema or consent evidence.

An injected monotonic clock is observed before admission, after backend returns
and after checking/digests. Deadline equality wins. Stream completion must be
observed before 100 ms from cycle admission (conservatively including upload),
and checking/readback before 250 ms, or an earlier absolute request deadline.
Repeated stop cannot renew a deadline. Stop intent is distinct from completion;
an already submitted operation can finish/check, but no successor is admitted.
Fault, identity change, clock regression, length error, mismatch, nonfinite or
stale output freezes the result. Raw backend faults remain retained even when a
deadline wins. Reports expose immutable slices, at most 4096 scalar check/digest
records, and at most one full failing vector (<=1 MiB). Successful vectors are
reused, not archived; digests do not reconstruct actual successful output bytes.
No report proves native/process exit or GPU quiescence.

The enforcement values 100 ms and 250 ms are read from
`Profile::approved_design().limits` (`dispatch_completion_ns`, `cycle_ns`); the
worker keeps no private copy of the frozen design.

## Caller API

A caller builds a `Request` (nonzero run ID, matching `Identity`, validated
`Recipe`, 1..4096 dispatches, `start_ns` and absolute `deadline_ns`) and calls
`Worker::new(backend, request)`. `Worker::new` returns
`Outcome::LiveAdmissionUnavailable` for non-fixture provenance and
`Outcome::IdentityFault` for an invalid request or a backend identity mismatch;
neither case calls the backend workload. The caller then polls
`advance(&mut clock, stop)` until `Report::outcome()` is `Some`:

- `CheckedFixture` — every requested dispatch was checked against the reference.
- `Stopped` — stop was requested; at most the already-submitted dispatch finished.
- `DeadlineUnknown` — the absolute deadline or a cycle cap was reached; the
  result is inconclusive, not a failure verdict.
- `ClockFault` — the injected clock moved backwards.
- `BackendFault(Fault)` — the backend returned an error; `raw_fault()` retains it.
- `IdentityFault` — the device identity changed after admission.
- `Incorrect` / `LengthFault` — the checked output mismatched or had the wrong length.

`Report::records()` returns the completed dispatches; `last_check()`,
`readback_length()`, `failure_output()`, `stop_ns()` and `provenance()` expose
the retained evidence. `Report` is immutable after each call, and every outcome
is sticky: later `advance` calls do not replace it. The injected clock is a
caller-supplied monotonic source; the worker never reads the OS clock.

## Injected monitor policy (E3.1/E3.2)

`monitor::{clock,schedule,controller}` is portable pure policy. It reads
no OS clock and calls no getter, process or backend. It always selects unchanged
`Profile::approved_design().limits`; there are no policy overrides.

The separate Windows-only `monitor::windows_clock` adapter reads real OS clocks;
it does not change this policy or enable native admission.

### Windows clock capture — 2026-10-28

`WindowsClock::new(nonzero_token)` captures the exact positive QPC frequency.
`capture()` returns an `Anchor` in this order: QPC-before, precise interrupt
time, precise unbiased interrupt time, QPC-after. The caller copies
`clock_token()` and `qpc_frequency()` into the existing `Binding`. Failed or
negative counters, invalid frequency, zero token and reversed brackets return
typed errors; no anchor is fabricated. The two interrupt APIs have no status
return. Raw 100 ns units are not an accuracy claim. Existing controller policy
still assesses uncertainty, suspend, disagreement and regression.

On Windows 10+ this uses the pinned `windows-sys` declarations, with no new
package/version or timer-resolution change. No GPU API, sampler getter, process
launch, live provenance or workload admission is added.

Parent source-bound verification on Windows GNU / installed Rust 1.99.1:

```text
cargo +stable test -p autotune-benchmark-worker --lib monitor::windows_clock --locked --offline
cargo +stable test -p autotune-benchmark-worker --test windows_clock --locked --offline -- --exact real_windows_clock_capture --nocapture
cargo +stable test -p autotune-benchmark-worker --test monitor --test evidence --test envelope --locked --offline
cargo +stable clippy -p autotune-benchmark-worker --lib --test windows_clock --test monitor --test evidence --test envelope --locked --offline -- -D warnings
cargo +stable fmt -p autotune-benchmark-worker -- --check
```

All commands passed (`mon_X7VCKCE3MBF6ZDE0`, `CLOCK_EXIT=0`): six deterministic
adapter tests, one real Windows capture, 36 monitor / 13 evidence / 26 envelope
tests. The library filter excludes its unrelated worker-protocol test; no tests
were ignored. Actual QPC frequency was 10000000; observed QPC brackets were
4364107628410..4364107628469 and 4364107628471..4364107628471. These prove
read-only capture and raw mapping, not calibrated accuracy or qualified timing.

Baseline: `c9bd2245d026cd8628c073bb86a11054b30ab937` plus this uncommitted adapter; source SHA-256:

| File | SHA-256 |
| --- | --- |
| `src/monitor/windows_clock.rs` | `a69f8f0b0c075baf09c24a50076df3cd00757d2f4dfbe36f4638dad7ce83939f` |
| `src/monitor/mod.rs` | `a017571d497b00d144a600f5e6dedef6ef3bcd8de425bc45183b42ee70f1b72a` |
| `tests/windows_clock.rs` | `b808b9a09aae2c42df09c9afa8a29c9f14e9815e3314e92ec23e11bfcefe5c51` |
| `Cargo.toml` | `c64a62a9aceddea9a4840cf0c16381d8e8def2f40d2c6c770de1a396526a114b` |

Independent static review `st_01a183cb` approved the snapshot with no findings;
it did not execute checks. No Miri, non-Windows execution, hosted
verification, GPU workload or physical timing qualification is claimed.

Caller flow:

1. Build `Binding` with nonzero session/run/clock token, worker `Identity` and
   positive QPC frequency. Supply an origin `Anchor` to
   `Controller::new(binding, origin, Session::{Pilot,Acceptance})`.
2. Supply a current anchor on every `begin`, `complete`, `poll` or `request_stop`.
   Ticket/record times are nanoseconds relative to the origin's interrupt time,
   **not absolute OS ticks**. The session budget starts at origin.
3. `begin` returns at most one `Ticket`. Inject its matching `Record` into
   `complete`: exact binding/metric/slot/scheduled/start, query-end, receipt,
   typed unit/status and optional integer value. No getter runs here.
4. Independently call `poll` while a getter is unresolved, including on the
   approved 10 ms control schedule. A blocked getter must not block control.
   Inspect immutable `report()`; the library does not drive a timer.

Each raw anchor retains QPC-before/after and interrupt/unbiased 100 ns readings.
Checked u128 conversions retain ticks/frequency, floor elapsed lower bounds and
ceil uncertainty. Brackets above 1 ms, epoch changes, regression, overflow,
interrupt/QPC disagreement above 5 ms or interrupt/unbiased divergence above
5 ms reject stickily. Both consecutive and origin-relative intervals are checked;
equality at the maxima passes. Sleep-including time is interrupt elapsed.
Rejected anchors never become trusted time. Clock-fault stops use the last
validated time; both deadline-reached helpers conservatively return true while
current time is invalid, without changing the original numeric stop/deadlines.

Six absolute 250 ms schedules use temperature, power, graphics clock, memory
clock, GPU utilization, memory utilization tie order. Only the latest due slot
is queried; older unstarted slots become inclusive missed ranges. Polling
records misses without replacing the outstanding ticket. Only exactly bound,
temporally ordered returns with a valid current clock resolve call uncertainty.
Query bracket <=50 ms and transport <=100 ms pass equality. Wrong units,
missing available values, negative physical values, utilization outside 0..100,
and values accompanying unavailable statuses reject. Non-safety unavailable
status **without a value** records separate baseline insufficiency. These flags
describe rejected returns only: false does not prove coverage. Consult missed
ranges, pending state and raw records; there is no baseline verdict.

`fixture_compute_eligible()` requires both fresh matching safety observations
and no stop/clock fault. Initially false, it stops at temperature >=75 degC,
power >=200000 mW, or either query-end age >=750 ms. Initial absence also stops
at 750 ms. Receipt never refreshes age; host query-end is not known sensor age.
Whole-session expiry (180/600 s) wins at equality. First stop fixes cooperative
and exit deadlines (+1/+3 s, capped by session expiry); late returns and repeated
stops cannot renew deadlines or reopen eligibility.

Evidence uses **three pre-reserved vectors**, never a full Record-sized union
per poll: compact control observations, raw samples paired with their begin
observations, and sparse missed ranges keyed by original sequence. One unresolved
begin is retained independently, including across malformed returns. The
allocation-free `report().events()` iterator reconstructs one event at a time in
original operation order (including raw rejected records, anchors and skips).
`terminal_evidence()` separately retains the first overflowing operation; no
eviction or unbounded late log occurs. Current state may still observe late
returns, but the first stop and terminal evidence remain unchanged.

Control events have 65535 ordinary slots and samples 32767, reserving one of each
profile ceiling for terminal evidence; paired begins do not consume a second
control slot. Sparse storage holds at most 65535 individual missed ranges; a
batch that cannot fit is retained whole in terminal evidence. All three actual
vector capacities plus inline controller/terminal state must fit the 20 MiB
ordinary evidence budget, leaving the 8 MiB writer and 4 MiB failure reserves
untouched. Allocation is fallible (`InitError::Capacity`); vectors never grow.
Separately, first use initializes a `OnceLock` via `Profile::approved_design()`,
which allocates its temporary ID `String` once; that standard allocation can
abort on OOM and is not covered by `InitError::Capacity`. `capacity()` exposes
retained counts and allocated policy bytes, **not total RSS, allocator overhead,
initialization peak, disk or host-process bounds**. The full 600-second fixture
retains 60002 controls and 14400 paired samples without a capacity stop.

This is fixture eligibility only, not thermal re-entry/utilization qualification,
physical GPU mapping, exact execution consent, live export, baseline/energy/search
assessment or termination/GPU-quiescence proof. Existing `Worker` native rejection
is unchanged; no monitor result can enable it.

## Selective-getter/monitor bridge — 2026-10-29

`sampler` is a portable pure conversion module, not a sampler process. It uses
the existing probe library through a local path dependency; no external package
or version changed. It never calls `NvmlSampler::open`, a getter, a clock or a
process API and cannot enable native worker admission.

- `sample_metric(Metric) -> SampleMetric` selects the identically named one of
  six existing getter kinds without invoking it.
- `map_field(Ticket, end_ns, receipt_ns, Field) -> MappedSample` returns the
  original owned `raw: Field` beside `record: Result<Record, MappingError>`.
  Both conversion success and failure preserve all original unit/symbol/value
  or reason/vendor-status/detail evidence. No failed conversion is a missing
  sample that callers may silently discard.
- `MappingError::{Unit, Symbol, Value, DeviceLost}` identifies the conversion
  failure. GPU loss by reason or vendor status 15 takes precedence over every
  other conversion error, including for optional memory utilization.

| Input | Conversion |
| --- | --- |
| Available, present symbol, exact unit, exact JSON integer fitting `i64` | `Status::Available`, `Some(value)`; no float/string coercion |
| Unsupported / symbol missing / unsupported platform with consistent symbol evidence | `Status::Unsupported`, no value; raw reason/code retained |
| Other unavailable reading with vendor status | `Status::Failed(original_code)`, no value |
| Other unavailable reading without vendor status | `Status::Unavailable`, no value |
| Wrong unit, contradictory symbol evidence, noninteger/unrepresentable value, device loss | Conversion error with unchanged raw evidence |

Exact source units are `degC`, `mW`, `MHz`, `%`. Missing-symbol evidence requires
`Absent`; runtime missing/load failed/unsupported-platform requires `NotChecked`;
other unavailable reasons require `Present`. Available readings require
`Present`. Physical negative/range checks stay in the existing controller.

The caller must retain the `MappedSample` and match `record`: deliver `Ok(record)`
to `Controller::complete` with the caller's receipt/processing anchor, or close
admission on `Err` with `Controller::request_stop` while retaining the raw error.
`StopReason::Requested` records that action; the exact mapping cause remains in
the retained `MappedSample`, not a fabricated controller reason. This module
does not store those results or implement the caller's supervision loop.

Ticket binding/start, query-end and receipt numbers are copied unchanged. Their
truth/order is not established by conversion. `Ticket.start_ns` remains the
controller issue time and a conservative interval start, not an actual getter
start; future transport must separately retain actual getter anchors. The
controller still owns ticket, chronology, query-bracket/transport limits,
physical ranges, missing metrics, safety freshness and sticky stops.

Local Windows GNU / installed Rust 1.99.1, locked offline checks:

```text
cargo +stable test -p autotune-benchmark-worker --test sampler --test monitor --test evidence --test envelope --locked --offline
cargo +stable test -p autotune-probe --lib --locked --offline
cargo +stable clippy -p autotune-benchmark-worker --lib --test sampler --test monitor --test evidence --test envelope --locked --offline -- -D warnings
cargo +stable fmt --all -- --check
```

The first mapping run reproduced the missing sampler API (Cargo exit 101) before
implementation; all seven mapping tests then passed. The four controller
composition tests passed against unchanged controller policy, for 11 sampler
tests total. The listed checks pass: 11 sampler + 36 monitor + 13 evidence + 26
envelope + 18 probe-library = 104 selected CPU-only tests, none ignored or
filtered; scoped warning-denied Clippy and workspace formatting pass. Final
source binding/review disposition is recorded in the roadmap handoff and
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-sampler-bridge-20261029-7fb6fa3a\manifest.json`.
Baseline is `eb8cdf51bdb4c55b6a272fe7b9004d99b26d6a2b` plus the uncommitted bridge
and preceding documented cleanup. This is not a fresh workspace test/Clippy,
hosted, native-feature, real getter/clock/process/GPU or hardware qualification
result. No live producer, baseline, energy, recovery or gate acceptance follows.

Next required integration is the independently killable timed sampler's bounded
transport/containment and nonblocking supervision, then exact manifest/worker/
clock/device/consent binding and live evidence. Do not count this converter as
the independent sampler itself.

## Compile-only opt-in CUDA adapter

The `native-cuda` feature builds a Windows x64, thread-affine adapter. Nothing
loads automatically. `CudaBackend::open` is an explicitly unsafe programmatic
native-entry boundary requiring separate execution authority. The portable worker
rejects its `Native` provenance before any identity or workload call.

The originally authored minimal declarations use the CUDA 12.9.1 Driver API
documentation and an independent signature/constant cross-check against cudarc
0.17.3, commit `fdf313cc10cf5504953ac9fe6a5dc78dc6ec4152`:
[declarations](https://github.com/chelsea0x3b/cudarc/blob/fdf313cc10cf5504953ac9fe6a5dc78dc6ec4152/src/driver/sys/mod.rs),
[archived Driver API](https://docs.nvidia.com/cuda/archive/12.9.1/cuda-driver-api/index.html),
[PTX 6.4](https://docs.nvidia.com/cuda/archive/10.1/parallel-thread-execution/index.html).
No upstream declaration block, loader, header, sample kernel or Toolkit is copied
or bundled. This is not public-distribution license clearance (E0.1/E8.4 remain open).

`cuda::EXPORTS` is the exact 21-symbol typed allowlist. Windows x64 system ABI,
32-bit raw statuses/devices, 64-bit device pointers/size_t, opaque handles and
16-byte C-layout UUID are pinned by compile/typed-fake tests. Constants are
success 0, not-ready 600, context flags 0, nonblocking stream 1; PCI
bus/device/domain attributes 33/34/50 and compute-capability major/minor 75/76.
Unknown raw statuses are preserved; missing exports fail closed before
initialization. Internal validation statuses `-1` (missing or mistyped export)
and `-2` (rejected pre-call validation) are sentinels, never CUDA statuses; a
real driver status is never rewritten.

The loader uses `GetSystemDirectoryW()/nvcuda.dll` with DLL-load-dir/system32-only
flags, verifies the loaded module's canonical path and retains path/API version.
No environment, CWD, PATH or user DLL/kernel choice exists. Mapping requires a
unique full UUID plus independently returned PCI fields, capability 7.5 and the
selected context device. Own buffers consume 16 bytes/lane (P3 exactly 4 MiB),
bounded by 16 MiB and a 512 MiB free-memory remainder; opaque driver allocations
are not covered by this counter. The only module is embedded originally authored
`fma.ptx`, fixed entry `autotune_fma`, PTX 6.4/sm_75, explicit sequential
`fma.rn.f32` loop and full stores. It is not a proved useful or GPU-qualified workload.

Pageable uploads are followed by an observed default-stream completion check
before launching on the nonblocking stream; a pending upload fails closed rather
than racing DMA. Query pending permits polling only for that outstanding launch.
Readback requires observed completion. Errors poison the adapter; later queries
cannot clear uncertainty. Explicit close requires known idle and checks every
stream/free/module/context teardown result. **Drop performs no native cleanup**:
abandoned or uncertain resources and the DLL are intentionally retained until
process exit, not represented as released. Any native call can block; independent
monitor/clock/consent/supervisor integration remains required before live admission.
No reset, tuning setter, NVML, NVRTC/runtime, arbitrary module or native CLI exists.

## Fixed owned Windows fake processes

`autotune-benchmark-fake-worker.exe` is a Windows-subsystem, quiet **fake-only**
binary, even when compiled with `native-cuda`. It can never select/load/call the
CUDA adapter or NVML. The launcher chooses only this fixed sibling executable,
never a caller-supplied executable/path. The fixture protocol has fixed 40-byte
frames with version 1, nonzero 16-byte run, strict sequence, fixed vocabulary and
reserved-zero validation; malformed/truncated/unknown/replayed frames terminalize
the channel. Ordinary status capacity is 64 frames; reads are peeked/nonblocking
and control is a separately inherited manual-reset stop event. No production
consent or hostile-same-account authentication is claimed.

Each worker launches suspended with an explicit inherited event/status handle
allowlist and empty child environment, is assigned before resume to a private,
noninherited, no-breakaway, kill-on-close job, and has applied limits read back:
one active process, 1 GiB process/job commit and 45 s user CPU. Unsupported setup
rejects without a fallback. Tests cover normal, cooperative, forced, done-but-
running, EOF-but-running, prohibited child and unrelated inheritable-handle cases.
The handle test inherits an explicit reference event duplicate and uses
`CompareObjectHandles` to distinguish a leaked excluded duplicate from numeric
handle-slot reuse during startup. The prohibited-child test requires the actual
`ERROR_NOT_ENOUGH_QUOTA` failure, not just any process-creation error.
Controller observations retain direct process-handle wait status/exit code and
independently query `ActiveProcesses==0`; neither messages, EOF nor termination
request alone is exit proof. No job-object signaled-state shortcut or PID-reopen
exit proof is used. Wait/observation helpers are capped at 3 s, read from
`Profile::approved_design().limits.exit_observation_ns`; the job's one-process,
commit and user-CPU limits are read from the same approved profile rather than
duplicated as local literals.

The controller-loss fixture runs a newly owned quiet controller outside its child's
job and duplicates only the original child process handle into the observer.
Killing that controller tests actual last-job-handle loss. The child exit is observed,
but job accounting is **explicitly unavailable**, not passed: retaining another job
handle would defeat the last-owner-loss condition. No surviving controller means
no successful session outcome. Process exit/empty jobs are never a GPU fence.

## Verification and execution boundary

Portable CI runs prepared-reference and injected-worker fixtures. Windows CI also
compiles/lints the opt-in feature and runs **only typed ABI fakes**; it does not
execute the ignored Windows process tests. No test invokes `CudaBackend::open`,
loads a native GPU DLL, probes a GPU or executes PTX on hardware.

Under the separately approved owned-process test scope only:

```text
cargo test -p autotune-benchmark-worker --test windows_fake --locked --offline -- --ignored --test-threads=1
```

The five owned-process cases require `--test-threads=1`: each owns a real job
object, handle table and stop event, so parallel cases would interleave
process/job observation and invalidate the containment evidence. The flag
changes test scheduling only, not any fixture limit.

Current source-bound local evidence and historical failures are in [ROADMAP.md](../ROADMAP.md).
No hosted, actual-commit, hardware, baseline, energy, efficiency, tuning or recovery
qualification follows. The next live step requires independent approved sampler,
clock and exact-consent integration; there is no launchable native worker here.

# Independent sampler integration — 2026-10-29

The [independent sampler design](superpowers/specs/2026-10-29-e3-independent-sampler-design.md)
and [implementation plan/log](superpowers/plans/2026-10-29-e3-independent-sampler.md)
now have an in-progress source implementation under `sampler/`. This extends the
previous delivered pure converter; it is not a completed live capture chain.

Typed canonical channels bind session/run/manifest/device/clock/frequency and
per-lane sequences. The injected supervisor keeps one getter pending, preserves
actual versus issued/received anchors, closes admission on faults and separates
termination requests from retained process-handle/job observations. Shared RAM
reservations and nested control partitions retain their parent charge until the
last frame/worker owner drops. Immutable sampler-only manifests retain exact
selected identity and artifact/approval assertions. A fresh Windows-only probe
description reuses existing runtime getters without re-enumeration or control
reads. Production/fake sampler binaries and Windows containment/I/O source
compile; no such child has been executed in this continuation.

The derived envelope remains blocked/invalid, with compute/correctness/baseline
unqualified, worker evidence unavailable and native completion unknown. It keeps
simulated fixtures separate from live provenance; its API cannot authenticate an
injected provenance argument or human consent. Raw export retains duplicate or
late incoming records independently of the unique metric-slot envelope view.

The source now includes a fixed controller, asynchronous measured startup,
independent shutdown watchdog and controller CPU/commit observations, an off-path
writer for the fixed artifact leaves and an exact-byte index excluding its own
hash. Decoded-message reservations survive through supervisor retention; child
cleanup messages preserve attempted/unknown semantics and never prove OS exit.
A full injected 10 s sampler-only capture/export passes within unchanged resource
budgets. The first full capture failed due to an overly broad raw-output reserve;
actual incremental buffer growth now charges both old/new allocations during
reallocation before releasing the old charge.

The current unchanged-source checkpoint passes 190 explicit CPU-only tests,
affected-package all-feature Clippy, workspace formatting and full-workspace
all-feature linking; the roadmap binds exact source/log hashes. Rejected/truncated
frame retention, draining queued observations, per-lane I/O completion evidence,
exact envelope serialization totals, launch/allocation audit, fuller fixture wire
coverage and final independent review remain open. No production executable or
process fixture was run. Real capture, Windows containment, telemetry, baseline
and Gate B remain unqualified; execution needs its exact separate approval.

The later October 29 audit checkpoint passes 214 selected CPU-only tests,
affected-package all-feature Clippy, workspace formatting and all-workspace
all-feature linking; exact source/log/artifact hashes are retained in the
roadmap's current handoff. Rejected/partial transport bytes, per-lane cancellation
observations, bounded supplemental draining and unknown invalid-timing
projections now have source/test coverage. Framing or codec-capacity rejection
poisons the decoder before any successor can be accepted. Final export requires
exact manifest/profile/raw byte accounting before `BoundEnvelope::encode`;
session/index bytes remain separately indexed without self-reference.

The shared child wire engine has three additional ignored Windows cases for
cooperative six-metric capture, blocked getter/control responsiveness and wrong
Ready identity. All five process cases were linked only. The corrected aggregate
metadata fixture proves the earlier shared-RAM limit, not the separate 4 MiB
metadata boundary. Full cleanup/allocation audit and independent whole-change
review remain open; no completed sampler milestone or live qualification follows.

## Independent sampler implementation milestone closeout

The later October 29 closeout supersedes the draft implementation status above.
The integrated source is complete for the software-only slice and independently
reviewed; **217 CPU-only tests**, scoped all-feature Clippy, workspace formatting
and all-workspace/all-feature compile/link pass. The roadmap retains the exact
source/log/artifact manifest. This closes the implementation slice, not E3 or
Gate B, Windows process acceptance, native sampling or a useful GPU baseline.

Review found and resolved failed-launch handle/cleanup loss and storage-failure
evidence loss. Failed launches retain their original process/job handles, job
assignment state, optional creation identity, original error, termination result
and per-lane I/O shutdown observations under the original deadlines. Unassigned
children are terminated through the original process handle. None of these
facts can establish successful launch or qualified exit.

Writer failures retain operation, signed raw OS code and error kind. Every
initial/final artifact send or finalization failure reaches an owned,
manifest-bound `writer::IncompleteCapture` error containing cancellation result,
independently observed writer completion and optional bounded raw events. Its
diagnostic reservation precedes optional raw export; unavailable raw/unfinished
threads stay explicit. The fixed CLI reports this incomplete diagnostic. A
failed or blocked sink cannot promise durable failure output, and a timeout
does not authorize a new writer or an extended deadline.

The separate metadata ceiling now has coverage through the actual shared
ingress/retention seam using owned reserved failure snapshots. The unused
`wire::Queues` model and its separate test were removed; they were never the
Windows transport. Five sampler process fixtures remain compiled only.
Source-level review of Win32 ownership is not executed OS evidence. Next is an
exact source/artifact-bound execution matrix, separate consent and continued
worker/device/manifest integration; there is still no native benchmark admission.

Publication `74eecb1` was observed in hosted run 40947677375: Linux passed;
Windows exposed a writer-test synchronization race. A deterministic gated-sink
regression reproduced it locally; terminal writer failure now disconnects its
command receiver before publishing error or dropping the sink. Local 217-test
verification and static/link checks pass after correction; hosted acceptance
remains pending for the corrective revision. The failed run remains evidence.

Corrective source `ef8786e` subsequently passed both Linux and Windows/MSVC in
[run 40948641366](https://github.com/sockRa/AutoTune/actions/runs/40948641366).
This observes configured hosted deterministic/compile checks only. The next
[fake-only Windows execution request](superpowers/specs/2026-10-29-e3-sampler-fixture-execution.md)
pins two local images and its verification-first runner in an exact matrix;
separate owner approval remains required. Its byte-pump tests use only memory
streams. No sampler process/native/GPU execution or qualified baseline follows.

## Pilot workload declaration and run binding

`workload::manifest` supplies an immutable `PilotManifest` for the three existing
approved pilot stages. `PilotDraft` selects nonzero unique run IDs, a session and
clock token, exact raw NVML identity, source/artifact hashes, approval/output
scope and hashes of starting-settings/environment artifacts. Those hashes bind
declarations; this API does not authenticate consent, read settings, validate
required facts or admit execution. Acceptance repeats are outside this scope.

`PilotManifest::freeze` reserves bounded storage before model construction.
`parse` reserves codec capacity, uses the existing strict canonical reader and
requires exact re-encoding against the frozen three-stage plans/profile. Seeds,
shapes, counts or limits cannot be silently substituted. `profile_canonical`
retains the exact scoped policy bytes whose profile digest the manifest binds.
The shared numeric-limit encoder preserves the readonly profile's prior digest
`d9ef3a0a5f1a138740a3ec657aac4d3b80515a910394dd8cf80aab2ead4bc6d9`.

`bind_run` borrows the manifest, preserves the selected run/device/artifacts and
original session-relative origin, and reserves all remaining stage envelopes
plus finalization/shutdown against the pilot ceiling. `BoundRun::phase_deadline`
is the existing pure RunPlan calculation: its caller must retain the actual
first phase entry. It neither advances lifecycle nor freezes caller-supplied
entries on repeated calls. Binding is not a readiness, safety or Start permit.

## Worker handshake channel and child gate

`workload::wire` adds bounded canonical worker-role frames for Bootstrap, Ready,
Start, Stop, StopAck and RoleFinished. Each lane/direction has its own sequence;
role, complete manifest/run/device/artifact/clock binding and frozen plan must
match. A rejected frame poisons its channel. Owned frames and decoded messages
retain RAM reservations until dropped; callers retain borrowed rejected bytes
and must poison the channel after external transport/framing failures.

`workload::gate` accepts one Start intent after Bootstrap and Ready. Setup has a
frozen deadline from the first Bootstrap receipt, capped at the original run
cumulative boundary. Ready and Start receipt must precede it. Warm-up uses the
controller's issue time conservatively, including IPC delay. Future Stop
triggers fail; late receipts retain already-expired original stop ceilings.
Repeated terminal messages cannot renew a deadline or erase the first fault.

Ready fields are self-reports. RoleFinished is a cleanup observation, never OS
exit or a GPU fence. The gate calls no backend and grants no native authority.
Parent safety/re-entry/required-fact/consent validation, DispatchRecord and
failure-vector transport, actual Windows pipe integration and independent
process supervision remain required. Existing simulated worker native rejection
and readonly sampler bytes remain unchanged. CPU/static evidence does not
establish process execution, GPU identity/correctness or a useful baseline.

## Dispatch records and first-failure vector transport

`workload::wire::Message` now carries `DispatchRecord` and `FailureChunk` on the
ordinary worker-to-controller lane. `workload::dispatch::Record` binds j,
warm-up/measured role, seed, shape/K, raw per-operation before/after anchors and
signed statuses, pending/readback/check counts, first mismatch diagnostics,
reported verified FMA count and optional input/output SHA-256. Upload, sentinel,
launch and query calls retain distinct operation kinds. Unknown anchors,
unreturned statuses and missing digests remain null. Raw clock-order conflicts
remain available for parent adjudication; decoding is not acceptance.

The input hash convention is plain SHA-256 of little-endian FP32 arrays A, B, X
in that order; output is plain SHA-256 of every little-endian FP32 output word.
These match the existing worker's buffer-digest convention and are distinct from
domain-separated manifest/profile/session/raw-artifact digests. The transport
checks the declared shape/count/work relationships; native call completeness,
clock validity, sequence j across records, phase coverage and correctness are
separate parent obligations. A child-reported `verified_fma` is no native proof.

Successful vectors are not archived after full checking and hashing. The first
full failing vector is split into at most 2048 words per frame. `FailureVector`
reserves the entire bounded vector before allocating, requires contiguous exact
dispatch/shape chunks and verifies the complete output SHA-256. Gap, duplicate,
foreign chunk, hash mismatch or explicit poison retains received words and
prevents completion. A complete matching hash binds bytes, not oracle correctness
or execution. Session composition must ensure this is the first failure, retain
the original record and stop; it must not instantiate replacement accumulators
to recover from loss. Missing/partial readback remains explicit raw evidence.

Each record holds at most 64 raw calls; each frame remains at most 64 KiB.
Exhausting a representation bound stops with caller-owned trace preserved,
never silently merges or drops calls. The worker codec's structural bound is
4096 JSON structures inside its reserved codec RAM; sampler framing retains its
256-structure bound and unchanged bytes. The existing global evidence/event,
queue, failure snapshot and disk ceilings still apply in session composition.
This supplies actual encoded result transport, with CPU coverage through P3's
complete 1 MiB failure vector. Native production, actual pipes, parent admission,
OS supervision and live baseline qualification remain unfinished.

## Parent dispatch-report assessment

`workload::assessment::Audit` borrows retained worker messages and controller
receipts; it allocates only fixed, reserved metadata. Its caller must retain
every raw message, receipt and rejection before assessment. It checks contiguous
j, the bound phase/shape/count/work assertions, no pending operation, full
readback/check assertions and distinct native upload/sentinel/launch/query stages
followed by readback/check/hash. Repeated CUDA NOT_READY query observations may
precede one returned completion. Aggregate Submit alone is insufficient for this
fixed native call trace. Input/check/hash are host stages with no native status.

Source anchors, receive observations and independent controller ticks each have
their own validator against the same original epoch. Buffered worker timestamps
may precede a previous controller receipt, and a queued receipt may precede a
newer tick. Those facts do not fabricate a clock regression. Raw call brackets
must still be ordered, epoch-valid and no later than their actual receipt.
Missing, conflicting, suspended or regressed clocks close assessment.

Warm-up retains the conservative original Start-issue deadline. A new bound
`PhaseEntered` message reports actual measurement entry before measured work.
After all warm-up records, measurement admission requires that entry and freezes
its existing relative cap against the immutable cumulative boundary. Before the
entry is observed only the cumulative deadline is known; the native producer
must send entry before work and stop on uncertain sends. Controller supervision
cannot wait for the producer or report auditor. Newer tick time prevents an old
entry/receipt from retroactively extending the relative deadline.

Stream submit-to-completion starts at the first upload, so upload/sentinel time
counts toward the existing 100 ms ceiling. The complete input-through-hash cycle
is capped at 250 ms; phase ceilings may expire earlier. Equality loses. Explicit
`tick` calls assess silent-worker deadlines independently of incoming records.
Fault/stop closes further assessment and preserves the first fault and counts.

`reports_complete()` states only that all 128+256 pilot reports were counted; it
does not erase a later fault. Callers inspect `fault()` separately. Matching
reports/digests do not replay successful output vectors or prove native execution,
physical identity, safety, consent, telemetry coverage, orderly teardown, exits
or baseline usefulness. The actual parent safety/re-entry/required-fact/consent
path and Windows/controller composition remain required; producer source is
described below.

## Actual CUDA call tracing

`CudaBackend::submit_observed`, `query_observed` and `readback_observed` instrument
the actual typed adapter calls. Existing Backend methods delegate with a no-op
observer; the simulated worker still rejects Native. Each upload, default-stream
upload-completion query, launch, kernel query and readback has a pre-call and raw
post-return callback. Callback failure keeps the adapter poisoned; no successor
native call or speculative cleanup follows. Upload completion must return zero
before launch. Its NOT_READY status is a failed submit, distinct from a pending
kernel-stream query. Parent assessment now requires this previously omitted
upload-completion stage explicitly.

`workload::trace::CallTrace` reserves its 64-call storage before allocation and
captures injected raw anchors/control state around these calls. It retains raw
return values before attempting the post-call clock. A native error plus clock
failure retains the original native error. Missing clocks, deadline equality,
stop or capacity exhaustion prevent further observed entry. Pending kernel query
does not renew the interval that began at first upload. Returned completion ends
that 100 ms interval; readback/check/hash still count toward the 250 ms cycle and
original phase ceiling. Input/check/hash host stages must be supplied explicitly.

The collector pins the original binding, phase and invocation. `bind` rejects a
different index/phase/shape/pending assertion and preserves both owners on error.
An accepted `TracedRecord` retains its RAM charge and exposes a borrowed record;
`Channel::encode_traced` enforces the exact original execution binding. A foreign
channel is poisoned without consuming the trace. The first fault and an unmatched
return remain owned supplemental observations; the future exporter must retain
them separately from the matched-call frame.

The producer below preserves a continuous clock validator across traces, uses
one frozen phase entry and binds host checking/hashing/output evidence. The
tracing seam is no archive or admission authority. Typed ABI fakes exercise
actual adapter paths without loading a DLL; portable tests cover the owned
trace-to-frame/report-audit path. Native admission and Windows composition remain
unfinished. No new binary/default native mode is added by tracing.

## Bound worker producer

`workload::producer::Producer` now composes prepared input/reference bases, fixed
x/output/sentinel buffers, actual observed Driver methods, full checking/hash
and the bound worker channel. CudaBackend implements Driver by delegating to its
observed methods; its existing native setup contract still applies. Metadata
accessors perform no driver I/O. `prepare` performs CPU preparation and reserves
owned host storage before allocation; it does not open a context or load a DLL.

`Gate::take_start` transfers a non-cloneable, single-use observation after the
validated Bootstrap/Ready/Start handshake. Fixture start rejects Native. Native
start additionally requires NativeAdmission, whose fields are private and which
currently has no issuer. The actual parent required-fact/safety/re-entry/exact
consent and supervision path must exist before an issuer can be implemented.
Existing simulated worker native rejection and executable modes are unchanged.

Each advance submits one dispatch or performs one query/completion/readback/
check/hash sequence, with no hidden successor dispatch. All fixed 128+256 counts
and seed/shape/K remain bound. A shared clock validator survives across traces;
measured PhaseEntered is encoded before measured work and freezes that phase's
original relative/cumulative ceiling. Completed warm-up does not keep applying
its old cap to measurement. All output lanes are checked, including warm-up;
hashes use the existing exact little-endian buffer convention. Successful output
can be reused only after complete checking/hash/report handoff.

Control supplies the original StopWindow. Child receipt has its own anchor;
delayed controls cannot renew cooperative/exit grace. New work closes, while an
already submitted dispatch can drain query/readback/check/hash under the earlier
original 100 ms stream, 250 ms cycle and phase bounds. Original cooperative
expiry prevents further drain calls or cleanup. An independent controller must
still supervise blocked driver, clock or sink calls and original OS handles.

FrameSink must be an immediate bounded queue handoff: success is owned enqueue,
not peer receipt/durability. Failure returns the exact frame; producer retains it,
poisons its lane and issues no retry or successor. Raw trace/summary, invalid
clock/control, readback length and first terminal outcome remain inspectable.
The first full bad output moves into the separately reserved 4 MiB failure pool,
transferring its existing charge and preserving exact bits/digest or its absence.
One export step emits one bounded chunk within original exit/run ceilings.
Capacity/encode/clock/send failures persist; freeing capacity cannot resume export.
Handed-off words are separate from complete-export status, including a late guard
after the last accepted chunk. Missing mandatory export prevents qualification.

Explicit cleanup is one shot after admitted terminal known-idle work. Attempt is
stored before driver entry and raw return before post-clock capture. No attempted
cleanup, return status or complete report establishes OS exit, empty job, released
VRAM or global native quiescence. Owned host accounting excludes opaque driver/
OS/allocator overhead and does not replace configured commit limits. Exporters
must retain all raw fault/control/clock/cleanup observations as well as frames.

CPU fixtures cover a complete encoded P1 plan through parent audit, stop-drain,
stale/partial outputs, native-status fakes, original timeouts, first P3 failure,
uncertain frames and sticky export limits. Typed ABI fakes cover the actual Driver
bridge; no native DLL, process or GPU workload ran. Parent admission, Windows
worker/supervisor and live artifact/session composition remain unfinished. This
is a software producer milestone, not E3/Gate B or baseline acceptance.

## Per-run current-setting facts

`NvmlSampler::setting` reads one requested current fact on its uniquely selected
handle: power limit in mW, modern graphics P0 offset in MHz or modern memory P0
offset in MHz. It uses the existing getter ABI/validation, with no legacy fallback,
setter lookup/invocation or inferred write qualification. `identity()` borrows
the selection identity without re-enumeration, runtime or setting queries. The
existing six telemetry getters and readonly capture formats are unchanged.

`workload::settings::Capture::collect_native` requires exclusive sampler access
and a separately approved native-read/containment/clock scope. It checks exact
declared raw identity, then reads each fact once with original before/after clock
anchors. Stop, invalid clocks, cumulative setup expiry or GPU loss close collection
without a successor; missing/unentered facts remain null. Raw status is retained
before post-clock processing, including GPU loss plus missing post-clock evidence.
Staging, retained model and canonical-codec capacity are reserved before getters;
finalization transfers existing reservations instead of requesting memory after
observations have been collected. Native calls can still block and require an
independent supervisor; this method must not run on the parent control thread.

The immutable artifact includes manifest/run/device/artifact/epoch binding,
expected and observed raw identity, all three observations and halt reason. Raw
clock tokens remain explicit even when they conflict with the declared epoch.
Scalar type, unit, symbol presence, unavailable reason/vendor status/detail and
missing anchors remain distinguishable; malformed floats are represented as raw
text, not accepted integer values. `fixture` always marks simulated provenance.
Canonical bytes have a RawArtifact-domain digest; this binds bytes, not truth or
authority. Per-capture representation bounds fit inside existing metadata/RAM
ceilings; the session must still account for all captures against its aggregate
4 MiB metadata limit. The capture adds no clock/settings freshness policy.

`current()` reports readable values at those recorded observations, not fresh
values at a later Start. `compare()` covers only these three observed integers
within the same manifest/session/device/clock/source scope. Neither method proves
unchanged complete settings, other P-states/V-F curve, ownership, safe operation,
write support, comparability or native admission. No 215 W/other historical value
is a baseline default, control target or restoration instruction.

This per-run artifact references an already frozen manifest. It cannot itself be
the manifest's pre-existing starting-evidence artifact, which would create a
digest cycle. The parent still needs the separate initial evidence, actual frozen
relative setup/Start timing, current runtime/environment, required safety/re-entry
facts, trusted native peers and exact independent human consent. The separate
settings exchange below supplies byte-level request/response integration; actual
Windows sampler dispatch and live artifact/session composition remain unfinished.
NativeAdmission has no issuer; no new native-read process,
default executable mode or hardware qualification follows from this source slice.

## Pilot current-settings exchange

`workload::settings_transport` connects the selected collector to a strict parent
reader without extending the readonly sampler protocol or its native modes. Each
bound pilot run permits one request and one response attempt. The independent
`pilot_settings_sampler` role binds the entire WorkerBinding, sequence zero,
request/response kind and canonical body. Four-byte little-endian inclusive frame
lengths remain bounded at 64 KiB. Schema, canonicality, complexity, artifact digest
and exact run/device/manifest/clock/image/profile binding are checked before a
reported artifact becomes accessible. An uncertain send must stop the exchange;
no replay/reconnect or fresh request is permitted on that instance.

The parent freezes setup's relative 10 s deadline from its first supplied setup
entry, capped at the original cumulative 20 s boundary. Entry, deadline and issue
anchor travel in the request; the child checks their arithmetic and chronology.
`Request::collect_native` invokes the actual selected collector once, under that
same frozen window and no earlier than request issue. Before/after getter expiry
preserves any raw return and prevents a successor. It requires an already selected
sampler and separately authorized native scope; it neither loads NVML nor proves
the setup entry, re-entry/safety state, consent or containment. Ordinary telemetry
scheduling and blocking-call supervision still need Windows composition.

The response borrows an immutable Capture and binds both its digest and the exact
request-frame digest. A response-encoding failure leaves the capture caller-owned
and closes the response attempt. `ReportedCapture` is a distinct parent type:
even `native_getter_path` is explicitly a sender claim, with no conversion to a
locally collected native Capture or a NativeAdmission. Its strict decoder retains
wrong raw identities/tokens, missing anchors/fields, reordered metric kinds,
unavailable reason/status/detail and scalar types for separate assessment. Generic
decode/re-encode cannot silently normalize numeric representations into evidence.

Parent ticks run independently of the getter. A structurally valid late or stopped
response is retained with its original issued/receipt anchors, exact raw frame and
capture; it cannot clear a frozen fault. Child anchors must follow issue, precede
receipt and respect the frozen setup deadline. Structural/capacity rejection
leaves the borrowed raw frame caller-owned and permanently closes that receive
attempt. Retained frames/models and temporary codecs reserve shared budget before
allocation; live composition must still partition aggregate metadata/queues/disk.
The receipt is exportable evidence, not a persisted session or complete admission.

CPU tests cover encoded end-to-end exchange, raw scalar/error/clock round trips,
binding/schema/digest tampering, replay, stop/late receipt, original relative and
cumulative setup boundaries, pre/post-getter window closure and capacity failure.
No native getter, process fixture or GPU workload executes in these tests.

## Workload Windows pipe bridge

`workload::windows_transport::InheritedTransport` connects worker and pilot-settings
frames to the existing Windows synchronous pipe workers, bounded queues, reserved
control pool, partial-I/O fault retention and cancellation accounting. It adopts
four unique live pipe endpoints after checking their handle validity/type. It does
not create a process, authenticate a peer, select/load a DLL or issue native work.
The fixed launcher still owes image/creation/job/handle-allowlist/consent checks.
No executable/native-mode behavior or readonly sampler bytes change here.

`workload::transport::Sender` implements the actual producer's FrameSink. Immutable
frame metadata binds the original complete WorkerBinding digest, lane and direction
without altering wire bytes. A foreign frame never enters the pipe queue. Destination
storage is reserved before copying; success transfers the charged copy into the
queue before releasing the original. A failed handoff returns the exact original
frame owner and freezes that sender lane's fault. Ordinary failure leaves the
reserved control path available for Stop. Later pipe-write failures retain their
queued copy plus raw status/count/rejection in the existing fault slot. Queue
acceptance, output-drained state or a successful write is no peer/disk/exit proof.

Each Windows transport owner permits only one sender construction attempt, so a
caller cannot obtain a fresh sender to clear a failed attempt. Before constructing
it, `split_control` may transfer the control receiver/writer ownership to a Send
control owner for a separate thread. After splitting, ordinary receive cannot steal
control packets or control fault slots, and its sender cannot write control frames.
The independent owner uses the same bounded sender with a control-only queue.
Application stop/deadline processing must still be composed on that thread; creating
the transport is not evidence that it was responsive during a blocked native call.

Receive polls capture the supplied receipt clock after the queue operation. Packet,
Idle and transport Fault retain that anchor or explicit missing-clock state. Owned
`RawReceived`/`WorkerReceived`/`SettingsReceived` keep original packet bytes, framing/
write rejection, lane, receipt and reservations alongside any decoded result.
Malformed/partial/foreign/capacity-rejected data therefore remains exportable.
Worker decode poisons on framing/lane/codec failure or missing receipt clock; a
structurally valid message can still be retained with a missing clock and must fail
parent timing/admission. Settings transport rejection closes the corresponding
parent receive attempt, so later bytes cannot turn the rejected exchange into success.

Explicit shutdown requests cancellation and reports the four I/O workers separately,
without joining or extending deadlines. The parent must retain these observations,
actual direct-process/job/resource facts and all raw receipts under aggregate session
metadata/RAM/disk budgets. A lost raw export or unresolved I/O shutdown cannot become
qualified completion. Source tests cover producer-to-queue-to-framer-to-parent audit,
exact original-frame return/no successor after queue failure, control availability,
scope/direction mismatch, partial reads, clock loss and settings receipt rejection.
The concrete Windows pipe adapter is compile/link checked; process acceptance has
not run. Fixed worker launch/entry, actual pilot sampler dispatch, full parent facts/
safety/re-entry/consent admission and aggregate live session export remain incomplete.

## Selected sampler settings dispatch

`workload::sampler_dispatch::Dispatch` composes the delivered pilot settings request
reader, actual selected NVML collector, response encoder and Windows queue sender.
Its Windows `poll_native` consumes the inherited transport's real receive result,
including the post-poll anchor/OS fault, and invokes collection on the sampler's
serial lane. This may block in NVML; it must not execute on the parent or control
thread. The caller must keep independent split control and its shared atomic
`sampler::child::NativeAdmission` stop gate active, and retain every returned Poll.
That sampler gate linearizes stop against collection; it is unrelated to the
unissued workload-compute NativeAdmission token and grants no consent.

The dispatcher freezes the child's first setup deadline, capped by original run
origin, and lets the parsed request tighten it. Receipt must follow issue on the
original clock. Collection's lower clock bound is now the child's receipt; its
upper bound is the earlier child/request deadline. Final getter clocks continue
into subsequent idle polls, so a backwards post-collection clock cannot be hidden
by restarting validation at receipt. No Start/settings freshness policy is added.

The first raw request (even partial/malformed), its receipt, parsed request,
complete bound Capture and any unsent response stay owned and charged together.
A second packet is returned with its exact original owner instead of overwriting
the first or accumulating unbounded rejections. Request-validation/preallocation
failure executes no getter. Collection acquires the existing atomic serial gate;
each getter checks its sticky stop state, and return clears only pending. A racing
stop cannot be reopened by successful completion. Native loading/description and
shutdown are outside this borrowed already-selected sampler and still need their
own supervised setup/cleanup observations.

Capture ownership is stored before response encoding or queue handoff. Missing or
failed facts can still be exported as a response whose raw artifact blocks readable
values; response queued is independent of fault(). Encoding/send failure retains
the capture, exact unsent frame if one exists, and typed queue/OS failure. No retry
can replace them. The explicit fixture entry always marks simulated provenance and
rejects a readable fixture that would conceal receipt/deadline contradictions.

CPU byte-stream fixtures cover parent request -> dispatch -> response queue ->
strict parent receipt, malformed/late/foreign/replayed requests, first-fault/raw
retention, original relative setup caps, backwards clocks and failed facts. The
actual Windows/NVML path is compile/link checked only. This is source composition,
not a runnable/qualified pilot sampler: fixed process entry/launch, independent
control-loop startup, telemetry scheduling, runtime/image/peer/environment/initial
facts, complete parent admission and bounded live session export remain open.

## Fixed pilot process startup

`workload::launch::Declaration` binds one pilot run, its complete WorkerBinding,
role and expected image digest. The fixed roles select `autotune-pilot-worker.exe`
or `autotune-pilot-sampler.exe` alongside the controller. There is no caller-supplied
program, shell, DLL path, kernel override or device ordinal. Declarations are intent,
not human authorization or an issuer of workload NativeAdmission.

The versioned argument vector has exactly 16 bounded tokens: fixed role flag, five
unique nonzero inherited handle values, manifest digest, run ID, frequency, epoch
token, four original epoch counters, original run origin and a role/binding digest.
The strict child reader checks decimal/hex shape and duplicates without adopting
handles. Its `bind` then checks the canonical pilot manifest and recomputes the full
role/run/device/artifact/profile/clock binding. Actual child entry still must check
the five live handle types/roles, its own measured image, runtime and parent scope.

`workload::windows_process::Launch` performs hashing/creation on a background
thread, with a bounded one-result handoff. It reuses the sampler's measured-image,
explicit handle allowlist, private Job Object, suspended creation, assignment,
creation-time and resume primitives. Job commit/user-CPU limits select the existing
approved role values and are read back before use. Sampled user+kernel CPU remains
separate; the controller must still enforce aggregate sampler/session CPU and
original phase/session deadlines. This is not a hard GPU duration/VRAM fence.

The measured absolute image path/hash and read-only shared file handle stay owned
with the process. Post-creation failure retains the original process handle, stage,
raw OS code, assignment/creation/termination facts, I/O cancellation observations
and image observation/lock in `RetainedFailure`. It remains observable before job
assignment; no lost handle or successful terminate request becomes proof of exit.
The new optional image observation does not manufacture one for historical failures.

Stop/termination share the same permanent cancellation flag through launch and
process handoff. The sender checks it at every ordinary queue handoff, even if
constructed before stop; control remains available. A late cancelled result keeps
its process owner, requests termination and stays closed to new ordinary work.
Dropping an untransferred launcher requests termination; dropping the process owner
also requests containment if none was requested, even while the launcher retains
its shared job reference. Without a surviving observer, completion stays unknown.
No automatic restart, replacement, reconnect, elevation or driver reset is added.

The worker originally received `CUDA_CACHE_DISABLE=1` only. The November 5
continuation below adds `CUDA_MODULE_LOADING=EAGER` to that exact child-only,
double-NUL-terminated block; the sampler retains its empty child environment.
This implements the integration contract's recommendation after checking the
[NVIDIA environment-variable reference](https://docs.nvidia.com/cuda/cuda-programming-guide/05-appendices/environment-variables.html).
It requests disabling disk JIT caching; driver behavior and JIT setup time still
need observation. It neither disables compilation nor changes/cleans parent/system
environment or user caches. No other CUDA loading/device/JIT defaults are claimed
as observed. Original setup deadlines still apply if compilation takes too long.

Six CPU tests cover exact argument/binding round trips, role/clock/manifest tampering,
invalid/aliased handles, capacity ownership, fixed role resources/environment, and
cancellation of an existing sender while preserving raw bytes/control. Concrete
Windows process startup is compiled/linked only, not executed. The fixed child
executables/entry/bootstrap/control startup, live telemetry, full parent admission
and session export remain unfinished. Missing images or unmet prerequisites must
fail; this source must not be reported as an executed pilot or useful baseline.

## Common child bootstrap

`workload::bootstrap` supplies a distinct first frame for both fixed pilot roles.
It binds exact process role, declaration digest, first parent setup entry and
canonical pilot manifest. `Sender::send_bootstrap` uses the same bounded Windows
queue path as worker/settings frames and returns the exact original owner on
failure. The existing readonly sampler protocol, worker report frames and launch
argument bytes are unchanged.

The child reader binds the supplied launch arguments to the received manifest,
recomputes the role/run/device/artifact/profile/epoch declaration, and validates
parent setup -> actual first child entry -> actual receipt chronology. Setup uses
the earlier first-parent-entry plus its approved cap and original run cumulative
boundary. `Gate::from_bootstrap` carries that frozen deadline into the worker
handshake; receiving bootstrap later cannot renew setup. Sampler bootstrap cannot
initialize a worker gate. This remains handshake intent, never driver/compute or
human-consent authority.

`workload::child_windows::Context::from_inherited` composes the actual inherited
Windows endpoint adoption, clock capture, image hashing/lock, real pipe polling
and strict bootstrap reader. Its unsafe contract requires one exclusive adoption
at real process entry; validating a numeric handle cannot establish Rust ownership.
All five values are validated before adoption, four pipe types are checked, and
child inheritance flags are cleared. The stop event's role still relies on the
fixed launcher's explicit handle allowlist; wait failure closes startup.

Before the manifest/device/session is known, the existing temporal clock engine
can validate only frequency/token/epoch and consecutive anchors, without inventing
an identity. Its public bound constructor still requires valid full binding. A
charged 64 KiB EntryClock history preserves successful and faulty observations,
explicit missing capture and the first overflow anchor without evicting history.
The concrete child continuously uses it during pre-manifest polling and then
requires complete binding. Regression, suspend/disagreement/bracket policy and
numeric limits are unchanged. A separate first-actual-child-entry setup cap bounds
bootstrap waiting even when stale or forged arguments could not establish an epoch.
The parent must still enforce its own original deadlines off blocking file/pipe I/O.

The context holds exact raw first packet, full bound manifest, first entry/receipt,
clock history, stop handle, transport, measured absolute image path/hash and file
lock. It checks the fixed image name and role's expected digest before returning.
Failures preserve partial available observations, raw packet if received, capture/
OS failures, image state and nonblocking I/O cancellation observations; missing
facts remain unknown. An initial validation/allocation failure manufactures no
clock/identity/image or successful cleanup. The caller retains the original
argument values and must export context/failure evidence within aggregate limits.

Eight CPU regressions cover both roles, mismatched/tampered/partial/canonical
bootstrap, full clock chronology, original setup equality, worker-gate deadline,
capacity ownership, clock-history loss/regression/overflow and sender-to-framer
byte preservation. Native endpoint adoption/clock/file/pipe work is compile/link
only in this slice. No executable main or native admission issuer is added. Fixed
child mains, independent control-loop startup, actual telemetry/driver/producer
composition, complete parent admission and live session export remain unfinished;
this context is no Ready, executed pilot, native qualification or baseline verdict.

## Independent pilot control source

`workload::control` now provides a bounded child stop state for both pilot roles.
The shared atomic gate closes before any control packet is decoded, including
malformed input. Parent trigger times, local triggers and the inherited stop event
merge into the earliest original cooperative/exit windows. Repeated messages or
delayed receipt cannot renew the one-second cooperative or three-second exit
reserve; equality expires. Acknowledgement records whether a native operation
was pending. Completing that operation never reopens the getter lane.

The state retains four complete received owners, the first overflow owner, and
returns any later owner to its caller. Its charged clock history has an explicit
limit derived from twice the run's tick count plus a fixed allowance. The default
bootstrap clock history retains its original 64 KiB limit. Missing, foreign,
regressing or excessive clock observations close admission without manufacturing
timestamps. A complete run with two observations per tick fits the reserved
history; overflow remains invalid rather than evicting earlier evidence.

`workload::control_windows::Handle` composes the split control pipe owner, stop
event and Windows clock on a separate thread, with no CUDA/NVML calls or blocking
join. Both codecs and queue copies use the already reserved control pool.
Readiness follows successful clock/sender initialization. The thread retains
native failure messages, unsent frames, unencoded messages, typed codec/send/
clock/OS failures and the bounded command receiver, including sends racing thread
exit. The report's mailbox still owns unprocessed commands. Callers must collect
and export these owners; dropping a handle closes admission but does not prove
exit, flush output, export a report or cancel a native call. The report can only
be joined after the thread has finished. Parent containment/exit observation
remains a separate required integration.

The new `pilot_sampler` wire role admits readiness and control facts, and rejects
worker bootstrap/start/dispatch messages. Its readiness carries real before/after
anchors, selected raw identity, sampler digest and runtime description. This is
transported child evidence, not trusted identity or native authority: the parent
must still compare the exact raw UUID/PCI pair and required runtime fields with
the manifest and observations. Normalized identity alone does not compare every
raw PCI component. Native-failure messages preserve absent/foreign raw anchors,
typed probe reason and signed vendor status without reclassifying them as valid
success clocks. Existing worker and readonly sampler frame bytes are unchanged.

`NvmlSampler::shutdown` exposes one actual shutdown attempt and its raw signed
status. `None` means no attempt, never success. The session is marked closed
before entering the call; subsequent selective getters fail without resolving or
calling a native getter, and Drop cannot issue a second shutdown, even after a
nonzero status. It remains potentially blocking and must be independently
supervised. The stopped gate has a separate cleanup-pending transition; callers
still enforce the operation count and original stop deadline themselves. Neither
shutdown return nor a child finished message proves physical GPU quiescence.

This is E3.1/E3.2 integration source with CPU-only state/codec/ABI-fake evidence;
the Windows thread is compiled/linked, not executed. Fixed executable mains,
actual pilot telemetry/producer composition, the complete parent fact/safety/
consent chain and aggregate live export remain unfinished. Workload native
admission still has no issuer. E3 remains partial and Gate B unmet.

Final source-bound verification at baseline `a31d9ca` plus the unchanged source
inventory passes 336 CPU-only tests (269 integration, 23 probe ABI fakes, 19 worker
feature-library fakes/static and 25 reference/prepared), formatting, scoped
all-feature warning-denied Clippy and full-workspace/all-feature compile/link.
The 14 additional tests cover original windows, raw ownership, clock loss, role
boundaries, reserved control memory, full tick history and cleanup/ABI shutdown.
Local manifest: `target/workload-verification/pilot-control-verified-manifest.json`,
SHA-256 `a0eb93ca0474001c6c0a7df57d84a9232af463c25e89e8a789ac80f19e5bfc8b`.
No independent-agent review or executed Windows control/native proof is claimed.

## Pilot telemetry composition

`workload::telemetry::Collector` now connects pilot-bound queries to the already
selected NVML sampler and the existing owned pipe sender. `poll_native` composes
the actual inherited ordinary pipe, strict sampler-role decoder, one selected
getter and response handoff. It does not open a runtime or issue compute
permission. The independent control thread shares the same atomic native gate.

Queries carry the full pilot binding, an exact metric/slot ticket and the original
parent issue anchor. Slot arithmetic is checked, issue must be inside the current
slot and run, and per-metric replay closes the lane. Receipt/getter-start/return
chronology remains on the original epoch. A stale query cannot enter a getter.
The existing 50 ms conservative issue-to-return bound is preserved alongside the
actual child before/after bracket; neither is a device sensor timestamp. There
is one serial getter invocation per accepted query, with no retry or replacement.

Raw return ownership precedes post-call clock capture and validation. Missing or
foreign anchors, failed getters, device loss, unit/symbol/value contradictions,
safety boundaries and a return after stop remain retained. A stopped operation
can export its original return but cannot admit a successor. Memory-utilization
unavailability remains optional; GPU loss still stops even on that metric.
Explicit fixture collection marks every response simulated. The parent receipt
reader rejects that source for native assessment and separately checks the exact
expected ticket, original issue, raw chronology, 50 ms conservative query bound,
100 ms transport age, run boundary and field conversion. The reported native bit
remains an untrusted claim requiring actual peer/image/execution evidence.

The charged ledger retains original query packets, typed responses, exact unsent
frames and codec/OS-send errors. Later rejected packets return to the caller.
Clock history reserves one idle observation per tick plus three per possible
metric slot and a fixed allowance, with no eviction or allocator-capacity-based
limit. The complete simulated 45-second, 1,080-query run and its raw owners fit
the session budget. Before-getter reservation failure cannot enter native code.
Callers still retain/export Poll failures and typed clock-capture errors, enforce
the tick cadence and aggregate session limits, and drain output within original
stop policy. Queued response bytes establish neither peer receipt nor durability.

Decoded owned messages now retain the complete immutable codec binding, with its
storage accounted before allocation. Telemetry receipts/queries and control state
reject an owner decoded against another manifest even if monitor identity is
equal. Existing worker and readonly sampler frame bytes remain unchanged. Probe
field equality derives and internal raw-field helper visibility do not change
the report schema.

Final source-bound verification at baseline `bfda12c` plus unchanged inventories
passes 351 CPU-only tests (284 integration, 23 probe ABI fakes, 19 feature-library
fakes/static, 25 reference/prepared), scoped all-feature warning-denied Clippy,
formatting and full-workspace/all-feature compile/link. Fifteen added regressions
cover serial metrics, original clocks/tickets, raw return retention, stop races,
replay, optional/required faults, safety boundaries, exact manifest binding,
capacity, source/role rejection, noncanonical scalar tags and full-run memory.
Manifest `target/workload-verification/pilot-telemetry-final-manifest.json`, SHA-256
`0c0ee2561b8e78fc6e7fbb12336cc091a7eb5dd83fb3bcde2d64931825aa55b3`.
No independent-agent review is claimed. The actual Windows pipe/NVML path is
compiled/linked only; no process, native runtime or GPU execution occurred.

The fixed sampler entry and feature-gated worker target exist in source. The
worker now constructs a one-shot `NativeAdmission` from its retained accepted
grant and exact setup artifact. ParentSampler source checks and queues the
matching grant, but no fixed parent executable composes these paths. Parent
orchestration, aggregate live export, independent local source truth, and real
Windows/native qualification remain unfinished. Parent monitor timing across
runs must preserve the original epoch and session ceiling. E3 remains partial
and Gate B unmet; this is not a useful baseline or an automatic optimization
delivery.

## Fixed pilot sampler entry

`autotune-pilot-sampler` now has a fixed quiet Windows entry that composes the
common inherited context, independent control thread, selected NVML startup,
Ready, current-setting dispatcher and pilot telemetry collector. The entry's
unsafe contract requires one exclusive inherited-endpoint adoption from the
fixed launcher. Strict bounded arguments and bootstrap/image validation precede
control startup; successful control readiness precedes NVML initialization.
The entry returns its context/trace/owners, including a successfully built
context if later trace construction fails. It does not perform driver/compute
initialization, grant human-consent authority or issue native admission.

The `autotune-pilot-worker` binary target is now declared behind the explicit
`native-cuda` feature and routes only the fixed Worker role to
`worker_entry::entry`. After the accepted Start, the worker now requires one
separate ordinary-channel `NativeGrant`, bound to its exact setup artifact and
received within both the original setup deadline and the approved observation
gap after issuance; its issue anchor must follow Start receipt. It retains that
grant packet separately, and `start_native` refuses to run without it.
The worker session's internal `native_admission` step issues the local token at
most once and only from the retained accepted grant and exact completed setup
artifact; the token is rechecked against every retained grant field before
Start. The fixed worker entry uses that method only after receiving through the
inherited transport; fixture packet APIs cannot issue the token. Its digest-only
fields rely on the fixed parent sender to check source facts; they do not
independently prove those facts in the child.
The context/peer/consent/process/safety digests identify parent assertions; the
worker cannot reconstruct the source records from hashes alone. The target's
`native-cuda` feature gate was confirmed in the earlier worker-entry checkpoint.
For this source set, Rust 1.99.1 formatting, warning-denied all-target/all-feature
Clippy and all-feature `cargo check` pass for `x86_64-pc-windows-gnu` and
`x86_64-unknown-linux-gnu`. No tests were run, and the binary/native path has
not been executed.

`ParentSampler::observe_native_grant_processes` now performs a separate fresh
observation of the retained starting, sampler and worker process owners after a
successful queued Start. It brackets those potentially blocking OS reads with
the original Windows clock, requires the bound context/peer/consent chain,
reported safety/re-entry and controller-watch readiness before and after, and
retains a distinct frame. A prior `process_frame` remains historical and cannot
substitute. This method runs on its caller, outside the independent controller
deadline path. `native_grant_commitments` derives canonical digests for the
retained setup reference, successful environment and starting-setting checks,
exact selected starting values, parent context references, received peer
records, interactive consent capture references, fresh retained-handle process
frame, and bracketing safety/re-entry observations. Digest construction
reserves bounded temporary budget before allocating canonical objects. It also
requires the host and starting artifact digests to match the accepted context
references. It
fails closed if Start, required facts, context, peer, consent, process,
controller-watch, or original-deadline evidence is missing or unsuccessful.
The digests bind locally retained records; they do not
attest that reported data is true. `queue_native_grant` captures and checks a
new issue-time parent observation, requires both the retained process frame and
the pre-send observation to be within the approved observation-gap bound,
binds issue-time safety into the commitment, and queues at most one exact frame
on the original Start sequence. Rejected or
uncertain sends retain the frame and poison that channel; a successful queue
does not prove child receipt. The worker token is accepted only after the
child's own grant/setup/chronology/freshness checks. No fixed parent main yet
composes this sender, so an end-to-end native session is still unavailable.
`collect_and_queue_native_grant` composes the process re-observation and
one-shot queue handoff so a caller cannot accidentally use an older frame.
Because those OS queries may block, invoke it only on a retained collector lane
outside the independent deadline controller.

`workload::parent_worker_channel::ParentWorkerChannel` is a Windows-only
composition adapter for the concrete process boundary. It takes the worker's
unique `Process::sender` once, constructs each `process_state::Processes`
observation with that same retained worker, and routes both `queue_start` and
`collect_and_queue_native_grant` through the same sender value. This prevents a
caller from accidentally restarting the sequence or pairing those process
observations with another worker owner through this adapter. Its nonblocking
`poll_setup` reads one stamped packet from that same process owner, advances
the setup receiver's original deadline on idle polls, and transfers the
`ReadyDelivery` only after the strict response and Ready are both present; the
caller retains the receiver and its raw failure evidence on every rejection.
Before its first `queue_start`, the adapter reserves bounded storage for both
the result packets and the audit. If reservation fails, Start is not sent.
After Start succeeds, it constructs the audit from the exact issued anchor
retained in the Start attempt using the already reserved storage.
`collect_and_queue_native_grant` refuses NativeGrant if that audit is absent,
faulted or overflowed, so the worker cannot enter native execution through this
adapter without a ready result-report owner. A construction or anchor failure
after Start still leaves the queued Start retained; the caller must stop the
worker and preserve that unresolved session rather than attempt NativeGrant.
It still does not launch the starting/sampler/worker processes, gather the
required artifacts or consents, create the controller, handle worker/sampler
and parent sampler receipts, export an aggregate live session, or construct a
fixed parent main.
Rust 1.99.1 formatting and warning-denied all-target/all-feature Clippy and
`cargo check` now pass on Windows GNU and Linux targets. No tests, process calls,
native entry or GPU execution were run; these source checks grant no execution
authority.

One sampler output channel owns Ready and all telemetry record sequence numbers.
A bounded canonical role selector only routes an ordinary packet to the existing
strict settings or sampler parser; it cannot authorize it. Telemetry may support
re-entry while the original settings/setup exchange remains pending. Replay,
wrong role/binding, packet rejection or transport loss stops the session and
retains the rejected owner or raw error. Source initialization, descriptor,
metric and setting returns remain owned before their post-call clock checks.

The original bootstrap setup deadline is now frozen in the control state's full
binding before native startup. Its independent tick can close a blocked startup
lane at that deadline and derive grace from the deadline itself, rather than a
later return or poll. Queued Ready/current facts may complete this setup watch
once, before expiry; completion neither acknowledges peer receipt nor grants
compute. Late publication synchronously tightens the original cooperative cap.
An expired stop never reopens after a native return or late setup completion.
The parent still owes its independent phase/session/process supervision.

`NvmlSampler::open_retained` preserves an initialized session when exact selection
fails. The original failure and a later shutdown result remain separate. Existing
`open` behavior stays compatible. The entry holds selected/failed sessions in
manual-drop owners, so unwind or a missed cleanup window cannot silently call
NVML shutdown. An explicit attempt requires the stopped idle native lane and
remaining original grace. Its raw status is stored before closed-library release
and post-clock validation. Otherwise the initialized owner remains retained;
process containment/exit and native quiescence remain independent unresolved facts.
Control cannot interrupt an in-flight native startup/helper or shutdown operation.

The common trace retains logical open/describe/cleanup brackets, failed/missing
clocks, original stop trigger, setup completion and drain receipt. Duplicate
attempts/results cannot overwrite the first evidence. Descriptor return at the
original setup equality fails even inside the later cumulative boundary. Control
clock initialization also retains its actual observed frequency, including a
mismatch. Cleanup completion, control-thread completion and output drain are
separate observations. Drain polls precede receipt capture and stop at the
original cooperative cap; no unbounded thread join occurs. Exit code zero denotes
only observed orderly child completion, never qualification, peer receipt,
durability, direct-process exit or a GPU fence. A parent still must validate all
required facts and export the complete bounded live evidence.

Final source-bound verification at baseline `4880469` plus unchanged inventories
passes 368 CPU-only tests (300 integration, 24 probe ABI fakes, 19 feature-library
fakes/static, 25 reference/prepared), scoped all-feature warning-denied Clippy,
formatting and full-workspace/all-feature compile/link. Seventeen added tests
cover original setup/cleanup/drain equality, blocked-lane deadline processing,
non-renewal, sticky stops/failures, raw status/clock ownership, role routing,
full-run trace capacity and retained failed-selection cleanup. Manifest
`target/workload-verification/sampler-entry-final-manifest.json`, SHA-256
`b2a69bb5640b209f96b26e16fadc2d7da140ce5ee0286d503cadac3ba7952f97`.
No independent-agent review is claimed. The new binary and actual Windows/NVML
composition were compiled/linked only, never executed or driver-loaded.

Complete parent pre-manifest facts/runtime/environment/image/peer observations,
fresh safety/re-entry/Start/consent admission, the worker entry/driver composition,
aggregate metadata/session budgets and live export remain unfinished. The child
trace's ownership is not a completed artifact export. No sampler/native/hardware
qualification, useful baseline, E3 completion or Gate B follows from this slice.

## Parent sampler scheduling and receipt assessment

`workload::parent_sampler::ParentSampler` now composes the sampler-side parent
exchange: one ordinary sequence for Ready/metric receipts, a separate settings
exchange, sequential issued queries, strict raw receipts, monitor policy,
re-entry observations and reserved control traffic. Its Windows factory binds
the retained process declaration and actual reserved control pool. `drive_process`
polls that original process transport, queues settings once and asks for one
metric at a time. Faults request the owned stop event and one control-frame send;
uncertain sends are retained without retry. Inputs-finished, receive, query and
stop-event observations remain distinct; transport closure is not process exit.
These Windows paths compile/link only in this delivery.

Ready checks compare the exact raw UUID/PCI text, including PCI function, then
parse the bounded runtime description with canonical round-trip validation and
required typed version fields. The current-settings reader requires its original
request/digest/window and a readable reported native path. Raw simulated claims,
malformed/duplicate/foreign frames, missing clocks and late replies remain owned.
Outstanding ticket/issue anchors, original unsent owners and typed codec/OS errors
are retained. The parent ends admission after the original combined maximum
query/transport interval if no reply has arrived; this necessary bound adds no
time beyond the existing 50 ms/100 ms policies.

The re-entry screen pairs temperature and GPU-utilization records by scheduled
slot and requires four consecutive qualifying pairs. An invalid newly observed
value or a missed required re-entry slot clears the old sequence immediately;
it cannot borrow an earlier four-pair result while awaiting a new pair. Paired
evidence must remain fresh under the unchanged observation-gap policy. A reported
screen is deliberately insufficient for native permission: it authenticates no
peer, environment or human consent. Its state/evidence are exposed read-only;
neither this actor nor the monitor can issue workload `NativeAdmission`.

Pilot monitor construction now reserves scoped arrays before allocating them in
the shared root. Structural capacities follow the declared run duration/ticks
and six metric schedules, inside the existing profile ceilings. The original
epoch and absolute 180-second session ceiling remain; per-run initial safety
grace begins at the declared run origin, never at delayed controller creation.
Slots keep their original session numbering (for example, 45 seconds starts at
slot 180), without inventing missed samples from earlier runs. Legacy standalone
monitor construction and the acceptance cadence/policy remain unchanged.

Planned sampler shutdown closes scheduling, freezes original grace, and retains
acknowledgement and reported cleanup/completion separately. Reported completion
requires a timely nonpending successful cleanup frame and cannot clear timeout.
No new query is admitted during shutdown. Only the outer controller can establish
worker completion, direct-process exit, job accounting and permission to proceed;
child completion or queue drain remains no physical GPU fence.

Final source-bound checks at baseline `7e12751` plus unchanged inventories pass
382 CPU-only tests (314 integration, 24 probe ABI fakes, 19 feature-library
fakes/static, 25 reference/prepared), scoped all-feature warning-denied Clippy,
formatting and full-workspace/all-feature compile/link. Fourteen added tests
cover report/settings composition, original epoch/slot/safety grace, re-entry
resets, pending/late queries, exact raw identity, planned/expired cleanup,
reserved control capacity and a full 45-second/1,080-reply parent run with the
12 MiB writer/failure pools reserved. Native-path claims in these fixtures are
explicitly synthetic reported inputs, never native execution evidence.
Manifest `target/workload-verification/parent-sampler-final-manifest.json`, SHA-256
`4b03fa49c3a5a5cfcaafa41fff8f1862ec652afef0402613d21cd90caf7670ed`.
No independent-agent review, process/native/GPU execution or qualification is
claimed. Complete pre-manifest/environment/peer/image/consent facts, worker entry,
native admission, aggregate live export and executed qualification still remain;
E3 stays partial and Gate B unmet.

## Starting settings before a pilot manifest

`workload::starting` introduces a separate starting-readout scope and immutable
raw artifact. The request binds session/capture nonce, original clock token/
frequency/epoch, exact selected UUID/PCI, source/artifact/profile digests and a
read-scope reference. It contains no invented run ID or placeholder pilot
manifest. Read approval references and later compute references remain separate
declarations; a string or digest establishes neither human approval nor truth.

The readout retains entry, every before/after anchor, original scalar fields and
typed halt. It uses the original ten-second preflight ceiling and temporal clock
checks without manufacturing a session/run/device monitor binding. The selected
NVML collector runs only on an already independently supervised serial lane.
It reads each of the three existing current-setting getters at most once and
closes that read lane on return, preserving a stop observed during a native
return. Raw results precede post-call clock checks. Cleanup remains a separate
supervised operation, and no native execution occurs while creating a scope or
parsing/validating the artifact.

Scope cloning, raw storage and finalization scratch are reserved before getters.
Completing a readout requires no later budget reservation, including when other
ordinary allocations consume the remaining root pool. A failed reservation or
identity check cannot enter a getter. The explicit fixture producer keeps
simulated provenance. The strict parent reader checks digest, exact request,
canonical schema, source tag and raw clock/scalar encodings, retaining contradictions for a
separate readability decision. Missing clocks, raw vendor failures and fractional
or mismatched-unit values are not repaired or replaced with defaults.

The reported artifact can validate a pilot declaration's starting reference,
selected identity, session, original epoch/frequency, source and all seven
artifact hashes. It rejects reuse of the capture nonce as a pilot run ID and
checks that capture completion precedes a bound run origin. The parent sampler
can retain this exact check and compare all three later reported current values;
a duplicate attempt cannot replace the first result. These operations bind
reported facts only. They authenticate no peer, native read, environment or
consent and cannot construct workload `NativeAdmission`. Equality concerns only
the three captured values, not factory settings or the complete operating state.

Existing settings readability validation and pilot-profile canonical encoding
are shared without changing their wire/profile bytes. Final source-bound checks
at baseline `60197df` plus unchanged inventories pass 396 CPU-only tests (323
integration, 24 probe ABI fakes, 24 feature-library fakes/static, 25 reference/
prepared), scoped all-feature warning-denied Clippy, formatting and full-workspace/
all-feature compile/link. Fourteen added cases cover scope/reference/clock binding,
strict parsing, raw failure retention, one serial readout, stop/clock failures,
pre-reserved finalization under pressure and the parent's first immutable check.
Native-source claims used in tests are explicitly fabricated inputs, not proof
of a native capture. Manifest
`target/workload-verification/starting-evidence-final-manifest.json`, SHA-256
`edd78fa59be4b94835e1ad88239378026ead12343e208a17c54a7bf6a2087ef7`.
No independent-agent review is claimed; no owned sampler/worker, native runtime
or GPU execution occurred. A dedicated pre-manifest bootstrap/entry/transport and
supervision path still must compose this collector before any real capture.
Complete environment/peer/image/consent facts, worker/native admission, aggregate
live export and executed qualification remain unfinished. E3 remains partial and
Gate B unmet; the automatic benchmark/search/validation goal is unchanged.

### Pre-manifest starting bootstrap and fixed launch — 2026-11-02

`workload::starting_bootstrap` adds a separate `--pilot-starting-v1` argument
vocabulary and canonical first-frame request for the existing fixed sampler
image. Its immutable declaration binds the starting scope and original parent
entry. Five unique nonzero inherited-handle tokens, scope digest, frequency,
epoch and declaration digest are parsed without adopting handles. The strict
reader checks the exact request, original epoch, parent/child/receipt chronology
and original-epoch 10 s preflight ceiling; a late child cannot renew that budget.
No pilot manifest or run identity is fabricated. Canonical wrapper re-encoding
and byte/frame/codec reservations precede allocation.

The distinct one-shot bootstrap sender returns the exact unsent frame on failure,
retains its first typed error and cannot retry after failure or successful queue
handoff. Queue handoff is not pipe completion or child receipt. Shared
`CancelQueue` closes an already-created ordinary sender after cancellation.

The actual Windows `Launch`/`Process` implementation now uses a sealed declaration
parameter with the existing pilot declaration as its default. The only additional
implementation is the starting declaration, which selects the same measured
sampler image, sampler job limits and exact child-only empty environment. Shared
image hashing/lock, private suspended-process/job startup, allowlisted handles,
retained failures and original process/resource/exit observation paths remain
the same. A starting process exposes only its separate bootstrap sender; it
cannot acquire a pilot worker/settings sender. This is compiled source, not an
executed process or aggregate session-resource qualification.

Eight added CPU/static tests cover roundtrip/scope, distinct modes and handle
tokens, original deadlines and clock faults, foreign/tampered/truncated frames,
one-shot exact-owner failures, capacity, cancellation and Windows API compilation.
The executable still needs the starting child context, control/entry and response
composition. Binding bytes or declaring a read approval reference grants no
native read/compute permission. E3 and Gate B remain open.

Source-bound verification at baseline `76f85a5b0f2383a6b30192caf9a1a3e013016218`
plus unchanged source inventories passes 404 CPU-only tests (331 integration,
24 probe ABI fakes, 24 worker feature-library fakes/static, 25 reference/prepared),
formatting, scoped all-feature warning-denied Clippy and full-workspace/all-target/
all-feature compile/link. Manifest
`target/workload-verification/starting-bootstrap-release-manifest.json`, SHA-256
`7b58f64a3bc5bb2e725a9772db71e0c7d85cbee9d5d8638495ba015b8e43263b`.
Both pinned old matrix images remain unchanged. No independent-agent review,
owned-process execution, native loading or GPU execution occurred.

### Starting child inherited context — 2026-11-02

`child_windows::Context<starting_bootstrap::Bound>::from_starting_inherited`
composes the actual starting-scope reader with the existing Windows inherited
endpoint adoption, real clock/frequency capture, own-image file lock/hash and
nonblocking first-packet polling. A private argument vocabulary shares this OS
path with the existing pilot context; pilot types and entry API remain the
default. Starting mode selects only the fixed sampler image, validates its exact
scope-bound digest, and creates no pilot manifest or run identity. The unsafe
entry requires exclusive inherited-handle ownership and exact separate execution
scope; numeric tokens and a matching hash do not establish that scope.

`Arguments::preflight_end_100ns` validates actual first entry against the original
epoch and rejects entry at or after 10 seconds. The raw interrupt-time end is
computed with checked arithmetic from that epoch, rather than child entry or
receipt. Both idle polling and first-packet receipt use this original end, while
the strict bootstrap reader checks the complete parent/child/receipt chronology.
No native loading/collection occurs in this context. Startup rejection keeps the
existing raw first packet, available raw clocks/frequency, typed clock/OS errors,
locked image facts and transport cancellation observations in the failure owner.

Three new CPU/static tests cover original and nonzero epochs, deadline equality,
late entry, bad clocks, raw end overflow and typed Windows entry/parts API
compilation. They do not execute inherited-handle adoption. Separate starting
control/read/cleanup/response composition and executable routing remain open,
along with complete actual parent facts/consent, native admission, live export
and qualification. This advances E3's measurement chain without closing Gate B.

Final source-bound checks at baseline `d9f0030b385a5f0bc04b5383c0c470ffe25ed885`
plus unchanged inventories pass 407 CPU-only tests (334 integration, 24 probe ABI
fakes, 24 worker feature-library fakes/static, 25 reference/prepared), formatting,
scoped all-feature warning-denied Clippy and full-workspace/all-target/all-feature
compile/link. Manifest `target/workload-verification/starting-context-final-manifest.json`,
SHA-256 `be0402447220a940775ca2a10a9b4060f4ca2985d08d3039f09c252ecfe85cd4`.
Old matrix images remain byte-identical. No owned process/native/GPU execution
or independent-agent review is claimed; actual context adoption is compile-only.

### Independent starting read control — 2026-11-02

`starting_control` supplies a distinct immutable scope/epoch-bound temporal state
and shared serial-read gate. It uses no pilot manifest/run/WorkerBinding. Work
closes at the original 10 s preflight boundary. First stop freezes 1 s cooperative
and 3 s exit-observation windows under the original session ceiling; delayed
deadline detection uses the original boundary rather than now plus grace. These
windows permit separately supervised cleanup/exit observation, never extra reads
or an extended preflight. Stop during a pending call remains stopped after its
return. Cleanup eligibility checks the original raw deadline, valid clocks and
absence of a pending call; callers still owe exact read scope and separately
bounded native entry. There is no workload NativeAdmission issuer.

Completion preserves initialized-session, cleanup-attempt and raw shutdown status
as distinct reported facts. Initialized sessions require attempted cleanup with
returned status zero for an orderly child report. Pending calls, missing/future/
late clocks, contradictory cleanup and a previous fault cannot become success.
The first command/actual receipt remains immutable. Reported completion is not
observed process exit, released resources or a GPU fence.

`starting_control_windows::Handle` composes the actual split control port, stop
event, Windows clock/frequency and bounded one-slot completion mailbox on an
independent thread with no NVML/CUDA calls. Readiness follows its first actual
event/clock tick; main must also check its fresh native-entry clock/gate. Every
poll's clock precedes and is retained separately from completion receipt. This
starting mode uses the stop event only: incoming control packets are terminal
rejections retained verbatim, including pilot-role Stop/Start packets. No wire
ack/final response or executable mode routing is supplied in this slice.

Raw event status/error, available frequency/clock/transport failures, rejected
packet and pending completion racing thread exit stay owned in the report.
Shared state and the extractable mailbox retain their own RAM reservations.
Report retrieval joins only a thread already observed finished; Drop closes the
read gate without a blocking join or implicit native cleanup. Process/job
termination and aggregate resource observations remain the parent's obligations.

Eleven new CPU/static cases cover blocked calls, no renewal, exact stop/cleanup
boundaries, immutable completion facts, initialized/pending/future/late failures,
epoch/overflow, full 13 s history under ordinary budget exhaustion, overflow/
capacity/shared-owner lifetime and actual Windows API compilation. They do not
start the watcher, load a driver or execute a getter. The native read/cleanup
entry, bound parent response, executable routing and integrated qualification
remain unfinished; E3/Gate B stay open.

Final source-bound verification at baseline `0483735469fbf777f812f188bd3eeb47a5b4efe0`
plus unchanged inventories passes 418 CPU-only tests (345 integration, 24 probe
ABI fakes, 24 worker feature-library fakes/static, 25 reference/prepared), formatting,
scoped all-feature warning-denied Clippy and full-workspace/all-target/all-feature
compile/link. Manifest `target/workload-verification/starting-control-closed-manifest.json`,
SHA-256 `ebf122baf2f9719c9dc13534c845b5f500aebefa57e0f309858ff511f35adf24`.
Earlier checks preceded the final poll-clock retention fix and remain historical.
Source review also closed readiness, shared reservation lifetime and initialized
cleanup-fact gaps; no independent-agent review or actual watcher/process/native/
GPU execution is claimed. Both pinned old matrix images remain unchanged.

### Starting serial native entry source — 2026-11-02

`starting_runtime::Trace` and `starting_entry::execute` compose the actual
starting child context, independent event/clock watcher, retained NVML open and
description, existing three-setting `Snapshot` collector and explicit cleanup.
The trace binds the starting scope directly, reserves raw history/metadata before
native entry and records each logical operation at most once. A returned native
result is owned before its post-call clock. Failed clocks/late returns remain
raw evidence, stop later groups and cannot be repaired by cleanup or drain.
The Snapshot collector owns its own per-getter brackets and serial gate pairing;
the outer Read operation denotes that collector invocation, not an extra getter.

Cleanup can enter only on a stopped, nonpending lane inside the first original
cooperative window. Its signed status is retained before releasing the closed
owner. Selected and initialized failed sessions share the extracted
`sampler_native::NativeOwner` with the existing pilot entry; the owner methods'
behavior is unchanged, and implicit native cleanup remains suppressed on
unwind/drop. Clock/control failure can leave an initialized owner retained for
parent containment rather than inventing successful cleanup.

The returned outcome owns raw native open failure, runtime description, Snapshot
or typed read error, call/clock/drain history, event-clone/clock/control errors,
failed original completion and available control report. It provides no final
wire response, artifact export, executable mode routing or success exit code.
An in-memory result is not delivered evidence, parent receipt or process exit.
The unsafe function invokes NVML if called; exact separate native execution
scope and independent parent process supervision remain caller prerequisites.

Open/Describe currently denote **logical probe API groups**, not individual
ABI-call boundaries. Internal selection/runtime helpers perform multiple native
calls. Their per-call stop/deadline barriers must be audited and implemented
before executable routing and live qualification; group brackets do not prove
that no internal successor call occurred after a stop. This limitation also
applies to the reused probe group APIs in the existing pilot entry. No runtime
qualification is inferred from composition or compile-only entry signatures.

Seven new CPU/static cases cover operation ordering, one-shot raw retention,
late/stopped group returns, cleanup/deadline/drain, clock loss, foreign scope,
capacity/exhaustion and actual Windows entry API compilation. No native entry,
watcher, owned process, driver or GPU execution occurs in these tests.

Final source-bound checks at baseline `ba1debbc312b86ad4eeb855be59adc0f25310f3d`
plus unchanged inventories pass 425 CPU-only tests (352 integration, 24 probe ABI
fakes, 24 worker feature-library fakes/static, 25 reference/prepared), formatting,
scoped all-feature warning-denied Clippy and full-workspace/all-target/all-feature
compile/link. Manifest `target/workload-verification/starting-entry-final-manifest.json`,
SHA-256 `289a5578061c2315d2fd6c058a1aec8e7f2eefef7097059575c9f73e7144ae8f`.
Both pinned old matrix images remain byte-identical. No independent-agent review
or process/native execution is claimed; this is source composition, not E3/Gate B
acceptance or qualification of the internal native-call groups.

### Parent-side starting session — 2026-11-04

`workload::starting_session::Session` composes the single starting reception under
the parent's original epoch. It freezes the declaration/scope binding and reserves
the receiver capacity before any chunk, derives the arrival bound from the approved
profile (preflight plus the cooperative-stop and exit-observation allowances,
capped by the pilot session), and exposes the launched process only by reference:
the session creates no process, issues no workload `NativeAdmission` and proves
neither peer exit nor qualification.

Every driver poll converts the observed anchor to elapsed nanoseconds from the
original epoch before a deadline or clock check, so a raw QPC reading can never be
compared against an elapsed bound. `poll_native` now takes that conversion
explicitly and keeps lane/framing rejection, a closed peer and an OS error distinct
from the receiver's own sticky chunk/clock/deadline fault. `Session::settle` merges
those supervision facts: a verified completion always wins and cannot be withdrawn
by a later transport or supervision observation, the first OS cause is retained,
and a closed peer without completion becomes an explicit stopped outcome. A stray
chunk after completion, an idle deadline, clock regression or missing clock remain
sticky and expose no payload.

Six added cases cover the exact verified payload/outcome, transport-fault retention
without payload exposure, closed-peer stop, idle-deadline and missing-clock faults,
scope/capacity rejection before reception, and Windows compile-only driver wiring.
No launched process, native entry, GPU execution, admission token or live
qualification occurs in these tests. The arrival bound is a reception bound, not a
granted runtime or an approved phase extension.

Final source-bound checks at baseline `b8bb700` plus unchanged before/after source
inventories pass 481 CPU-only tests (400 integration across 40 test binaries,
including 19 reception/session cases; 32 probe ABI fakes, 24 worker feature-library
fakes/static, 25 reference/prepared), formatting, scoped all-feature warning-denied
Clippy and full-workspace/all-target/all-feature compile/link. Manifest
`target/workload-verification/starting-session2-manifest.json`, SHA-256
`7e961e87fe1478fb07439a4f19b1919fd15686a34a90f8bf1ae3b3e36fa8296c`; the
cross-target `x86_64-unknown-linux-gnu` workspace/all-target check passes in an
isolated `target/linux-check` root. A first attempt failed only the formatting gate
and remains recorded. Both pinned matrix images remain byte-identical. No
independent-agent review, launched process, native entry/load, driver or GPU
execution is claimed; E3 remains partial and Gate B unmet. The child worker
entry/driver/setup, complete consent/environment facts, the Start/native admission
issuer and aggregate live export remain missing.

### Parent-side starting launch handoff — 2026-11-04

`workload::starting_launch::Handoff` (Windows only) composes the two parent-side
pieces established above: it builds the `Session`, freezes the encoded starting
`Frame`, then acquires the one-shot starting sender and sends exactly that frame.
`Handoff::send` reports at most one attempt: a failed send re-stores the exact
unsent frame and the first `SendFailure`, and a repeated send never re-acquires a
new sender. `Handoff::poll` delegates a nonblocking process poll to
`Session::poll_process`, so the reception deadline, original-epoch conversion and
sticky fault handling are the session's, not a second copy.

`Session::observe` exposes that epoch conversion as a portable accessor: one parent
`Anchor` becomes original-epoch elapsed nanoseconds, while a mismatched token,
regression or missing observation yields `None` instead of a restarted base and
must be retained as a clock fault by the driver. This keeps the session clock
domain meaningful on non-Windows targets and removes the cross-target dead-code
warnings without a target-specific allow.

Three compile-only cases cover the handoff composition and the session driver
wiring, and one portable case covers anchor conversion and rejection. No process is
created, no sender is exercised against a real pipe, and no native entry, GPU
execution, admission token or live qualification occurs. The handoff proves only
parent-side composition; the launched process stays owned by its caller.

An initial slice (`starting-handoff1`, 482 tests) is recorded. The amended slice at
baseline `c17ac64` plus unchanged before/after source inventories passes 483
CPU-only tests (402 integration across 40 test binaries, including 21
reception/session/handoff cases; 32 probe ABI fakes, 24 worker feature-library
fakes/static, 25 reference/prepared), formatting, scoped all-feature
warning-denied Clippy and full-workspace/all-target/all-feature compile/link.
Manifest `target/workload-verification/starting-handoff2-manifest.json`, SHA-256
`add9dab0777591e4dcc149343514ca153a1118dcabc12abc68c28eb3c5b0321e`; the
cross-target `x86_64-unknown-linux-gnu` workspace/all-target check passes
warning-free in the isolated `target/linux-check` root. Both pinned matrix images
remain byte-identical. No independent-agent review, launched process, native
entry/load, driver or GPU execution is claimed; E3 remains partial and Gate B
unmet. The child worker entry/driver/setup, complete consent/environment facts,
the Start/native admission issuer and aggregate live export remain missing.

### Strict parent starting-response reader — 2026-11-04

`starting_read::Receipt` validates the bounded canonical response against the
parent's frozen scope/declaration, delivered digest, original epoch/frequency
and read deadline. The child's first/receipt anchors receive the scope's original
clock token; parent-entry ordering, clock agreement and deadline equality are
checked. The read deadline is the original 10 s preflight ceiling, separately
from the receiver/session's chunk arrival bound.

The reader checks exact nested schema/order, fixed ABI scalar widths and raw
buffer sizes, declared fault variants and embedded Runtime serialization. It
reserves codec storage before decoding and bounds structural/runtime complexity.
Nested starting artifacts use the existing ReportedSnapshot reader, verify their
own digest/request/source consistency and remain owned for subsequent manifest
reference and readability checks. Raw failures, invalid values, missing facts
and simulated/reported-native sources stay explicit. A typed receipt grants no
native execution, peer authenticity, process-exit or baseline qualification.

Session::read_report exposes this reader only after verified chunk delivery and
uses the session's retained digest. It returns Incomplete for pending/failed
reception. Nine CPU tests cover actual exporter/reader/session composition,
maximal ledger count, original/nonzero epochs, deadline equality, schema/digest/
source tampering, raw signed failures, absent fields and capacity/complexity.
Local verification is authoritative for its declared scope; GitHub Actions was
removed entirely at the owner's request on 2026-11-04.

Final source-bound verification for the strict parent reader at baseline
0b342c7aeb6905dab95ae48d1703199f01b9359c plus unchanged inventories passes 492
CPU-only tests (411 integration, 32 probe ABI fakes, 24 feature-library fakes/static,
25 reference/prepared), formatting, scoped all-feature Clippy and full-workspace/
all-target/all-feature compile/link. Manifest
 target/workload-verification/starting-read-closed-manifest.json SHA-256
 1bce9c333040be4db7b37a117283a8b1c7e5e5053ee20e7e3d3ecb711b6639e1.
Local Linux workspace/all-target check passes; log starting-read-linux.log SHA-256
89bdd1406dabc9807a5e45d2680ffe8e371d7849f1b039276e2ea8ee90439596.
No process/native/GPU execution or independent-agent review. Both pinned images
remain unchanged. GitHub Actions workflows were removed and repository Actions
permissions were also verified disabled (enabled:false) at the owner's request.

### Observed CUDA setup and retained failure — 2026-11-04

The opt-in Windows CUDA adapter's unsafe open_observed now checks an observer
before every actual setup call and reports actual signed status and initialized
output fields before post-clock validation. It covers driver init/version/count,
unique UUID/PCI/capability selection, context/current-device check, fixed PTX and
function, memory headroom, four buffers and the single stream. No observer or
hash grants execution consent. Integer, UUID and memory values are recorded;
opaque pointers stay in the original owner and are exposed only as null/zero
presence metadata, which does not prove failed outputs are valid resources.

A failed setup retains the loaded library, expected/observed paths and the
partially initialized backend in SetupFailure. Its resource view is read-only;
there is no speculative cleanup/retry method. No native call occurs during Drop.
The same owner-transfer helper is used by production and typed failure tests.
Legacy unsafe open delegates to the same implementation with an unobserved
observer; its existing execution prerequisites still apply.

Four new typed ABI-fake cases exercise all 20 one-device setup operations: every
pre/post observer rejection, every native error, exact statuses/partial ownership,
identity ambiguity, count bound, null context and headroom, and absence of compute
or implicit teardown. No public loader/native entry is invoked. The setup observer
still must be bound to the original worker clock/control/phase deadlines; observed
teardown and actual worker entry, complete parent facts/consent, admission and
aggregate export remain open. No workload NativeAdmission issuer is added.

Final source-bound verification at baseline a723d207e36803d7456466d46089ce44e7a3d646
plus unchanged inventories passes 496 CPU-only tests (411 integration, 32 probe
ABI fakes, 28 worker feature-library fakes/static, 25 reference/prepared), formatting,
scoped all-feature warning-denied Clippy and full-workspace/all-target/all-feature
compile/link. Manifest target/workload-verification/cuda-setup-retained-manifest.json
SHA-256 68f1a5f37dc28465a526ffdb480d430d6cbd1c8e29279231851e2b341bd37a43.
Local Linux workspace/all-target check passes; log cuda-setup-retained-linux.log
SHA-256 6c209f588f83a4d31f84d7ebb728b26d4f51b3c7c7f519f492b972b034c41786.
The included Rust source/tests pass explicit rustfmt checks. The initial large
error lint is explicitly allowed to transfer original partial ownership without
fallible allocation after native entry. Earlier checks preceded the final
production owner-transfer test path. No independent-agent review or actual DLL
loader/native/process/GPU execution. Pinned old matrix images unchanged. Actions
remains removed and disabled; local verification only.

### Observed CUDA teardown — 2026-11-04

Source-bound local verification passes 500 CPU-only tests (411 integration,
32 probe ABI fakes, 32 feature-library fakes/static and 25 reference/prepared),
formatting, scoped warning-denied Clippy, full-workspace/all-target/all-feature
linking and a Linux workspace/all-target check. The exact baseline, manifest
and hashes are retained in the current ROADMAP.md handoff. Included source/test
files pass explicit rustfmt checks; both pinned old matrix images are unchanged.

CudaBackend::close_observed now places pre/return hooks around stream destruction,
each of the four buffer frees, module unload, context destruction and library
release. CUDA return codes and library OS outcomes are separate typed facts;
FreeLibrary is never reported as a fabricated CUDA status. Hooks retain the raw
return before checking a post-call clock, and a rejection prevents every successor.

Teardown accepts only Ready/Complete and poisons the backend before its first
attempt. A successful release clears its token before the return hook. A failed
call preserves its uncertain token and all remaining owners; any later close is
rejected, including after a late successful return whose observer failed. DLL
close uses the installed library's consuming close API; a failure retains only
the original opaque owner without a second OS call or a safety/validity claim.
No automatic native cleanup occurs in Drop. Legacy close delegates to the same
path with an unobserved observer.

Four new typed ABI-fake cases cover all eight pre/post boundaries, every cleanup
native error, known/unknown library errors, no successor/retry/implicit Drop,
resource token state and idle-state requirements. Fake DLL tokens are never
passed to an actual library-release call; those fixtures only forget the token.
No real driver/DLL/process/GPU execution occurs. The first fixture compile error
was an HMODULE type mismatch and was fixed before final verification.

The producer-bound integration below now supplies the frozen clock/control
checks for Driver close. The standalone legacy close remains unobserved and
retains its external execution prerequisites. No worker-entry, native admission,
physical quiescence or baseline qualification follows from this source seam.
Actions remains disabled; verification is local.

### Bound CUDA setup clock and control — 2026-11-04

`workload::cuda_setup_trace::Ledger` implements the actual opt-in adapter's
`SetupObserver`. Construction requires an exact control binding and its existing
one-shot bootstrap setup deadline, retains the original epoch/frequency, and
reserves the maximum 146 ABI-call entries before any native call. Each entry
retains attempted/not-entered state, raw return and both clock observations.
Rejected returns and capacity overflow have separate retained slots. The shared
serial gate marks native entry/return and preserves pending uncertainty across a
mismatched return; a concurrent stop prevents any successor.

Raw results are owned before the post-clock callback. Missing/regressing clocks,
deadline equality, native errors, stop, protocol and capacity faults close
admission. A callback unwind also closes admission, retains the available raw
result and never clears a native call that has not returned. The ledger does not
mark Ready or setup completion; the existing receipt step still owns that.
`Shared::expire_setup` is shared with late Ready completion so a late native
return tightens grace from the frozen setup deadline rather than its return time.

Eight CPU/ABI-fake tests compose the production setup engine with this observer,
including all 40 per-call stop/clock-loss boundaries and 64-device enumeration.
This is source integration, with no actual driver load or GPU execution. Actual
loader/entry/export ownership, teardown under the producer's original exit
window, complete parent facts/consent and native admission remain open. Exact
verification evidence is retained in ROADMAP.md.

### Producer-bound per-call teardown — 2026-11-04

`producer::Driver::close` now requires the portable `teardown::Observer`; the
actual CUDA bridge forwards it to `close_observed`. `Producer::cleanup` uses its
existing continuous Context clock and frozen exit deadline at each of the seven
CUDA releases and optional DLL release. Its original control window can tighten
that ceiling. There is no new epoch, grace period or evidence allocation during
cleanup: eight inline entries and bounded rejection/overflow slots are charged
inside the producer's preparation reservation.

`bind_control` attaches the exact independent Shared state before Start and
cannot replace it. Native Start requires attached, open control and completed
setup, in addition to the still-unissued workload NativeAdmission. Cleanup closes
work admission and checks the shared raw cooperative ceiling at every pre/post
boundary, even without a delivered stop mailbox. Each release uses the shared
serial cleanup gate; an unresolved pending call blocks cleanup and stays pending.

Raw returns are owned before post-clock checking, and successful CUDA/OS releases
remain distinct from validation failures. `cleanup_observation().result` retains
the actual Driver result; `teardown_ledger()` separately exposes per-call clocks,
attempted state, raw CUDA/library outcomes, first validation fault, pending state
and rejected/overflow evidence. A returned success without observed releases is
a protocol failure. Callback unwind retains raw and pending evidence; the
producer and actual backend remain one-shot and perform no implicit native Drop.

Six added CPU regressions cover every release/final boundary, original exit and
cooperative deadline equality, late shared stop without mailbox delivery, every
native/library failure, silent Driver success, mismatch/unwind, pending/rebind
rejection and no retry. The actual CUDA Driver forwarding test uses typed ABI
fakes only. Current source-bound verification is in ROADMAP.md. Worker executable
composition, complete parent facts/consent, native admission, aggregate raw export
and real process/native qualification remain open; no live run was performed.

### Worker teardown transfer in RoleFinished — 2026-11-08

The worker now snapshots the producer's fixed eight-slot teardown ledger into the
existing bounded `RoleFinished` control frame. The canonical `teardown` body
retains ordered operations, raw CUDA/library returns, optional before/after
anchors, pending state, the first ledger fault, and overflow/rejected-return
details. The frame reader bounds the entry count, checks operation indices and
anchor chronology, and requires a reported successful worker cleanup to contain
the seven ordered CUDA releases plus the optional library release, with successful
raw statuses. The parent control audit applies that check to the retained worker
terminal packet.

The worker-aware workload raw exporter retains the complete control packet bytes.
The finalizer now also emits the decoded `RoleFinished` teardown snapshot as a
separate canonical owner artifact, with a domain-separated digest and hex bytes.
It requires at most one decoded terminal teardown; duplicate/missing terminal
evidence is marked unavailable while the raw packet and control-audit fault remain
available. The retained `worker_setup` artifact contains the canonical CUDA setup
ledger, so setup and teardown call evidence are both represented in the aggregate
raw stream when the finalizer path is called. The `starting_handoff` artifact is
the canonical completed starting response; its `native_calls`, `native_fault`,
overflow and rejected-return fields already carry the bounded NVML call and
shutdown ledger. The finalizer does not need another NVML owner transfer. These
are child-reported facts: a return still cannot establish GPU quiescence or OS
process exit. The fixed parent
does not yet call the finalizer/archive composition; no Windows process, driver,
GPU, or runtime execution was performed. Local checks are limited to formatting
and warning-denied all-target/all-feature Clippy for Windows GNU and Linux; no
tests were run.

### Worker preparation, Ready and encoded Start — 2026-11-04

`worker_runtime::Session` composes the original bootstrap Gate and exact frozen
Shared setup deadline with producer preparation, Ready handoff and one retained
encoded Start. Ready and dispatch/phase/failure reports use the same producer
Channel, preserving one ordinary output sequence. A failed Ready send retains
its exact frame and message, closes admission and cannot be retried. Queue-return
and later setup polls retain raw observations in a bounded prepaid EntryClock;
a late queue return cannot become successful setup completion.

`Producer::prepare_retained` preserves the original driver and clock/control
callback owners on recoverable failure. The legacy prepare API delegates to it;
the new composition uses the retained failure. Setup's final observed anchor is
inherited into the producer's existing original-epoch validator. Parent-issued
Start may precede the worker's post-Ready queue receipt while still following
Ready itself. Gate checks that peer chronology, and producer Start validates it
against the original epoch without rewinding the local continuous clock. Start
receipt must still follow the last worker observation. This is not a fresh epoch
or a renewed setup/warm-up allowance.

The opt-in Windows x64 `worker_native::prepare` composes the actual observed CUDA
loader/setup and this Session. Before loading, it checks worker role, full shared
binding and the exact bootstrap deadline. The initial clock must continue from
bootstrap receipt. Its outcome retains the setup ledger and either the prepared
Session/facts, original SetupFailure, or original failed-preparation owner and
callbacks. CUDA setup already verifies actual context-device equality and exact
7.5 capability before supplying the Ready facts. This API needs separate native
execution authorization and has not been executed; no native-admission issuer
or executable main is introduced.

Nine CPU regressions exercise real codecs and an injected driver, including
Ready/report sequence continuity, racing Start timestamps, raw late receipts,
invalid/duplicate Start, stop/clock failures, original-owner/budget retention and
rejection of the fixture entry for native provenance. The actual native factory
is compile/link only. The receive/drain continuation below now permits finite
query/readback only for the existing submitted dispatch under a known original
stop window. Unknown/invalid control still freezes native progression with
retained uncertainty. Actual fixed executable adoption and the complete control/
finalization event loop remain unfinished, alongside aggregate export,
parent required facts/consent, workload admission and real qualification. Exact
source-bound verification and publication are recorded in ROADMAP.md.

### Original shared stop/drain and concrete receive — 2026-11-04

Shared retains the earliest actual trigger in original-epoch elapsed nanoseconds,
independently of the raw cooperative cap. `stop_window` checks the full worker
binding and derives the unchanged approved cooperative/exit windows from that
trigger. A cap clipped at run end never invents an earlier trigger. Later events
can tighten the trigger/cap, never renew them; unknown or failed control remains
explicit and cannot admit drain.

Producer reads that window directly from Shared while retaining the injected
control path. Eight prepaid inline history entries preserve observed earlier
windows and their actual receipt anchors; a separate overflow owner stops work
without eviction. With a known original window, Session permits the one existing
dispatch's finite query/readback/full check/hash under its unchanged stream,
cycle, phase and cooperative limits. No next dispatch starts. Terminal stop and
teardown retain original exit deadlines. Unknown control or uncertain native
returns still prohibit successors and retain unresolved owners.

CallTrace now brackets each native host call with the same serial Shared gate.
Normal entry uses begin; only existing-dispatch query/readback may use begin_drain
after stop and before the original cap. Raw return precedes gate completion and
post-clock observation. StopAck pending therefore describes the host-call lane;
stream pending separately describes submitted work. A NOT_READY return clears
the returned host call while retaining stream pending. Mismatched/no returns
never clear unknown pending or authorize replacement work. This bookkeeping does
not prove physical GPU entry order, cancellation or quiescence.

Session also owns one ordinary input decoder and a concrete Windows poll_native
bridge. Raw packets, idle/missing/regressing clock observations, OS failure and
EOF stay distinct. Each receive advances the producer's original continuous
clock; idle equality expires original setup. A bracketed Start receipt already
observed during reception is consumed once, not rejected as a duplicate
observation of overlapping brackets. The Windows adapter is compile/link only.

Thirteen new CPU regressions cover original/clipped/nonzero-epoch windows, finite
pending/completed drain, expiry without a query or new grace, tighter-history
retention/overflow, encoded receive/Start, clock/OS/EOF failures, and separate host/
stream pending with rejected unmatched returns. Receive history is prepaid from
the frozen run envelope for two observations per control tick; a complete
simulated original run fits without growth and still expires at the original
end. Review corrected the initial setup-only history capacity before the final
manifest. Source-bound final verification
is in ROADMAP.md. Fixed executable/Context/control/native preparation composition,
final output/control/I/O drain, aggregate live export, complete parent facts and
consent, workload admission issuance and real qualification remain unfinished.
No real native, pipe/process or GPU execution was performed. Actions remains
removed and disabled; verification is local.

### Worker entry and original-bound finalization — 2026-11-04

The opt-in Windows x64 `worker_entry::entry` composes one fixed-role Context
adoption from parsed Arguments, independent split control, frozen setup,
observed CUDA preparation, Ready/encoded Start, producer execution/failure-vector
handoff, explicit teardown and bounded finalization. Its caller must supply an
admission callback returning the still-unissued NativeAdmission. No executable
main invokes this entry. Exact native setup consent and complete required
pre-setup authority are unsafe caller prerequisites; the later Start callback
cannot retroactively authorize setup. A fixed main and real qualification remain
unfinished, alongside the parent facts/consent/admission chain.

Outcome retains original Context and native/preparation failure owners, startup
history, raw split/sender/event-clone causes, admission rejection, surplus packet,
control handle/report and finalization/I/O observations. A sticky ClockChain
validates every main-thread capture across entry/setup/producer/finalization and
retains the first bad anchor or missing-clock condition. Cloning WindowsClock
copies its immutable token/frequency, without a new frequency query or epoch.
Valid raw anchors remain in their respective retained stage ledgers.

`worker_finish::State` uses the already frozen producer exit deadline clipped by
the independently published Shared cooperative cap. Failure paths require an
existing original stop window; no fallback now-plus-grace is created. Missing
original deadline stays distinct from a missing clock. Native cleanup's actual
return and teardown validation remain separate from a completion command's
queue result, local output drain, control-thread finish and actual I/O-thread
finish. None proves peer receipt, durability, process/job exit or GPU quiescence.
A failed cancellation request is retained even when the I/O thread is later
observed finished; completion is not attributed to that request. Late thread
completion remains supplemental and cannot clear a timeout. Unknown native host
pending or missing/invalid cleanup prevents successful completion.

Output/control drain and the subsequent I/O phase share the original deadline;
neither starts a new window. I/O cancellation is requested explicitly once by
this entry, then `io_completion` only observes retained JoinHandles, without
reissuing cancellation or joining an unfinished thread. Failed/unresolved I/O
owners remain in the returned Context. Exit code zero requires the full producer
report count, successful cleanup/finalization and an accepted control report;
it remains child completion only, not native qualification or baseline acceptance.

Nine CPU regressions cover these contracts with actual injected-producer cleanup,
raw failures, deadline equality, late completion, ordering, pending, resource and
clock cases. A feature-library test forces code generation/linking of the full
generic entry by taking its function pointer through black_box; the pointer is
never called. Actual entry/threads/pipes/DLL/GPU execution has not been tested.
The original fake-process matrix remains separately unapproved.

The November 5 continuation below adds disposition of successful preparation
before Start. Partial or failed CUDA setup remains opaque and retained. The
complete pre-setup parent facts/consent chain, fresh safety/re-entry/Start and
workload admission issuance, fixed main, bounded aggregate raw export and real
qualification remain open.
Exact source-bound verification and publication are in ROADMAP.md. Hosted Actions
stays removed and disabled; all current verification is local.

### Prepared-before-Start resource disposition — 2026-11-05

Session cleanup now handles an idle prepared owner stopped or denied before
Start. A prepaid inline `prestart::Report` retains the exact binding, raw clock
seed, original Shared stop window, summary and eight per-call teardown entries.
There is no Start fabrication or compute admission. A prior Start rejection
remains the first terminal fault; a failed Session's unknown control cannot be
reopened by a later valid stop observation. One-shot
bookkeeping is set before callbacks; retry cannot enter another release call.

The same engine handles successful observed CUDA setup followed by failed CPU
Session preparation, using the returned driver/clock owners and last setup
anchor. The actual native branch is compile/link only. Failed or partial CUDA
setup is deliberately excluded: nonzero/uncertain outputs do not establish valid
resource tokens or safe destruction. Those owners remain retained and unresolved.

Release entry requires the exact full Shared binding, known original trigger,
closed admission, no pending host call, valid continuous original clock and time
strictly below the original cooperative/exit/run ceilings. No current-time grace
or allocation is introduced. Unknown control, deadline equality, clock failure
or uncertain pending prevents release; native and library failures retain their
raw returns and prohibit successors/retry. Released host handles and DLL returns
do not prove GPU quiescence or physical memory release.

Six CPU regressions cover stop before/after Ready, denied/rejected encoded Start,
first-fault retention, absent/expired/failed control, pending calls, every release
failure, exhausted prepaid budget, no retry/compute and earliest original exit.
Source-bound checks/publication are recorded in ROADMAP.md. No native loader,
entry, process/pipe, GPU execution or hardware qualification was performed.

### Pre-manifest host environment — 2026-11-05

`host_environment` captures a separate `pilot_host_environment` v1 raw artifact
under the existing starting Scope, without an invented manifest/run. It records
five ordered host-only observation groups: OS version/build/platform, native and
process architecture, current parent process-token elevation, system power
status and active power-scheme GUID. The GUID is encoded in field order
(`data1/data2/data3` big-endian followed by `data4`), independently of host layout.
BOOL/GetLastError, signed NTSTATUS, direct Win32 status, query output length and
explicit token/local-allocation cleanup outcomes remain distinct. Unknown
battery values are retained, never converted into measured percentages/durations.
Architecture records the successful IsWow64Process2 process/native-machine pair
and compile target separately. The native AMD64/non-WOW64/x64 screen does not
infer native architecture from GetNativeSystemInfo's emulation view.

Before/after anchors use the original clock and strict 10 s preflight boundary;
deadline equality, missing/regressing/mismatched clocks or cancellation retain
the current raw return and prevent successor reads. All owned/model/codec space
is reserved before read one. The 16 KiB artifact has a domain-separated digest
and strict bounded canonical reader, including typed raw returns on protocol
failure. A reported source label authenticates no OS call. Fixture collection is
always labelled simulated; matching hashes cannot issue workload admission.

Owner assertions about known background load/competing tuners retain their own
source, reference and raw assertion anchor. They are not sensor reads or human
workload consent. Missing/competing assertions, unreadable OS/power facts,
elevation, unsupported architecture or unsuccessful OS-resource cleanup prevent
the host-facts check. This check does not prove exclusive GPU use or stable
conditions throughout a run. Declared child cache settings derive from the same
constant as the launcher's exact block; they are not effective child readback.

`Reported::freeze_manifest` composes exact host and starting-setting artifact
reference/readability checks under the same Scope. `ParentSampler` retains a
one-shot environment digest plus exact host-check result, validates the original
run binding and rejects preflight observations later than the run origin. A
failed or duplicate check closes the existing parent path while preserving its
first failure/stop. This is still reported evidence; native admission remains
unavailable. Driver/DLL/runtime versions/builds, effective child options, actual
peer/image/process facts, pre-setup and workload consent, fresh safety/re-entry/
Start, aggregate export and real qualification remain separately required.

`host_environment_windows::capture` supplies the actual host-only source using
the existing WindowsClock. It preserves the first raw capture error and any
failed OS cleanup owners in `Captured`, with no implicit retry or native Drop.
The caller must run collection off its deadline/control thread and retain that
owner even when snapshot creation fails. No main invokes it in this milestone.
The Windows function is monomorphized/linked through a non-invoked function
pointer; twelve CPU integration regressions exercise the portable engine,
strict reader, failure/stop/budget paths and parent composition. Actual host API,
process/pipe, vendor DLL and GPU invocation were not performed for this slice.

ABI/ownership sources reviewed for the adapter: [RtlGetVersion](https://learn.microsoft.com/en-us/windows/win32/devnotes/rtlgetversion),
[IsWow64Process2](https://learn.microsoft.com/en-us/windows/win32/api/wow64apiset/nf-wow64apiset-iswow64process2),
[GetTokenInformation](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-gettokeninformation),
[GetSystemPowerStatus](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getsystempowerstatus),
[PowerGetActiveScheme](https://learn.microsoft.com/en-us/windows/win32/api/powersetting/nf-powersetting-powergetactivescheme)
and [LocalFree](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-localfree).
Exact source-bound verification/publication is recorded in ROADMAP.md. Actions
remains removed/disabled; current checks are local.

### Retained CUDA DLL file evidence — 2026-11-05

Before loading the fixed system `nvcuda.dll`, the opt-in CUDA setup now opens and
retains a read handle shared only for reads, then hashes the complete file with
64 KiB scratch. `autotune_probe::runtime_file` retains initial/final file identity,
size, write time, attributes/type, read count, last raw I/O return, partial byte
progress and plain SHA-256 of the file bytes. Empty, directory/reparse/non-disk,
oversized, truncated, growing, changed or unreadable input cannot complete.
Fragmented reads have a fixed call ceiling; failure/unwind cannot retry the same
report or turn a partial digest into a successful file observation.

The file-observation implementation bounds are 128 MiB input, 4097 read calls
(including the EOF check) and 4096 UTF-16 path-buffer units. These are fail-closed
input/metadata capacities, not extra workload/session time, output quota or
changes to the approved qualification profile. The worker prepays file-check
history plus 512 KiB for bounded path/scratch work before native loading. File
checks preserve raw stages/anchors in their own bounded history; overflow has a
separate retained slot and cannot evict old evidence or allocate more storage.

The actual setup observer checks the same continuous original clock, frozen
setup deadline, Shared stop and pending state at each file/loader checkpoint.
File I/O does not acquire compute admission or pretend to be a CUDA host call.
Missing/bad clocks, stop, deadline equality, setup completion or an unknown
pending call prevents successors. A blocked OS read/loader can still outlive a
check; independent supervision remains necessary and no new grace is granted.

After load, setup retains the library before the post-load callback, including
the original libloading error on a failed load. The reported module path is
opened and its volume/file index, size, write time, attributes and type must match
the measured retained file; the existing trusted canonical-path check remains.
These fields follow [GetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle).
Read sharing/locking follows [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
and the module path follows [GetModuleFileNameW](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulefilenamew).
`FILE_TYPE_UNKNOWN` retains its observed GetLastError value (including zero)
under [GetFileType](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfiletype);
it never qualifies as a disk file. A truncated system-directory query retains
its required length, without inventing an error code that the API does not
specify for that case.

The live backend and every setup-failure owner retain file facts and the read
handle. A read-only producer accessor exposes those facts after Session
preparation/explicit teardown as well. Typed CUDA fakes carry no file owner and
cannot supply DLL evidence. The ordinary file handle lives with its owner; no
new vendor teardown/retry or NativeAdmission issuer is introduced.

This records file bytes and an OS-reported module-path/file relationship. It is
not a hash of mapped image pages, attestation of an already loaded image,
authentication of a remote claim, consent or runtime qualification. The NVML
continuation below connects its observed loader to the same file owner. Complete runtime/build/version and
effective child-option evidence, parent references/peer/consent admission,
bounded aggregate raw export and real qualification remain open. No fixed main,
native DLL/file observation, process/pipe or GPU execution was performed here.

Eight portable file-engine regressions and four actual observer/typed-CUDA
regressions cover golden hashes, all read/guard failures, metadata/file mismatch,
bounded fragmentation, raw-return retention across unwind, original deadlines,
stop/pending, fixed history overflow and exhausted prepaid budget. Initial
unwind-test counter expectation was corrected to compare the existing shared
fake counter baseline; no CUDA call had occurred. Exact source-bound checks and
publication are recorded in ROADMAP.md. Actions remains removed/disabled.

### Observed NVML DLL file and failed-start ownership — 2026-11-05

`NvmlSampler::open_observed` now uses the shared fixed-system-file measurement
before loading `nvml.dll`, retaining the full file hash/identity and validating
the loaded module's reported path/file relationship. It uses the same bounded
scratch, read/path limits and restricted search flags as the CUDA continuation.
The legacy unobserved probe/open APIs keep their historical behavior and do not
gain file proof from their path/version strings.

The observed path returns original pre-init symbols/library/file ownership on
stopped startup, missing init/shutdown exports and failed init. Loader errors
and the actual init status are retained separately before post-call observation.
Failed init does not establish an initialized session or permit shutdown/retry.
A successful init whose observer rejects its return still retains the initialized
session for supervised cleanup. ManuallyDrop locals prevent observer unwind
from silently invoking shutdown or unloading the retained library. Ordinary
caller Drop remains a possible release operation; the benchmark entries keep
these owners wrapped and release only through their existing supervised path.

The NVML ledger prepays file-check history and 512 KiB path/scratch allowance
before loading, checks its original continuous clock/deadline and stopped lane,
and preserves fixed overflow/first fault. A file checkpoint cannot interleave an
unreturned NVML ABI call. It neither enters nor clears the outer startup/read
group's pending state. Observer unwind closes admission without clearing unknown
pending. A nonzero ABI status is now made sticky before its post-clock callback,
so a later unwind cannot replace the already retained native failure.

`ReleaseFacts` transfers the file owner, original loader error and init status
across the entries' explicit native-owner release. Both pre-manifest and pilot
sampler Outcomes expose them afterward; repeating release cannot erase them.
These transferred facts are not an observation that FreeLibrary succeeded, an
empty-job result, GPU quiescence or mapped-page attestation.

Five ABI/ownership-fake regressions cover pre-init stop/missing exports, failed
init, successful-init post-return rejection, observer unwind and sticky file
stop. Five portable ledger regressions cover full prepaid memory pressure,
continuous original time, outer pending, bad clocks/expiry/stop, unreturned ABI,
fixed overflow and first-native-fault retention across unwind. Fakes supply no
DLL file proof. Full source-bound verification/publication is in ROADMAP.md.
Actual filesystem/DLL loading, entry/threads/process/pipe/GPU execution and
hardware qualification were not performed.

The continuation below exports those facts in the starting response. A complete
bound runtime/environment bundle and bounded aggregate export across roles still
need composition, together with effective child options, parent peer/image/consent
facts, fresh safety/re-entry/Start and workload admission. Application-ledger
reservations do not qualify OS commit or aggregate metadata/session limits.
No workload issuer/main or release-gate completion follows; E3 remains partial.

## Starting runtime response v2 — 2026-11-05

The starting entry now exports a version 2 canonical response with retained
NVML file, loader and initialization results and the full file-check clock
history, including its fixed overflow observation. File facts include requested
and loaded paths encoded as exact UTF-16LE hex, before/after and loaded-path
identities, streamed SHA-256, partial read/error facts and held/bound assertions.
No filesystem or driver operation is needed to detach these retained facts.

The parent reader accepts exact closed schemas for v1 and v2. Legacy v1 bytes
and their meaning remain unchanged and expose no runtime section. V2 requires
its explicit runtime section, retaining missing, failed and bad-clock facts
without inventing success. Owned chunk delivery recognizes both versions and
still binds the original scope, declaration and payload digest. The canonical
cap remains 2 MiB; a full file history fits, while combined histories exceeding
that cap fail explicitly without truncating either ledger. Codec reservations
remain unchanged; the typed v2 receipt additionally reserves 2 MiB. The reader's
bounded structure count accommodates the full 8218-check transcript.

Recorded fixtures are labeled simulated. The separate runtime assessment checks
reported file/identity/path/read and original-clock/deadline consistency; even a
consistent native-source claim authenticates no read, peer, consent or native
admission. Eleven CPU regressions cover version separation, strict schema and
integer widths, raw failure retention, lone UTF-16 surrogates, transcript/clock
rejection, maximum history, aggregate cap failure, budget release and owned
delivery. Final source-bound local evidence is in ROADMAP.md. No actual file/DLL,
entry/threads/process/pipe/GPU execution or qualification was performed.

## Child CUDA environment and module-loading readback — 2026-11-05

The fixed worker environment now requests both `CUDA_CACHE_DISABLE=1` and
`CUDA_MODULE_LOADING=EAGER`. The host artifact derives its declaration from the
same block. Before opening the driver DLL, the actual opt-in setup reads each
variable once from its own process using bounded initialized UTF-16 scratch and
GetEnvironmentVariableW. Missing, empty, oversized, nonexact and failed returns
block setup without a retry or a DLL load. Raw lengths, error codes and scratch
are retained before the post-clock callback, including on rejection or unwind.
Undefined/partial failure buffers never supply an accepted value. No environment
variable is changed by readback and no cache is deleted.

After successful cuInit and driver-version observation, setup calls the exact
`cuModuleGetLoadingMode` export and requires EAGER (1). Missing export, failed
status, LAZY (2) or unknown raw mode prevents enumeration/context/module setup;
there is no fallback. The output is retained as an integer, without constructing
a Rust enum from unknown driver memory. SetupFailure and CudaBackend expose
original environment and mode facts alongside the existing file/version owners.
The original supervised setup clock/stop/deadline spans host reads, file loading
and CUDA calls. The ledger prepays two host entries and one extra driver entry;
this changes implementation capacity, not any approved workload/time ceiling.

The getter ABI and numeric modes were checked against the [CUDA 12.9.1 module
reference](https://docs.nvidia.com/cuda/archive/12.9.1/cuda-driver-api/group__CUDA__MODULE.html)
and the existing pinned cudarc 0.17.3 declaration source. The [Win32 readback
reference](https://learn.microsoft.com/en-us/windows/win32/api/processenv/nf-processenv-getenvironmentvariablew)
defines the calling-process scope and oversized/failure semantics. These reads
and CUDA's mode report cannot prove absence of all driver filesystem activity,
JIT cost, exclusive GPU use or hardware qualification. They grant no consent.

Seven new CPU/typed-ABI regressions cover exact values, failures and unknowns,
pre/post rejection, raw retention across unwind, original deadline/stop and full
prepaid memory pressure. Existing every-call failure and clock-boundary tests now
include the mode query. Final source-bound verification/publication is recorded
in ROADMAP.md. No actual environment API, DLL, process, pipe, thread or GPU
execution was performed. The following continuation exports these retained facts;
transport, acceptance assessment and parent composition with host/starting
artifacts before Ready/Start admission remain open. E3 partial/Gate B unmet.

## Retained worker setup artifact and reader — 2026-11-05

`worker_setup::Artifact` now serializes one retained Prepared outcome: CUDA setup
failed, CPU preparation failed, or ready. It binds the exact WorkerBinding and
original raw setup deadline, and preserves environment/mode/version values,
native/CPU failures, file/loader facts, every setup entry/return, pending state,
first ledger fault, rejected returns, and full file-clock/overflow history.
Portable setup types preserve the CUDA-facing aliases and raw integer ABI.
The file/path codec is shared with the starting response; its v1/v2 bytes and
semantics are unchanged. Exact UTF-16 and unknown status/clock facts stay raw.

The strict structural parent reader checks canonicality, the delivered RawArtifact
digest, exact run binding/deadline, closed schemas/enums, numeric widths and
bounded paths/histories. Bad or missing per-call clocks and failed/contradictory
reported facts remain readable evidence; parsing does not assess native setup
acceptance. Fixture construction always labels simulated. A native-source label
authenticates no observation, ownership or consent, and there is no admission
issuer or qualification result in the codec.

The canonical artifact cap is 2 MiB. Codec storage is prepaid at 16 MiB and
retained canonical/typed storage at 4 MiB plus owner size. Oversized reports fail
explicitly, without truncating their original ledgers. The tested maximum-count
fixture fits both 149 setup entries and all 8218 file checks, with overflow.
Application reservations are not an OS-commit or aggregate metadata proof.

The actual opt-in worker entry freezes and retains this artifact before Ready.
Failure to build it stops that path while preserving the original Prepared owner
and the typed export error. Its future admission callback now receives both the
session and artifact. This entry is compiled/linked through a non-invoked pointer;
no executable calls it. Seven CPU regressions cover all operation/output shapes,
partial file/native/CPU failures, exact binding/deadline/digest rejection,
malformed schema/widths/unions, maximum histories, budget release and untrusted
source/clock claims. Source-bound final verification/publication: ROADMAP.md.

The continuation below supplies chunk transport and parent retention; setup
acceptance assessment and controller/main composition remain open. No executed
pipe delivery is claimed. Early startup/prepare failures
before a Prepared ledger exists remain separately retained and need aggregate
export. Complete host/starting/runtime references, peer/image/process/consent,
fresh safety/re-entry/Start, native admission/main, cross-role resource limits
and real qualification remain open. No actual environment API, file/DLL, entry,
thread/process/pipe/GPU invocation or hardware qualification was performed.

## Worker setup delivery before Ready — 2026-11-05

The worker-only `setup_chunk` message carries bounded hex segments on the existing
ordinary FromWorker channel. Each segment binds the artifact digest, index,
count and total bytes. Raw payload segments are at most 16 KiB, with at most
128 segments under the unchanged 2 MiB artifact cap; encoded frames remain under
the existing 64 KiB limit. Sampler roles, control lanes, invalid widths/hex and
inconsistent segment geometry reject the message. Other message bytes and the
read-only sampler protocol are unchanged.

`Session::send_setup` makes one attempt, checks exact artifact/run/original
deadline and unfinished setup, prepays scratch, and observes the original setup
clock/control gate before and after every handoff. It uses the producer's existing
encoder and failed-frame owner. Progress retains the artifact digest and handed
off segment count, including post-handoff expiry. Failure closes the existing
path; no retry, reconnect, renewed deadline or subsequent Ready is permitted.
The actual opt-in entry calls this before announce. Original artifact and
Prepared owners remain retained. Queue acceptance is not pipe completion.

The parent Receiver consumes the existing owned RawReceived packets. One decoder
checks the complete chunk sequence, then Ready. Before Ready it requires every
byte, matching digest and successful strict worker_setup::Receipt reading against
its frozen run and original raw deadline. Missing/bad/regressing clocks, deadline
equality, stop, transport failure, gaps, duplicates, transfer replacement, corrupt
payloads or an early Ready close reception. Partial/full payload bytes, clock
history, per-frame digest/receipt observations and the first rejected packet or
transport cause remain retained. The constructor prepays bounded payload/history
storage; parent receipt allocation failure is explicit and cannot expose Ready.

`ReadyDelivery` retains the strict receipt, Ready owner and observations and hands
off the same decoder for later phase/dispatch messages. This is a delivery state,
not setup acceptance, authenticated peer evidence, consent or native admission.
The existing NativeAdmission still has no issuer or fixed main.

Sixteen CPU regressions exercise the actual byte codecs/framer, parent receiver,
Session, injected driver and common sequence through Ready/fixture dispatch;
they include first-frame queue failure, late return, prequeue stop/full budget,
run/deadline/completion mismatch, malformed protocol, hash/schema corruption,
partial reception, clocks, role/lane rejection and parent allocation failure.
Initial mutation fixtures used the wrong total-length prefix and were corrected;
a redundant constant assertion was removed after warning-denied Clippy. Final
source-bound verification/publication: ROADMAP.md. No real pipe, process, thread,
DLL/environment API or GPU execution, independent-agent review or qualification.

The continuation below adds reported setup consistency checks; complete parent
host/starting/runtime/peer/image/process/consent/safety composition remains next.
Failed/early preparation without a ready Session still retains its artifact/
owners and needs separate aggregate failure export. Aggregate metadata and OS
resources remain unqualified; byte/ledger reservations do not close those gates.
E3 remains partial and Gate B unmet; the full automatic optimization goal remains.

## Reported worker setup assessment — 2026-11-05

`worker_setup_assessment::screen` now checks the immutable strict receipt against
its exact parent binding and Ready/delivery observations. It rejects simulated
source, unfinished/failed preparation, pending/overflow/rejected entries, loader
or native/CPU failures, missing calls and every nonzero status. Rejections retain
a typed cause and report/file/call/Ready/arrival site. Raw receipt bytes, return
values and unknowns remain available; native status precedes a later post-clock
failure rather than being overwritten by it.

The screen requires the complete ordered environment/file/CUDA setup sequence,
matching raw environment readback, API version and EAGER mode, bounded readable
device enumeration and one exact target UUID/PCI match. Device handles and UUIDs
must be readable and unique; reordered ordinals remain valid. Target attributes,
context device, nonnull resource returns, planned allocation/headroom checks,
all four buffers and final stream creation must agree. Extra calls are rejected.
The shared file consistency check is reused without changing starting-response
bytes or NVML assessment semantics. CUDA adds fixed DLL basename/path-join and
logical read-count checks; hash/identity/held assertions still authenticate no file.

The original clock/deadline spans both environment reads, all file checkpoints,
every ABI entry/return and Ready. The parent receive timeline is validated as a
separate branch from completed setup: reception can overlap the worker's Ready
capture, but cannot precede completed setup, and final Ready receipt must follow
the Ready capture. No phase deadline or grace window is recreated.

`ReadyDelivery::assess_setup` retains one result and the exact receipt. Rejection
poisons its ordinary decoder without losing evidence; independent stop/control
remains the parent's responsibility. Successful Summary contains reported facts
and the exact receipt digest, with no admission or qualification authority. Even
a consistent native-source claim remains untrusted until peer/image/process and
separate consent evidence are established. Cache storage is prepaid; screening
uses bounded stack state and creates no new heap model.

Eleven CPU regressions cover claimed-native versus simulated provenance, all
sequence/return/clock failures, environment/mode/version, mapping/reordering,
capability/context/memory, file lifecycle/path/read counts, causal timing, binding,
full ledger pressure and retained delivery assessment/decoder poisoning. A test
initially held redundant artifacts across the 32 MiB limit; it was corrected to
reuse the same artifact, without increasing the quota. Source-bound final local
verification/publication: ROADMAP.md. No independent-agent review, actual OS/
environment API/DLL/process/pipe/thread/GPU invocation or hardware qualification.
Complete parent fact composition, fresh safety/consent, qualified native issuance,
aggregate resources/export, mains and real qualification remain open. E3 partial,
Gate B unmet; full automatic benchmark/search/validation goal unchanged.

## Reported parent context references — 2026-11-05

`ParentSampler::check_context` composes nine independent reported checks once:
exact manifest/run/worker binding, shared capture Scope, host facts, original
settings, starting completion/ABI flags and causal boundaries, NVML file/runtime
assessment, cross-role versions/path, retained worker setup assessment and current
settings equality. The retained Record includes all outcomes and exact artifact
references, including sampler Ready and current-response raw-frame digests.
Missing and contradictory facts remain distinct. Rejection faults the parent and
poisons ordinary worker decoding; a repeated check keeps the first Record and
rejects the successor. Independent stop/control remains the caller's obligation.

The strict starting reader now retains the validated SamplerInfo and complete
Runtime fields under an additional reservation of twice its JSON length plus
64 KiB. Response v1/v2 bytes and numeric limits are unchanged. Derived flags
require successful ordered discovery/version ABI entries and retained lifecycle
completion flags; versions must agree with raw NVML results and later sampler/
worker reports. The screen compares exact requested NVML UTF-16 path bytes.
These checks authenticate no OS observation, process identity or human consent.
Lifecycle flags and selected causal boundaries do not qualify the entire trace.
Original artifact owners must remain retained/exported by the caller.

Eight CPU regressions cover consistent claimed-native reports, missing/conflicting
versions, failed completion, missing ABI history, changed settings, aliased Scope,
wrong run, duplicate checks and prepaid storage under full ledger pressure.
Initial fixture/API/import/lint failures and two oversized late reader attempts
remain in historical logs. Constructing negative input in the pre-parent phase
fixed those attempts without raising the 32 MiB quota. Final source-bound local
checks/publication are recorded in ROADMAP.md. No independent-agent review or
actual OS/file/DLL/thread/process/pipe/GPU invocation/qualification is claimed.
Native admission remains unavailable. Peer/image/process evidence, separate
pre-setup/workload consent, fresh safety/re-entry/Start, aggregate export/resources,
fixed mains and real qualification remain open; E3 partial and Gate B unmet.

### Ordered parent preflight composition — 2026-11-08

`ParentSampler::verify_reported_preflight` now binds the existing one-shot checks
into one ordered call: host environment, starting settings, cross-artifact context,
all three local process/session peers, then setup/workload consent. The input retains
the exact `BoundRun`, scope, environment and starting report owners, consent records,
and process/session/setup references. Each rejection returns unconsumed consent
decisions where applicable, closes the setup reference path and leaves detailed
check results with `ParentSampler`.

This is only the reported preflight boundary. It does not start or wait for the
controller, queue Start or NativeGrant, launch processes, or establish that child
reports are true. `ParentSampler::queue_start` still requires its attached watch to
be ready, and NativeGrant still needs fresh process/safety/re-entry checks. The
fixed parent session and runtime qualification remain open. Formatting and
warning-denied all-target/all-feature Clippy pass for Windows GNU/Linux; no tests or
process/native/GPU execution were run.

## Reported starting timeline assessment — 2026-11-05

The strict reader now retains `Summary::lifecycle_timing` independently from raw
completion flags. It returns the final drain anchor or a typed missing/clock/
order/deadline/window/native/settings rejection. Failed or incomplete reports
remain readable with unchanged canonical bytes. The parent context requires this
assessment and a final drain no later than the original run origin.

The reported serial timeline binds bootstrap receipt to Open, Describe and Read,
the original stop trigger, Cleanup, completion, its separate control receipt and
final drain. Work/stop must precede the original preflight deadline. Both trace
and control must reference the same stop trigger; cooperative/exit times must
exactly derive from the frozen profile, with no renewed grace. Cleanup,
completion, receipt and drain must precede that cooperative deadline; equality
expires. Each branch uses the existing original-epoch clock policy.

NVML discovery and file checkpoints must fit inside Open, version queries inside
Describe, and the exact starting snapshot entry and all getter pairs inside Read.
This assesses exported reported boundaries, not omitted polling history, OS
observation authenticity, actual cleanup, process exit or GPU quiescence. It
creates no native admission or new heap model. Response v1/v2 bytes and resource/
time limits remain unchanged. The successful fixture now includes the producer's
required stop window and drain facts rather than relying on completion booleans.

Three new regressions cover 15 malformed timeline variants, valid original
cleanup/drain facts and a drain after run origin. Raw rejected reports remain
available; the existing full-budget parent regression still applies. An initial
test used a private object builder and was corrected to the public Object API;
its failed compile log remains historical. Final source-bound verification and
publication are in ROADMAP.md. No independent-agent review or actual OS/DLL/
thread/process/pipe/GPU execution/qualification. Peer/image/process composition,
separate consent, fresh safety/re-entry/Start, native issuance/mains, aggregate
export/resources and real qualification remain open. E3 partial/Gate B unmet.

## Local launcher/channel provenance — 2026-11-05

The fixed Windows launcher now reads PID from its retained process handle after
job assignment and creation-time observation, before resume. Zero PID/creation
rejects through the existing retained failure path. A private-construction Origin
binds kind (starting/sampler/worker), exact scope or transport binding, declaration,
measured image, PID and creation time. The locked image and original process/job/
transport owners remain with Process. Later failed launch retains any issued
origin alongside its image/creation/error/handle facts.

Process::receive attaches this local Origin to raw packets. Public raw-byte
constructors and decoders cannot manufacture it from matching JSON, hashes or
native-source labels. Setup delivery retains the origin of every chunk and Ready.
ParentSampler tracks all received origins, binds for_process to one exact process
and rejects replacement or unstamped packets once bound. Starting Session tracks
each accepted chunk through its concrete process poll; public borrowed-byte
acceptance marks provenance missing. Missing/mixed origins remain sticky even
after subsequent matching packets. Canonical wire bytes are unchanged.

`ParentSampler::check_processes` borrows all three retained Process owners plus
the completed starting Session and worker setup. It retains one record comparing
all received origins to the exact declarations, scope/run/image bindings and
distinct PIDs, current cancellation state, starting-response digest and the setup
digest already screened by context. It retains independent per-role outcomes;
failure or duplicate attempts close ordinary paths without replacing the first
record. Parent origin binding then guards subsequent sampler receipt. This
composes local channel provenance, not current process liveness, independent
job/exit observation, mapped-image attestation, DLL truth, human consent, fresh
safety or native admission. Caller must retain the borrowed owners. Native
admission remains unavailable; this record is not a transferable execution token.

Origin/record/arrival storage is prepaid through updated type sizes; Starting
Session now explicitly reserves its own inline state. Cached binding digests are
computed under constructor codec reservations, leaving comparison allocation-free.
No quota or numeric contract changed. Eleven new CPU model tests cover invalid
identity, missing/mixed/changed provenance, cancellation, artifact/binding drift,
role aliasing, public raw input and framing-error retention. A non-invoked pointer
test forces Windows composition codegen/linking. Initial private-access/arrival-
fixture errors and naming/function-pointer lints were corrected; failed logs stay
historical. Final source-bound evidence/publication: ROADMAP.md. No independent-
agent review or actual OS/PID/file/DLL/thread/process/pipe/GPU invocation or
qualification. Fresh process/containment observations, separate pre-setup/workload
consent, fresh safety/re-entry/Start, native issuance/mains, aggregate resources/
export and real qualification remain open; E3 partial/Gate B unmet.

## Separate consent requests and guarded launch — 2026-11-05

`consent::Request` freezes separate StartingRead, PilotSetup and Workload requests.
The read request binds the original Scope/declaration/image; setup binds the exact
worker/sampler declarations, manifest bytes/digest, fixed profile and original
setup window. Workload request construction requires the successful parent
context and local peer record, binding their complete artifact references and
original owners to the exact run/plan. These are requests, never approval inferred
from an approval_ref, digest, source label or Ready message. Human waiting cannot
rebase the original preflight/setup deadline; an expired request must fail.

The concrete Windows console adapter records a deliberately submitted exact
`approve <request digest>` line, or a decline/error, separately from the request.
Only that private capture path can construct a console-source Record; public
fixture recording stays untrusted even with an exact affirmative response.
Record has no Clone/Deserialize or wire reconstruction path. It retains bounded
edited ASCII, raw key fields/ignored event kinds, terminal facts, I/O failures,
capture/policy errors, observed clock identity and rejected anchors. Input is
limited to 96 bytes and 256 events, with a bounded poll count and original clock
checks at the existing controller cadence; expiry/clock faults stay sticky.
This records a local console response, not cryptographic human identity or proof
that the operator independently verified the supplied evidence.

The adapter requires exclusive console input ownership. It holds the Rust stdin
lock and reads one event only after a positive console event count; another
same-account console reader is outside that fixed-controller contract. Output
can block, so the future controller must supervise original deadlines separately
and retain the prompt/thread owner off the deadline path. This is an engineering
qualification adapter, not the E8 desktop approval workflow, observed console
acceptance or permission to invoke it during implementation.

`windows_process::Launch::start` now requires a separately captured Record and an
actual WindowsClock. Before job/event/process construction, it checks exact
kind/binding/declaration/image, current clock identity/time and one unused role
slot. A starting record has one slot; setup has one worker and one sampler slot;
a workload record cannot launch either role. Wrong target, duplicate, clock loss
or expiry closes further use while retaining the first attempts/rejection.
Consumed attempts cannot be retried after a later startup failure. Existing child
startup/native deadlines and independent stop supervision remain mandatory; the
consent check neither renews time nor supplies the other pre-setup prerequisites.

Canonical requests and decision/attempt storage are prepaid; assessment uses
bounded inline state. Eleven new CPU model tests cover exact/changing decisions,
untrusted fixtures, distinct stages/roles, negative I/O/clock/input, expiry/replay,
key editing/repetition, sticky faults and full ledger pressure. Four concrete
request tests cover exact scope/run/profile, role/expiry/scope rejection, missing
peer evidence and capacity retention. Existing non-invoked pointers additionally
force console/guard codegen/linking. Initial private-helper/test-layout and
portability/name-resolution failures were corrected; a missing separate manifest
digest assertion led to an explicit field alongside full bytes. Failed logs stay
historical; source-bound final evidence/publication is in ROADMAP.md.

No terminal/console/clock-API/process/DLL/GPU execution, human approval receipt or
qualification is claimed, and no main invokes capture. NativeAdmission still has
no issuer. Parent consumption of setup/workload receipts, fresh process/job and
safety/re-entry/Start checks, prompt ownership/cancellation/export, aggregate
resources, mains and real qualification remain open; E3 partial/Gate B unmet.

## Parent consent receipt ownership and launch capture binding — 2026-11-05

Fixed launch now carries the exact CaptureRef returned by its consent guard into
the created process Origin and every locally stamped packet. CaptureRef identifies
the request digest and actual response-receipt anchor; it is a historical local
reference, not a digest/authentication of all decision facts or an execution token.
It stays available after later rejection, independently of eligibility. Fixture
records cannot manufacture a concrete capture reference. Origin equality now
includes this reference, so another capture for the same declaration is distinct.
The larger inline receipt remains prepaid; it does not add a per-poll heap owner.

Consent request v2 includes launch capture references in workload peer details.
Its private workload fingerprint retains the exact binding, context references and
three original Origins. Workload request issuance must follow worker readiness,
sampler readiness and current-settings receipt. The context summary now retains
its screened worker Ready anchor. Existing sampler/worker/starting wire bytes are
unchanged; earlier consent request v1 remains historical, not silently reissued.

`ParentSampler::accept_consent` consumes setup/workload Record owners into one
retained Delivery, even on rejection. A duplicate returns the new original owners
and preserves the first assessment. The original starting-read Record is borrowed
as reusable evidence for this unchanged pre-manifest Scope, not another launch;
the session caller must retain it and the original process/artifact owners.
All three independent stage outcomes, capture references and the final observed
anchor remain visible. Rejection closes ordinary worker decoding and faults the
parent. The existing independent stop/control path remains mandatory.

The screen requires concrete interactive valid decisions, exact stages/scope/
epoch/frequency and the actual captures used by each retained launch Origin. Role
slots must have successful original attempts/clock identity, with approval before
launch and the reported first boundary. Setup entry/deadline must equal the
parent's original window; the worker's raw deadline must agree. Workload must
bind the exact current context/Origins, follow all readiness/settings receipts and
remain inside the original setup window at final observation; equality expires.
Independent stage failures are retained. No regeneration of JSON or allocation is
needed while comparing; updated inline type sizes reserve storage up front.

Ten new CPU comparison models, one changed-capture origin case and two concrete
rejection/duplicate ownership cases cover consistent data, retrospective approval,
stage/clock/window/attempt/fingerprint failures, allocation pressure and preserved
owners. The existing private console-source model additionally verifies historical
reference stability after invalidation; it calls no console adapter or OS API.
An initial digit-grouping lint was corrected. Final source-bound verification and
publication are in ROADMAP.md. No independent-agent review, actual console/clock-
API/process/pipe/DLL/GPU invocation, human approval or qualification is claimed.
The result assesses owned local consent evidence, not fresh process/containment
status, safety/re-entry/Start, ongoing owner state or native admission. Native
issuance/mains, prompt/aggregate export/resources and real qualification remain
open; E3 partial/Gate B unmet and the full product goal is unchanged.

## Retained-handle process state observations — 2026-11-07

`process_state` records up to six ordered operations per original process: fresh
PID, fresh creation/exit/user/kernel times, job handle flags, current job limits,
direct wait/exit/job accounting, and live job membership. Every attempted operation
retains before/after anchors, typed selected raw outputs and errors. Stop, missing/
invalid clock or original setup expiry prevents successor reads. A returned OS
failure stays primary if the post-call clock also fails; both facts survive.
Unobserved final cancellation stays explicitly unknown. Public fixture construction
cannot create retained-handle provenance or pass the observed-state assessor.

The concrete fixed-process adapter reads only its original owned process/job
handles and actual WindowsClock. It never reopens a PID, waits on a job, changes
process/driver/device state or loads a DLL. Startup configuration and fresh
readback now share the same desired-job-limits helper, preserving the existing
kill-on-close/process-count/commit/user-CPU configuration. The check compares fresh
PID/creation, noninherited job handle flags, exact configured limits/returned size,
and independent wait/exit/accounting/termination facts. Worker and sampler must
be unsignaled with one active process and membership in their original jobs.
The starting reader instead requires observed exit code zero and empty original
job; post-exit membership is deliberately not queried or inferred.

`ParentSampler::observe_processes` binds all three original Origins to accepted
context/peer/consent evidence before collection. It retains one complete or partial
frame, independent role outcomes, outer clock facts and raw snapshots; rejection
faults parent admission and closes ordinary worker decoding. Duplicate collection
preserves the first frame and rejects. Fixed record storage is prepaid through
updated type sizes. Shared policy helpers are reused; these ledger checks do not
qualify allocator behavior or aggregate OS resources.

This is a historical observation interval, not an atomic OS snapshot or a lease
on later liveness. No new numeric process-age allowance is invented. A future
Start issuer must collect as part of that decision and combine it with fresh
safety/re-entry checks, not reuse a cached frame as permission. OS queries can
block: collection must remain off the independent deadline path, with retained
collector/process owners and separate stop/termination supervision. Collector
ownership/cancellation and actual main composition are not implemented here.
Job/exit evidence does not prove GPU quiescence, memory release or safe recovery.

Ten new CPU model tests cover live/exited roles, untrusted fixtures, every capture
failure/stop boundary, raw-error precedence, token/regression/expiry, identity/job/
limits/membership drift, wait/exit/accounting/termination, and batch scope/clock
failures. The existing non-invoked Windows pointer test additionally forces the
concrete parent/adapter path to codegen/link. Initial import/type inference and
test rounding lints were corrected; a Linux-only unused-function warning was
resolved by platform gating. Source-bound final checks/publication: ROADMAP.md.
No independent-agent review, actual retained-handle/clock-API/OS/process/pipe/
DLL/GPU invocation or qualification. Fresh safety/re-entry/Start composition,
native issuance/mains, aggregate export/resources and real qualification remain
open; E3 partial/Gate B unmet, with the full product goal and frozen limits intact.

## Parent Start intent with decision-time process collection — 2026-11-07

E3.1/E3.2/E3.3 now has a concrete `ParentSampler::queue_start` source path.
It requires the accepted context/peer/owned-consent chain and the existing
reported safety/re-entry screen, then collects the three original process
owners inside the same one-shot attempt. An earlier cached process frame is
ineligible. The worker's ordinary parent-to-child Channel emits one Start;
this is separate from the sampler's ordinary sequence. No public fixture or
stored result can construct NativeAdmission, and no main calls this path.

Four retained boundaries bracket entry, post-collection/issued, pre-queue and
post-queue. Each stores the actual clock anchor/error/identity, parent fault,
context/screen results, raw temperature/power records and latest re-entry pair
and consecutive count. Parent ticking reuses the existing monitor, required
metric and four-observation re-entry policy; no new numeric age allowance is
introduced. All boundaries must precede the original setup deadline, even when
sampler readiness has already marked its own setup complete. Encoding and queue
time do not renew the deadline. Deadline rejection uses that original boundary
for the stop window. The fixed attempt slot is prepaid through ParentSampler's
existing metadata reservation and the added worker Channel has its own charge.

The complete/partial process frame remains in the parent. A failed process read
still gets a post-call observation; its original failure takes precedence over
a later clock failure. The original encoded Start stays owned if a pre-queue
check or the queue rejects it. A post-queue expiry retains `queued: true` and a
failed overall result; queue acceptance cannot be relabelled as unsent or as a
peer receipt. Send errors and later clock/expiry rejection remain separately
visible. Duplicate attempts preserve the first record, close ordinary paths
and fault the parent. Independent worker stop/termination remains required.

The private shared engine has thirteen CPU model tests: continuous original
clocks, every boundary's expiry, identity/regression/suspend/bracket faults,
missing context/telemetry, capture-error retention, fixed observation ordering,
complete collection/encoding/queue order, cached-frame rejection, process/codec/
queue failure and secondary post-call failure. Existing transport regressions
retain adjacent codec/ownership coverage; an extended non-invoked function
pointer forces the concrete Windows composition to codegen/link. Final
source-bound verification and publication are recorded in ROADMAP.md.

OS queries and prompt operations can block. This source does not implement the
independently owned collector/controller stop lane, an ongoing safety lease,
qualified native issuance, authority transfer to the worker, or fixed mains.
These must precede actual workload integration; a decoded Start and an `Ok`
result are not permission. No actual clock/retained-handle/process/job/pipe/
vendor DLL/GPU adapter invocation or qualification occurred. Aggregate export,
resource qualification, real pilot/repeated-baseline and recovery/write gates
remain open. E3 is partial and Gate B unmet; frozen limits and the full automatic
benchmark/search/validation product goal are unchanged.

## Separate original-handle stop owner — 2026-11-07

E3.1 now has `Process::stop_owner`: one unique Send + Sync control owner can be
moved to an independent controller while the original process/transport/image
owner stays with a potentially blocked collector. It retains the original
Origin and Arc references to the same job, stop event and cancellation flag.
Construction performs no PID lookup, handle duplication, new inheritance or
process-state query. It owns no transport, vendor callback or collector lock.
Existing launch arguments, handle allowlist and numeric limits are unchanged.
This applies to successfully launched pilot/starting process owners; early
launch failure and launch-observer operations retain their separate paths.

A separately charged shared request ledger is allocated before file measurement
and process startup, and survives either owner until the final reference drops.
Each stop/termination operation publishes cancellation before its OS callback,
claims one atomic request slot and retains the first actual return. There is no
lock spanning a callback. A pending/reentrant stop cannot reserve the independent
termination lane. A duplicate request returns the original pending/result state
without invoking a replacement or retrying a failed native request. Unwind keeps
requested=true and returned=None; it cannot fabricate a completed operation.
Snapshots are historical observations, not an atomic cross-operation OS snapshot.

Process and StopOwner expose the same requested/returned observations. The
existing process observation reports the shared termination return, including
returns issued through StopOwner. A missing return does not mean no request;
callers must retain the explicit requested flag. The existing io::Result APIs
map pending to host WouldBlock with no invented raw OS error, and preserve
returned native errors. Dropping either owner requests containment; StopOwner
abandonment requests termination directly, without first waiting for a separate
stop-signal call. An already requested operation is not retried. Neither a
request nor an OS success proves observed exit, empty job or GPU quiescence.

Six CPU atomic/request-owner models and one Windows-module CPU conversion case
cover cancellation ordering, duplicate/pending/reentrant requests, independent
lanes, callback unwind, retained budget ownership, capacity failure and truthful
host-pending/native-return distinction. Non-invoked pointers force pilot/starting
owner construction, request and drop codegen/linking; static bounds require
Send + Sync. Review removed the sequential stop-before-terminate abandonment
path. Initial test-module placement and explicit-destructor-pointer errors were
corrected with failed logs retained. Final source-bound checks/publication are
in ROADMAP.md; earlier passes do not verify the final source.

No actual stop-owner construction/OS request, clock/retained-handle/process/job/
pipe/vendor DLL/GPU invocation or qualification occurred. This separates control
ownership; it does not yet implement the independent controller thread, original
stop-window timing, per-call clock retention, asynchronous collector ownership,
ongoing safety supervision, native authority transfer or mains. OS request APIs
still have no measured return bound. Aggregate resource/export, pilot/baseline,
recovery/write and product release gates remain open. E3 partial/Gate B unmet;
the full automatic benchmark/search/validation goal is unchanged.

## Independent parent controller and request lanes — 2026-11-07

E3.1/E3.2 now has a fixed `parent_watch_windows::Handle` source, backed by the
portable `parent_watch::State`. It validates the original sampler/worker stop
Origins, distinct PIDs, binding/image references, parent setup deadline and clock
identity before starting. One controller owns its own continuous clock history;
four pre-created request lanes separately own sampler/worker stop and termination
calls. No process-state query, transport receive/write, vendor callback, operation
join or evidence writer runs on the controller loop. Both ordinary cancellation
flags close before an operation is scheduled. Stop/termination lanes remain
independent even if another OS call or its clock capture never returns.

The original setup deadline applies until a timely locally retained successful
Start attempt supplies the setup/safety handoff. Subsequent updates come only
from the bound ParentSampler's retained records and accepted original peers.
Temperature and power retain the existing query/transport/value checks; their
original sample ends determine the 750 ms freshness deadlines. Updates cannot
revive an expired window, rebase the original run end or grant compute. This
controller supplements existing worker phase/count policy; full phase/controller
composition and native permission remain unfinished. A ready flag or historical
setup-observed flag is not NativeAdmission.

The first stop/fault closes admission, preserves its original 1 s/3 s windows
and may only tighten them. Late ticks never start fresh grace. Missing/invalid
clock or unknown current time forces containment without borrowed grace. A
controller's final report does not observe process exit or job emptiness; those
need separately retained original process owners off its deadline path.

Controller clocks and bounded update receipts/first overflow remain owned.
Mailbox receipt clocks follow each actual try_recv, including racing updates;
failed capture cannot substitute the previous clock. Pending commands survive
in the returned report, retaining their mailbox charge and original control roots. A rejected mailbox update is retained before fail-closed
abort. Each request slot owns actual before/after captures, raw errors, returned
request snapshot and thread completion facts. An explicit invoked marker prevents
historical/shared owner results from being described as newly bracketed OS calls;
current owner ledgers remain available separately. Results are stored before
post-call capture. Only observed finished threads may be joined; pending handles
and original control owners remain retained, including partial spawn failures.
Handle loss or thread unwind closes flags and requests independent containment.

Application reservations precede history/mailbox/owner/thread metadata setup;
OS thread stacks, scheduling and request return bounds are still unqualified.
Fourteen new CPU models cover original expiry/grace, late ticks, clock faults,
setup handoff, safety continuity through the full 45 s run, malformed/replayed
records, missing receipts, capacity retention and budget release. Non-invoked
pointers force concrete start/publish/request-lane codegen/linking. A Clippy
manual-is-multiple-of finding was corrected with its log retained. Final checks
and publication: ROADMAP.md. No controller/request thread, clock adapter, process,
job/event, pipe, vendor DLL or GPU adapter was actually invoked or qualified.

No main invokes this source. Continuous collector ownership, guard readiness and
publication tied directly to Start/telemetry, nonfault normal-exit finalization,
per-call timing/termination/exit qualification, native authority transfer and fixed
mains still need composition. Aggregate export/resources, real pilot/repeated
baselines, recovery/write and product release gates remain open. E3 partial/Gate B
unmet; frozen limits and the full benchmark/search/validation goal are unchanged.

## Controller attachment to Start and telemetry — 2026-11-07

E3.1/E3.2 now composes the actual controller publisher into ParentSampler.
`attach_watch` binds one original Window and consumes the controller's unique
publisher claim; duplicate transfer fails closed without replacing the first
publisher. The parent retains its publisher and fixed status/error slots while
the caller retains the Handle, operation handles and report through finalization.
Publisher or handle loss closes admission and requests independent containment.
These are source/lifetime rules, not observed Windows behavior.

Every actual Start boundary now retains `watch_ready`. Readiness requires the
same original sampler/worker peers, original run/setup window, live ready request
lanes and no closed/faulted publication. Missing or lost supervision prevents
successors; loss after a successful queue handoff retains queued=true with a
failed overall result. Existing context/consent, process, safety and re-entry
checks still apply. Readiness cannot substitute for them or issue NativeAdmission.
Poll the controller's own readiness for preparation and the parent's bound
watch_ready before the one-shot Start attempt; no new grace is introduced.

Successful Start automatically queues its first setup/safety update. Parent
receipt handling publishes changed temperature/power records immediately after
assessment; ordinary ticks deduplicate unchanged values. The external Handle
publish API is removed, so integration does not depend on an additional caller
remembering it. Queued setup is distinct from the controller's setup-observed
acknowledgment. A failed handoff never commits publication progress, and later
failure preserves the last accepted pair. Rejected data and actual queue error
remain with the controller, while parent raw receipts remain owned as before.
An initial setup-publication failure also faults parent admission and the retained Start attempt
without relabelling an already accepted Start frame as unsent.

Parent faults and planned finish forward the first original StopWindow directly;
this closes both ordinary flags without an OS call on the receipt path. Tick/
receipt notification additionally detects controller closure. The private shared
publication policy retains first failure, accepted setup state and last accepted
safety. No retry/reconnect/replacement occurs after rejection, even if an external
signal is erroneously reset. Fixed state is prepaid through updated parent/control
type sizes; original failure owners remain inline without a new allocation.

Eight new CPU publication models and two Start-engine cases cover startup waiting,
one setup publication, changed-record deduplication, failed first/later handoff,
missing clock, peer/window mismatch, parent/controller closure, original stop
forwarding and controller loss at every Start boundary. Concrete attachment and
receive/start composition are forced to codegen/link without invocation. Initial
large-Err/enum lint findings were resolved with scoped allowances that preserve
prepaid packet/process owners instead of allocating on failure. Final source-bound
verification/publication: ROADMAP.md.

No actual controller/request threads, clock adapter, original process/job/event,
pipe/vendor DLL/GPU calls or qualification occurred. Normal-exit finalization,
collector ownership, pending-query/phase/native-call deadlines, actual request/
exit timing, native authority transfer and fixed mains remain open. Readiness,
publication and historical acknowledgment are not compute permission. E3 partial/
Gate B unmet; frozen limits and the full automatic optimization goal are unchanged.

## Normal host-controller retirement — 2026-11-07

E3.1 now distinguishes planned sampler shutdown from an unplanned stop. A planned
StopWindow retains its original trigger/cooperative/exit deadlines without itself
creating a Requested fault. Safety and run deadlines continue to apply; no mode
change creates new grace. A later unplanned stop remains separately retained and
vetoes normal retirement, even when the first stop was planned.

`parent_watch_windows::Handle::retire` is a one-shot original-handle observation,
intended off the deadline path after child lifecycle/exit readiness. It requires
matching original parent/peer ownership, a successful retained Start, no parent
fault, planned finish and the sampler's validated RoleFinished receipt. It reads
wait/exit/job accounting from the two original process owners, retaining outer/
per-role clocks, actual raw returns, before/after request state, frequency/token
and complete/partial failure. Original creation identities, signalled handles,
exit code zero, empty jobs and no termination request are all required. Missing,
pending, failed, stale, regressing or conflicting evidence cannot retire control.
This is a single final observation, not a polling/retry API; callers must retain
the collector and both original processes through acknowledgment/finalization.

Detached public exit fixtures are untrusted and cannot disarm a real owner.
Successful collection means only a retained retirement request. The independent
controller checks its original window, fresh causal receipt and prior safety/
fault state, then atomically arbitrates retirement against unplanned stop/force
issuance. Force already reserved cannot be cancelled by later exit. Immediate
abort reserves force before publishing signal gates, preventing a signal from
being newly scheduled after retirement wins. Pending request methods/threads
cannot be turned into successful cleanup evidence. Actual operation handles,
controller report and raw exit frame remain required through finalization.

After accepted host retirement, idle request threads withdraw without invoking
OS APIs or fabricating returned success. Process and StopOwner abandonment
cleanup skips a new termination request only after that private observed-exit
marker is set. Explicit request histories remain unchanged, and ordinary
admission stays closed. Dropping handles/publication before acknowledgment still
requests containment. This does not prove GPU quiescence or physical memory
release, validate workload counts/checks/cleanup, authorize another repeat, or
accept a benchmark/profile.

Launch, Process and StopOwner now share the same first-request ledger from before
background startup; early/late launch-observer termination can no longer be
absent from a successful process owner's exit assessment. Native errors, pending
requests and exact original owners survive transfer. Existing job/event handles,
launch vocabulary, stop ceilings and numeric profile remain unchanged. Fixed
retirement/exit-marker state is prepaid through updated type sizes; no failed
owner transfer adds an allocation.

Sixteen new CPU models cover raw exit/clock/OS faults and untrusted fixtures,
planned-vs-unplanned shutdown, retirement-vs-force arbitration, prior faults/
expiry, shared launch/control history and non-erasing exit markers. Concrete
normal observation and drop codegen/linking are forced without invocation. Review
also separated known stop errors from pending returns, preserved later unplanned
stop escalation and used the current try_update API; intermediate deprecation/
format failures remain logged. Final source-bound verification/publication is
in ROADMAP.md. No actual retirement/query/controller/request/clock-adapter/
process/job/event/pipe/vendor DLL/GPU invocation or qualification occurred.

Retained asynchronous collector ownership and session orchestration, remaining
query/phase/native-call deadlines, actual request/exit timing, aggregate export/
resources, native authority transfer and fixed mains remain open. E3 partial/
Gate B unmet; the full automatic benchmark/search/validation goal is unchanged.

The same arbitration is bound into both original request ledgers before controller
threads start. Launch/process force calls therefore reserve force before their
actual API invocation, not merely before a controller slot runs. Attachment
racing existing cancellation rejects startup. Controlled signal callbacks are
distinct from later unplanned legacy signals. Late new calls after retirement
invoke no OS callback and retain original request snapshots; an unrequested
retired io result is NotConnected, not fabricated pending/native success. These
rules close the race between final exit assessment and a force call by another
original owner. Observed exit still cannot authorize another GPU run.

## Session finalizer for retained retirement assets — 2026-11-08

`workload::session_finalizer::SessionFinalizer<E>` is a caller-owned,
nonblocking drain around the fixed retirement collector. It retains one opaque
caller evidence owner `E` (whose own reservations live with it) alongside the
collector assets. After final observation completes, each poll checks the
controller's four request-lane handles and controller thread, joining only
handles already reported finished. The terminal `Finalized<E>` retains the
collector outcome, original assets, caller evidence, controller report/error,
joined per-lane results and whether retirement was acknowledged. Collection,
controller, or lane failure remains visible; `complete()` is true only for
successful collection/frame, acknowledged retirement, a successful controller
report with a committed retirement state and no controller/clock/overflow fault,
an empty retained mailbox, and successful joined operations. The mailbox tracks
its exact accepted-but-unreceived count atomically; checking `complete()` never
consumes a queued update. `Report.pending` remains owned by the final result and
can be drained explicitly. Request cancellation works through the independent
controller reference before or after collection.

The typed `PilotOwners<E>` bundle now names the
pilot manifest, starting scope/process, closed starting handoff (including an
unsent bootstrap frame and send fault), starting consent record, strict starting
receipt, pre-manifest reports and worker setup delivery, plus caller
workload output evidence. `Handoff::close` drops the process-borrowing sender
before transferring its session and unsent evidence, so the starting process
can be retained in the same owner bundle. `start_pilot` transfers this bundle
with the sampler/worker/controller assets into the finalizer. Before spawning
the collector it checks fixed manifest/scope identity, starting declaration and
response digest, original consent/process provenance, pre-manifest/environment
artifact digests, worker setup binding/delivery, parent cross-artifact reference
record (including the starting response's separately reported settings snapshot)
and all three peer origins. It also requires a queued successful Start, accepted
RoleFinished/parent stop and matching planned controller window, no prior parent
or controller fault, and no existing retirement. These checks bind retained reports to one
session; they do not prove source truth, and no fixed main constructs this bundle
yet. A rejected binding returns all assets and evidence without issuing process
queries. The lower-level collector also records a context failure without
querying process handles. A never-returning final
OS observation or controller operation can keep polling pending indefinitely;
pending-query/phase/native-call deadlines and qualification of actual request
timing remain open. Even `complete()` says only that the host retirement path
was acknowledged and its retained controller operations joined. It does not
establish GPU quiescence, physical release, workload correctness, profile
acceptance, repeat permission or native authority.

## Retained asynchronous retirement collector — 2026-11-08

`workload::retirement_collector::Collector` moves one final host-retirement
observation off the deadline/controller path. Its fixed `Assets` owner keeps the
parent sampler, controller, both original process owners, original Windows clock
and partial retirement frame together. The private `collector_task` owns this
value in a mutex cell and invokes only a fixed function pointer; there is no
public caller-supplied native callback. Capacity or thread-spawn failure returns
the original assets and signals cancellation. Operation panics and outer thread
panics retain their payloads separately from partial evidence. Cancellation is
through the controller's independent control reference and never takes the
collector-owner mutex. `poll` is nonblocking until `JoinHandle::is_finished`,
then joins once and transfers the assets and reservation once.

The scoped CPU model suite has eight cases for retained ownership/charges,
partial state and panic payloads, distinct operation and outer panics, missing
operation rejection, independent cancellation, capacity failure and nonblocking
poll. `target/workload-verification/async-retire-final3-cpu.log` records 8/8
passing; `async-retire-final2-clippy.log` records warning-denied scoped Clippy.
No actual Windows thread, clock adapter, process/job query, controller
acknowledgment, native call or GPU workload was invoked. The collector performs
only one final observation; its successful result is not retirement
acknowledgment, joined controller/operation threads, GPU quiescence, or permission
to repeat. Starting-read/artifact/consent owners and complete session
orchestration are not yet composed here.
