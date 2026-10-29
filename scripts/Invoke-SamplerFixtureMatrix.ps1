# Default is verification only. Execution needs separately retained owner consent
# for the exact matrix SHA; passing a string is not authenticated human approval.
param(
    [switch]$ExecuteApprovedMatrix,
    [string]$ApprovalMatrixSha256
)
$ErrorActionPreference = 'Stop'
$repository = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$matrixPath = Join-Path $repository 'docs/superpowers/specs/2026-10-29-e3-sampler-fixture-matrix.json'
$matrixHash = (Get-FileHash -LiteralPath $matrixPath -Algorithm SHA256).Hash.ToLowerInvariant()
$matrix = Get-Content -LiteralPath $matrixPath -Raw | ConvertFrom-Json
if ($matrix.runner_sha256 -cne (Get-FileHash -LiteralPath $PSCommandPath).Hash.ToLowerInvariant()) {
    throw 'Runner hash differs from the matrix'
}
if ($matrix.version -ne 1 -or $matrix.scope -ne 'windows_fake_sampler_only' -or
    $matrix.source_revision -ne 'ef8786e0a4ea0831b17d14355ce98ecdcbe2883e') {
    throw 'Unknown fixture matrix/scope/revision'
}
& git -C $repository diff --exit-code $matrix.source_revision -- Cargo.toml Cargo.lock crates .github
if ($LASTEXITCODE -ne 0) { throw 'Source differs from the reviewed matrix revision' }
$expectedTests = @(
    'cancelled_async_launch_never_allows_a_late_sampler_to_continue',
    'fixed_hung_sampler_requires_observed_exit_after_force',
    'shared_wire_fixture_blocked_getter_keeps_control_responsive_until_force',
    'shared_wire_fixture_samples_six_metrics_and_drains_after_stop',
    'shared_wire_fixture_wrong_ready_identity_never_admits_a_query'
)
if (($matrix.tests -join ',') -cne ($expectedTests -join ',') -or
    $matrix.outer_wall_seconds -ne 90 -or $matrix.diagnostic_bytes -ne 16777216) {
    throw 'Fixture cases or outer bounds differ from the reviewed runner'
}
$fixedArtifacts = @(
    'target/sampler-verification/build/debug/deps/sampler_process-4d4cad9aff7075cc.exe',
    'target/sampler-verification/build/debug/autotune-sampler-fixture.exe'
)
if (($matrix.artifacts.path -join ',') -cne ($fixedArtifacts -join ',')) {
    throw 'Artifact allowlist differs from the fixed matrix'
}
$held = @()
$pumps = @()
$process = $null
$cleanupAttempted = $false
function Advance-Pump($pump, $counter) {
    if ($pump.complete -or $counter.quota) { return }
    if ($null -ne $pump.write -and $pump.write.IsCompleted) {
        $pump.write.GetAwaiter().GetResult()
        $pump.write = $null
    }
    if ($null -ne $pump.flush -and $pump.flush.IsCompleted) {
        $pump.flush.GetAwaiter().GetResult()
        $pump.flush = $null
        $pump.complete = $true
        $pump.file.Dispose()
        return
    }
    if ($null -ne $pump.read -and $pump.read.IsCompleted) {
        $count = $pump.read.GetAwaiter().GetResult()
        $pump.read = $null
        if ($count -eq 0) {
            $pump.eof = $true
            $pump.flush = $pump.file.FlushAsync()
        } else {
            $counter.bytes += $count
            if ($counter.bytes -ge $counter.limit) { $counter.quota = $true; return }
            $pump.write = $pump.file.WriteAsync($pump.buffer, 0, $count)
        }
    }
    if (!$pump.eof -and $null -eq $pump.read -and $null -eq $pump.write) {
        $pump.read = $pump.stream.ReadAsync($pump.buffer, 0, $pump.buffer.Length)
    }
}
try {
    foreach ($artifact in $matrix.artifacts) {
        $path = [IO.Path]::GetFullPath((Join-Path $repository $artifact.path))
        if (!$path.StartsWith($repository + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Artifact escapes workspace'
        }
        # Keep both exact images open without write/delete sharing through exit.
        $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        $held += $stream
        $sha = [Security.Cryptography.SHA256]::Create()
        try {
            $actual = ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
        } finally { $sha.Dispose() }
        if ($actual -cne $artifact.sha256) { throw "Artifact hash mismatch: $($artifact.path)" }
    }
    Write-Output "Verified matrix SHA256: $matrixHash"
    if (!$ExecuteApprovedMatrix) {
        Write-Output 'Verification only: no sampler/controller/test process started.'
        return
    }
    if ($ApprovalMatrixSha256 -cne $matrixHash) {
        throw 'Execution requires the exact separately approved matrix SHA256'
    }
    if ($env:OS -ne 'Windows_NT') { throw 'Execution requires the selected Windows host' }
    $output = Join-Path $repository 'target/sampler-verification/fixture-execution'
    if (Test-Path -LiteralPath $output) { throw 'Execution output already exists; no automatic retry or overwrite' }
    [IO.Directory]::CreateDirectory($output) | Out-Null
    $stdout = Join-Path $output 'stdout.log'
    $stderr = Join-Path $output 'stderr.log'
    $testImage = Join-Path $repository $fixedArtifacts[0]
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $testImage
    $start.WorkingDirectory = $repository
    $start.Arguments = '--ignored --test-threads=1 --nocapture'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    # Two explicit bounded byte pumps, rather than hidden redirect helpers.
    # Only completed tasks are consumed; EOF and flushed output are separate
    # from root-process exit, and all remain within the original time bounds.
    foreach ($log in @($stdout, $stderr)) {
        $pumps += @{
            path = $log; stream = $null; buffer = (New-Object byte[] 8192)
            file = [IO.FileStream]::new($log, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write,
                [IO.FileShare]::Read, 8192, [IO.FileOptions]::Asynchronous)
            read = $null; write = $null; flush = $null; eof = $false; complete = $false
        }
    }
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $start
    if (!$process.Start()) { throw 'Test driver did not start' }
    $pumps[0].stream = $process.StandardOutput.BaseStream
    $pumps[1].stream = $process.StandardError.BaseStream
    $counter = @{ bytes = 0L; limit = $matrix.diagnostic_bytes; quota = $false }
    # Materialize and retain this owned test-driver handle before observation.
    $retainedHandle = $process.Handle
    $reason = 'completed'
    $exitObserved = $false
    while ($true) {
        $exitObserved = $process.WaitForExit(100)
        foreach ($pump in $pumps) { Advance-Pump $pump $counter }
        # Equality wins; outer stop bounds never extend individual case limits.
        if ($watch.Elapsed.TotalSeconds -ge $matrix.outer_wall_seconds) { $reason = 'outer_deadline'; break }
        if ($counter.quota) { $reason = 'diagnostic_quota'; break }
        if ($exitObserved -and $pumps[0].complete -and $pumps[1].complete) { break }
        if ($exitObserved) { Start-Sleep -Milliseconds 10 }
    }
    $termination = 'not_requested'
    $shutdownEnd = $watch.Elapsed.TotalSeconds + 3
    if (!$exitObserved) {
        $cleanupAttempted = $true
        try { $process.Kill(); $termination = 'requested' }
        catch { $termination = $_.Exception.GetType().FullName + ':' + $_.Exception.HResult }
    }
    if ($reason -ne 'completed') {
        while ($watch.Elapsed.TotalSeconds -lt $shutdownEnd) {
            $exitObserved = $process.WaitForExit(100)
            foreach ($pump in $pumps) { Advance-Pump $pump $counter }
            if ($watch.Elapsed.TotalSeconds -ge $shutdownEnd) { break }
            if ($exitObserved -and $pumps[0].complete -and $pumps[1].complete) { break }
            if ($exitObserved) { Start-Sleep -Milliseconds 10 }
        }
    }
    $drained = $pumps[0].complete -and $pumps[1].complete
    $receipt = [ordered]@{
        matrix_sha256 = $matrixHash
        source_revision = $matrix.source_revision
        scope = $matrix.scope
        reason = $reason
        elapsed_ms = $watch.ElapsedMilliseconds
        termination = $termination
        exit_observed = $exitObserved
        stdout_drained = $pumps[0].complete
        stderr_drained = $pumps[1].complete
        diagnostic_bytes_observed = $counter.bytes
        exit_code = $(if ($exitObserved) { $process.ExitCode } else { $null })
        logs = @($pumps | ForEach-Object {
            if (Test-Path -LiteralPath $_.path) {
                [ordered]@{
                    name = [IO.Path]::GetFileName($_.path)
                    complete = $_.complete
                    bytes = $(if ($_.complete) { (Get-Item -LiteralPath $_.path).Length } else { $null })
                    sha256 = $(if ($_.complete) { (Get-FileHash -LiteralPath $_.path).Hash.ToLowerInvariant() } else { $null })
                }
            }
        })
    }
    $receipt | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $output 'receipt.json')
    if ($reason -ne 'completed' -or !$exitObserved -or !$drained -or $process.ExitCode -ne 0) {
        throw 'Fixture run failed/incomplete; preserve evidence and do not retry'
    }
    Write-Output 'Test driver completed. Review all five cases; no native/GPU qualification follows.'
} catch {
    $originalFailure = $_
    # Unexpected observation/storage errors must not abandon an owned live
    # test driver. This targets only the Process object created above.
    if ($null -ne $process -and !$cleanupAttempted) {
        $cleanupAttempted = $true
        $cleanup = 'not_requested'
        $cleanupExit = $false
        try {
            if (!$process.HasExited) { $process.Kill(); $cleanup = 'requested' }
            $cleanupExit = $process.WaitForExit(3000)
        } catch { $cleanup = $_.Exception.GetType().FullName + ':' + $_.Exception.HResult }
        Write-Warning "Owned test-driver cleanup=$cleanup exit_observed=$cleanupExit"
    }
    throw $originalFailure
} finally {
    # Do not synchronously dispose a file with pending I/O on this bounded
    # observation path. Unfinished pumps remain unknown until process teardown.
    foreach ($pump in $pumps) {
        if ($pump.complete) { $pump.file.Dispose() }
    }
    if ($null -ne $process) { $process.Dispose() }
    foreach ($stream in $held) { $stream.Dispose() }
}
