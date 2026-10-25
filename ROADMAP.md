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

**Current track: E3 measurement foundations**, per the owner's priority. E1.1 is complete after an observed hosted CI run; synthetic E3.2 recording and the E3.3 energy-evidence subtask are delivered, with full E3 stories still partial. E5/E6 recovery remains partial, not the default expansion track. We are not yet implementing the optimizer.

| Delivered slice | Evidence and limitation |
| --- | --- |
| Read-only NVIDIA probe and modern P0 offset reads | [GPU qualification](docs/nvidia-feasibility.md); observed on the RTX 2070 SUPER on 2026-10-15. Getter success is not write qualification. |
| Portable guarded power-limit simulation | [Model evidence](docs/power-trial-mock.md); completed mock milestone reports 55 tests including the probe. No Windows/native recovery guarantee. |
| Real Windows processes, IPC, journal and fake round trips | [Windows evidence](docs/windows-recovery.md); latest 2026-10-24 selected-control record reports 137 workspace tests passing / 14 ignored and 13/13 direct serial round trips. Historical timing/receipt failures remain unqualified. |
| Workspace checks and synthetic telemetry/energy evidence | [CI scope and observed run](docs/ci.md), [measurement foundation](docs/measurement.md); local Windows GNU fmt, workspace Clippy, test linking and 124 selected tests pass (13 recorder + 12 energy tests). Hosted Linux/MSVC run passed for foundation commit `8778c5f4f241631b8282631039abd50b7d35d881`; that run predates the energy helper. No recovery acceptance execution or live sampling. |

**Still missing:** an accepted Windows recovery milestone, qualified real writes/restoration, a benchmark/correctness pipeline, automatic search, and the desktop product workflow. Current recovery work is partly uncommitted; progress here describes the inspected working tree, not a released or committed baseline.

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
| E3.1 Isolated workload lifecycle | Blocked — backend/license decision; not implemented | Fixed, licensed workload; explicit GPU selection; bounded execution, cancellation and crash handling. No approved backend/license was found. Recovery fake workers are not benchmark workers. |
| E3.2 Timestamped telemetry | Partial — synthetic recording only | [Recorder](crates/autotune-measurement) retains run/workload/device binding, raw units, run-relative observation/receipt times, typed missing data, receipt-time freshness and rejection marker under caller-supplied capacity/age policy. 13 synthetic tests pass. No live sampler, cadence/gap detection, clock qualification or workload mapping/integration; [limits](docs/measurement.md). |
| E3.3 Baselines and inconclusive results | Partial — synthetic energy-evidence subtask only | [Energy helper](crates/autotune-measurement/src/energy.rs) conservatively integrates complete fresh synthetic power windows under explicit caller gap policy, reporting estimated/unavailable, never measured energy. 12 tests pass. Repeated-run conditions/settings/versions/seeds, warm-up, variation/drift and inconclusive comparisons remain unimplemented; E7.2 objective policy remains unapproved. |

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

1. **Owner-prioritized track — E3 after E1.1:** E1.1 now has successful hosted evidence. Synthetic telemetry and estimated/unavailable energy foundations are delivered. Verify each selected commit snapshot independently of unrelated dirty recovery work, push relevant deliveries and observe hosted results separately. Next scope repeat-run evidence or workload lifecycle without inventing workload/license, collection or objective-policy decisions.
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

