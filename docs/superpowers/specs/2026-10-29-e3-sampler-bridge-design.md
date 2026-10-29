# E3 selective-getter to monitor bridge

Date: 2026-10-29. Engineering proposal under the owner's roadmap-completion
direction. This is a small implementation boundary in delivery package 2, not
approval for a live sampler process or hardware execution.

## Intent

Connect the existing `autotune-probe::report::Field` and selective getter metric
types to `autotune-benchmark-worker::monitor::{Ticket, Record}` without losing
raw failures or inventing timestamps. Today those components are disconnected.
The future sampler must use this boundary rather than independently reinterpret
units/status/value conventions in its process/transport code.

## Goals and non-goals

- A deterministic, portable adapter maps the six metrics and exact NVML unit
  strings, preserving the complete owned raw `Field` beside conversion success
  or error. Integers never pass through floating-point conversion.
- A conversion is not validation/admission. The existing controller remains
  responsible for ticket, timing, value-range, freshness and sticky-stop rules.
- Inputs supply the existing ticket and run-relative query-end/receipt times.
  The adapter reads no clock, derives no sensor timestamp and cannot establish
  their truth. Actual sampler query-start anchors still need separate transport
  evidence; the ticket's controller start is not relabeled as a physical getter
  start. Existing monitor intervals stay conservative and unchanged.
- Malformed fields and device-loss evidence cannot become an available sample.
  Callers must retain the returned raw field and close admission on conversion
  error; tests show that composition with the existing controller.
- No getter invocation, native DLL loading, process launch, new IPC format,
  manifest/consent authority, live-provenance label, evidence writer, baseline,
  energy, recovery or worker native-admission change.

## Approaches

1. **Map probe fields once at the sampler/monitor boundary — recommended.** Add
   a small pure module to the worker crate and a path dependency on the existing
   probe library. Reuses the real getter result type; introduces no new package
   or external dependency. This is the agent's engineering recommendation, not a
   separately selected owner API preference.
2. **Convert independently inside a future process executable.** Avoids a small
   module now, but leaves unit/status handling coupled to Windows transport and
   harder to test portably. Rejected for this draft.

Neither approach replaces the independently killable sampler required by the
integration contract. Completing this adapter must lead to that process and
supervisor work, not another series of unrelated synthetic helpers.

## Engineering decisions for review

The API is defined in the paired implementation plan. Available values require
the exact metric unit, `SymbolPresence::Present` and an exact JSON integer that
fits `i64`. Physical range checks remain with `Controller::complete`.
Unavailable readings preserve raw reason/vendor status/detail/symbol evidence:
unsupported/symbol-missing/unsupported-platform map to `Status::Unsupported`;
other vendor-coded failures map to `Status::Failed(code)`; other unavailable
readings map to `Status::Unavailable`. GPU loss (reason or vendor status 15)
is a conversion error, never a benign missing optional metric. Contradictory
symbol/reading evidence is rejected according to the plan's exact table.

No new numeric policy. Keep the approved profile and all existing monitor,
probe-report and canonical evidence contracts unchanged. Raw `Field` ownership
is not a new bounded archival guarantee; future IPC/retention must enforce the
approved resource limits before accepting external lengths.

## Acceptance

Table-driven CPU-only mapping tests and controller-composition regressions prove
the exact conversions, failure retention and no reopening after a sticky stop.
The existing worker still rejects live admission. Independent review and
source-bound test evidence are required before marking this subtask delivered.
The implementation plan is ready for review, not an implementation/test result.

Next after this subtask: separately scoped bounded sampler transport, independent
Windows containment and nonblocking supervision; then physical GPU/worker/clock/
manifest/consent binding and live evidence production. Native execution remains
gated by an exact approved manifest. E3 stays partial and Gate B unmet.

References: [roadmap](../../../ROADMAP.md), [selective getter](../../nvidia-feasibility.md),
[monitor](../../benchmark-worker.md), and local
`_bmad-output/specs/spec-e3-first-real-benchmark/{integration-contract,qualification-profile}.md`.
The local contracts must be present and read before dependent execution work.

## Implementation record — 2026-10-29

The owner approved the paired implementation plan with `ja kör`. Its two tasks
are implemented with 11 new CPU-only tests and unchanged controller/probe/native
policy. Independent final static review found no defects; actual command counts,
red/green history and final source binding are recorded in the paired plan,
shared worker documentation and canonical roadmap handoff. This status update
does not revise the approved intent or expand hardware/process authority.
