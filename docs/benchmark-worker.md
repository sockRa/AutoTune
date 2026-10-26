# E3.1 bounded benchmark-worker foundation

`autotune-benchmark-worker` is an independent package. Its portable worker admits
**simulated fixtures only**; native/live admission is unavailable. It moves the
automated benchmark/search/validation product toward an isolated checked workload,
but does not supply live monitoring, baseline acceptance, search or any release gate.
No existing recovery, probe or synthetic measurement implementation is changed.

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
