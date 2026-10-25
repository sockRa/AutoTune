# AutoTune product roadmap and current status

Last reconciled: **2026-10-25**. Owner: the project owner; implementation agents maintain the evidence and session handoff below.

## Product goal — do not substitute a different product

Automate the tedious GPU tuning loop that currently requires manual increments and repeated benchmarks in MSI Afterburner:

1. Choose **Performance**, **Efficiency** or **Balanced** and a time budget.
2. Approve the testing session; AutoTune captures a repeatable baseline.
3. Try qualified settings, benchmark candidates, reject errors and stop safely.
4. Validate the best candidate more thoroughly and restore the starting settings.
5. Present a before/after comparison and an optional, separately consented profile application.

A valid outcome is **no better profile found** or **inconclusive**. A few benchmarks can find a promising configuration, not prove universal stability or a global optimum.

Initial target: Windows and the owner's **RTX 2070 SUPER**. Direct undervolting/V/F-curve editing is not established. Power-limit efficiency tuning must not be marketed as undervolting. CPU/BIOS/RAM writes and additional vendors are deferred.

Read-only discovery and benchmarking are foundations, **not fulfillment of the automatic-optimization goal**. A benchmark-first public MVP requires an explicit owner scope decision. If a required tuning gate proves infeasible, stop and discuss the scope; do not silently replace the goal or weaken safety requirements.

## How to read this plan

- This is the canonical **product-level progress and priority overview**, reconstructed from the owner's pasted feasibility session of 2026-10-14 and reconciled against the repository on 2026-10-24. The original research was delivered in chat, not as a checked-in PRD. This document preserves its E0–E8/story IDs and A–E release gates; it does not claim every research proposal was approved.
- **Done** means the stated story's exit criterion has supporting evidence. **Partial** means only a subset exists. **In progress** means active unfinished work. **Not started** means no implementation was identified in the inspected workspace. **Blocked** names an unmet prerequisite, not an estimate.
- Epic status is not the average of checked boxes. All required stories and applicable acceptance criteria must pass before an epic closes. A completed narrow implementation spec does not automatically complete its broader epic.
- This overview does not override detailed safety contracts or grant permission to execute workloads, touch hardware, elevate, commit or push. Existing approvals remain scoped to their respective work. Resolve contradictions explicitly with the owner.
- Evidence labels: **documented**, **simulated**, **Windows-observed**, **hardware-verified**, or **unresolved**. Fake success cannot qualify real tuning. Historical results remain dated; current verification is bound to exact revisions/manifests below.

## Where we are now

**Current track: E3 measurement foundations**, per the owner's priority. E1.1 is complete after an observed hosted CI run; synthetic telemetry, energy/repeat provenance, a pure direct-worker lifecycle model and per-metric warm-up/descriptive summaries are delivered, with full E3 stories still partial. E5/E6 recovery remains partial, not the default expansion track. We are not yet implementing the optimizer.

