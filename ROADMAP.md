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
- Evidence labels: **documented**, **simulated**, **Windows-observed**, **hardware-verified**, or **unresolved**. Fake success cannot qualify real tuning. Test counts below are historical reports, not tests rerun for this documentation change.

## Where we are now

**Current track: E1.1 workspace/CI and E3 measurement foundations**, per the owner's 2026-10-24 priority change. Local CI plumbing and the synthetic E3.2 recorder slice are delivered; full stories remain partial. E5/E6 recovery remains partial, not the default expansion track. We are not yet implementing the optimizer.

| Delivered slice | Evidence and limitation |
| --- | --- |
| Read-only NVIDIA probe and modern P0 offset reads | [GPU qualification](docs/nvidia-feasibility.md); observed on the RTX 2070 SUPER on 2026-10-15. Getter success is not write qualification. |
| Portable guarded power-limit simulation | [Model evidence](docs/power-trial-mock.md); completed mock milestone reports 55 tests including the probe. No Windows/native recovery guarantee. |
| Real Windows processes, IPC, journal and fake round trips | [Windows evidence](docs/windows-recovery.md); latest 2026-10-24 selected-control record reports 137 workspace tests passing / 14 ignored and 13/13 direct serial round trips. Historical timing/receipt failures remain unqualified. |
| Workspace checks and synthetic telemetry recording | [CI scope](docs/ci.md), [measurement foundation](docs/measurement.md); current local Windows GNU fmt, workspace Clippy and all workspace test linking pass; 112 selected deterministic tests pass (13 recorder tests). No recovery test execution, live sampling or hosted CI evidence in this session. |

**Still missing:** an accepted Windows recovery milestone, qualified real writes/restoration, a benchmark/correctness pipeline, automatic search, and the desktop product workflow. Current recovery work is partly uncommitted; progress here describes the inspected working tree, not a released or committed baseline.

Reconciliation baseline: HEAD `11b0881603bdfc9e0dec7fdadd45c63dc9837e01` plus the existing dirty working tree inspected on 2026-10-24. Historical test counts above were **not verified against the current tree** in this documentation session. The detailed Windows evidence links retained test logs/manifests; do not assume they match later edits. A local SHA-256 preservation inventory of the 20 pre-existing modified/untracked files was saved for this documentation change at `C:\Users\manni\AppData\Local\Temp\opencode\autotune-roadmap-baseline-20261024.json`; it proves preservation only, not test acceptance.

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
| E1.1 Rust workspace and CI | Partial — hosted verification pending | [CI](.github/workflows/ci.yml) and [coverage/commands](docs/ci.md) added and reviewed. Recorded 2026-10-24 local Windows GNU fmt, workspace Clippy, workspace test linking and 112 deterministic-package tests pass after E3.2 integration. Owner authorized relevant pushes on 2026-10-25; no hosted Linux/MSVC result observed yet. Windows recovery execution remains separate acceptance. |
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
| E3.1 Isolated workload lifecycle | Blocked — backend/license decision; not implemented | Fixed, licensed workload; explicit GPU selection; bounded execution, cancellation and crash handling. No approved backend/license was found. Recovery fake workers are not benchmark workers. |
| E3.2 Timestamped telemetry | Partial — synthetic recording only | [Recorder](crates/autotune-measurement) retains run/workload/device binding, raw units, run-relative observation/receipt times, typed missing data, receipt-time freshness and rejection marker under caller-supplied capacity/age policy. 13 synthetic tests pass. No live sampler, cadence/gap detection, clock qualification or workload mapping/integration; [limits](docs/measurement.md). |
| E3.3 Baselines and inconclusive results | Not started | Repeated runs record versions/settings/seeds/conditions, warm-up policy, variation and drift; noisy comparisons can remain inconclusive. Retain the samples needed for the explicit objective policy in E7.2, including distinguishing measured/estimated/unavailable energy. |

### E4 — Correctness and error observation · Not started

