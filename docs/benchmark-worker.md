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

The worker receives the exact child-only environment `CUDA_CACHE_DISABLE=1` with
double-NUL termination; the sampler keeps the existing empty child environment.
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

Fixed executable entries, complete parent fact/safety/re-entry/consent composition,
the worker native-admission issuer and aggregate live export remain unfinished.
Parent monitor timing across runs must preserve the original epoch and session
ceiling. E3 remains partial and Gate B unmet; this is not a useful baseline or an
automatic optimization delivery.

## Fixed pilot sampler entry

`autotune-pilot-sampler` now has a fixed quiet Windows entry that composes the
common inherited context, independent control thread, selected NVML startup,
Ready, current-setting dispatcher and pilot telemetry collector. The entry's
unsafe contract requires one exclusive inherited-endpoint adoption from the
fixed launcher. Strict bounded arguments and bootstrap/image validation precede
control startup; successful control readiness precedes NVML initialization.
The entry returns its context/trace/owners, including a successfully built
context if later trace construction fails. It introduces no worker executable,
driver/compute initialization, human-consent authority or native-admission issuer.

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
