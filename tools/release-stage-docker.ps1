# tools/release-stage-docker.ps1
# Stage two accepted Docker file pairs on one existing GitHub draft.
#Requires -Version 7.3
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('\Av[0-9]+\.[0-9]+\.[0-9]+(?:-(?:rc|beta)\.[0-9]+)?\z')][string]$Tag,
    [Parameter(Mandatory)][ValidatePattern('\A[1-9][0-9]*\z')][string]$DraftId,
    [Parameter(Mandatory)][string]$NotesFile,
    [Parameter(Mandatory)][string]$AssetDirectory,
    [Parameter(Mandatory)][string]$ReadyDirectory,
    [string]$Bash,
    [ValidateRange(1, 86400)][int]$WaitSeconds = 14400
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot 'build-cache.ps1')

function Get-LocalPath([string]$Path, [string]$Type) {
    $item = Get-Item -LiteralPath $Path
    if (!(Test-Path -LiteralPath $item.FullName -PathType $Type)) { throw "Unexpected path type: $Path" }
    Assert-AudioLibUnlinkedPath $item.FullName
    return $item.FullName
}

function Get-GitObject([string]$Ref) {
    $object = & git -C $root rev-parse --verify $Ref
    if ($LASTEXITCODE -ne 0 -or $object -cnotmatch '\A(?:[a-f0-9]{40}|[a-f0-9]{64})\z') {
        throw "Cannot resolve the frozen checkout ref $Ref."
    }
    return $object
}

if (!$Bash) {
    $Bash = 'bash'
    if ($IsWindows) {
        $gitPath = (Get-Command git -CommandType Application | Select-Object -First 1).Source
        $Bash = Join-Path (Split-Path (Split-Path $gitPath -Parent) -Parent) 'bin/bash.exe'
    }
}
$Bash = (Get-Command $Bash -CommandType Application | Select-Object -First 1).Source
$NotesFile = Get-LocalPath $NotesFile Leaf
$AssetDirectory = Get-LocalPath $AssetDirectory Container
$ReadyDirectory = Get-LocalPath $ReadyDirectory Container
$commit = Get-GitObject HEAD
$tagObject = Get-GitObject "refs/tags/$Tag"
if ((Get-GitObject "refs/tags/$Tag^{commit}") -cne $commit) { throw 'Use the checkout for the frozen release tag.' }
$notesHash = (Get-FileHash -LiteralPath $NotesFile).Hash
$archives = @('amd64', 'arm64' | ForEach-Object { "AudioLibServer-$Tag-docker-linux-$_.tar.gz" })
$expected = & (Join-Path $PSScriptRoot 'verify-release-assets.ps1') -Tag $Tag -ListExpectedAssets
foreach ($archive in $archives) {
    if ($archive -cnotin $expected -or "$archive.sha256" -cnotin $expected) { throw "Unexpected Docker pair: $archive" }
}

$evidence = Join-Path $root ".rust-validation/release-upload/$Tag/$DraftId"
Assert-AudioLibUnlinkedPath $evidence
$null = [IO.Directory]::CreateDirectory($evidence)
$statePath = Join-Path $evidence 'state.json'
$lockPath = Join-Path $evidence 'coordinator.lock'
$serialLock = Join-Path $evidence 'serial.lock'

function Open-UploadLock([string]$Path) {
    Assert-AudioLibUnlinkedPath $Path
    try { return [IO.File]::Open($Path, 'OpenOrCreate', 'ReadWrite', 'None') }
    catch [IO.IOException] { throw "Docker staging is busy. Wait for the existing uploader to exit. Lock: $Path" }
}

function Save-UploadState {
    $tempPath = "$statePath.$([guid]::NewGuid().ToString('N')).tmp"
    [IO.File]::WriteAllText($tempPath, (ConvertTo-Json -InputObject $state -Depth 4))
    [IO.File]::Move($tempPath, $statePath, $true)
}

