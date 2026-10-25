# AutoTune documentation

**Start with [the project roadmap and current status](../ROADMAP.md).** It preserves the automatic GPU-optimization goal, E0–E8 epics/stories, release gates, evidence links, next work and the session handoff/update routine.

## Implementation and evidence

- [Build and CLI usage](../README.md) — current tools, not a desktop MVP.
- [Workspace CI scope](ci.md) — deterministic checks and unverified hosted-run boundary.
- [Synthetic telemetry foundation](measurement.md) — E3.2 recording semantics, not live sampling or workload execution.
- [NVIDIA read-only qualification](nvidia-feasibility.md) — API sources, observed reads and unverified writes.
- [Power-trial simulation](power-trial-mock.md) — model behavior, test mapping and native blockers.
- [Windows fake-only recovery](windows-recovery.md) — dated results, failures and outstanding acceptance cases.
- [Windows recovery contract](windows-recovery-contract.md) — shared protocol and policy.
- [Windows storage contract](windows-storage-v1.md) — journal/ownership/publication semantics and proof limits.
- [Third-party notices](third-party-notices.md) — dependency notices, not a completed release license audit.

For BMAD planning, use the roadmap's local-artifact map. A document marked final/done can describe a completed design or narrow implementation slice without closing the broader product epic or qualifying hardware.
