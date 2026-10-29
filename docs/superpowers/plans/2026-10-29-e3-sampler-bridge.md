# E3 Selective Sampler Bridge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect the existing selective getter result to monitor records with exact units/statuses and lossless raw failure retention, without native execution.

**Architecture:** Add a portable pure `sampler` module to `autotune-benchmark-worker`, consuming the existing probe-library result types. Return the original owned field beside a typed conversion result; leave controller policy and native rejection unchanged. This is the first subtask of roadmap delivery package 2, not completion of the independent sampler or measurement chain.

**Tech Stack:** Existing Rust 1.99.1 workspace, local `autotune-probe` path dependency, existing JSON value APIs. No new external dependency or feature.

**Spec:** [E3 sampler bridge design](../specs/2026-10-29-e3-sampler-bridge-design.md); `ROADMAP.md`; `docs/{nvidia-feasibility,benchmark-worker}.md`; local `_bmad-output/specs/spec-e3-first-real-benchmark/{integration-contract,qualification-profile}.md`.

The roadmap cleanup handoff also locates the recovered OMO G003 sampler/controller
draft in the hash-verified external archive. It contains prior transport/process
analysis, not delivered code or safety authority. Consult it before extending
this bridge to process integration; do not repeat that analysis or silently
adopt its proposed wire schema as an approved contract.

## Global Constraints

- Owner approved this plan with `ja kör` on 2026-10-29; execute the two tasks inline under the existing CPU-only boundaries.
- Rust version floor: `1.99.1`. Use installed tools and existing locked dependencies offline; do not install tooling or dependencies.
- Native admission remains unavailable. No native DLL loading, real probe/sampler/clock/process/GPU execution, ignored tests, workload, writes, installation/elevation or recovery expansion.
- Preserve Q1–Q4 and `e3-fma-qualification-v0.1` unchanged. No numeric defaults or new safety policy.
- Preserve the existing probe report schema, monitor behavior, phase-1 canonical bytes and envelope semantics.
- Worktree/isolation choice is checked at execution time. The owner subsequently requested tooling cleanup: `.omo/` and the clean same-HEAD tool worktree were retired with a hash-verified external archive, not discarded. Do not recreate the old automation. No blanket commit/push authorization is inferred from `ja kör`.
- Record progress only in the canonical roadmap and this execution checklist; do not create a second product-status ledger.

## Review Focus

- A floating-point JSON number, string, bool, null or integer above `i64::MAX` must not become a rounded/truncated available reading — Task 1.
- GPU-lost evidence on a non-safety metric must close admission rather than look like tolerable missing telemetry — Tasks 1–2.
- Symbol presence inconsistent with an available reading or declared missing symbol must not be silently repaired — Task 1.
- Rejected conversion and rejected/late controller returns must retain raw details without refreshing safety or reopening a sticky stop — Task 2.
- Supplied timestamps are assertions, not observed clocks; the mapper must not replace start/end/receipt with convenient values or claim a physical query-start — Tasks 1–2.

## File responsibilities

- Create `crates/autotune-benchmark-worker/src/sampler.rs`: metric selection and pure owned-field conversion only; no getter/clock/process calls.
- Modify `crates/autotune-benchmark-worker/{Cargo.toml,src/lib.rs}` and `Cargo.lock`: register the module and local probe dependency; no dependency version changes.
- Create `crates/autotune-benchmark-worker/tests/sampler.rs`: conversion and controller-composition tests, fixtures local to this target.
- Modify `docs/benchmark-worker.md` and `ROADMAP.md`: API/boundary documentation and exact evidence after verification; no broader story closure.

### Task 1: Exact raw-field conversion

**Files:** module/registration/lockfile and new test target listed above.

**Interfaces:**
- Consumes `monitor::{Metric, Ticket, Record, Status, Unit}` and `autotune_probe::{nvml::SampleMetric, report::{Field, Reading, Reason, SymbolPresence}}`.
- Produces `pub fn sample_metric(metric: Metric) -> SampleMetric` with explicit one-to-one mapping of all six variants.
- Produces `pub enum MappingError { Unit, Symbol, Value, DeviceLost }`, deriving `Clone, Copy, Debug, PartialEq, Eq`.
- Produces `pub struct MappedSample { pub raw: Field, pub record: Result<Record, MappingError> }`, deriving `Debug`.
- Produces `pub fn map_field(ticket: Ticket, end_ns: u64, receipt_ns: u64, raw: Field) -> MappedSample`.

