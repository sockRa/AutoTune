# Rust workspace verification — E1.1

The [workflow](../.github/workflows/ci.yml) checks the package-declared Rust
1.99.1 baseline on Ubuntu 24.04 and Windows Server 2025 (default MSVC target).
It uses the committed lockfile, read-only repository permissions and no stored
checkout credentials. It does not publish, upload artifacts or run executables
against a GPU. Toolchain/dependency provisioning occurs only on hosted runners;
the verification commands then use offline Cargo resolution.

Both jobs check formatting and Clippy for **every workspace test target** with
warnings denied. Linux runs the entire workspace's portable paths. Windows runs
tests in `autotune-probe`, `autotune-power-trial`, `autotune-recovery-contract` and
`autotune-recovery-core`, plus the independent synthetic-only
`autotune-measurement` library and pure `autotune-fma-reference` design/input/oracle
library, plus `autotune-benchmark-worker` injected fixtures. Probe tests explicitly use synthetic adapters; FMA tests check supplied
CPU vectors and do not execute a GPU workload.

The benchmark-worker's Windows process tests are ignored and never executed in
hosted CI. Windows additionally compiles/lints its `native-cuda` feature and runs
only library tests using typed fake exports/static PTX checks, never a GPU DLL
loader. The fake-only executable has no native mode. See [worker boundaries](benchmark-worker.md).

Windows recovery/storage/harness tests are **compiled, linked and linted, not executed**
by this CI definition. Their approved scratch-root, NTFS/ACL and enclosing-job
requirements need a separately qualified runner. In particular, the assertion
that Cargo's enclosing job rejects guardian launch is not portable to arbitrary
hosted runners. Existing assertions and ignored acceptance cases are unchanged.
This scope is not Windows recovery acceptance, hardware qualification, or a
passing product release gate. No `--ignored`, live probe, benchmark or elevation
command belongs in this workflow.

## Local equivalent (installed toolchain)

Use the installed toolchain matching 1.99.1 and the host's existing linker setup:

```text
cargo +1.99.1 fmt --all -- --check
cargo +1.99.1 clippy --workspace --all-targets --locked --offline -- -D warnings
cargo +1.99.1 test --workspace --no-run --locked --offline
cargo +1.99.1 test -p autotune-probe -p autotune-power-trial -p autotune-recovery-contract -p autotune-recovery-core -p autotune-measurement -p autotune-fma-reference -p autotune-benchmark-worker --locked --offline
cargo +1.99.1 clippy -p autotune-benchmark-worker --all-targets --features native-cuda --locked --offline -- -D warnings
cargo +1.99.1 test -p autotune-benchmark-worker --lib --features native-cuda --locked --offline
```

On Linux, replace the test commands with `cargo +1.99.1 test --workspace --locked --offline`.
If the existing toolchain is named `stable` rather than `1.99.1`, first verify
`rustc +stable --version` reports 1.99.1 and substitute `+stable`; do not install
a named toolchain just to run these commands.
These commands never authorize installing missing tools or fetching dependencies
locally. Missing prerequisites are failures/blockers, not permission to bypass a
check. Local Windows GNU results do not verify either hosted Linux or Windows
MSVC execution. See [ROADMAP.md](../ROADMAP.md) for current evidence and handoff;
configured CI is not an observed remote pass.

## Verification evidence

### Separate bounded-worker local verification — 2026-10-26

Installed Rust 1.99.1 Windows GNU with existing LLVM-MinGW and a fresh target
passes fmt, warning-denied workspace Clippy/linking and 211 selected deterministic
tests; five ignored worker-process tests are then explicitly run serially and all
pass. Opt-in native-feature all-target Clippy/linking and four library tests pass
using only typed exports/static checks (one protocol test overlaps default tests).
No native GPU DLL is loaded or hardware executed. No hosted/actual-commit claim.
Source/results manifest
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-worker-final03-20261026-manifest.json`
SHA256 `95557c80710f7a4e3c5b73b3dc3df2ed8aa550e7f3907ee533952aac3b863509`
binds full baseline `c6e3821d0cbf31e039fdc68b3b055963822a0123`, before/after source,
contract/binary hashes, retained logs/exitcodes and initial fixed failures.
These results follow the independent-review patches (frozen profile limits,
driver retention, evidence/deadline/upload coverage, documentation and CI
boundary); final03 supersedes final02, whose logs remain retained. Later
doc-only evidence annotations do not change tested Rust/CI source. Parent-loss
child exit is observed via the original duplicated process handle; accounting for
the lost sole-owned job is explicitly unavailable, not a successful-session claim.

### Separate local-only FMA delivery — 2026-10-26

The expanded pure-package scope passes locally on installed Rust 1.99.1 Windows
GNU: fmt, warning-denied workspace Clippy, all workspace test linking and
**200 selected deterministic tests, including 23 FMA tests**, zero failed/ignored/
filtered. This is uncommitted working-tree evidence, not a hosted result for
the new crate/CI definition. Source/results manifest
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-fma-final-20261026-manifest.json`
SHA256 `8fe0f2fd56e2f1fb2d1d3ec4b76c87b935aded76419505a8c1880d295041bfaf`
binds baseline `c6e3821d0cbf31e039fdc68b3b055963822a0123`, source hashes,
fresh target and native command results. Later evidence-only roadmap/CI text
does not change tested Rust/CI behavior. No recovery acceptance, live workload,
probe, GPU or system changes. An initial new-test Clippy type-complexity error
was fixed with a test type alias; its failure log is retained separately.

