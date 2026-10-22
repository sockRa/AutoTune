# AutoTune unattended loop

## Scope

The automation checkout is separate from the original checkout and uses
`automation/fake-recovery`. The existing guarded-power-limit spec and architecture
are retained; `stories.yaml` schedules only unfinished fake-only recovery work.
`AGENTS.md` records the user's authorization and the hard safety boundaries.
No work is automatically merged into main.

The first story is a documentation/evidence pilot. Subsequent stories cover real
fake-worker fault notifications, an external synthetic witness and deterministic
barriers, then worker-loss/blocked-call evidence. Unsupported safety prerequisites
remain blockers, never passes. None of this authorizes a physical GPU experiment.

## Models and approvals

- Development and inline reviews: `opencode/deepseek-v4.1-flash-free`, high.
- Recommended independent follow-up reviews: `openai/gpt-6.1-sol`, medium.
- Routine spec/epic approval pauses are disabled for this approved queue.
- Two development attempts and bounded review cycles remain configured.
- Provider quota/transport errors and unresolved intent/safety conflicts pause.
- This is not a ChatGPT-quota meter or automatic difficulty-based model router.
- Existing interactive OpenCode sessions retain their selected model.

## Local integration

`.bmad-loop/profiles/opencode.toml` overlays the packaged `opencode-http` profile
to use `.agents/skills` rather than `.claude/skills`. The adapter starts its own
OpenCode HTTP server and does not need Claude hooks. The loop's control/dashboard
still needs a Windows multiplexer.

The launcher adds portable PowerShell 7.6.8 and psmux 3.4.0 from
`%LOCALAPPDATA%\Temp\opencode\bmad-loop-tools` to PATH for its process only. The
official GitHub release archives were SHA-256 checked before extraction. No
administrator installation or persistent PATH setting is used. If those temporary
tools are removed, restore them or provide compatible `pwsh` and `psmux` on PATH.

The BMAD installation and machine-specific policy stay ignored. A fresh clone
needs BMAD skills and the loop tool installed before these commands can work.

## Commands

Run from the automation checkout, in a terminal:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/bmad-loop.ps1 -Action Validate
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/bmad-loop.ps1 -Action Plan
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/bmad-loop.ps1 -Action Run
```

`Run` defaults to **one story** for the pilot. After verifying the pilot, use
`-Action Run -MaxStories 4` for the remaining approved queue. A completed, clean,
successful batch is pushed normally to the same automation branch on origin.
A failed/paused run is not force-pushed, reset or discarded.
Before pushing, the launcher also rejects private/configuration artifacts and
recognized credential/user-path patterns in the changed files. This conservative
check can pause for a false positive; it is not a complete secret scanner.

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/bmad-loop.ps1 -Action Status
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/bmad-loop.ps1 -Action Stop -RunId <id-from-status>
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/bmad-loop.ps1 -Action Resume -RunId <id-from-status>
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/bmad-loop.ps1 -Action Dashboard
```

Resume is only for a reviewed/resolved pause; it does not grant permission to
bypass a safety blocker. Runtime logs and state stay under ignored
`.bmad-loop/runs`. Do not publish private evidence or credentials with logs.

## Verification

The loop runs `.bmad-loop/verify.ps1` separately for format, warnings-as-errors
Clippy and locked workspace tests. No bare probe, live device operation or ignored
hardware/OS acceptance test is a gate. A normal test pass does not close G1--G7.

Setup baseline on 2026-10-22: format and strict Clippy passed; 129 workspace tests
passed, with the three explicit outside-Cargo round trips still ignored. No Rust
source changed during setup. A dry run selected the documentation pilot on the
free DeepSeek model. A real autonomous run is a separate verification step.
