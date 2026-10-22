---
id: SPEC-guarded-power-limit-experiment
status: architecture-adopted-live-blocked
created: 2026-10-17
updated: 2026-10-18
companions:
  - experiment-protocol.md
  - failure-tests.md
  - recovery-infrastructure-scope.md
  - ../../planning-artifacts/architecture/architecture-AutoTune-2026-10-17/ARCHITECTURE-SPINE.md
  - ../../planning-artifacts/architecture/architecture-AutoTune-2026-10-17/ACCEPTANCE-GATES.md
sources:
  - ../../../docs/nvidia-feasibility.md
  - ../../../docs/power-trial-mock.md
  - ../../../docs/windows-recovery.md
  - ../../../docs/windows-recovery-contract.md
  - ../../../docs/windows-storage-v1.md
---

# First guarded power-limit experiment

This spec and its companions form the contract. Architecture adoption is documentation only, not implementation or hardware authorization; `[PROPOSED]` choices and unmet gates remain explicit.

## Scoped continuation authorization — 2026-10-22

The user subsequently authorized unattended continuation of the already planned
fake-only Windows infrastructure, normal verification, local commits and ordinary
pushes on a separate automation branch. `stories.yaml` enumerates only remaining
work. This supersedes the historical documentation-only and no-Git restrictions
below for those stories only; it does not authorize any real hardware experiment,
elevation, system provisioning, destructive test or relaxation of AD/W/M/G gates.
`AGENTS.md` at the repository root defines execution and publication boundaries.
The source references now point to tracked evidence summaries rather than private
local implementation logs. Current source and tracked recovery/storage/contract
documents are the continuation baseline. Private local evidence is not published.

## Why

AutoTune needs narrowly scoped evidence of power-limit write access and restoration on the RTX 2070 SUPER before considering tuning. Modern NVML P0 offset reads work, but writes and restoration remain unverified. The human-approved portable mock milestone reports 55 passing tests; real Windows recovery infrastructure must next be demonstrated using fake workers only. Historical 240 W current and 215 W default are not restoration instructions.

## Capabilities

- **CAP-1**
  - **intent:** The user can authorize exactly one fully specified hardware trial.
  - **success:** Missing, stale, replayed or mismatched approval produces zero candidate permissions; approval binds the complete immutable manifest. Fake approval tests establish policy behavior only, never authenticated human hardware consent.
- **CAP-2**
  - **intent:** The experiment can select one unambiguous device and a bounded reversible candidate.
  - **success:** Fresh UUID/PCI, two stable current-limit captures and valid bounds are rechecked before mutation; only that captured current value is restored. Current infrastructure uses synthetic device identity and values exclusively.
- **CAP-3**
  - **intent:** Recovery intent survives a coordinator crash or uncertain mutation outcome.
  - **success:** Published mutation intent and independent guardian acknowledgement precede candidate dispatch; unresolved journals block another trial. Evidence distinguishes modeled publication, observed Windows process-crash behavior and unproven OS/power-loss durability.
- **CAP-4**
  - **intent:** The experiment can expire and recover without the coordinator's cooperation.
  - **success:** Immutable expiry closes candidate admission on cancellation, parent death, heartbeat loss or expiry; recovery never races an unresolved earlier operation. Unknown quiescence remains recovery-required, not a promise of restoration.
- **CAP-5**
  - **intent:** The user receives distinct write, readback and restoration evidence.
  - **success:** Exact candidate readback, explicit captured-baseline restore and two exact fresh baseline reads are recorded separately from API outcomes, timing violations and evidence durability; uncertain recovery remains unresolved.
- **CAP-6**
  - **intent:** Safety behavior can be demonstrated without physical hardware.
  - **success:** Inherited M01–M32 invariants and Windows W01–W24 acceptance cases are satisfied or explicitly blocked with proof-tier evidence, while existing probe/simulator regressions remain unchanged. A blocked required case prevents acceptance; fake tests never qualify hardware.

## Constraints

- **This update is documentation only.** No implementation, test execution, live probe, hardware setter, elevation, reset or workload. A later implementation needs its own authorization; a real experiment always needs separate explicit approval of its exact fresh manifest.
- Preserve both existing crates, their dependency graphs and fixtures, including `autotune-probe`'s read-only trait, schema 1, opaque-only setter inspection, trusted DLL loading and mutation-boundary tests. Its controls retain `write_verification: not_tested`.
- Adopt the architecture and acceptance companions without changing AD-1–AD-12. The next infrastructure milestone has real Windows process/storage/IPC behavior but only fake device workers, no NVML loader, native setter, live backend or arbitrary executable override.
- A future separate tool may use only the documented original power-limit setter for this trial. No offset writes, alternate APIs, reset, driver restart, persistence changes or silent privilege escalation.
- Proposed limits, deadlines and resource ceilings remain design choices awaiting review, not safe hardware specifications. A captured baseline is observed, not qualified known-good.
- No default, historical value, index-zero assumption, rounding, candidate retry or cleanup-on-drop assumption may replace the specified recovery protocol.
- Serialize all device operations, including getters. Timeout, cancellation or process termination cannot establish native quiescence; no replacement worker may race a possibly unresolved call.
- G1–G7 remain unmet; guardian expiry cannot guarantee driver completion, physical restoration or power-loss survival. An infeasible safety prerequisite blocks progress rather than silently weakening the invariant. No staging, commit, push or discard without separate authorization.

## Non-goals

- Optimizer, desktop UI, stress/performance/thermal qualification, undervolting, clock offsets, V/F curves, multi-device writes, profile application, boot integration or automatic journal replay.
- Hardware adapters, recovery-only hardware writes, services, scheduled tasks, destructive OS/power-loss tests and machine-wide provisioning in this documentation adoption or next fake-only milestone.
- Establishing that 215 W or 240 W is desirable, or that any returned bounds are validated safe limits.

## Success signal

- This adoption succeeds when the spec exposes the complete architecture, milestone boundary and acceptance matrices through valid companion paths, preserving CAP and AD IDs with all live gates closed.
- A later separately authorized infrastructure milestone succeeds only with reviewed contracts, passing required fake-worker/regression cases and explicit proof-tier limitations; prior 55-test success is not new Windows evidence.
- A later approved trial succeeds only with one exact candidate write/readback and verified restoration of its freshly captured baseline, durably recorded; it proves only that device/control/value/session round trip, not general tuning support.

## Assumptions

- A small downward-only change with immediate restoration is proposed to limit scope, not asserted safe or approved. Driver quantization and GeForce write permissions remain unknown.
- “Go ahead” here authorizes spec adoption only; it does not accept every proposed mechanism, timer or privilege choice or begin implementation.

## Open Questions

- Resolve shared schema/bootstrap vectors, exact Windows build/binding/toolchain selection, secure path/ACL opening and clock-discontinuity semantics before the relevant implementation/acceptance gates.
- Approve or revise proposed mechanisms, the 1–5 W envelope, timers, resource ceilings and protected-root policy; no exact hardware target is set.
- What are the fresh UUID, PCI identity, baseline and bounds, and can competing tuning tools and automatic profile reapplication be stopped?
- What privilege level, protected journal directory and device-specific manual recovery procedure will the user approve? NVIDIA documents admin requirements; no elevation is currently authorized.
- Can the unmet G1–G7 prerequisites be demonstrated adequately, especially native fencing and storage persistence? Fake process tests cannot settle these hardware gates; if a required prerequisite is infeasible, the experiment remains blocked.