### Prior hosted observations — 2026-10-25, unchanged scope

[Run 40179174853](https://github.com/sockRa/AutoTune/actions/runs/40179174853)
completed successfully for commit `8778c5f4f241631b8282631039abd50b7d35d881`.
Both Ubuntu 24.04 and Windows 2025/MSVC passed their configured formatting,
Clippy and test scopes; Windows also linked all workspace test binaries.
This closes E1.1's workspace/CI exit criterion, not E1 as a whole or recovery
acceptance. The run covers that exact commit, not later changes or the dirty
recovery working tree. OS-conditional steps skipped on the other platform are
expected; Windows recovery tests remained unexecuted as documented above.

[Run 40180217837](https://github.com/sockRa/AutoTune/actions/runs/40180217837)
also completed successfully on both platforms for the energy-helper delivery,
commit `f722522d0491ee0d9d12aa8c3ac753ec7b62b46b`. The same coverage limits apply.
The checkout action emitted a nonblocking Node 20 deprecation annotation; its
pinned replacement is a maintenance follow-up, not a waived failing check.

[Run 40225364733](https://github.com/sockRa/AutoTune/actions/runs/40225364733)
completed successfully on both platforms for the synthetic repeat-provenance ledger,
commit `d97671a0e1014db12b080e850632a3e67a238121`. Local Windows GNU verification
also passed 137 selected tests, fmt, workspace Clippy and all test linking from a
separate byte-exact commit archive. Neither result establishes repeated-baseline
acceptance, physical energy, workload execution or Windows recovery acceptance.
Later evidence-only documentation is not a separate observed CI pass.

[Run 40228219662](https://github.com/sockRa/AutoTune/actions/runs/40228219662)
passed both platforms for lifecycle delivery `ad73825d232008c74ffc98a5057258bd8204af21`.
Its separate byte-exact local commit archive passes 151 selected tests (14 lifecycle),
fmt, workspace Clippy and test linking. Scope exclusions above are unchanged:
synthetic lifecycle tests do not establish real workload/recovery acceptance.
The nonblocking checkout Node 20 deprecation annotation persists.

[Run 40229676176](https://github.com/sockRa/AutoTune/actions/runs/40229676176)
passed both platforms for warm-up/summary delivery
`e85c3d9b982de15d4871d1edc166403513b16e4a`. Separate byte-exact local commit
verification passes 163 selected tests (12 summary), fmt, workspace Clippy and
test linking. These descriptive synthetic summaries do not establish statistical
sufficiency, baseline comparability, efficiency or workload qualification.
Evidence-only follow-ups retain the source-delivery observation boundary.

[Run 40231558147](https://github.com/sockRa/AutoTune/actions/runs/40231558147)
passes both platforms for owner-authorized preservation publication
`dad05f7d8f78d2b2946f24354409349e30be57e9`. The existing dirty recovery source
was preserved unchanged, not declared accepted. A separate byte-exact commit
archive passes Windows GNU fmt/Clippy/workspace linking and 163 selected tests.
Windows recovery tests remain unexecuted; historical failures, native feasibility
and G1–G7 blockers are unchanged. This observation does not cover later 6A source.

[Run 40232305689](https://github.com/sockRa/AutoTune/actions/runs/40232305689)
passes both platforms for selected metadata delivery
`98f7ac20e53913b3de6eeb8e17bc9430bdafe100`. Separate byte-exact local commit
verification passes 177 selected tests (14 metadata), fmt, warning-denied workspace
Clippy and all test linking. These exact caller-assertion observations do not
assess comparability, completeness, baselines, correctness or improvement. All
execution exclusions above remain unchanged; the nonblocking checkout Node 20
annotation persists. Later evidence-only documentation is not another observed
runtime CI pass.