Dependencies: E3. Exit: injected errors are detected and finite coverage/attribution limits are reported.

| Story | Status | Exit criterion |
| --- | --- | --- |
| E4.1 Known-answer workload | Not started | Verify GPU results against defined references/tolerances; a successful process exit alone cannot pass validation. |
| E4.2 Qualified VRAM backend | Not started | Approve licensing, GPU selection, cancellation, results and memory-coverage reporting before integration. No mandatory OCCT/3DMark bundling is assumed. |
| E4.3 Windows error observation | Not started | Correlate device loss, workload failures and relevant Windows events without claiming definitive causation or complete error detection. |

### E5 — Recovery foundation · In progress; active track

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

1. **Owner-prioritized track — E1.1, then E3:** workspace/CI configuration, local verification and a synthetic E3.2 recorder are delivered. Relevant pushes are now authorized (2026-10-25); verify the selected commit snapshot independently of unrelated dirty recovery work before publication and observe hosted results separately. Then select a bounded E3 slice; resolve workload/license and live collection policy before backend integration. Do not invent those decisions or expand recovery by default.
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

### Current handoff — 2026-10-24

2026-10-25 continuation: the owner has explicitly authorized relevant pushes, superseding the earlier no-staging/commit/push restriction for coherent task deliveries. Hardware/workload/elevation/install prohibitions remain. A separately exported staged snapshot (tree `5a0db68b17f658da47db256b42a2d27cfbe5545e`, source unchanged by this documentation note) passes fresh offline fmt, workspace Clippy, workspace test linking and 112 deterministic tests on Windows GNU, without the unrelated dirty recovery edits. Retained logs: `C:\Users\manni\AppData\Local\Temp\opencode\autotune-publish-verification-20261025.log` and `autotune-publish-link-retry-20261025.log` in the same directory. The cold build initially failed because the installed GNU dlltool could not launch its assembler; the retry uses the already-installed LLVM-MinGW dlltool/ld.lld with process-local linker flags and passes. Nothing installed or weakened. Publication is prepared; hosted CI remains unobserved. Prior verification below is dated evidence, not this new run.

