# E3 sampler Windows fixture execution request

Prepared 2026-10-29 for E3.1/E3.2 package 2. **Not execution approval.**
The owner-approved [sampler design](2026-10-29-e3-independent-sampler-design.md)
requires a separately approved source/artifact-bound matrix before fake-process
acceptance. This request makes that next action concrete. Product progress and
results belong in ROADMAP.md, not in this execution contract.

## Exact scope

Run the five ignored `sampler_process` cases once, serially, on the current
Windows host, unelevated, using only the two images pinned in the accompanying
[matrix](2026-10-29-e3-sampler-fixture-matrix.json). Source revision:
`ef8786e0a4ea0831b17d14355ce98ecdcbe2883e`; subsequent documentation/script
commits may exist, but the runner rejects differences in Cargo/workspace source
or CI configuration. The verified local build is Rust 1.99.1 Windows GNU with
`native-cuda` and `sampler-process-fixture` compiled. The fake child uses the
production protocol/control loop with fixed fake readings; it selects no native
NVML backend. Compiled CUDA support is never opened or executed by these cases.

| Case | Required observation |
| --- | --- |
| `cancelled_async_launch_never_allows_a_late_sampler_to_continue` | Cancelled startup cannot leave a returned child running; any retained created-child handle must show exit before the original 3 s observation bound. Before-creation errors are distinct from observed exit. |
| `fixed_hung_sampler_requires_observed_exit_after_force` | Independent stop does not imply exit; after force, retained process exit and empty-job accounting are observed within 3 s. |
| `shared_wire_fixture_blocked_getter_keeps_control_responsive_until_force` | Shared wire/control loop remains responsive with one blocked fake getter; force, pending uncertainty, exit and invalid simulated envelope are retained. |
| `shared_wire_fixture_samples_six_metrics_and_drains_after_stop` | Fixed fake metrics flow through the real transport/supervisor; stop/drain/exit evidence and a blocked simulated envelope result. |
| `shared_wire_fixture_wrong_ready_identity_never_admits_a_query` | Wrong Ready identity stops without a query; retained exit evidence and invalid simulated result. |

Each wire case has its existing 18 s absolute fixture bound and unchanged
production admission/stop/exit limits. The outer test-driver cutoff is **90 s**
for the entire serial matrix, followed by at most **3 s** of shutdown observation.
It cannot extend any inner case deadline. Stop or fail on the first incomplete
matrix; preserve all attempted-case evidence. There is no automatic rerun.

Only the fixed fake sampler can be launched by the test harness. The production
sampler/controller, benchmark fake worker, GPU probe, NVML/CUDA DLLs, PTX, tuning
setters, external tools and system configuration are outside this request.

## Artifacts and runner

The machine-readable matrix pins full SHA-256 values for the runner and both
executed images. Its source-bound build inventory is
`target/sampler-verification/writer-ci-final-manifest.json`, SHA-256
`34ab1074c36b8ec7d681c9d34d64c549c1345b5a3d1f9abfc48b27614bbd3c57`.
Rebuilding or changing a pinned image/runner requires refreshed review and
approval of a new matrix digest. A digest string is not authenticated consent.

The [runner](../../../scripts/Invoke-SamplerFixtureMatrix.ps1) defaults to
verification only: compare source, cases and scope; hash and hold the fixed
images without write/delete sharing; print the matrix digest; launch nothing.

```powershell
powershell.exe -NoProfile -File scripts/Invoke-SamplerFixtureMatrix.ps1
```

After explicit approval of the printed matrix SHA-256, the exact execution
command adds `-ExecuteApprovedMatrix -ApprovalMatrixSha256 <approved-digest>`.
It starts the test driver directly and hidden through `ProcessStartInfo`, passing only
`--ignored --test-threads=1 --nocapture`. It does not rebuild or execute another
target. The script checks the aggregate log size during observation and stops
at 16 MiB; scheduling and redirected I/O can overshoot that observed limit.
This is not a hard OS disk quota or a controller resource-qualification claim.

Outputs are fixed under `target/sampler-verification/fixture-execution/`:
`stdout.log`, `stderr.log`, and `receipt.json` with matrix identity, stop reason,
elapsed time, termination request, observed exit/code and separate stream-drain
observations. Two bounded byte pumps observe stdout/stderr EOF and async write/
flush completion within the original bounds. Final lengths/digests exist only
for completed pumps; unresolved streams have unavailable lengths/digests and
cannot produce a successful runner result. Memory-only runner tests cover binary
retention, quota equality, pending I/O and faults without launching a fixture.
An existing output directory rejects another attempt. Unexpected errors request
termination of only the owned test driver, with bounded observation; missing
exit remains unknown. Killing that driver relies on the private child job's
kill-on-close mechanism, which this execution is intended to observe, not assume.
Output failures cannot guarantee a durable receipt; preserve available logs and
the tool result, and do not manufacture a successful outcome.

## Evidence limits and subsequent work

Inspect all five case results, the retained root exit, timing and diagnostics.
A zero exit alone is not blanket Windows containment acceptance. These cases
do not cover every parent-loss/inheritance/launch-fault scenario or the complete
fixed production controller's artifact path. No real GPU identity, native call
completion, useful baseline, energy estimate, recovery, real write, optimizer or
Gate B qualification follows. Real read-only capture and later GPU workloads
require their own exact manifest and consent after the remaining prerequisites.