function Get-ReadyPair([string]$Archive) {
    $receiptPath = Join-Path $ReadyDirectory "$Archive.ready.json"
    if (!(Test-Path -LiteralPath $receiptPath -PathType Leaf)) { return }
    $receipt = Get-Content -LiteralPath (Get-LocalPath $receiptPath Leaf) -Raw | ConvertFrom-Json -AsHashtable
    if ($receipt.tag -cne $Tag -or $receipt.commit -cne $commit -or $receipt.archive -cne $Archive -or
        $receipt.sha256 -isnot [string] -or $receipt.sha256 -notmatch '\A[0-9a-f]{64}\z') {
        throw "Invalid acceptance receipt for $Archive."
    }
    $digest = $receipt.sha256.ToLowerInvariant()
    if ($state.accepted.Contains($Archive) -and $state.accepted[$Archive] -cne $digest) {
        throw "Accepted archive SHA256 changed for $Archive."
    }
    $package = Join-Path $AssetDirectory $Archive
    $checksum = "$package.sha256"
    return [pscustomobject]@{
        Archive = $Archive
        Digest = $digest
        Package = Get-LocalPath $package Leaf
        Checksum = Get-LocalPath $checksum Leaf
        Ready = (Get-Item -LiteralPath $receiptPath).LastWriteTimeUtc
    }
}

function Convert-BashPath([string]$Path) {
    if ($IsWindows) { return $Path.Replace('\', '/') }
    return $Path
}

function Start-PairStage($Pair, [switch]$Probe) {
    if ((Get-FileHash -LiteralPath $NotesFile).Hash -cne $notesHash -or
        (Get-GitObject 'HEAD') -cne $commit -or (Get-GitObject "refs/tags/$Tag") -cne $tagObject) {
        throw 'The frozen release checkout or reviewed notes changed during staging.'
    }
    $kind = if ($Probe) { 'check' } else { 'stage' }
    $attempt = Join-Path $evidence "$kind-$($Pair.Archive)-$([guid]::NewGuid().ToString('N'))"
    $null = [IO.Directory]::CreateDirectory($attempt)
    $commandFile = Join-Path $attempt 'command.json'
    $modeFlag = if ($Probe) { '--pair-status' } else { '--timings' }
    $command = @{
        root = $root
        lock_path = Join-Path $evidence "$($Pair.Archive).lock"
        command = $Bash
        arguments = @('./tools/release-create.sh', $Tag, '--resume-draft', '--stage-independent',
            '--draft-id', $DraftId, '--notes-file', (Convert-BashPath $NotesFile),
            '--accepted-sha256', $Pair.Digest, $modeFlag, '--',
            (Convert-BashPath $Pair.Package), (Convert-BashPath $Pair.Checksum))
    }
    if ($state.mode -ceq 'serial') { $command.shared_stage_lock_path = $serialLock }
    Assert-AudioLibUnlinkedPath $command.lock_path
    [IO.File]::WriteAllText($commandFile, (ConvertTo-Json -InputObject $command -Depth 4))
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Join-Path $PSHOME $(if ($IsWindows) { 'pwsh.exe' } else { 'pwsh' })
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'release-stage.ps1'), '-CommandFile', $commandFile)) {
        $start.ArgumentList.Add($argument)
    }
    # A worker may have exited while this launch was being prepared.
    foreach ($worker in $active) {
        if ($worker.Process.HasExited) { return }
    }
    $process = [Diagnostics.Process]::Start($start)
    return [pscustomobject]@{
        Archive = $Pair.Archive
        Attempt = $attempt
        Process = $process
        Output = $process.StandardOutput.ReadToEndAsync()
        ErrorOutput = $process.StandardError.ReadToEndAsync()
    }
}

function Receive-PairStage($Worker) {
    try {
        $Worker.Process.WaitForExit()
        $stdout = $Worker.Output.GetAwaiter().GetResult()
        $stderr = $Worker.ErrorOutput.GetAwaiter().GetResult()
        [IO.File]::WriteAllText((Join-Path $Worker.Attempt 'stdout.log'), $stdout)
        [IO.File]::WriteAllText((Join-Path $Worker.Attempt 'stderr.log'), $stderr)
        return [pscustomobject]@{ Code = $Worker.Process.ExitCode; Stdout = $stdout; Stderr = $stderr }
    } finally {
        $Worker.Process.Dispose()
    }
}

