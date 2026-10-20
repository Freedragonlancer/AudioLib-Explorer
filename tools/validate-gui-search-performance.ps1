# tools/validate-gui-search-performance.ps1
# Explicit own-window diagnostics only. This script injects no UI input and changes no environment.
#Requires -Version 7.5
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$BinaryPath,
    [Parameter(Mandatory=$true)][string]$DatabasePath,
    [Parameter(Mandatory=$true)][string]$RequestFile,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [ValidateRange(1,1000)][int]$Repeats = 5,
    [uint32]$PointerOverlayProcessId = 0
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

function Write-NewJson([string]$Path, [object]$Value) {
    $stream = [IO.File]::Open($Path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
    try {
        $text = $Value | ConvertTo-Json -Depth 30
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($text + [Environment]::NewLine)
        $stream.Write($bytes,0,$bytes.Length)
        $stream.Flush($true)
    } finally { $stream.Dispose() }
}

function Get-LibraryMetadata([string]$Path) {
    # Do not open SQLite, hash content, inspect sidecars or change the library.
    $file = Get-Item -LiteralPath $Path
    if ($file.PSIsContainer) { throw 'DatabasePath must identify an existing file' }
    return [pscustomobject]@{Path=$file.FullName;Bytes=$file.Length;LastWriteTimeUtc=$file.LastWriteTimeUtc.ToString('o')}
}

function Test-FiniteNonnegative([object]$Value) {
    if ($null -eq $Value -or !($Value -is [int] -or $Value -is [long] -or $Value -is [double] -or $Value -is [decimal])) { return $false }
    $number = [double]$Value
    return ![double]::IsNaN($number) -and ![double]::IsInfinity($number) -and $number -ge 0
}

function Test-JsonEqual([object]$Left, [object]$Right) {
    if ($null -eq $Left -or $null -eq $Right) { return $null -eq $Left -and $null -eq $Right }
    if ($Left -is [bool] -or $Right -is [bool]) { return $Left -is [bool] -and $Right -is [bool] -and $Left -eq $Right }
    if ($Left -is [string] -or $Right -is [string]) { return $Left -is [string] -and $Right -is [string] -and $Left -ceq $Right }
    if ($Left -is [Management.Automation.PSCustomObject] -or $Right -is [Management.Automation.PSCustomObject]) {
        if (!($Left -is [Management.Automation.PSCustomObject]) -or !($Right -is [Management.Automation.PSCustomObject])) { return $false }
        $leftNames = @($Left.PSObject.Properties.Name | Sort-Object -CaseSensitive)
        $rightNames = @($Right.PSObject.Properties.Name | Sort-Object -CaseSensitive)
        if ($leftNames.Count -ne $rightNames.Count) { return $false }
        for ($index = 0; $index -lt $leftNames.Count; $index++) {
            if ($leftNames[$index] -cne $rightNames[$index] -or !(Test-JsonEqual $Left.($leftNames[$index]) $Right.($rightNames[$index]))) { return $false }
        }
        return $true
    }
    if ($Left -is [Array] -or $Right -is [Array]) {
        if (!($Left -is [Array]) -or !($Right -is [Array]) -or $Left.Count -ne $Right.Count) { return $false }
        for ($index = 0; $index -lt $Left.Count; $index++) { if (!(Test-JsonEqual $Left[$index] $Right[$index])) { return $false } }
        return $true
    }
    # Preserve exact signed-64-bit IDs while accepting JSON 0 and 0.0 as the same duration.
    try { return [decimal]$Left -eq [decimal]$Right } catch { return [double]$Left -eq [double]$Right }
}

function Get-LatencyStatistics([object[]]$Samples) {
    $values = @($Samples | Where-Object Valid | ForEach-Object { [double]$_.TotalMs } | Sort-Object)
    if (!$values.Count) { return [pscustomobject]@{Count=0;MinMs=$null;P50Ms=$null;P95Ms=$null;MaxMs=$null;AtOrOver500=0} }
    return [pscustomobject]@{
        Count=$values.Count
        MinMs=$values[0]
        P50Ms=$values[[int][Math]::Ceiling(($values.Count-1)*0.5)]
        P95Ms=$values[[int][Math]::Ceiling(($values.Count-1)*0.95)]
        MaxMs=$values[-1]
        AtOrOver500=@($values | Where-Object { $_ -ge 500 }).Count
    }
}

function Assert-LibraryMetadata([object]$Before) {
    $after = Get-LibraryMetadata $Before.Path
    if ($after.Bytes -ne $Before.Bytes -or $after.LastWriteTimeUtc -cne $Before.LastWriteTimeUtc) { throw 'Database size or modification time changed; stop collecting against a changed source' }
    return $after
}

$binary = (Resolve-Path -LiteralPath $BinaryPath).ProviderPath
$database = (Resolve-Path -LiteralPath $DatabasePath).ProviderPath
$requestPath = (Resolve-Path -LiteralPath $RequestFile).ProviderPath
if ((Get-Item -LiteralPath $binary).PSIsContainer) { throw 'BinaryPath must identify an existing executable file' }
$requestInfo = Get-Item -LiteralPath $requestPath
if ($requestInfo.PSIsContainer -or $requestInfo.Length -gt 1MB) { throw 'RequestFile must be a JSON array file no larger than 1 MiB' }
$requests = ConvertFrom-Json -InputObject (Get-Content -LiteralPath $requestPath -Raw) -Depth 30 -NoEnumerate -DateKind String
if (!($requests -is [Array]) -or !$requests.Count) { throw 'RequestFile must contain a nonempty JSON array' }
foreach ($request in $requests) {
    if (!($request -is [Management.Automation.PSCustomObject])) { throw 'Every request must be a SearchRequest object' }
    if (($request | ConvertTo-Json -Compress -Depth 20).Length -gt 65536) { throw 'A request exceeds the diagnostic CLI 64 KiB input bound' }
}
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw 'OutputDirectory must be new; existing batches are never overwritten' }
New-Item -ItemType Directory -Path $output | Out-Null
if ((Get-Item -LiteralPath $output -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Linked output directory is not supported' }
$binaryHash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash
$requestHash = (Get-FileHash -LiteralPath $requestPath -Algorithm SHA256).Hash
$databaseBefore = Get-LibraryMetadata $database
$normalDataDirectory = $env:AUDIOLIB_DATA_DIR
$processTimeoutMs = 240000
$samples = [Collections.Generic.List[object]]::new()
$pointerIdentity = $null
$startedUtc = [DateTimeOffset]::UtcNow.ToString('o')
$blockedReason = $null
$completed = $false
Write-NewJson (Join-Path $output 'identity.json') ([pscustomobject]@{
    Binary=$binary;BinarySHA256=$binaryHash;RequestFile=$requestPath;RequestsSHA256=$requestHash
    Database=$databaseBefore;RequestCount=$requests.Count;Repeats=$Repeats;ExpectedSamples=$requests.Count*$Repeats
    InputMode='Explicit CLI diagnostic; synthetic Slint KeyPressed/Released text dispatch through production LineEdit, not OS keyboard injection'
    Endpoint='First observed WGC compositor frame with decoded matching nonce/generation phase ALL; not panel scanout'
    OSPageCacheCleared=$false;EnvironmentModified=$false;UIScriptingPerformed=$false;ForcedTerminationAllowed=$false
    ProcessTimeoutMs=$processTimeoutMs;StartedUtc=$startedUtc
    PointerOverlayExceptionEnabled=$PointerOverlayProcessId -ne 0;PointerOverlayProcessId=$PointerOverlayProcessId
    PointerOverlayException='Only explicitly verified fixed Sky process identity cursor decorations; application/unknown occluders remain blocking; no absolute unobscured-display claim'
})

try {
    for ($repeat = 1; $repeat -le $Repeats; $repeat++) {
        for ($caseIndex = 0; $caseIndex -lt $requests.Count; $caseIndex++) {
            [void](Assert-LibraryMetadata $databaseBefore)
            $name = 'r{0:D3}-c{1:D3}' -f $repeat,$caseIndex
            $reportPath = Join-Path $output ($name + '.json')
            $stdoutPath = Join-Path $output ($name + '.stdout.log')
            $stderrPath = Join-Path $output ($name + '.stderr.log')
            $requestJson = $requests[$caseIndex] | ConvertTo-Json -Compress -Depth 20
            $startInfo = [Diagnostics.ProcessStartInfo]::new()
            $startInfo.FileName = $binary
            $startInfo.WorkingDirectory = Split-Path -Parent $binary
            $startInfo.UseShellExecute = $false
            # Suppress a console only; the diagnostic creates its required visible native GUI.
            $startInfo.CreateNoWindow = $true
            $startInfo.RedirectStandardOutput = $true
            $startInfo.RedirectStandardError = $true
            foreach ($argument in @('--gui-search-benchmark',$requestJson,'--gui-search-database',$database,'--gui-search-report',$reportPath)) { $startInfo.ArgumentList.Add($argument) }
            if ($PointerOverlayProcessId -ne 0) {
                $startInfo.ArgumentList.Add('--gui-search-pointer-overlay-process')
                $startInfo.ArgumentList.Add($PointerOverlayProcessId.ToString([Globalization.CultureInfo]::InvariantCulture))
            }
            $process = [Diagnostics.Process]::new()
            $process.StartInfo = $startInfo
            $stdout = $null
            $stderr = $null
            try {
                $stdout = [IO.File]::Open($stdoutPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
                $stderr = [IO.File]::Open($stderrPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
                if (!$process.Start()) { throw 'Failed to start diagnostic process' }
                $stdoutTask = $process.StandardOutput.BaseStream.CopyToAsync($stdout)
                $stderrTask = $process.StandardError.BaseStream.CopyToAsync($stderr)
                if (!$process.WaitForExit($processTimeoutMs)) {
                    Write-NewJson (Join-Path $output ($name + '.hang.json')) ([pscustomobject]@{Blocked=$true;Reason='Diagnostic process exceeded its deadline; left running without forced termination';ProcessId=$process.Id;Report=$reportPath;TimeoutMs=$processTimeoutMs;RecordedUtc=[DateTimeOffset]::UtcNow.ToString('o')})
                    throw "Diagnostic process $($process.Id) hung; no forced termination was attempted. Do not start another sample until it exits normally."
                }
                if (![Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($stdoutTask,$stderrTask),10000)) { throw 'Exited diagnostic process stdout/stderr did not finish draining' }
                $stdoutTask.GetAwaiter().GetResult()
                $stderrTask.GetAwaiter().GetResult()
                $exitCode = $process.ExitCode
                $stdout.Flush($true)
                $stderr.Flush($true)
            } finally {
                # Disposing our redirection/process handles does not terminate a live child.
                $process.Dispose()
                if ($stdout) { $stdout.Dispose() }
                if ($stderr) { $stderr.Dispose() }
            }
            if (!(Test-Path -LiteralPath $reportPath -PathType Leaf)) { throw "Diagnostic exited $exitCode without its required report: $name" }
            try { $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json -Depth 30 -DateKind String } catch { throw "Diagnostic report is incomplete or invalid JSON: $name. $($_.Exception.Message)" }
            $issues = [Collections.Generic.List[string]]::new()
            if ($exitCode -ne 0) { $issues.Add("process exit code $exitCode") }
            if ($report.schema_version -ne 1) { $issues.Add('unsupported report schema') }
            if ($report.valid -ne $true) { $issues.Add("diagnostic invalid: $($report.invalid_reason)") }
            if (!(Test-JsonEqual $requests[$caseIndex] $report.expected_request) -or !(Test-JsonEqual $requests[$caseIndex] $report.actual_request)) { $issues.Add('expected/actual request differs from the fixed request file') }
            if (!($report.page.search_ranking_cache_hit -is [bool]) -or $report.page.search_ranking_cache_hit) { $issues.Add('first final search did not explicitly report a ranking-cache miss') }
            if ($report.oracle_matches -ne $true) { $issues.Add('page IDs/counts or all folder counts did not match the post-timing oracle') }
            if ($report.source_same -ne $true -or $report.source_current -ne $true -or $report.source_read_only -ne $true -or $report.owned_copy -ne $false -or $report.source_size_mtime_unchanged -ne $true -or $report.source_database_bytes -ne $databaseBefore.Bytes) { $issues.Add('read-only current-source identity check failed') }
            if (!(Test-FiniteNonnegative $report.total_ms)) { $issues.Add('total_ms is missing, nonnumeric, nonfinite or negative') }
            if ($report.input_timestamp_source -cne 'Slint KeyPressed dispatch, complete text' -or $report.native_text_events -ne 0) { $issues.Add('input was not the isolated synthetic Slint-dispatch condition') }
            if ($report.final_frame.phase -ne 2 -or $report.final_frame.generation -eq $report.startup_wgc_calibration.generation -or $report.final_frame.nonce -ne $report.startup_wgc_calibration.nonce -or $report.final_frame.visibility_before_passed -ne $true -or $report.final_frame.visibility_after_passed -ne $true) { $issues.Add('matching final WGC phase/nonce/generation/visibility evidence is missing') }
            if ($report.startup_library_info_published -ne $true -or $report.resource_conditions.startup_library_stats_completion_gated -ne $true -or $report.foreground_calibration_epochs -lt 1 -or $report.calibration_epoch_generation -ne $report.startup_wgc_calibration.generation) { $issues.Add('startup statistics and fresh foreground calibration gates are missing') }
            if ($report.normal_data_directory_environment_changed -ne $false -or $report.isolated_profile -ne $true -or $report.network_started -ne $false) { $issues.Add('profile/environment/network isolation check failed') }
            if ($report.pointer_overlay_policy.enabled -ne ($PointerOverlayProcessId -ne 0)) { $issues.Add('pointer-overlay exception differs from the explicitly requested policy') }
            if ($PointerOverlayProcessId -ne 0) {
                $verifiedPointer = $report.pointer_overlay_policy.verified_process
                if (!$verifiedPointer -or $verifiedPointer.process_id -ne $PointerOverlayProcessId) { $issues.Add('fixed Sky pointer process was not verified') }
                elseif ($null -eq $pointerIdentity) { $pointerIdentity = $verifiedPointer }
                elseif (!(Test-JsonEqual $pointerIdentity $verifiedPointer)) { $issues.Add('verified pointer process creation/image identity changed during the batch') }
            }
            if ($report.occlusion_observations.armed_potential_occluders_max -ne 0 -or $report.pointer_overlay_policy.absolute_unobscured_display_claimed -ne $false) { $issues.Add('application/unknown occlusion or unsupported absolute visibility claim') }
            $valid = $issues.Count -eq 0
            $total = if (Test-FiniteNonnegative $report.total_ms) { [double]$report.total_ms } else { $null }
            $sample = [pscustomobject]@{Repeat=$repeat;CaseIndex=$caseIndex;Request=$requests[$caseIndex];ExitCode=$exitCode;Valid=$valid;InvalidReasons=$issues.ToArray();TotalMs=$total;Slow=$valid -and $total -ge 500;TotalCount=$report.page.total_count;ReturnedCount=$report.page.returned_count;RankingCacheHit=$report.page.search_ranking_cache_hit;OracleMatches=$report.oracle_matches;InputTimestampSource=$report.input_timestamp_source;Window=$report.window;PointerOverlayPolicy=$report.pointer_overlay_policy;OcclusionObservations=$report.occlusion_observations;Report=$reportPath;Stdout=$stdoutPath;Stderr=$stderrPath}
            $samples.Add($sample)
            [void](Assert-LibraryMetadata $databaseBefore)
            if ($env:AUDIOLIB_DATA_DIR -cne $normalDataDirectory) { throw 'Normal AUDIOLIB_DATA_DIR environment changed during validation' }
            Write-Host "$name valid=$valid total_ms=$total exit=$exitCode (all evidence retained)"
        }
    }
    $completed = $true
} catch {
    $blockedReason = $_.Exception.Message
    Write-NewJson (Join-Path $output 'blocked.json') ([pscustomobject]@{Blocked=$true;Reason=$blockedReason;CollectedSamples=$samples.Count;ForcedTerminationAttempted=$false;RecordedUtc=[DateTimeOffset]::UtcNow.ToString('o')})
} finally {
    $databaseAfter = $null
    $databaseUnchanged = $false
    $binaryUnchanged = $false
    $requestsUnchanged = $false
    try { $databaseAfter = Assert-LibraryMetadata $databaseBefore; $databaseUnchanged = $true } catch { if (!$blockedReason) { $blockedReason = $_.Exception.Message } }
    try { $binaryUnchanged = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash -ceq $binaryHash } catch { if (!$blockedReason) { $blockedReason = $_.Exception.Message } }
    try { $requestsUnchanged = (Get-FileHash -LiteralPath $requestPath -Algorithm SHA256).Hash -ceq $requestHash } catch { if (!$blockedReason) { $blockedReason = $_.Exception.Message } }
    $environmentPreserved = $env:AUDIOLIB_DATA_DIR -ceq $normalDataDirectory
    if (!$databaseUnchanged -or !$binaryUnchanged -or !$requestsUnchanged -or !$environmentPreserved) { if (!$blockedReason) { $blockedReason = 'Final binary/request/database/environment identity gate failed' } }
    if ($blockedReason) { $completed = $false }
    $validCount = @($samples | Where-Object Valid).Count
    $invalidCount = $samples.Count-$validCount
    $slowCount = @($samples | Where-Object Slow).Count
    $sampleArray = $samples.ToArray()
    $perCase = for ($caseIndex = 0; $caseIndex -lt $requests.Count; $caseIndex++) {
        $caseSamples = @($samples | Where-Object CaseIndex -EQ $caseIndex)
        [pscustomobject]@{CaseIndex=$caseIndex;Request=$requests[$caseIndex];Collected=$caseSamples.Count;Valid=@($caseSamples | Where-Object Valid).Count;Invalid=@($caseSamples | Where-Object { !$_.Valid }).Count;SlowValid=@($caseSamples | Where-Object Slow).Count;Total=Get-LatencyStatistics $caseSamples}
    }
    Write-NewJson (Join-Path $output 'summary.json') ([pscustomobject]@{
        Completed=$completed;BlockedReason=$blockedReason;GoalSuccess=$completed -and $invalidCount -eq 0 -and $slowCount -eq 0
        BinarySHA256=$binaryHash;BinaryIdentityPreserved=$binaryUnchanged;RequestsSHA256=$requestHash;RequestsPreserved=$requestsUnchanged
        DatabaseBefore=$databaseBefore;DatabaseAfter=$databaseAfter;DatabaseSizeMtimePreserved=$databaseUnchanged;DatabaseContentHashed=$false
        NormalDataDirectoryEnvironmentPreserved=$environmentPreserved;ExpectedSamples=$requests.Count*$Repeats;CollectedSamples=$samples.Count
        Valid=$validCount;Invalid=$invalidCount;SlowValid=$slowCount;Total=Get-LatencyStatistics $sampleArray;PerCase=@($perCase);Samples=$sampleArray
        StatisticsPopulation='All and only valid samples; every invalid and slow report is retained; no sample replacement or automatic retry'
        Percentile='Sorted ceil((n-1)*p)';TargetMs=500;InputMode='Synthetic Slint event dispatch, not native OS keyboard input'
        Endpoint='First observed decoded WGC phase ALL compositor frame; startup and provider opening excluded; normal production debounce/worker/model/render path included'
        OSPageCacheCleared=$false;RealKeyboardCrossCheckPerformed=$false;FullWindowImagesSaved=$false;ForcedTerminationAttempted=$false
        PointerOverlayExceptionEnabled=$PointerOverlayProcessId -ne 0;PointerOverlayProcessId=$PointerOverlayProcessId;VerifiedPointerProcess=$pointerIdentity
        PointerOverlayException='Explicit verified Sky cursor decorations only; application/unknown occluders block; not an absolute unobscured-display guarantee'
        CompletedUtc=[DateTimeOffset]::UtcNow.ToString('o')
    })
}
if ($blockedReason) { throw "GUI search validation blocked: $blockedReason" }
if (!$completed -or $invalidCount -or $slowCount) { throw "GUI search validation failed: $invalidCount invalid, $slowCount valid totals at or above 500ms; all samples retained" }
Write-Host "GUI search validation passed: $validCount fresh-process samples, every valid total below 500ms"
