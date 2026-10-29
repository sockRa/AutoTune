# AutoTune project navigation

- Read [ROADMAP.md](ROADMAP.md) before project planning, status reporting or implementation. It is the canonical product-level overview, with E0–E8 story IDs, A–E release gates, current priorities and handoff.
- Follow its start/end-of-session protocol: map the work to a story, consult the relevant detailed spec/evidence, and update affected statuses and the handoff when implementation, evidence or priorities change. Do not create a competing progress summary.
- The goal is automated GPU benchmark/search/validation, not a silently substituted read-only MVP. Preserve the roadmap's distinction between product release gates and recovery W/G requirements and proof tiers.
- The roadmap is not hardware/workload/elevation authorization and does not override detailed safety contracts. Simulated or Windows fake-worker evidence is not real tuning qualification. Missing or conflicting evidence stays explicitly unresolved.
- See [docs/README.md](docs/README.md) for documentation entry points. Detailed local BMAD artifacts are listed in the roadmap; `_bmad-output/` is ignored and may be absent in another checkout.

## Agent skills

### Issue tracker

Use GitHub Issues in `sockRa/AutoTune` for tickets and issue-backed specs.
Read `docs/agents/issue-tracker.md` before tracker operations.

### Triage labels

Use the five canonical triage labels.
Read `docs/agents/triage-labels.md` before triaging.

### Domain docs

Use a single-context glossary and ADR layout.
Read `docs/agents/domain.md` before domain exploration or design.