function Read-StageRecord([string]$Output, [string]$Prefix, [string]$Archive) {
    $lines = @($Output -split '\r?\n' | Where-Object { $_.StartsWith("$Prefix ", [StringComparison]::Ordinal) })
    if ($lines.Count -ne 1) { return }
    try { $record = $lines[0].Substring($Prefix.Length + 1) | ConvertFrom-Json -AsHashtable }
    catch { return }
    if ($record.tag -ceq $Tag -and $record.draft_id -is [string] -and
        $record.draft_id -ceq $DraftId -and $record.archive -ceq $Archive) { return $record }
}

function Test-InterruptedPost($Result, [string]$Archive) {
    $timing = Read-StageRecord $Result.Stdout RELEASE_UPLOAD_TIMING $Archive
    if ($timing.post_failures -le 0 -or $timing.exit_code -le 0 -or
        $timing.failed_post_asset -cnotin @($Archive, "$Archive.sha256")) { return $false }
    $workerErrors = @($Result.Stderr -split '\r?\n' | Where-Object { $_ -cmatch '^release-create:' })
    foreach ($message in $workerErrors) {
        if ($message -cne 'release-create: Cannot inspect draft identity/assets') { return $false }
    }
    $name = [regex]::Escape([uri]::EscapeDataString($timing.failed_post_asset))
    return $Result.Stderr -match "(?im)https://uploads\.github\.com/[^""\r\n]*[?&]name=$name(?=[""&])[^\r\n]*(?:connection (?:was forcibly closed|reset)|\b(?:unexpected )?EOF\b|i/o timeout|broken pipe)"
}

function Format-StageFailure($Worker, $Result) {
    $details = @($Result.Stderr -split '\r?\n' | Where-Object { $_ -and $_ -notmatch '^Native stage exited with code \d+\.$' } | Select-Object -Last 1)
    return "Docker staging paused for $($Worker.Archive). $details See $($Worker.Attempt)."
}