- **Authorization/publication:** owner authorized relevant commits/pushes, superseding the earlier restriction. Hardware, live probes/workloads, elevation, system changes and installation remain prohibited. Focused foundation commit `8778c5f4f241631b8282631039abd50b7d35d881` was pushed to `origin/main`; unrelated recovery edits stayed unstaged. Its separately exported source snapshot passed 112 tests, fmt, Clippy and workspace linking before publication. Cold GNU dlltool/assembler failure and the successful existing-LLVM-MinGW retry are retained in `autotune-publish-verification-20261025.log` / `autotune-publish-link-retry-20261025.log` under approved OpenCode temp; no installation or weakened assertion.
- **E1.1 complete:** [hosted run 40179174853](https://github.com/sockRa/AutoTune/actions/runs/40179174853) succeeded on Ubuntu 24.04 and Windows 2025/MSVC for exactly `8778c5f4f241631b8282631039abd50b7d35d881`. Coverage remains limited as [documented](docs/ci.md); no Windows recovery acceptance claim. Retained metadata/log: `C:\Users\manni\AppData\Local\Temp\opencode\autotune-ci-40179174853-20261025.json` (SHA-256 `AA914292236B4F005AA915D53613D0FDE828773E7F79069EBAE06834D417F0E5`) and `.log` (SHA-256 `DF78CC476AF68C423F58E28C98B10FCFF511D0186739A7F00D99B3E1407515F7`).
- **New E3.3 subtask delivered:** pure synthetic energy estimate consumes the E3.2 recorder, requires exact window endpoints and explicit max gap, preserves missing/stale/rejected uncertainty, and exports exact checked arithmetic as estimated/unavailable only. No measured-energy or efficiency claim. Three review layers completed; source-binding documentation and three test-coverage cases improved. Full baseline/optimizer stories remain open.
- **Fresh local verification:** Rust 1.99.1 Windows GNU; fmt, warning-free workspace Clippy, all workspace test binaries linked, and **124 selected tests pass** (13 recorder + 12 energy), zero ignored/filtered. `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e3-3-postreview-working-20261025.log` retains the parent run. Earlier hosted success is not evidence for this new helper; publication snapshot verification and its own hosted observation are separate.
- **Independent publication evidence:** exact staged-source snapshot tree `f3edbf56b026932215dd5257a1409bb4c1a7cba5` also passes fmt, Clippy, workspace linking and 124 tests, excluding unrelated dirty recovery changes. Later roadmap-only evidence annotations do not alter tested source. Manifest `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e3-3-final-evidence-20261025.json` (SHA-256 `4753588F075F7F5670D0BC5B890544F63F0370734DA789985F186A1C5566EBBD`) binds source hashes, full baseline and retained transcripts. All 20 original dirty inventory files and 25 unrelated publication-baseline files remain unchanged.
- **Implementation records:** `_bmad-output/implementation-artifacts/spec-e1-1-workspace-ci.md`, `spec-e3-2-synthetic-telemetry.md`, and `spec-e3-3-synthetic-energy.md` in that directory. Frozen intent/safety contracts unchanged. Earlier source-bound 2026-10-24 recorder evidence remains at `C:\Users\manni\AppData\Local\Temp\opencode\autotune-e3-2-postreview-evidence-20261024.json` (SHA-256 `48188F9F2CB402DE28F0D252B9F04AB0BAC368306D8E4D76104916ABCC467FDC`).
- **Next bounded action:** publish the independently verified slice and observe its hosted result, then start a fresh bounded E3 specification for repeat-run evidence/conditions or lifecycle modeling. Stop implementation at this accumulated-context boundary after publication rather than start another incompletely reviewed slice. Actual backend integration still needs owner workload/license/GPU-selection/cancellation decisions; live sampling needs collection/time/cadence/gap policy; baseline comparisons need warm-up/conditions/variation policy. No defaults or approved objective thresholds were invented.
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

## Evidence map and BMAD navigation

Checked-in/source-level entry points: [documentation index](docs/README.md), [probe](crates/autotune-probe), [simulator](crates/autotune-power-trial), [recovery evidence](docs/windows-recovery.md), [shared Windows contract](docs/windows-recovery-contract.md), [storage contract](docs/windows-storage-v1.md).

**Local BMAD artifacts** below exist in this workspace but `_bmad-output/` is git-ignored. They may be absent from a fresh clone. Keep the product overview here version-control eligible; do not assume local evidence is shared or silently change ignore rules. Historical entries in those files may describe an older implementation state; use the dated evidence and current task list together.

- Completed implementation specs: `_bmad-output/implementation-artifacts/spec-read-only-nvidia-feasibility-probe.md`, `spec-modern-nvml-clock-offset-reads.md`, `spec-mock-guarded-power-limit-trial.md` (all in that directory).
- Active implementation spec: `_bmad-output/implementation-artifacts/spec-windows-fake-worker-recovery.md`.
- Completed E1/E3 implementation specs: `_bmad-output/implementation-artifacts/spec-e1-1-workspace-ci.md`, `spec-e3-2-synthetic-telemetry.md`, `spec-e3-3-synthetic-energy.md` (same directory). Only E1.1 has closed its full documented roadmap story; E3 specs close narrow synthetic subtasks.
- Guarded experiment contract: `_bmad-output/specs/spec-guarded-power-limit-experiment/SPEC.md`, with `experiment-protocol.md`, `failure-tests.md` and `recovery-infrastructure-scope.md` in the same directory.
- Architecture and detailed gates: `_bmad-output/planning-artifacts/architecture/architecture-AutoTune-2026-10-17/ARCHITECTURE-SPINE.md` and `ACCEPTANCE-GATES.md`.

This is a product roadmap, **not** a generated BMAD sprint-status file or a claim that a formal PRD/story backlog has been completed. Use its stable IDs when creating later implementation stories; retain traceability to these exit criteria and detailed safety contracts. For an explicit BMAD status check, read this roadmap alongside the active spec rather than relying on sprint-file discovery alone.