- [x] **Step 1: Write the failing mapping tests in `tests/sampler.rs`.**

Use a fixed valid nonzero binding/ticket and table-driven cases:

| Test | Exact assertions |
| --- | --- |
| `maps_all_six_metrics_and_units` | Temperature/Power/GraphicsClock/MemoryClock/GpuUtilization/MemoryUtilization map to the identically named selective metric; exact source unit strings are `degC`, `mW`, `MHz`, `MHz`, `%`, `%`. Available integers 42/123000/1500/7000/80/10 yield the corresponding typed units, `Status::Available` and `Some(value)`. |
| `preserves_ticket_and_supplied_times` | A ticket with slot 7, scheduled 1750000000 and start 1750000010 is returned byte-for-field unchanged; end 1750000020 and receipt 1750000030 are unchanged. No timing is generated. |
| `rejects_wrong_unit_and_unrepresentable_values` | Wrong/case-changed unit is `Unit`; JSON `42.0`, `"42"`, `true`, `null`, array/object and unsigned `i64::MAX + 1` are `Value`; exact integer `i64::MAX` converts unchanged for later controller rejection if applicable. |
| `rejects_symbol_contradictions` | Available plus `Absent` or `NotChecked` is `Symbol`. Unavailable `Reason::SymbolMissing` requires `Absent`; all other reasons require the same presence returned by `Field::unavailable` (runtime missing/load failed/unsupported-platform: `NotChecked`; remaining reasons: `Present`). Mutating it is `Symbol`. |
| `maps_unavailable_without_losing_raw_evidence` | Supported field shapes: unsupported/symbol-missing/unsupported-platform map to `Unsupported`; remaining failures with a vendor code map to `Failed(code)`; other failures map to `Unavailable`; every unavailable record has `None` value. Unknown vendor status -999 remains -999, never fabricated as a different code. |
| `device_loss_is_not_optional_missing_data` | `Reason::GpuLost` with/without vendor code, and any failure with vendor status 15, yield `DeviceLost` for all six metrics. Check this before other conversion errors so device loss cannot be downgraded. |
| `raw_field_survives_success_and_every_error` | Compare serialized raw field before/after using the existing serialization stack: unit, symbol presence, typed reason, optional vendor code, arbitrary detail text and exact value are unchanged for successful and rejected conversions. No additional serialization dependency may be introduced; if the worker cannot directly serialize the type with its existing direct dependencies, compare each field and the `Value` structurally instead. |

- [x] **Step 2: Run only the new CPU-only test target and observe the expected missing-module/API failure.**

Run: `cargo +stable test -p autotune-benchmark-worker --test sampler --locked --offline`

Expected: compilation failure because `sampler` does not yet exist. Retain the result; an unavailable toolchain/linker is a blocker, not the expected red test.

- [x] **Step 3: Implement the declared API and module registration.**

Use the existing path dependency `autotune-probe = { path = "../autotune-probe" }`.
Update only its local workspace dependency edge in `Cargo.lock`; resolve offline
without changing external versions. Validate device loss first, exact unit and
symbol consistency next, then reading shape/status. Use `Value::as_i64` without
float/string coercion. Do not range-check negative physical integers or percent
101 here: preserve them as records for the unchanged controller to reject.
Map unsupported reasons before the remaining vendor-status mapping. Return the
original moved `Field` on every branch; do not clone or rewrite it. `Record`
copies the supplied ticket/end/receipt unchanged and uses the ticket metric's
unit. No getter invocation, process API, allocation based on external lengths,
clock read or retention/admission claim is added.

- [x] **Step 4: Run the new mapping tests and require all to pass.**

Run: `cargo +stable test -p autotune-benchmark-worker --test sampler --locked --offline`

Expected: all mapping tests pass, none ignored. A Cargo lockfile update, if needed,
must be the local edge only and inspected before the locked rerun.

### Task 2: Controller composition and bounded closeout

**Files:** same test target, `docs/benchmark-worker.md`, `ROADMAP.md`.

