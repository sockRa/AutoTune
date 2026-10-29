# Memory-only stream tests. Dot sourcing uses the runner's verification-only
# default; it does not enable the execution branch or start any test process.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Invoke-SamplerFixtureMatrix.ps1')
function New-MemoryPump([byte[]]$bytes) {
    return @{
        stream = [IO.MemoryStream]::new($bytes, $false)
        file = [IO.MemoryStream]::new()
        buffer = (New-Object byte[] 3)
        read = $null; write = $null; flush = $null; eof = $false; complete = $false
    }
}
function Assert-FixtureCondition([bool]$condition, [string]$message) {
    if (!$condition) { throw $message }
}
$leftBytes = [byte[]]@(0, 255, 128, 10, 13, 0, 1, 2, 3, 4)
$rightBytes = [byte[]]@(239, 187, 191, 240, 159, 152, 128, 0)
$left = New-MemoryPump $leftBytes
$right = New-MemoryPump $rightBytes
$counter = @{bytes = 0L; limit = 1024; quota = $false}
for ($step = 0; $step -lt 100 -and !($left.complete -and $right.complete); $step++) {
    Advance-Pump $left $counter
    Advance-Pump $right $counter
}
Assert-FixtureCondition ($left.complete -and $right.complete) 'Both streams need EOF and flush completion'
Assert-FixtureCondition ($counter.bytes -eq 18) 'Aggregate observed bytes must cover both streams'
Assert-FixtureCondition ([Convert]::ToBase64String($left.file.ToArray()) -ceq [Convert]::ToBase64String($leftBytes)) 'Binary stdout bytes changed'
Assert-FixtureCondition ([Convert]::ToBase64String($right.file.ToArray()) -ceq [Convert]::ToBase64String($rightBytes)) 'Split UTF-8 stderr bytes changed'

$quotaPump = New-MemoryPump ([byte[]]@(1, 2, 3, 4, 5, 6))
$quotaCounter = @{bytes = 0L; limit = 3; quota = $false}
Advance-Pump $quotaPump $quotaCounter
Advance-Pump $quotaPump $quotaCounter
Assert-FixtureCondition $quotaCounter.quota 'Quota equality must stop'
Assert-FixtureCondition (!$quotaPump.complete) 'Quota rejection must not claim EOF/flush'
Assert-FixtureCondition ($quotaPump.file.Length -eq 0) 'Quota-crossing bytes must not be written as a complete log'

$pending = New-MemoryPump ([byte[]]@())
$pendingSource = [pscustomobject]@{ completion = (New-Object 'System.Threading.Tasks.TaskCompletionSource[int]') }
$pendingSource | Add-Member -MemberType ScriptMethod -Name ReadAsync -Value { return $this.completion.Task }
$pending.stream = $pendingSource
$pendingCounter = @{bytes = 0L; limit = 1024; quota = $false}
Advance-Pump $pending $pendingCounter
for ($step = 0; $step -lt 20; $step++) { Advance-Pump $pending $pendingCounter }
Assert-FixtureCondition (!$pending.complete -and !$pending.read.IsCompleted) 'Pending read must remain unknown without waiting'
Assert-FixtureCondition ($pendingCounter.bytes -eq 0) 'Pending read must not invent bytes'
$pendingSource.completion.SetException([IO.IOException]::new('injected read failure'))
$failed = $false
try { Advance-Pump $pending $pendingCounter } catch { $failed = $true }
Assert-FixtureCondition $failed 'Faulted read must fail instead of becoming EOF'
Assert-FixtureCondition (!$pending.complete) 'Faulted read must not qualify a complete log'

$pendingWrite = New-MemoryPump ([byte[]]@(1, 2, 3, 4, 5, 6))
$writeSink = [pscustomobject]@{ completion = (New-Object 'System.Threading.Tasks.TaskCompletionSource[object]') }
$writeSink | Add-Member -MemberType ScriptMethod -Name WriteAsync -Value { return $this.completion.Task }
$pendingWrite.file = $writeSink
$writeCounter = @{bytes = 0L; limit = 1024; quota = $false}
Advance-Pump $pendingWrite $writeCounter
Advance-Pump $pendingWrite $writeCounter
for ($step = 0; $step -lt 20; $step++) { Advance-Pump $pendingWrite $writeCounter }
Assert-FixtureCondition (!$pendingWrite.complete -and !$pendingWrite.write.IsCompleted) 'Pending write must remain unknown'
Assert-FixtureCondition ($pendingWrite.stream.Position -eq 3 -and $writeCounter.bytes -eq 3) 'No successor read may overwrite an in-flight write buffer'

$pendingFlush = New-MemoryPump ([byte[]]@())
$flushSink = [pscustomobject]@{ completion = (New-Object 'System.Threading.Tasks.TaskCompletionSource[object]'); disposed = $false }
$flushSink | Add-Member -MemberType ScriptMethod -Name FlushAsync -Value { return $this.completion.Task }
$flushSink | Add-Member -MemberType ScriptMethod -Name Dispose -Value { $this.disposed = $true }
$pendingFlush.file = $flushSink
$flushCounter = @{bytes = 0L; limit = 1024; quota = $false}
Advance-Pump $pendingFlush $flushCounter
Advance-Pump $pendingFlush $flushCounter
Assert-FixtureCondition ($pendingFlush.eof -and !$pendingFlush.complete) 'EOF alone must not establish flushed output'
$flushSink.completion.SetResult($null)
Advance-Pump $pendingFlush $flushCounter
Assert-FixtureCondition ($pendingFlush.complete -and $flushSink.disposed) 'Completed flush must close the final output before hashing'
Write-Output 'PASS: memory-only dual-stream, binary retention, quota equality, pending read and read-failure checks. No fixture process started.'