- **Delivered E1.1 slice:** Linux/Windows CI configuration, locked/offline verification after hosted provisioning, explicit Windows recovery execution boundary, and build documentation. Three review findings patched. E1.1 remains partial until a hosted run is observed; the prohibition on remote operations prevented closure, not local test failures.
- **Delivered E3.2 slice:** independent synthetic-only telemetry recorder with immutable identity/policy, six typed metrics, original values/timestamps, freshness-at-receipt, explicit unavailable reasons, bounded retention and sticky rejection evidence. It is a benchmark-pipeline building block, not workload execution, a baseline comparison or automatic optimization. Broad E3.2 stays partial. Three review layers completed; ordering/export test gaps patched; no frozen recovery contract changed.
- **Current verification:** installed Rust 1.99.1 Windows GNU; fmt, warning-free workspace Clippy, all workspace test binaries linked, and **112 selected deterministic tests passed (13 new)**, zero ignored/filtered in that scope. Recovery tests, Linux/MSVC, live probes/workloads/hardware and hosted jobs were not run. Initial implementer PATH/link-tool failures were resolved using already-installed tools, with no installation; retained in the detailed spec/log.
- **Evidence binding:** full baseline `11b0881603bdfc9e0dec7fdadd45c63dc9837e01` plus source/CI SHA-256 manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e3-2-postreview-evidence-20261024.json` (SHA-256 `48188F9F2CB402DE28F0D252B9F04AB0BAC368306D8E4D76104916ABCC467FDC`), with retained post-review transcript. Earlier E1.1 manifest remains at `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e1-1-evidence-20261024.json`; its 99-test record predates the new library. Detailed specs: `_bmad-output/implementation-artifacts/spec-e1-1-workspace-ci.md` and `spec-e3-2-synthetic-telemetry.md` in the same directory, both done only for their narrow local slices.
- **Stop/continuation:** two coherent deliveries implemented, reviewed and verified; stop at this context boundary rather than start another implementation with a long accumulated review context. Fresh-session next action: read these specs and select a bounded measurement-pipeline slice, keeping existing product/safety decisions explicit. Before E3.1 integration, the owner must settle workload/backend license and GPU-selection/cancellation contracts; before live sampling, settle collection/time/cadence/gap policy. E3.3 warm-up/conditions/variation and E7.2 objective policies remain unapproved. No benchmark-only product substitution.
- **Still blocked:** hosted CI observation (remote operations prohibited); approved real workload/backend and execution; native write/restoration qualification and all prior recovery acceptance gaps. A pure witness CI allowlist suggestion is deferred, not prioritized over measurement progress.
- **Preservation/version control:** all 54 pre-existing crate files match the E1.1 source inventory; all 20 earlier dirty-work inventory files remain unchanged. Nothing staged, committed, pushed or fetched. No recovery implementation expansion.

### Checkpoint history

| Date | Checkpoint | Result |
| --- | --- | --- |
| 2026-10-24 | Original chat roadmap recovered and reconciled with workspace | E5/E6 infrastructure active; E3/E4/E7 and integrated E8 still missing. Automatic GPU optimization remains the goal. |
| 2026-10-24 | E1.1 local CI delivery; owner prioritizes E3 next | CI configured/reviewed; local 99-test deterministic scope, fmt, Clippy and workspace linking pass. Remote acceptance still unverified. |
| 2026-10-24 | E3.2 bounded synthetic telemetry recording | Pure recorder and 13 tests delivered/reviewed. Post-review 112-test deterministic scope, fmt, workspace Clippy/linking pass; live collection and full E3 remain incomplete. |
| 2026-10-25 | Owner authorizes relevant pushes; isolated publication verification | Selected staged source snapshot independently passes fmt, Clippy, workspace linking and 112 tests; unrelated recovery edits excluded. Hosted execution still pending. |

## Evidence map and BMAD navigation

Checked-in/source-level entry points: [documentation index](docs/README.md), [probe](crates/autotune-probe), [simulator](crates/autotune-power-trial), [recovery evidence](docs/windows-recovery.md), [shared Windows contract](docs/windows-recovery-contract.md), [storage contract](docs/windows-storage-v1.md).

**Local BMAD artifacts** below exist in this workspace but `_bmad-output/` is git-ignored. They may be absent from a fresh clone. Keep the product overview here version-control eligible; do not assume local evidence is shared or silently change ignore rules. Historical entries in those files may describe an older implementation state; use the dated evidence and current task list together.

- Completed implementation specs: `_bmad-output/implementation-artifacts/spec-read-only-nvidia-feasibility-probe.md`, `spec-modern-nvml-clock-offset-reads.md`, `spec-mock-guarded-power-limit-trial.md` (all in that directory).
- Active implementation spec: `_bmad-output/implementation-artifacts/spec-windows-fake-worker-recovery.md`.
- Completed narrow E1/E3 implementation slices: `_bmad-output/implementation-artifacts/spec-e1-1-workspace-ci.md` and `spec-e3-2-synthetic-telemetry.md` (same directory). Neither closes its whole roadmap story.
- Guarded experiment contract: `_bmad-output/specs/spec-guarded-power-limit-experiment/SPEC.md`, with `experiment-protocol.md`, `failure-tests.md` and `recovery-infrastructure-scope.md` in the same directory.
- Architecture and detailed gates: `_bmad-output/planning-artifacts/architecture/architecture-AutoTune-2026-10-17/ARCHITECTURE-SPINE.md` and `ACCEPTANCE-GATES.md`.

This is a product roadmap, **not** a generated BMAD sprint-status file or a claim that a formal PRD/story backlog has been completed. Use its stable IDs when creating later implementation stories; retain traceability to these exit criteria and detailed safety contracts. For an explicit BMAD status check, read this roadmap alongside the active spec rather than relying on sprint-file discovery alone.