**Interfaces:** consumes Task 1 API and existing
`Controller::new(binding, origin, Session::Pilot)`, `begin(anchor)`,
`complete(anchor, record)`, `poll(anchor)`, `request_stop(anchor)` and `report()`.
No new controller method, status or stop reason is introduced.

- [x] **Step 1: Add failing composition tests before changing implementation, if any change is necessary.**

Use deterministic anchors matching the existing monitor fixtures, not
`WindowsClock`. Assert:

- `converted_records_obey_existing_controller_validation`: matching pending
  tickets deliver available readings; negative physical values, utilization 101,
  query bracket 50000001 ns and transport age 100000001 ns are rejected by the
  controller. Equality at 50000000/100000000 passes where all other constraints
  hold. Preserve raw fields in `MappedSample` in both cases.
- `conversion_error_closes_admission_and_retains_raw`: explicit caller example
  matches `record`; on `Err`, retain `MappedSample` and call
  `Controller::request_stop` using the supplied receipt anchor. A valid subsequent
  sample never clears the first stop or makes fixture compute eligible. Include
  GPU loss on memory utilization and malformed temperature cases. Do not silently
  discard an error and continue polling until a convenient success.
- `late_return_cannot_reopen_stopped_controller`: start a ticket, request stop,
  convert and deliver its permitted late completion. First stop and deadlines
  remain identical; raw field and returned-record evidence remain available.
- `missing_optional_metric_is_not_zero_or_fresh_safety`: unsupported memory
  utilization has no value and cannot refresh temperature/power; the monitor
  records baseline insufficiency under its existing policy.

If an expectation disagrees with the frozen monitor contract, investigate rather
than changing that contract to fit this adapter. Additional tests may pass
immediately because the controller already implements the required behavior;
do not manufacture a red test by modifying existing policy.

- [x] **Step 2: Verify focused deterministic targets, library lint and formatting.**

Run each command separately, retain exit codes and source hashes:

```text
cargo +stable test -p autotune-benchmark-worker --test sampler --test monitor --test evidence --test envelope --locked --offline
cargo +stable test -p autotune-probe --lib --locked --offline
cargo +stable clippy -p autotune-benchmark-worker --lib --test sampler --test monitor --test evidence --test envelope --locked --offline -- -D warnings
cargo +stable fmt --all -- --check
```

Expected: each exits 0; record actual test counts, failures and exclusions. Do not
run `--all-targets` tests, CLI/native/ignored/process/Windows-clock targets or GPU
features to fill a coverage gap. Require the existing tests' native-admission
rejection to remain unchanged; source review confirms this adapter has no native
entry call. Where no test covers a claim, mark it unverified instead.

- [x] **Step 3: Obtain an independent final review of the adapter, tests and dependency diff.**

Review the seven mapping cases, four composition cases, native-entry absence,
raw-field retention, exact lockfile edge and unchanged existing contracts.
Resolve supported findings and repeat relevant checks on the final source.
No reviewer is authorized to execute native/CLI/process tests.

- [x] **Step 4: Update shared API docs and canonical handoff with source-bound evidence.**

In `docs/benchmark-worker.md`, describe APIs, mapping table, caller error-stop
obligation, conservative ticket-start convention and absence of actual clock/
getter/process execution. Update `ROADMAP.md` to report this subtask delivered
only after its final review/checks pass. Preserve E3 partial/Gate B unmet. Name
the next package-2 subtask: independent sampler transport/containment/supervision,
requiring its own reviewed scope before process implementation/execution.

- [x] **Step 5: Review final diff and hand off without unauthorized publication.**

Run `git diff --check` and `git status --short`; verify only intended files changed.
Do not stage/commit/push absent changeset-specific authorization. Report actual
outcome, limits and the next integration boundary, not merely a test count.

## Plan self-review

The short design's conversion/retention/error rules map to Task 1; controller
policy ownership and stopped/late behavior map to Task 2. All five review-focus
classes have named tests. Every new public signature is declared in Task 1;
Task 2 consumes it without changing controller APIs. Process isolation, actual
timestamp capture, IPC bounds, physical GPU mapping, consent, live export and
baseline/energy acceptance are explicitly outside this plan and remain required
by roadmap package 2/3. No completion claim for those gaps is permitted.

