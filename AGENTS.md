# AutoTune unattended work

## Authorized scope

The user authorized unattended implementation, testing, review, local commits and
ordinary pushes on `automation/fake-recovery`. This supersedes historical
documentation-only / no-Git instructions only for the scoped continuation queue.
Do not ask for routine approval again. Stop on an unresolved intent conflict,
unsupported environment, exhausted retries, or a proposed scope expansion.

Use the existing guarded-power-limit specification and its architecture and
acceptance companions. The queue is a breakdown of unfinished work, not a new
product plan. Preserve AD-1--AD-12, M01--M32, W01--W24, CAP-1--CAP-6 and closed
hardware gates G1--G7. A fake test is never hardware qualification.

## Hard boundaries

- Only synthetic devices and harness-owned temporary files/processes/jobs.
- No live probe, NVML loading, hardware setters, GPU/power changes, elevation,
  services/tasks, persistent system configuration, clock changes, reboot, reset,
  stress testing, automatic journal replay or production-root provisioning.
- Preserve `crates/autotune-probe` and `crates/autotune-power-trial`, their
  fixtures, public behavior and dependency separation. Never run the bare probe.
- Never terminate user consoles or unrelated processes. Unsupported isolation or
  console-close delivery is an explicit blocker, not permission to bypass it.
- Never weaken a safety condition or mark unexecuted/blocked evidence as passed.
- Do not edit the original sibling `AutoTune` checkout. Work in this checkout.

## Execution and publication

The loop owns the queue. Implement only the dispatched story, use the installed
`bmad-build-auto` skill and write its required terminal result. Implementation
and inline reviews use the session model; do not launch Sol separately. The
orchestrator selects Sol for recommended follow-up reviews.

Run the format, lint and test gates in `.bmad-loop/verify.ps1`. Tests may use only
fake fixtures. Never silently run ignored tests; any direct-run acceptance test
must be identified as fake-only and restricted to owned processes and roots.

Commit only task-related source, tests, execution specs and documentation after
verification. Do not publish credentials, private evidence, raw device reports,
machine-specific config, review journals or `.memlog.md`. Never `git add .`.
Push only to `origin` on `automation/fake-recovery`, without force. No merges to
main, history rewrites, branch deletion, repository-setting or visibility changes.
The launcher pushes a clean, successful completed batch; stop if push fails.
