# tools/benchmark-local-waveforms.ps1
# Measure the actual local pre-generation worker with owned WAVs and an empty cache.
# FixtureParent moves only the source WAVs; the library DB remains in system temp.
param(
    [ValidatePattern('^[A-Za-z0-9_-]+$')][string]$Label = 'current',
    [ValidateRange(12,1200)][int]$Files = 120,
    [string]$ReportPath,
    [string]$FixtureParent,
    [string]$BaselinePath,
    [ValidateRange(1,1000)][double]$MinimumSpeedup = 10,
    [string]$Toolchain = '1.97.1-x86_64-pc-windows-msvc'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'console-utf8.ps1')
$taskRepo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($ReportPath)) {
    $ReportPath = Join-Path $taskRepo ".rust-validation/local-waveforms/$Label.json"
}
$taskReport = [IO.Path]::GetFullPath($ReportPath)
$taskReportParent = Split-Path -Parent $taskReport
New-Item -ItemType Directory -Force -Path $taskReportParent | Out-Null
$taskSavedEnvironment = @{}
$taskEnvironment = @{
    AUDIOLIB_WAVEFORM_BENCH_FILES = [string]$Files
    AUDIOLIB_WAVEFORM_BENCH_LABEL = $Label
    AUDIOLIB_WAVEFORM_BENCH_REPORT = $taskReport
    AUDIOLIB_WAVEFORM_BENCH_PARENT = $null
    RUSTUP_TOOLCHAIN = $Toolchain
}
if (![string]::IsNullOrWhiteSpace($FixtureParent)) {
    $taskFixtureParent = (Resolve-Path -LiteralPath $FixtureParent).Path
    if (!(Test-Path -LiteralPath $taskFixtureParent -PathType Container)) {
        throw 'FixtureParent must be an existing directory.'
    }
    $taskEnvironment.AUDIOLIB_WAVEFORM_BENCH_PARENT = $taskFixtureParent
}
foreach ($taskKey in $taskEnvironment.Keys) {
    $taskSavedEnvironment[$taskKey] = [Environment]::GetEnvironmentVariable($taskKey, 'Process')
    if ($null -eq $taskEnvironment[$taskKey]) {
        Remove-Item -LiteralPath "Env:$taskKey" -ErrorAction SilentlyContinue
    } else {
        [Environment]::SetEnvironmentVariable($taskKey, $taskEnvironment[$taskKey], 'Process')
    }
}
Push-Location -LiteralPath $taskRepo
try {
    & cargo test -p audiolib-desktop --release --locked local_waveform_pregeneration_benchmark -- --ignored --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "Local Waveform benchmark failed: cargo exit $LASTEXITCODE" }
    if (!(Test-Path -LiteralPath $taskReport)) { throw 'Benchmark did not produce its report.' }
    $taskResult = Get-Content -Raw -LiteralPath $taskReport | ConvertFrom-Json
    $taskCommit = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Unable to identify source commit.' }
    $taskResult | Add-Member -NotePropertyName source_commit -NotePropertyValue $taskCommit.Trim()
    $taskChanges = @(& git status --porcelain)
    if ($LASTEXITCODE -ne 0) { throw 'Unable to identify source changes.' }
    $taskResult | Add-Member -NotePropertyName source_worktree_dirty -NotePropertyValue ($taskChanges.Count -gt 0)
    $taskResult | Add-Member -NotePropertyName measured_at_utc -NotePropertyValue ([DateTime]::UtcNow.ToString('o'))
    if ($env:OS -eq 'Windows_NT') {
        $taskProcessors = @(Get-CimInstance -ClassName Win32_Processor | ForEach-Object {
            @{ name = $_.Name; cores = $_.NumberOfCores; logical_processors = $_.NumberOfLogicalProcessors }
        })
        $taskResult | Add-Member -NotePropertyName processors -NotePropertyValue $taskProcessors
    }
    if (![string]::IsNullOrWhiteSpace($BaselinePath)) {
        $taskBaseline = Get-Content -Raw -LiteralPath $BaselinePath | ConvertFrom-Json
        foreach ($taskField in @('workload_version','files','source_bytes','source_frames','sample_rate','pcm_bits','channels','durations_seconds','source_parent','database_parent','database_location','available_parallelism','os','architecture')) {
            $taskBefore = ConvertTo-Json -InputObject $taskBaseline.$taskField -Compress
            $taskAfter = ConvertTo-Json -InputObject $taskResult.$taskField -Compress
            if ($taskBefore -ne $taskAfter) { throw "Baseline workload differs: $taskField" }
        }
        if ($env:OS -eq 'Windows_NT') {
            $taskBeforeCpu = (@($taskBaseline.processors) | ForEach-Object {
                '{0}|{1}|{2}' -f $_.name, $_.cores, $_.logical_processors
            } | Sort-Object) -join ';'
            $taskAfterCpu = (@($taskResult.processors) | ForEach-Object {
                '{0}|{1}|{2}' -f $_.name, $_.cores, $_.logical_processors
            } | Sort-Object) -join ';'
            if ($taskBeforeCpu -ne $taskAfterCpu) { throw 'Baseline processor hardware differs.' }
        }
        if ($taskBaseline.cold_seconds -le 0 -or $taskResult.cold_seconds -le 0) {
            throw 'Both measured cold durations must be positive.'
        }
        $taskSpeedup = [double]$taskBaseline.cold_seconds / [double]$taskResult.cold_seconds
        $taskResult | Add-Member -NotePropertyName baseline_label -NotePropertyValue $taskBaseline.label
        $taskResult | Add-Member -NotePropertyName baseline_cold_seconds -NotePropertyValue $taskBaseline.cold_seconds
        $taskResult | Add-Member -NotePropertyName speedup -NotePropertyValue $taskSpeedup
        $taskResult | Add-Member -NotePropertyName minimum_speedup -NotePropertyValue $MinimumSpeedup
        $taskResult | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $taskReport -Encoding utf8
        Write-Host ('Speedup: {0:N2}x ({1:N3}s -> {2:N3}s)' -f $taskSpeedup, $taskBaseline.cold_seconds, $taskResult.cold_seconds)
        if ($taskSpeedup -lt $MinimumSpeedup) {
            throw "Measured speedup $taskSpeedup is below required $MinimumSpeedup. Report: $taskReport"
        }
    } else {
        $taskResult | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $taskReport -Encoding utf8
    }
    Write-Host ('PASS: {0} files, {1:N3}s, {2:N2} files/s; all peaks match; warm pass decodes zero files.' -f $Files, $taskResult.cold_seconds, $taskResult.cold_files_per_second)
    Write-Host "Report: $taskReport"
} finally {
    Pop-Location
    foreach ($taskKey in $taskSavedEnvironment.Keys) {
        if ($null -eq $taskSavedEnvironment[$taskKey]) {
            Remove-Item -LiteralPath "Env:$taskKey" -ErrorAction SilentlyContinue
        } else {
            [Environment]::SetEnvironmentVariable($taskKey, $taskSavedEnvironment[$taskKey], 'Process')
        }
    }
    Restore-AudioLibConsoleEncoding
}