## Execution log — 2026-10-29

- Baseline: `eb8cdf51bdb4c55b6a272fe7b9004d99b26d6a2b` plus the preceding documentation/ignore cleanup, preserved unchanged except explicit plan/roadmap updates. Initial state was Task 1 in progress / Task 2 pending; completed task evidence follows chronologically. Command artifacts: `C:\Users\manni\AppData\Local\Temp\opencode\autotune-sampler-bridge-20261029-7fb6fa3a`.
- Pre-flight Task 1 → Task 2: `MappedSample` retains raw `Field` and a `Result<Record, MappingError>`; existing `Controller::complete` accepts `Record` and `request_stop` accepts the caller's anchor. Interfaces match; no controller/source-policy change required.
- Ruling: continue in the existing main checkout under the retained explicit dirty-main implementation preference and current plan approval; preserve all preceding work — avoids recreating tool worktrees immediately after owner-requested cleanup; cost if wrong is mixed uncommitted changes, mitigated by baseline/source inventories and no automatic commit.
- Ruling: use this execution section as the task ledger and approved external temp for command artifacts rather than adding a new `.superpowers/` scratch tree — the reviewed plan prohibits a second product-status ledger and the owner requested project cleanup; cost if wrong is reduced compatibility with the skill's Bash helper scripts, not loss of execution evidence.
- Ruling: run only the approved explicit CPU-only targets, not bare `cargo test` — broad execution would include prohibited CLI/OS-clock/process paths; cost is no fresh full-workspace runtime verification, reported explicitly.
- Task 1: Ruling: register the existing local probe dependency and its single lockfile edge before the red run — otherwise tests fail on an inaccessible dependency as well as the missing sampler API; cost is a harmless configuration-only ordering change. No external package/version changed.
- Task 1: complete (uncommitted; baseline monitor/evidence/envelope passed; `task1-red.log` records missing sampler API, Cargo exit 101; `task1-green.log` records all 7 mapping tests passing, exit 0). No source-policy/native-admission change. Task 2 in progress.
- Task 2 pre-review checkpoint: all four composition regressions pass without changing existing policy. `task2-tests.log`: 11 sampler + 36 monitor + 13 evidence + 26 envelope; `probe-library.log`: 18 ABI-fake tests. Scoped Clippy and workspace formatting pass. Total 104 selected CPU-only tests, none ignored/filtered. Independent whole-change review was pending at this checkpoint; no completion claim for process/live integration.
- Final review: independent read-only reviewer `ses_e792e253cffeEppHZ60oe7Bs7q` approves the bounded slice with no Critical/Important/Minor findings; no reviewer commands or independent runtime proof. No fix pass is needed and no minors are deferred.
- Final: Ruling: caller-enforced stopping/retention remains the declared API boundary — the wrapper retains raw evidence and tests demonstrate the caller's explicit stop; cost is that callers which ignore `Err` can violate safety, so future integration must enforce this obligation.
- Final: Ruling: `Field` does not authenticate metric/device origin — keep those as mandatory future transport/binding checks, not an invented guarantee of conversion; cost is possible same-unit misattribution if later integration omits them.
- Final: Ruling: raw detail ownership is not a bounded archival guarantee — this adapter does not allocate from external length claims; cost is caller-owned arbitrary field size until future IPC/retention bounds are enforced.
- Final: Ruling: the previously missing final manifest/current handoff are closeout work, not defects — update the canonical handoff and generate the before/after-bound manifest after final checks; cost is no source-bound completion claim until that final verification succeeds.
- Final: Ruling: earlier reviewed cleanup remains outside sampler-code review — verify preservation against the initial inventory and retain its separate archive evidence; cost is no new independent certification of cleanup operations.
- Task 2: complete (uncommitted; all 104 approved CPU-only tests, scoped Clippy and workspace formatting passed; independent whole-change review clean; shared API docs/design status and roadmap handoff updated). `git diff --check` passes with Git's configured LF→CRLF notices only; index is untouched. Final `final-*` command logs and `manifest.json` retain the post-review repeated checks, exact source before/after hashes and 115 preserved initial repository files. No commit/push or workspace/native/hardware acceptance is implied.
