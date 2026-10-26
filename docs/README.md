# AutoTune documentation

**Start with [the project roadmap and current status](../ROADMAP.md).** It preserves the automatic GPU-optimization goal, E0–E8 epics/stories, release gates, evidence links, next work and the session handoff/update routine.

## Implementation and evidence

- [Build and CLI usage](../README.md) — current tools, not a desktop MVP.
- [Workspace CI scope](ci.md) — deterministic checks and unverified hosted-run boundary.
- [Synthetic telemetry and energy evidence](measurement.md) — E3.2 recording and E3.3 estimation semantics, not live sampling or workload execution.
- [Pure FMA contract and CPU reference](fma-reference.md) — frozen design validation, deterministic inputs and checking supplied vectors, not GPU execution or baseline acceptance.
- [Canonical live-evidence primitives](live-evidence.md) — phase-1 compact bytes, decimal integers/ratios, domain-separated digests and strict decoding; no envelope fields, live producer, clock/process execution or evidence authority.
- [Isolated benchmark-worker foundation](benchmark-worker.md) — prepared reference, injected fixtures, compile-only CUDA adapter and owned Windows fake containment; no live admission.
- [NVIDIA read-only qualification](nvidia-feasibility.md) — API sources, observed reads and unverified writes.
- [Power-trial simulation](power-trial-mock.md) — model behavior, test mapping and native blockers.
- [Windows fake-only recovery](windows-recovery.md) — dated results, failures and outstanding acceptance cases.
- [Windows recovery contract](windows-recovery-contract.md) — shared protocol and policy.
- [Windows storage contract](windows-storage-v1.md) — journal/ownership/publication semantics and proof limits.
- [Third-party notices](third-party-notices.md) — dependency notices, not a completed release license audit.

For BMAD planning, use the roadmap's local-artifact map. A document marked final/done can describe a completed design or narrow implementation slice without closing the broader product epic or qualifying hardware.