| Delivered slice | Evidence and limitation |
| --- | --- |
| Read-only NVIDIA probe and modern P0 offset reads | [GPU qualification](docs/nvidia-feasibility.md); observed on the RTX 2070 SUPER on 2026-10-15. Getter success is not write qualification. |
| Portable guarded power-limit simulation | [Model evidence](docs/power-trial-mock.md); completed mock milestone reports 55 tests including the probe. No Windows/native recovery guarantee. |
| Real Windows processes, IPC, journal and fake round trips | [Windows evidence](docs/windows-recovery.md); latest 2026-10-24 selected-control record reports 137 workspace tests passing / 14 ignored and 13/13 direct serial round trips. Historical timing/receipt failures remain unqualified. |
| Workspace checks and synthetic telemetry/energy/repeat evidence | [CI scope and observed runs](docs/ci.md), [measurement foundation](docs/measurement.md); fresh-target local Windows GNU fmt, workspace Clippy, test linking and 137 selected tests pass (13 recorder + 12 energy + 13 repeat tests), also separately verified from the actual repeat-delivery commit. Hosted Linux/MSVC runs passed for foundation `8778c5f4f241631b8282631039abd50b7d35d881`, energy `f722522d0491ee0d9d12aa8c3ac753ec7b62b46b` and repeat ledger `d97671a0e1014db12b080e850632a3e67a238121`. No recovery acceptance execution or live sampling. |
| Synthetic lifecycle model | [Lifecycle](crates/autotune-measurement/src/lifecycle.rs) and [semantics](docs/measurement.md); 14 lifecycle tests / 151 selected tests, fmt, workspace Clippy/linking pass locally and in a separate byte-exact commit archive. Pushed `ad73825d232008c74ffc98a5057258bd8204af21`; [hosted run 40228219662](https://github.com/sockRa/AutoTune/actions/runs/40228219662) passes Linux and Windows/MSVC. No process/workload execution, termination/resource proof or full E3.1 acceptance. |
| Synthetic warm-up/descriptive summaries | [Summary](crates/autotune-measurement/src/summary.rs) preserves raw recordings and per-metric warm-up/source-index roles; complete post-warm-up evidence yields exact n/min/max, otherwise typed insufficient; variation/drift not assessed. 12 summary / 163 selected tests and workspace checks pass locally and from a byte-exact commit archive. Pushed `e85c3d9b982de15d4871d1edc166403513b16e4a`; [hosted run 40229676176](https://github.com/sockRa/AutoTune/actions/runs/40229676176) passes Linux/MSVC. No baseline sufficiency or improvement verdict. |

**Still missing:** an accepted Windows recovery milestone, qualified real writes/restoration, a benchmark/correctness pipeline, automatic search, and the desktop product workflow. Recovery acceptance remains incomplete regardless of source publication; preservation commits are not qualified recovery baselines.

Historical reconciliation baseline (2026-10-24): HEAD `11b0881603bdfc9e0dec7fdadd45c63dc9837e01` plus the then-existing dirty working tree. Recovery counts above are **not current verification**; detailed Windows evidence retains its own logs/manifests. New foundation baseline is pushed commit `8778c5f4f241631b8282631039abd50b7d35d881`, with later working-tree evidence below kept separate. The original 20-file preservation inventory remains at `C:\Users\manni\AppData\Local\Temp\opencode\autotune-roadmap-baseline-20261024.json`; preservation is not test acceptance.

## Dependency-ordered epics and small stories

### E0 — Feasibility contract · Partial

Dependencies: none. Exit: a coherent, reviewed API/license/safety contract with unresolved capabilities explicitly gated.

| Story | Status | Exit criterion / remaining work |
| --- | --- | --- |
| E0.1 API and license evidence | Partial | Probe ABI sources and [notices](docs/third-party-notices.md) exist. Consolidate the original research, control/API matrix, workload redistribution decisions and project-license decision; do not treat preliminary recommendations as clearance. |
| E0.2 Identity and capability schemas | Partial | [Probe report](crates/autotune-probe/src/report.rs) and synthetic recovery contracts exist. Reconcile product-level units, scope, privileges, readback, persistence and qualification states without asserting unsupported tuning access. |
| E0.3 Qualification protocol and prohibitions | Partial | A guarded trial protocol and failure matrix are specified in the local planning companions listed below; current [qualification boundary](docs/nvidia-feasibility.md) prohibits inference from reads to writes. Resolve/review proposed mechanisms, timers, resource ceilings and root policy before closing the contract. Execution/qualification remains E6. |

### E1 — Deterministic foundation · Partial

Dependencies: E0 contract relevant to each slice. Exit: testable Rust boundaries and reproducible fault fixtures with an automated CI check.

| Story | Status | Exit criterion / remaining work |
| --- | --- | --- |
| E1.1 Rust workspace and CI | Done — documented deterministic scope | [CI](.github/workflows/ci.yml), [coverage/commands](docs/ci.md) and [successful hosted run 40179174853](https://github.com/sockRa/AutoTune/actions/runs/40179174853) for commit `8778c5f4f241631b8282631039abd50b7d35d881`. Linux workspace portable paths and Windows/MSVC deterministic packages pass; Windows recovery targets compile/link but are not executed. This closes workspace/CI, not recovery or the entire E1 epic. |
| E1.2 Domain types and read/write boundaries | Partial | Read-only probe, independent simulator and recovery types exist. Reuse these boundaries when adding measurement/profile types; no general product domain layer is claimed complete. |
| E1.3 Scripted mock adapter | Done — existing probe/trial scope | Deterministic unavailable/error/late-completion/recovery fixtures and tests exist in [probe](crates/autotune-probe) and [trial](crates/autotune-power-trial). New product behavior must add its own mock coverage. |

### E2 — Read-only discovery · Partial; core probe delivered

Dependencies: E1. Exit: stable device mapping and a capability report that explicitly distinguishes availability, read access, privileges and unqualified writes.

| Story | Status | Exit criterion / remaining work |
| --- | --- | --- |
| E2.1 Enumerate and map GPUs | Done — discovery scope | Probe enumerates per-device identity and preserves independent unavailable readings; multi-GPU fixtures exist. Workload-to-GPU mapping must be verified when E3 adds workloads. |
| E2.2 API/driver availability | Done — implemented query set | Probe reports driver/runtime information and missing/unsupported/denied APIs; modern offset reads were observed. No writable-control qualification follows. |
| E2.3 Capability report with unsupported reasons | Partial | Versioned JSON with typed reasons exists. Complete/reconcile product privilege, persistence, effective-scope and qualification reporting under E0.2; add presentation under E8. |

### E3 — Measurement pipeline · Partial synthetic foundation

Dependencies: E2. Can be developed without E6 writes; workload execution still needs appropriate approval. Exit: a bounded, repeatable baseline pipeline with honest uncertainty.

| Story | Status | Exit criterion |
| --- | --- | --- |
| E3.1 Isolated workload lifecycle | Partial — pure synthetic model only; real backend blocked | Owner approved the recommended synthetic phase/stop/termination/deadline/unknown-outcome model. [Implementation](crates/autotune-measurement/src/lifecycle.rs) retains bound direct-worker evidence and immutable terminal outcomes/conflicts (14 tests). No fixed licensed backend, verified GPU mapping, real process isolation/cancellation/termination/resource evidence or execution approval. Recovery fake workers are not benchmark workers; full E3.1 remains open. |
| E3.2 Timestamped telemetry | Partial — synthetic recording only | [Recorder](crates/autotune-measurement) retains run/workload/device binding, raw units, run-relative observation/receipt times, typed missing data, receipt-time freshness and rejection marker under caller-supplied capacity/age policy. 13 synthetic tests pass. No live sampler, cadence/gap detection, clock qualification or workload mapping/integration; [limits](docs/measurement.md). |
| E3.3 Baselines and inconclusive results | Partial — synthetic energy, provenance and descriptive summaries | [Energy helper](crates/autotune-measurement/src/energy.rs): estimated/unavailable only (12 tests). [Repeat ledger](crates/autotune-measurement/src/repeat.rs): exact recordings, conditions/settings/version assertions/seeds and explicit unknowns (13 tests). [Summary](crates/autotune-measurement/src/summary.rs): explicit per-metric leading-count exclusions with raw retention, n/min/max or typed insufficient, variation/drift not assessed (12 tests). Qualified warm-up/required baseline conditions, actual variation/drift/inconclusive comparisons and E7.2 objectives remain unresolved. Approved 6A metadata-difference reporting remains unimplemented. |

### E4 — Correctness and error observation · Not started

Dependencies: E3. Exit: injected errors are detected and finite coverage/attribution limits are reported.

| Story | Status | Exit criterion |
| --- | --- | --- |
| E4.1 Known-answer workload | Not started | Verify GPU results against defined references/tolerances; a successful process exit alone cannot pass validation. |
| E4.2 Qualified VRAM backend | Not started | Approve licensing, GPU selection, cancellation, results and memory-coverage reporting before integration. No mandatory OCCT/3DMark bundling is assumed. |
| E4.3 Windows error observation | Not started | Correlate device loss, workload failures and relevant Windows events without claiming definitive causation or complete error detection. |

### E5 — Recovery foundation · In progress; acceptance open

Dependencies: E1; required before real writes. Exit: reviewed journal/reconciliation policy and required fake-worker Windows cases accepted with explicit proof limits.

| Story | Status | Exit criterion / remaining work |
| --- | --- | --- |
| E5.1 Pre-write journal | Partial | Model and protected Windows test-root publication exist. Resolve required storage/security cases and live/cold validation asymmetry; do not promote flush/readback into a power-loss guarantee. |
| E5.2 Startup reconciliation | Partial | Cold history is read-only and unresolved history blocks another trial. Complete required malformed-history, ownership and production-environment acceptance. No automatic restore replay. |
| E5.3 Guardian fault/recovery tests | In progress | Portable mock milestone is done; selected real-process Windows cases pass. Required console closure, asynchronous boundaries, reliability and remaining W-matrix cases still prevent full acceptance. |

### E6 — Conditional hardware control · Partial infrastructure; hardware blocked

Dependencies: E2, E5 and separate exact hardware consent. Exit: at least one useful control has qualified write/readback/restoration semantics on the target configuration; each extra control needs separate qualification.

| Story | Status | Exit criterion / remaining work |
| --- | --- | --- |
| E6.1 Secure on-demand guardian IPC | Partial — fake only | Authenticated real-process IPC and policy exist in [recovery crates](crates/autotune-recovery-harness). Finish Windows acceptance; production privileges/trust/root policy and native adapter remain unqualified. No elevation is authorized by this roadmap. |
| E6.2 Qualify one power-limit control | Blocked | Resolve applicable G1–G7 requirements and an exact fresh approved manifest; demonstrate one bounded write, trustworthy readback and explicit restoration. Unsupported/unsafe prerequisites mean stop. |
| E6.3 Qualify offsets/clock controls | Blocked | Only investigate after a viable qualification path; establish units, effective scope, persistence and restoration per control. Offset reads are not proof of write access or direct undervolting. |

### E7 — Conditional optimizer · Not started; blocked on E3/E4/E6

Dependencies: E3, E4, E6. Exit: bounded automatic exploration produces an independently validated recommendation or an honest original/no-improvement outcome.

| Story | Status | Exit criterion |
| --- | --- | --- |
| E7.1 Single-parameter search | Not started | Versioned deterministic policy, qualified bounds, time budget and safety stops; start with one parameter, not unrestricted multi-control search. |
| E7.2 Ranking and stopping rules | Not started | Define/review versioned objective metrics and budgets; repeat promising candidates, distinguish improvement/regression/inconclusive, retain raw evidence and terminate within policy. |
| E7.3 Independent final validation | Not started | Validate finalist beyond ranking tests, restore original settings before recommendation and allow no better candidate. Profile application remains separate consent. |

Objective-policy decision owned by E7.2, with measurement requirements fed into E3.3: the original research **proposed** maximizing measured performance, minimizing GPU energy per unit of work subject to a performance-loss budget, and a Balanced performance/efficiency trade-off. Define units, workload aggregation, uncertainty handling, loss budgets, thresholds and Balanced selection rules before claiming those modes implemented. These policies/values remain unapproved; missing power/energy evidence cannot support an efficiency claim.

### E8 — User workflow and release · Not started as an integrated product

Dependencies: relevant E2–E7 slices; planning/UI mocks may precede hardware qualification. Exit: a complete usable workflow plus reviewed release evidence, not merely a diagnostic CLI.

| Story | Status | Exit criterion / remaining work |
| --- | --- | --- |
| E8.1 Discovery/consent/stop/recovery UI | Not started | User can choose objective/time budget, understand limitations, stop and see unresolved recovery. Tauri remains a preliminary choice, not an implemented desktop. |
| E8.2 JSON and human reports | Partial | Probe/model/harness JSON exists. Add integrated session history, before/after comparison, coverage/limitations and human-readable export. |
| E8.3 Consented profile application | Blocked | Qualified control and compatible hardware/software identity are rechecked; fresh explicit consent required. No login-time automatic application. |
| E8.4 Packaging and license audit | Not started | Review dependencies/notices, project license, trusted packaging/signing and release safety documentation. |

## Release gates — original A–E, not G1–G7

| Gate | Current state | Required evidence |
| --- | --- | --- |
| A — Coherent API/license/safety contract | Partial | Close/review E0 decisions; resolve relevant contract inconsistencies and licensing questions. |
| B — Useful repeatable read-only baseline | Not met | E2–E4 baseline/correctness evidence, not just successful telemetry reads. |
| C — One recoverable real control | Blocked | Accepted applicable recovery prerequisites and E6.2 hardware evidence. Fake tests or a successful setter alone do not pass. |
| D — Complete tuning workflow | Not met | E7 plus relevant E8 workflow validated on an explicitly qualified configuration. |
| E — Release review | Not met | Safety, packaging, licensing and documentation reviewed for the declared support envelope. |

The recovery architecture has separate **W01–W24** acceptance cases and **G1–G7** prerequisites: guardian independence; protected ownership; storage consistency; native quiescence; physical-entry timing; trusted consent/isolation; and hardware recovery. Its **P0–P3** labels are proof tiers, **not** the original product-priority labels. Selected P1 Windows evidence does not prove P2 OS-crash persistence or P3 hardware qualification. None of G1–G7 is declared fully closed here; especially G4/G5 cannot be solved merely by adding more fake tests.

## Next work and decision checkpoints

1. **Owner-prioritized track — E3 after E1.1:** E1.1 and synthetic telemetry/energy/repeat/lifecycle/summary foundations have hosted results. Owner accepts recommended A choices 1–9: lifecycle first and E3.3 warm-up/descriptive summaries are now delivered/reviewed, independently commit-verified and hosted-passing. Caller-selected metadata differences/unknowns without a comparability claim (6A) is the next approved independent slice; do not ask the owner to approve that mechanism again. Backend/license, live execution/clock/resource policy and actual baseline/ranking decisions remain gated. No guessed thresholds, winner or recovery expansion.
2. **Before expanding recovery again:** report which remaining gates are software tasks, host-dependent tests or unresolved native feasibility constraints. Ask the owner about any unavailable prerequisite rather than inventing a stronger guarantee or endlessly widening implementation.
3. **Next product-facing track — E3.1–E3.3:** build on the synthetic recorder toward an approved workload's bounded lifecycle and telemetry integration; define cadence/freshness/gap and repeat-baseline acceptance without inventing numeric defaults. This may proceed independently of real writes. It does not authorize benchmark execution now or replace the tuning goal with a read-only release.
4. **Before any real write:** explicitly assess Gate C feasibility, applicable G1–G7 and the separate consent/privilege/recovery decisions. Passing fake infrastructure alone never authorizes E6.2.

These are documentation/priority checkpoints, not new implementation or hardware authorizations. No calendar promises are made before feasibility is settled.

## Session protocol — keep the roadmap current

**At the start of each implementation/planning session:**

1. Read this file and its latest handoff before selecting work; inspect the working tree without discarding changes.
2. Identify the exact epic/story and bounded exit criterion. Read its active implementation spec, safety contracts and latest evidence. Do not infer completion from artifact presence or old headings. If a required local contract is absent, stop the affected implementation/acceptance work and ask the owner to supply/recover it; do not reconstruct safety authority from this summary. Unrelated documentation can still proceed with the missing evidence marked unresolved.
3. If the user requests different work, record its epic mapping/priority decision. Ask before changing the product goal, safety contract or public-MVP scope.

**At the end of a session that changes implementation, evidence or priorities:**

1. Update affected story/epic/gate states here and the mutable task/evidence/change-log sections of the detailed implementation spec. Never silently edit frozen approved intent or acceptance contracts; changed scope/contract requires explicit owner renegotiation. Include dated source/test evidence, failures, ignored/blocked cases and what was not run. Bind new test claims to the full baseline commit plus relevant working-tree/source/artifact hashes or a retained evidence manifest; otherwise state that results are not verified against the current tree. Do not rewrite past failures into passes.
2. Replace the current handoff below with: work completed, remaining blockers, next bounded action and required owner decisions. Append a short dated checkpoint entry.
3. Mark `Last reconciled` accurately. If evidence conflicts or cannot be verified, keep the item partial/unresolved and explain it; do not create a competing progress file.
4. Report progress toward the product goal in the user handoff, not only test counts. Completion of documentation or a local spec does not mean product/release acceptance.

### Current handoff — 2026-10-25

- **Owner-authorized preservation checkpoint:** owner explicitly requested continuing on `main` and publishing all existing eligible changes to leave a clean tree. The 20 pre-existing dirty files are preserved without source edits, separately from approved 6A. Static publication inspection found no obvious secrets or unintended executable/evidence payloads; README's mixed-encoding/NUL tail already exists in HEAD. Ignored local planning/evidence remain ignored. This is preservation, not recovery acceptance or a claim that prior failures are fixed. Fresh offline deterministic verification and separate hosted observation are pending; hardware/workload/probe/elevation/install/system prohibitions remain unchanged. Next implementation is approved E3.3/6A metadata assertion differences/unknowns, without comparability claims.

### Previous summary delivery evidence — retained 2026-10-25

- **Second delivery under approved A choices:** E3.3 summary retains exact original recorder JSON and metric/source-index roles, explicitly excludes caller-supplied leading observation counts per metric (including missing/stale observations), and emits exact n/min/max only for complete fresh available included evidence. Otherwise typed insufficient has no extrema. Sticky recorder rejection blocks every metric. Variation/drift remain not assessed; this is not a baseline/comparability/correctness/efficiency verdict or qualified warm-up choice. Full E3.3/Gate B remain open.
- **Review/local evidence:** all three layers complete; five regression improvements cover nonmonotonic/repeated extrema, freshness boundary, unavailable metric isolation, capacity rejection and nested Unicode/escape retention. Rust 1.99.1 Windows GNU fresh-target fmt, warning-denied workspace Clippy, workspace test linking and **163 selected tests / 12 summary tests pass**, zero failed/ignored/filtered. Source/results manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-summary-parent-working-manifest-20261025.json` SHA-256 `19FA5AF418CB9507E64834BC9EB15DF4EAC9D6A83DCD5044C8AB5E959A43B88E`; native results/log prefix `autotune-summary-parent-working-20261025`. Actual commit and hosted evidence are recorded separately below, not inferred from local results.
- **Publication/actual commit:** pushed `e85c3d9b982de15d4871d1edc166403513b16e4a` from baseline `2c6ef15cd8ace6d4edc6bb0657acc629beafc2bc`. Separate fresh-target verification from a byte-exact LF commit archive passes fmt, warning-denied workspace Clippy, all test linking and **163 tests**, excluding dirty recovery. Manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-summary-committed-20261025\manifest.json` SHA-256 `480889BA5A250FBD49199E73DFF665E66E4F29503B64E8F41545F4745A5C21FD` binds full commit/tree, source/archive hashes and command results/native logs. All 20 unrelated dirty files still match start-of-lifecycle inventory. No existing assertions/schema/frozen recovery contracts changed; no hardware, workload/probe, elevation, installation or system configuration.
- **Hosted observation:** [run 40229676176](https://github.com/sockRa/AutoTune/actions/runs/40229676176) succeeds on Ubuntu 24.04 and Windows 2025/MSVC for exactly `e85c3d9b982de15d4871d1edc166403513b16e4a`, under docs/ci.md exclusions. Metadata/log retained in approved temp as `autotune-ci-40229676176-20261025.json` SHA-256 `01E126B18662CFFDB05B5BEAA84F35B8ACDDA86BE5C73EABFDE8614889865BC6` and `.log` SHA-256 `67836BA7AA054E20E0277726382A5ED64C170436AC750B5306F53963DD0D6E4B`. This evidence-only follow-up changes no Rust/CI and is not a separate observed runtime CI pass.
- **Records/next action / stop reason:** local `_bmad-output/implementation-artifacts/spec-e3-3-warmup-summary.md`, shared API in [measurement docs](docs/measurement.md). Both lifecycle and summary source deliveries are published and hosted-observed; stop at this accumulated-context boundary with no half-implemented task. Next bounded implementation is approved 6A caller-selected metadata difference/unknown reporting without comparability claims; no new owner question is needed for that mechanism. Do not reclassify this safe follow-up as owner-blocked. Real workload/backend/license/GPU mapping, resource/clock qualification and actual baseline/objective policies remain separately gated. Automatic GPU optimization remains the goal.

### Previous lifecycle delivery evidence — retained 2026-10-25

- **Owner decision:** "kör på det du rekommenderar" accepts all recommended A answers from the preceding nine-question checkpoint, with lifecycle first (9A). Approvals are for pure synthetic modeling and later metadata/warm-up/descriptive mechanisms, not workload/backend/license selection, actual thresholds, live execution or hardware authority. Original hardware/elevation/system/install prohibitions and focused publication authorization remain.
- **E3.1 delivery:** additive pure lifecycle model with full immutable run/device/workload/seed/direct-worker binding, explicit phases and supplied effective event times, positive caller absolute phase deadlines/event capacity, sticky rejection/conflict evidence and deterministic JSON. Stop does not establish termination; expiry freezes unknown (ties precede completion); late evidence cannot overwrite terminal phase/outcome. Descendants/orphans/resource release remain unknown and correctness not evaluated. Full E3.1 is partial, Gate B not met.
- **Review/local evidence:** three independent layers completed. Strengthened hidden-state rejection and destination deadline tests; 14 lifecycle tests / **151 selected tests** pass, zero failed/ignored/filtered, with fmt, warning-denied workspace Clippy and all workspace linking on a fresh parent Windows GNU target (Rust 1.99.1). Native logs/results under approved temp: `autotune-lifecycle-parent-working-20261025-{toolchain,fmt,clippy,link,tests}.log` and `-results.json`. Implementation-agent initial toolchain/linker failures are retained in its manifest/notes, not rewritten into passes. No real workload, probe, recovery acceptance or hardware execution.
- **Publication/actual commit:** pushed `ad73825d232008c74ffc98a5057258bd8204af21` from baseline `42381327b3a30318a151d67badbba16af03ad534`. Separate fresh-target verification from a byte-exact LF commit archive passes the same fmt/Clippy/link/151-test scope, excluding dirty recovery. Commit source/results manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-lifecycle-committed-20261025\manifest.json` SHA-256 `15D7CFF510DE763A9C892BFCA035420DE5522D84F9B7D9E0AA6C76CD3BCA6751`. Dirty-tree manifest `autotune-lifecycle-parent-working-manifest-20261025.json` SHA-256 `9E52702ACA9EC70909AA3B08EF6EF3272FE25DBA80C41B6C195BCCEDF951DA3A` remains separate. All 20 unrelated dirty files match `autotune-lifecycle-preservation-20261025.json`; preservation is not acceptance.
- **Hosted observation:** [run 40228219662](https://github.com/sockRa/AutoTune/actions/runs/40228219662) passes Ubuntu 24.04 and Windows 2025/MSVC for exactly `ad73825d232008c74ffc98a5057258bd8204af21`; docs/ci.md exclusions remain. Metadata/log retained in approved temp as `autotune-ci-40228219662-20261025.json` SHA-256 `30859C167996292ADA5D702826488265513FCF057A6B5325240A0F053DDB4F90` and `.log` SHA-256 `B55B2298EE136BDD620FE81E80F9AFE6A2E36C07431E2CBCFAD2C4DFAA6E3668`. Checkout's nonblocking Node 20 annotation persists. This documentation follow-up is not a separately observed runtime pass.
- **Records/next action:** local `_bmad-output/implementation-artifacts/spec-e3-1-synthetic-lifecycle.md`; shared API/owner intent in [measurement docs](docs/measurement.md). Lifecycle publication/observation is complete. Next scope the separately approved E3.3 warm-up/descriptive summary without changing existing recording or energy contracts; metadata-difference reporting is independent follow-up. Real backend/license/device mapping, process isolation/termination, resource and live-clock guarantees remain blocked; E7.2 objectives remain unapproved. Automatic GPU optimization remains the goal, not a benchmark-only MVP.

### Previous repeat delivery evidence — retained 2026-10-25

- **Delivered E3.3 slice:** pure synthetic repeat-run provenance ledger retains exact original recorder exports, immutable run/workload/device/seed labels and explicit conditions/settings/software-version assertions or unavailable reasons. Caller supplies positive run/category capacities; invalid input and duplicate IDs reject atomically with a sticky ledger rejection marker. Assessment remains `not_evaluated`; no baseline comparability, applied-setting proof, statistics, ranking or efficiency claim. Full E3.3/Gate B remain open.
- **Review/local verification:** three independent review layers completed; four export-regression improvements applied. Fresh-target Rust 1.99.1 Windows GNU fmt, warning-denied workspace Clippy, all workspace test linking and **137 selected tests pass**, zero failed/ignored/filtered, including 13 repeat tests. Initial reused-target Clippy failed on recovery imports; unchanged original dirty inventory and successful fresh-target rebuild are recorded without deleting artifacts or modifying recovery. Final output: `C:\Users\manni\AppData\Local\Temp\opencode\autotune-repeat-postreview-fresh-20261025.log`; failure retained in `autotune-repeat-postreview-20261025.log`. Working-tree source manifest `autotune-repeat-working-final-20261025.json` SHA-256 `82B5CCD9A92100BAF95225E40BE077A3CE430DC28B5304E7BB32301746CFAB48` binds that scope.
- **Publication/independent verification:** `d97671a0e1014db12b080e850632a3e67a238121` pushed to origin/main from baseline `af082f028053e02052edee14342e0cc04693cc27`. A separately exported LF archive matches every committed blob and passes the same checks/137 tests with its own fresh target, excluding unrelated dirty recovery. Manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-repeat-committed-20261025\manifest.json` SHA-256 `997C68D87922F4B49194BF6C71B65AD93662593CC5FE8967D7C4CBC2F4115A8D` binds commit/tree, source hashes, archive and `verification-lf.log`. Initial host-CRLF archive is superseded for byte-exact evidence, not silently relabeled.
- **Hosted observation:** [run 40225364733](https://github.com/sockRa/AutoTune/actions/runs/40225364733) succeeded for exactly `d97671a0e1014db12b080e850632a3e67a238121` on Ubuntu 24.04 and Windows 2025/MSVC, under the documented CI scope (no Windows recovery acceptance execution). Metadata/log retained in approved temp as `autotune-ci-40225364733-20261025.json` SHA-256 `BBAC184B7151C32FCAB7221CBCEE0486695C75E7970CEFBA10BA1F4CA5E66863` and `.log` SHA-256 `2AC6DC500FB6F0A2601FB89A814941D9F278E9B2F4866BA4BDEA1E0BFC33ED6D`. This documentation follow-up is not itself a separately observed runtime CI pass.
- **Implementation record:** `_bmad-output/implementation-artifacts/spec-e3-3-repeat-evidence.md` (local/ignored); API and limits are shared in [measurement documentation](docs/measurement.md). Existing recorder/energy tests and frozen contracts remain unchanged. No hardware, workload, live probe, elevation, installation or system configuration.
- **Next bounded action / stop reason:** E3.1 investigation found only high-level lifecycle requirements, not a transition/termination/cancellation contract. Before modeling, owner must settle observable phases, stop-versus-termination evidence/race rules, crash/descendant scope and phase/resource/deadline meaning; backend/license/GPU mapping and real execution remain separately gated. Before baseline comparison, decide required conditions/settings/version evidence, warm-up and variation/drift/seed rules; E7.2 owns ranking/objectives. These are real intent gaps, not values to invent through caller-supplied defaults. Investigation is recorded in local `e3-1-lifecycle-investigation-20261025.md`; no lifecycle code started. Stop at this completed-delivery/context boundary with no half-implemented task. Automatic benchmark/search/validation remains the product goal, not a benchmark-only MVP.

### Previous delivery evidence — retained 2026-10-25

- **Authorization/publication:** owner authorized relevant commits/pushes, superseding the earlier restriction. Hardware, live probes/workloads, elevation, system changes and installation remain prohibited. Focused foundation commit `8778c5f4f241631b8282631039abd50b7d35d881` was pushed to `origin/main`; unrelated recovery edits stayed unstaged. Its separately exported source snapshot passed 112 tests, fmt, Clippy and workspace linking before publication. Cold GNU dlltool/assembler failure and the successful existing-LLVM-MinGW retry are retained in `autotune-publish-verification-20261025.log` / `autotune-publish-link-retry-20261025.log` under approved OpenCode temp; no installation or weakened assertion.
- **E1.1 complete:** [hosted run 40179174853](https://github.com/sockRa/AutoTune/actions/runs/40179174853) succeeded on Ubuntu 24.04 and Windows 2025/MSVC for exactly `8778c5f4f241631b8282631039abd50b7d35d881`. Coverage remains limited as [documented](docs/ci.md); no Windows recovery acceptance claim. Retained metadata/log: `C:\Users\manni\AppData\Local\Temp\opencode\autotune-ci-40179174853-20261025.json` (SHA-256 `AA914292236B4F005AA915D53613D0FDE828773E7F79069EBAE06834D417F0E5`) and `.log` (SHA-256 `DF78CC476AF68C423F58E28C98B10FCFF511D0186739A7F00D99B3E1407515F7`).
- **New E3.3 subtask delivered:** pure synthetic energy estimate consumes the E3.2 recorder, requires exact window endpoints and explicit max gap, preserves missing/stale/rejected uncertainty, and exports exact checked arithmetic as estimated/unavailable only. No measured-energy or efficiency claim. Three review layers completed; source-binding documentation and three test-coverage cases improved. Full baseline/optimizer stories remain open.
- **Fresh local verification:** Rust 1.99.1 Windows GNU; fmt, warning-free workspace Clippy, all workspace test binaries linked, and **124 selected tests pass** (13 recorder + 12 energy), zero ignored/filtered. `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e3-3-postreview-working-20261025.log` retains the parent run. Earlier hosted success is not evidence for this new helper; publication snapshot verification and its own hosted observation are separate.
- **Independent publication evidence:** exact staged-source snapshot tree `f3edbf56b026932215dd5257a1409bb4c1a7cba5` also passes fmt, Clippy, workspace linking and 124 tests, excluding unrelated dirty recovery changes. Later roadmap-only evidence annotations do not alter tested source. Manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e3-3-final-evidence-20261025.json` (SHA-256 `4753588F075F7F5670D0BC5B890544F63F0370734DA789985F186A1C5566EBBD`) binds source hashes, full baseline and retained transcripts. All 20 original dirty inventory files and 25 unrelated publication-baseline files remain unchanged.
- **Energy publication/hosted result:** pushed `f722522d0491ee0d9d12aa8c3ac753ec7b62b46b` to `origin/main`; committed source matches the independently verified tree. [Run 40180217837](https://github.com/sockRa/AutoTune/actions/runs/40180217837) completed successfully on both Ubuntu 24.04 and Windows 2025/MSVC. Retained `C:\Users\manni\AppData\Local\Temp\opencode\autotune-ci-40180217837-20261025.json` (SHA-256 `7D21411C02920B2191636B0C5E6627BA57A028CEDE980082C0427011851564EC`) and `.log` (SHA-256 `024FE7AE4534AC2BAF527566902B5740E73AB92B2181F52B3C2B8FC020F8ED2F`). This evidence-only follow-up changes no tested Rust source or CI configuration; do not mislabel it as its own observed hosted run. A nonblocking checkout Node 20 deprecation annotation remains a maintenance follow-up, not a failed check.
- **Implementation records:** `_bmad-output/implementation-artifacts/spec-e1-1-workspace-ci.md`, `spec-e3-2-synthetic-telemetry.md`, and `spec-e3-3-synthetic-energy.md` in that directory. Frozen intent/safety contracts unchanged. Earlier source-bound 2026-10-24 recorder evidence remains at `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e3-2-postreview-evidence-20261024.json` (SHA-256 `48188F9F2CB402DE28F0D252B9F04AB0BAC368306D8E4D76104916ABCC467FDC`).
- **Next bounded action:** publication and both hosted observations are complete. Stop implementation at this accumulated-context boundary; in a fresh context, scope E3 repeat-run evidence/conditions or lifecycle modeling rather than recovery expansion. Actual backend integration still needs owner workload/license/GPU-selection/cancellation decisions; live sampling needs collection/time/cadence/gap policy; baseline comparisons need warm-up/conditions/variation policy. No defaults or approved objective thresholds were invented.
- **Still blocked/unqualified:** E3.1 real backend and execution; useful repeated baselines/correctness; E7.2 objective policy; native writes/restoration and prior recovery acceptance. No benchmark-only product substitution. Unrelated dirty recovery work remains preserved and outside publication.

### Checkpoint history

| Date | Checkpoint | Result |
| --- | --- | --- |
| 2026-10-24 | Original chat roadmap recovered and reconciled with workspace | E5/E6 infrastructure active; E3/E4/E7 and integrated E8 still missing. Automatic GPU optimization remains the goal. |
| 2026-10-24 | E1.1 local CI delivery; owner prioritizes E3 next | CI configured/reviewed; local 99-test deterministic scope, fmt, Clippy and workspace linking pass. Remote acceptance still unverified. |
| 2026-10-24 | E3.2 bounded synthetic telemetry recording | Pure recorder and 13 tests delivered/reviewed. Post-review 112-test deterministic scope, fmt, workspace Clippy/linking pass; live collection and full E3 remain incomplete. |
| 2026-10-25 | Owner authorizes relevant pushes; isolated publication verification | Selected staged source snapshot independently passes fmt, Clippy, workspace linking and 112 tests; unrelated recovery edits excluded. Hosted execution still pending. |
| 2026-10-25 | E1.1 hosted verification complete | Foundation `8778c5f4f241631b8282631039abd50b7d35d881` pushed; run 40179174853 passes Linux and Windows/MSVC. E1.1 done only for documented deterministic coverage. |
| 2026-10-25 | E3.3 synthetic estimated/unavailable energy | Reviewed exact integration with explicit caller window/gap; 12 energy tests and 124-test selected scope pass locally. Full baselines and objective policies remain incomplete. |
| 2026-10-25 | Energy delivery published and hosted-verified | `f722522d0491ee0d9d12aa8c3ac753ec7b62b46b` pushed; run 40180217837 passes Linux and Windows/MSVC. Broad E3.3 remains partial; unrelated dirty work preserved. |
| 2026-10-25 | E3.3 synthetic repeat-run provenance | Exact recorder snapshots and explicit conditions/settings/version assertions retained without baseline verdict. Three-layer review, 137-test working-tree and independent actual-commit verification pass. Pushed `d97671a0e1014db12b080e850632a3e67a238121`; hosted run 40225364733 passes Linux/MSVC. |
| 2026-10-25 | E3.1 lifecycle investigation | No approved lifecycle observation/cancellation/termination/bounding contract found. Implementation remains blocked pending intent decisions; no recovery-policy substitution or speculative lifecycle module. |
| 2026-10-25 | Owner approves A choices 1–9; E3.1 synthetic lifecycle | Model reviewed; 14 lifecycle / 151 selected tests and workspace fmt/Clippy/linking pass in dirty tree and actual commit archive. Pushed `ad73825d232008c74ffc98a5057258bd8204af21`; hosted run 40228219662 passes Linux/MSVC. Full E3.1 partial; E3.3 follow-up mechanisms approved separately. |
| 2026-10-25 | E3.3 per-metric warm-up/descriptive summary | Reviewed raw-retaining n/min/max or typed-insufficient helper; variation/drift not assessed. 12 summary / 163 selected tests and workspace checks pass in dirty tree and byte-exact commit archive. Pushed `e85c3d9b982de15d4871d1edc166403513b16e4a`; hosted run 40229676176 passes Linux/MSVC. Full E3.3 remains partial; approved metadata reporting is next. |
| 2026-10-25 | Owner requests publication of existing dirty recovery work | All eligible existing changes preserved separately from 6A; static publication inspection only at this checkpoint. No recovery acceptance or real hardware authority follows from committing/pushing. |

## Evidence map and BMAD navigation

Checked-in/source-level entry points: [documentation index](docs/README.md), [probe](crates/autotune-probe), [simulator](crates/autotune-power-trial), [recovery evidence](docs/windows-recovery.md), [shared Windows contract](docs/windows-recovery-contract.md), [storage contract](docs/windows-storage-v1.md).

**Local BMAD artifacts** below exist in this workspace but `_bmad-output/` is git-ignored. They may be absent from a fresh clone. Keep the product overview here version-control eligible; do not assume local evidence is shared or silently change ignore rules. Historical entries in those files may describe an older implementation state; use the dated evidence and current task list together.

- Completed implementation specs: `_bmad-output/implementation-artifacts/spec-read-only-nvidia-feasibility-probe.md`, `spec-modern-nvml-clock-offset-reads.md`, `spec-mock-guarded-power-limit-trial.md` (all in that directory).
- Active implementation spec: `_bmad-output/implementation-artifacts/spec-windows-fake-worker-recovery.md`.
- Completed E1/E3 implementation specs: `_bmad-output/implementation-artifacts/spec-e1-1-workspace-ci.md`, `spec-e3-2-synthetic-telemetry.md`, `spec-e3-3-synthetic-energy.md` (same directory). Only E1.1 has closed its full documented roadmap story; E3 specs close narrow synthetic subtasks.
- Repeat-run retention implementation record: `_bmad-output/implementation-artifacts/spec-e3-3-repeat-evidence.md`; only a synthetic E3.3 subtask, not baseline acceptance.
- Synthetic lifecycle implementation record: `_bmad-output/implementation-artifacts/spec-e3-1-synthetic-lifecycle.md`; owner decisions and internal model only, not a licensed/executing benchmark backend.
- Synthetic warm-up summary implementation record: `_bmad-output/implementation-artifacts/spec-e3-3-warmup-summary.md`; descriptive observation summaries only, not statistical sufficiency or completed baselines.
- Guarded experiment contract: `_bmad-output/specs/spec-guarded-power-limit-experiment/SPEC.md`, with `experiment-protocol.md`, `failure-tests.md` and `recovery-infrastructure-scope.md` in the same directory.
- Architecture and detailed gates: `_bmad-output/planning-artifacts/architecture/architecture-AutoTune-2026-10-17/ARCHITECTURE-SPINE.md` and `ACCEPTANCE-GATES.md`.

This is a product roadmap, **not** a generated BMAD sprint-status file or a claim that a formal PRD/story backlog has been completed. Use its stable IDs when creating later implementation stories; retain traceability to these exit criteria and detailed safety contracts. For an explicit BMAD status check, read this roadmap alongside the active spec rather than relying on sprint-file discovery alone.
