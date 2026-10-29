# Check repository ignore behavior without creating files or running workloads.
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot

# Literal expectations: catch leaked local state/secrets and overbroad source ignores.
$ignored = @(
    'target/debug/autotune-probe.exe',
    'live.probe.json',
    'nested/live.probe.json',
    '.env',
    '.env.local',
    'nested/.env.production',
    '_bmad-output/specs/local.md',
    '_bmad/config.user.toml',
    '.bmad-loop/policy.toml',
    '.bmad-loop/runs/local.json',
    '.omo/notes/local.md',
    '.omo/worktrees/local/Cargo.toml',
    '.agents/skills/local/SKILL.md',
    '.claude/skills/local/SKILL.md',
    '.opencode/opencode.jsonc',
    'docs/opencode-setup.md',
    'docs/source-backup-review.md'
)
$eligible = @(
    '.env.example',
    '.env.production.example',
    'nested/.env.example',
    'Cargo.lock',
    'Cargo.toml',
    'AGENTS.md',
    'ROADMAP.md',
    '.github/workflows/ci.yml',
    'crates/autotune-probe/src/lib.rs',
    'crates/autotune-probe/tests/probe.rs',
    'docs/measurement.md',
    'docs/contracts/example.json',
    'docs/superpowers/plans/example.md',
    'scripts/check-gitignore.ps1'
)
$failures = @()
foreach ($path in @($ignored + $eligible)) {
    & git -C $repository -c core.excludesFile= check-ignore --no-index --quiet -- $path
    if ($LASTEXITCODE -notin @(0, 1)) {
        throw "git check-ignore failed for $path (exit $LASTEXITCODE)"
    }
    $actual = $LASTEXITCODE -eq 0
    $expected = $path -in $ignored
    if ($actual -ne $expected) {
        $failures += "$path : expected ignored=$expected, got $actual"
    }
}
if ($failures.Count) {
    $failures | Write-Output
    exit 1
}
Write-Output "PASS: $($ignored.Count + $eligible.Count) ignore checks; no files or workloads created."
exit 0
