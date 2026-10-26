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

### Canonical live-evidence primitives (phase 1) — 2026-10-26

Portable `autotune_benchmark_worker::evidence` (`canonical`, `digest`, `parse`)
adds compact canonical bytes, decimal-string integers/ratios, four
domain-separated SHA-256 digests and a bounded strict reader with re-encode
canonicality. It is CPU-only primitives with no envelope fields, live producer,
clock read, process launch or native call. After three independent reviews and
the resulting patches, a fresh offline Windows GNU target on installed Rust 1.99.1
and the existing LLVM-MinGW passes workspace fmt, warning-denied workspace
all-target Clippy, all test linking and **256 explicitly selected CPU-only tests
(13 evidence)**, plus feature all-target Clippy/linking and four feature-library
typed/static tests.

Exact restricted selection (not the broader package/hosted selection):

```text
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets --locked --offline -- -D warnings
cargo +stable test --workspace --no-run --locked --offline
cargo +stable test -p autotune-recovery-contract -p autotune-recovery-core -p autotune-measurement -p autotune-fma-reference --locked --offline
cargo +stable test -p autotune-probe --lib --locked --offline
cargo +stable test -p autotune-probe --test probe --locked --offline -- --skip every_matrix_scenario_is_deterministic_and_cli_matches_schema --skip usage_help_and_default_mock_exit_behavior
cargo +stable test -p autotune-power-trial --lib --test safety --locked --offline
cargo +stable test -p autotune-benchmark-worker --lib --test worker --test monitor --test evidence --locked --offline
cargo +stable clippy -p autotune-benchmark-worker --all-targets --features native-cuda --locked --offline -- -D warnings
cargo +stable test -p autotune-benchmark-worker --all-targets --features native-cuda --locked --offline --no-run
cargo +stable test -p autotune-benchmark-worker --lib --features native-cuda --locked --offline
```

The 256 tests are 13 evidence, 36 monitor, 8 worker integration, one worker
library, 147 model/FMA/measurement, 12 probe library, 9 probe pure (both CLI tests
filtered) and 30 trial safety. Worker-process and trial-CLI targets are not
selected and no `--ignored` run occurred; workspace and feature all-target builds
use `--no-run`, not execution. No new process/native/GPU tests, installation or
system changes. Manifest
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-evidence-parent-final-20261026-manifest.json`,
SHA256 `2d86bf46d385d8835681757a30d417a2b73ee9803f25df7aae87fdb160996976`,
binds baseline `f44ccf01b0f0e5d77172ad7b72c7a52cd1e5033a` plus the uncommitted
phase-1 tree (later committed unchanged as
`c1cc27e637c8c113a219a93250543fe437f08d49`), all commands, log/exit-code hashes,
identical before/after hashes
of 105 source/CI files and the independent vector derivation. Documentation,
spec and roadmap annotations followed the run; no tested Rust/CI source changed.
The earlier dev-run manifest
`autotune-evidence-phase1-20261026-manifest.json`, SHA256
`189575d2fb70c2fa34008638a8f7de487e14bce257ee6ac617a7c55c6532604e`, is
historical because its test run preceded the `digest.rs` lint fix. No hosted or
actual-commit/native/hardware/release-gate qualification is claimed, and a codec
round trip is not live evidence or authority to execute anything.

### Injected monitor policy — 2026-10-26

Loop 2 replaces loop 1's oversized union-event storage with compact controls,
paired sample/begin observations and sparse missed ranges. Installed Rust 1.99.1
Windows GNU/existing LLVM-MinGW and a fresh offline target pass workspace fmt,
warning-denied all-target Clippy, all test linking, **236 explicitly selected
CPU-only tests (29 monitor)**, feature all-target Clippy/linking and four
feature-library typed/static tests. No monitor tests are ignored or filtered.
The full injected 600-second trace has 60001 10 ms polls and six queries every
250 ms, verifies the independent 600 s deadline and every retained operation's
original anchor/order, and does not stop prematurely for capacity.

Exact restricted test selection (not the broader package/hosted selection):

```text
cargo +stable test -p autotune-recovery-contract -p autotune-recovery-core -p autotune-measurement -p autotune-fma-reference --locked --offline
cargo +stable test -p autotune-probe --lib --locked --offline
cargo +stable test -p autotune-probe --test probe --locked --offline -- --skip every_matrix_scenario_is_deterministic_and_cli_matches_schema --skip usage_help_and_default_mock_exit_behavior
cargo +stable test -p autotune-power-trial --lib --test safety --locked --offline
cargo +stable test -p autotune-benchmark-worker --lib --test worker --test monitor --locked --offline
cargo +stable test -p autotune-benchmark-worker --lib --features native-cuda --locked --offline
```

Worker-process and trial CLI targets are not selected; both probe CLI tests are
filtered. Workspace and feature all-target builds use `--no-run`, not execution.
No new process/native/GPU tests, ignored tests, installation or system changes.
Manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-monitor-loop2-01-20261026-manifest.json`,
SHA256 `24f91bb27bfcbcd6009a14a13f1695947729b67f0dc23227a6166fa109255be9e`,
binds actual baseline `7eaef9e9ed5e9a1329fd1c8e7e24dac83b7e0ef3`, all commands,
log hashes and identical before/after source hashes. Documentation/spec/roadmap
annotations followed the run; no tested Rust/CI source changed. No hosted or
actual-commit/native/hardware/release-gate qualification is claimed. These counts
predate the loop-2 review patches; the post-patch run below is the current
source-bound result.

Historical loop-1 evidence is not a new pass: final03 manifest
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-monitor-final03-20261026-manifest.json`,
SHA256 `c867d97a79c01f0f05bdf56c03682637acca6c3a9c78ce865ae8c03891720022`.
Retained scope-deviation history, verbatim:

**Scope deviation retained, not a clean-scope claim:** the earlier `final` and
`final02` broad package selections inadvertently ran four existing mock CLI
subprocess tests (two probe/two trial), twice. Their initial manifest scope
strings incorrectly stated no process execution; final03 explicitly corrects
that record and excludes those tests. No live probe, owned-worker/recovery
process test, native DLL or GPU ran. Initial missing-dlltool-PATH and new-test
Clippy `int_plus_one` failures are retained and fixed without installation or
lint suppression. Parent review must disposition the scope deviation; a later
restricted pass does not erase it. No hosted/hardware/release-gate claim.

Loop-2 review patches (ten fixture gaps plus a one-line terminal-completion
guard) were then re-verified from a fresh
`autotune-monitor-parent-final-20261026-target` against the same baseline:
workspace fmt, warning-denied all-target Clippy/linking, 243 audited CPU-only
tests (36 monitor), feature all-target Clippy/linking and four feature-library
typed/static tests pass. Manifest
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-monitor-parent-final-20261026-manifest.json`,
SHA256 `ccc81b75fad31b24a83fdeed6d5ddd73f5cef2a72e5ddcd657d9d10fe9306527`,
binds commands, logs and unchanged before/after hashes of 105 source/CI files.
No worker-process/trial-CLI target, ignored test, native load or GPU execution
ran. The deviation above stays historical noncompliance. The verified monitor
source is preserved by owner-approved local commit
`898ff279f01442dc67bb1c94bbe050369c59fc1c` (no push); the commit does not add a
hosted, hardware or release-gate result.

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
