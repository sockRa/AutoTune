param(
    [ValidateSet('Validate', 'Plan', 'Run', 'Resume', 'Status', 'Stop', 'Dashboard')]
    [string]$Action = 'Status',
    [ValidateRange(1, 20)]
    [int]$MaxStories = 1,
    [string]$RunId = ''
)

# Invoke in a child PowerShell process. No persistent PATH or system changes.
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$expectedBranch = 'automation/fake-recovery'
$expectedRemote = 'https://github.com/sockRa/AutoTune.git'
$tools = Join-Path $env:LOCALAPPDATA 'Temp\opencode\bmad-loop-tools'
$originalPath = $env:PATH
$originalEncoding = $env:PYTHONIOENCODING
$originalLocation = Get-Location
$result = 1

try {
    $env:PATH = "$(Join-Path $tools 'pwsh-7.6.8');$(Join-Path $tools 'psmux-3.4.0');$env:PATH"
    $env:PYTHONIOENCODING = 'utf-8'
    Set-Location -LiteralPath $project
    $loop = (Get-Command bmad-loop.exe -CommandType Application -ErrorAction Stop).Source

    switch ($Action) {
        'Validate' { & $loop validate --project $project; $result = $LASTEXITCODE }
        'Plan' { & $loop run --project $project --max-stories $MaxStories --dry-run; $result = $LASTEXITCODE }
        'Status' { & $loop status --project $project; $result = $LASTEXITCODE }
        'Stop' {
            if (-not $RunId) { throw 'Stop needs -RunId from Status. No unrelated run will be selected automatically.' }
            & $loop stop $RunId --project $project --graceful
            $result = $LASTEXITCODE
        }
        'Dashboard' { & $loop tui --project $project; $result = $LASTEXITCODE }
        default {
            $branch = (& git branch --show-current).Trim()
            if ($LASTEXITCODE -ne 0 -or $branch -ne $expectedBranch) {
                throw "Run only on $expectedBranch; found '$branch'."
            }
            $remote = (& git remote get-url --push origin).Trim()
            if ($LASTEXITCODE -ne 0 -or $remote -ne $expectedRemote) {
                throw 'Unexpected publication remote. Inspect it before running.'
            }
            $batchBaseline = (& git rev-parse HEAD).Trim()
            if ($LASTEXITCODE -ne 0) { throw 'Cannot capture the publication baseline.' }
            if ($Action -eq 'Resume') {
                if (-not $RunId) { throw 'Resume needs -RunId from Status, after reviewing the pause reason.' }
                & $loop resume $RunId --project $project
            } else {
                $dirty = & git status --porcelain
                if ($LASTEXITCODE -ne 0 -or $dirty) {
                    throw 'The automation checkout must be clean. Preserve and review changes; do not reset/stash automatically.'
                }
                & $loop validate --project $project
                if ($LASTEXITCODE -ne 0) { throw 'Loop preflight failed; no session launched.' }
                & $loop run --project $project --max-stories $MaxStories
            }
            $result = $LASTEXITCODE
            if ($result -eq 0) {
                $branch = (& git branch --show-current).Trim()
                if ($LASTEXITCODE -ne 0 -or $branch -ne $expectedBranch) { throw 'Branch changed during run; refusing push.' }
                $remote = (& git remote get-url --push origin).Trim()
                if ($LASTEXITCODE -ne 0 -or $remote -ne $expectedRemote) { throw 'Remote changed during run; refusing push.' }
                $dirty = & git status --porcelain
                if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Run left uncommitted changes; refusing push.' }
                $paths = @(& git diff --name-only "$batchBaseline..HEAD")
                if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect the completed batch; refusing push.' }
                foreach ($path in $paths) {
                    if ($path -match '(^|/)(\.memlog\.md|\.env[^/]*|[^/]*\.(pem|key|pfx))$|^(\.bmad-loop|\.opencode|\.agents|\.claude|_bmad)/|\.(probe\.json|exe|zip|jsonl)$') {
                        throw "Private/configuration/binary artifact changed: $path. Refusing push."
                    }
                    # A deletion has no HEAD content to scan.
                    if (Test-Path -LiteralPath (Join-Path $project $path) -PathType Leaf) {
                        $content = & git show "HEAD:$path"
                        if ($LASTEXITCODE -ne 0) { throw "Cannot inspect $path; refusing push." }
                        if ($content -match '-----BEGIN .*PRIVATE KEY-----|sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|/private_[A-Za-z0-9]+|C:\\Users\\') {
                            throw "Sensitive content pattern in $path. Refusing push; inspect locally."
                        }
                    }
                }
                & git push --set-upstream origin "${expectedBranch}:${expectedBranch}"
                $result = $LASTEXITCODE
                if ($result -ne 0) { throw 'Push failed. Local work is retained; no force-push attempted.' }
            }
        }
    }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    $result = 1
} finally {
    $env:PATH = $originalPath
    $env:PYTHONIOENCODING = $originalEncoding
    Set-Location -LiteralPath $originalLocation
}
exit $result