$active = [Collections.Generic.List[object]]::new()
$completed = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
$lease = Open-UploadLock $lockPath
try {
    # The worker keeps its own lock if the coordinator is stopped.
    foreach ($path in @($serialLock) + @($archives | ForEach-Object { Join-Path $evidence "$_.lock" })) {
        (Open-UploadLock $path).Dispose()
    }
    Assert-AudioLibUnlinkedPath $statePath
    $identity = [ordered]@{
        schema_version = 1; tag = $Tag; draft_id = $DraftId; commit = $commit; tag_object = $tagObject; notes_sha256 = $notesHash
    }
    if (Test-Path -LiteralPath $statePath) {
        $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json -AsHashtable
        foreach ($key in $identity.Keys) {
            if ($state[$key] -cne $identity[$key]) { throw "Saved Docker staging identity differs ($key)." }
        }
        if ($state.mode -cnotin @('parallel', 'serial') -or $state.accepted -isnot [Collections.IDictionary] -or
            $state.pending -isnot [array]) { throw 'Invalid saved Docker staging state.' }
        foreach ($name in $state.accepted.Keys) {
            if ($name -cnotin $archives -or $state.accepted[$name] -cnotmatch '\A[0-9a-f]{64}\z') {
                throw 'Invalid saved Docker acceptance identity.'
            }
        }
        foreach ($name in $state.pending) {
            if ($name -cnotin $archives -or !$state.accepted.Contains($name)) { throw 'Invalid saved Docker recovery entry.' }
        }
    } else {
        $state = $identity
        $state.mode = 'parallel'
        $state.accepted = [ordered]@{}
        $state.pending = @()
        Save-UploadState
    }

    $failure = $null
    $idle = [Diagnostics.Stopwatch]::StartNew()
    :staging while ($completed.Count -lt $archives.Count) {
        foreach ($worker in $active.ToArray()) {
            if (!$worker.Process.HasExited) { continue }
            $null = $active.Remove($worker)
            $result = Receive-PairStage $worker
            if ($result.Code -eq 0) {
                $null = $completed.Add($worker.Archive)
                Write-Host "Verified Docker pair $($worker.Archive) on draft $DraftId."
            } elseif ($state.mode -ceq 'parallel' -and (Test-InterruptedPost $result $worker.Archive)) {
                $state.pending += $worker.Archive
                Save-UploadState
                Write-Warning "Docker POST interrupted for $($worker.Archive). Waiting for active uploads before checking draft $DraftId."
            } elseif (!$failure) {
                $failure = Format-StageFailure $worker $result
            }
            $idle.Restart()
        }

        if ($failure -and $active.Count -eq 0) { throw $failure }
        if (!$failure -and ($state.pending.Count -eq 0 -or $active.Count -eq 0) -and $completed.Count -lt $archives.Count) {
            $recovering = $state.pending.Count -gt 0
            $busy = @($active | ForEach-Object { $_.Archive })
            $pairs = @(
                foreach ($archive in $archives) {
                    if ($archive -cin $busy -or (!$recovering -and $completed.Contains($archive))) { continue }
                    $pair = Get-ReadyPair $archive
                    if ($pair) { $pair }
                    elseif ($state.accepted.Contains($archive)) { throw "Missing saved acceptance receipt for $archive." }
                }
            )
            $pairs = @($pairs | Sort-Object @{ Expression = { $_.Archive -cnotin $state.pending } }, Ready)
            # A strict snapshot is only safe once our in-flight uploads have drained.
            if ($active.Count -eq 0 -and $pairs.Count -gt 0) {
                foreach ($pair in $pairs) {
                    $state.accepted[$pair.Archive] = $pair.Digest
                    Save-UploadState
                    $probe = Start-PairStage $pair -Probe
                    $result = Receive-PairStage $probe
                    if ($result.Code -ne 0) { throw (Format-StageFailure $probe $result) }
                    $status = Read-StageRecord $result.Stdout RELEASE_PAIR_STATUS $pair.Archive
                    if ($status.missing -isnot [long] -or $status.missing -notin 0, 1, 2) {
                        throw "Invalid draft inspection result. See $($probe.Attempt)."
                    }
                    if ($status.missing -eq 0) { $null = $completed.Add($pair.Archive) }
                    else { $null = $completed.Remove($pair.Archive) }
                }
                if ($recovering) {
                    $missing = @($state.pending | Where-Object { !$completed.Contains($_) })
                    if ($missing.Count -gt 0) { $state.mode = 'serial' }
                    $state.pending = @()
                    Save-UploadState
                    if ($missing.Count -gt 0) { Write-Host "Switched to serial Docker staging on draft $DraftId." }
                }
            }
            $limit = if ($state.mode -ceq 'parallel') { 2 } else { 1 }
            foreach ($pair in $pairs) {
                if ($active.Count -ge $limit) { break }
                if ($completed.Contains($pair.Archive)) { continue }
                $state.accepted[$pair.Archive] = $pair.Digest
                Save-UploadState
                $worker = Start-PairStage $pair
                if (!$worker) { continue staging }
                $active.Add($worker)
                Write-Host "Started $($state.mode) Docker staging for $($pair.Archive) on draft $DraftId."
                $idle.Restart()
            }
        }
        if ($completed.Count -eq $archives.Count) { break }
        if ($active.Count -eq 0 -and $idle.Elapsed.TotalSeconds -ge $WaitSeconds) {
            throw "Timed out waiting for accepted Docker pairs. See $evidence."
        }
        Start-Sleep -Milliseconds 500
    }
    Write-Host "Both Docker pairs verified on draft $DraftId."
} finally {
    try {
        foreach ($worker in $active) { $null = Receive-PairStage $worker }
    } finally {
        $lease.Dispose()
    }
}
