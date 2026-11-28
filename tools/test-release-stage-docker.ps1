# tools/test-release-stage-docker.ps1
# Run the release staging coordinator against synthetic readiness receipts and Bash workers.
#Requires -Version 7.3
param([string]$Bash)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest

if (!$Bash) {
    $gitBash = if ($IsWindows) { Join-Path $env:ProgramFiles 'Git/bin/bash.exe' }
    $Bash = if ($gitBash -and (Test-Path -LiteralPath $gitBash -PathType Leaf)) { $gitBash }
        else { (Get-Command bash -CommandType Application -ErrorAction Stop).Source }
}
$tag = 'v1.2.3'
$arches = @('amd64','arm64')
$script:nextDraft = 12340
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$fixture = Join-Path $tempParent ('AudioLibDockerStageTest-' + [guid]::NewGuid().ToString('N'))
$repo = Join-Path $fixture 'synthetic repo'
$tools = Join-Path $repo 'tools'
$null = New-Item -ItemType Directory -Path $tools -Force

function New-TestCase([string]$Name) {
    $script:nextDraft++
    $caseRoot = Join-Path $fixture $Name
    $payload = Join-Path $caseRoot 'accepted assets'
    $ready = Join-Path $caseRoot 'ready receipts'
    $control = Join-Path $caseRoot 'mock worker'
    $notes = Join-Path $caseRoot 'release notes.md'
    $null = New-Item -ItemType Directory -Path $payload,$ready,$control -Force
    [IO.File]::WriteAllText($notes, "Synthetic release notes." + [char]10)
    [IO.File]::WriteAllText((Join-Path $control 'scenario'), $Name + [char]10)
    [pscustomobject]@{ Draft = $script:nextDraft; Assets = $payload; Ready = $ready; Notes = $notes; Control = $control }
}

function Set-Ready($TestCase, [string]$Pair) {
    $archive = switch ($Pair) {
        'mac-client' { "AudioLibExplorer-$tag-macos-arm64.zip" }
        'win-client' { "AudioLibExplorer-$tag-win-x64-portable.zip" }
        'mac-server' { "AudioLibServer-$tag-macos-arm64.tar.gz" }
        'win-server' { "AudioLibServer-$tag-win-x64.zip" }
        'linux-server' { "AudioLibServer-$tag-linux-x64.tar.gz" }
        default { "AudioLibServer-$tag-docker-linux-$Pair.tar.gz" }
    }
    $path = Join-Path $TestCase.Assets $archive
    [IO.File]::WriteAllText($path, "Synthetic payload for $Pair")
    $digest = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$path.sha256", "$digest  $archive" + [char]10)
    $receipt = Join-Path $TestCase.Ready "$archive.ready.json"
    $json = @{
        tag = $tag; commit = $commit; archive = $archive; sha256 = $digest
        ready_utc = [DateTimeOffset]::UtcNow.ToString('o')
    } | ConvertTo-Json -Compress
    [IO.File]::WriteAllText("$receipt.tmp", $json)
    [IO.File]::Move("$receipt.tmp", $receipt)
}

function Start-Coordinator($TestCase, [switch]$BlockGitCheck, [switch]$AllPairs) {
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME $(if ($IsWindows) { 'pwsh.exe' } else { 'pwsh' })))
    $start.WorkingDirectory = $repo
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.Environment['AUDIOLIB_TEST_DOCKER_CONTROL'] = $TestCase.Control
    $start.Environment['AUDIOLIB_TEST_DOCKER_DRAFT'] = [string]$TestCase.Draft
    $entry = if ($BlockGitCheck) { 'coordinator-git-gate.ps1' } else { 'release-stage-docker.ps1' }
    foreach ($argument in '-NoProfile','-File',(Join-Path $tools $entry),
        '-Tag',$tag,'-DraftId',"$($TestCase.Draft)",'-NotesFile',$TestCase.Notes,'-AssetDirectory',$TestCase.Assets,
        '-ReadyDirectory',$TestCase.Ready,'-Bash',$Bash,'-WaitSeconds','40') { $start.ArgumentList.Add($argument) }
    if ($AllPairs) { $start.ArgumentList.Add('-AllPairs') }
    $process = [Diagnostics.Process]::Start($start)
    [pscustomobject]@{ Process = $process; Output = $process.StandardOutput.ReadToEndAsync(); Error = $process.StandardError.ReadToEndAsync() }
}

function Get-CoordinatorResult($Run) {
    if (!$Run.Process.WaitForExit(40000)) { throw 'Timed out waiting for the offline Docker coordinator.' }
    [pscustomobject]@{ Code = $Run.Process.ExitCode; Output = $Run.Output.Result; Error = $Run.Error.Result }
}

function Close-Coordinator($Run) {
    if (!$Run.Process.HasExited) {
        $Run.Process.Kill($true)
        $null = $Run.Process.WaitForExit(5000)
    }
    $Run.Process.Dispose()
}

function Test-WorkerMarker($TestCase, [string]$Marker) {
    Test-Path -LiteralPath (Join-Path $TestCase.Control $Marker) -PathType Leaf
}

function Wait-WorkerMarker($TestCase, $Run, [string[]]$Markers) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        foreach ($marker in $Markers) { if (Test-WorkerMarker $TestCase $marker) { return $marker } }
        if ($Run.Process.HasExited) {
            $result = Get-CoordinatorResult $Run
            throw "Coordinator exited before $($Markers -join ', '). $($result.Output) $($result.Error)"
        }
        Start-Sleep -Milliseconds 50
    } while ($timer.Elapsed.TotalSeconds -lt 20)
    throw "Timed out waiting for synthetic worker $($Markers -join ', ')."
}

function Let-WorkerFinish($TestCase, [string]$Gate) {
    [IO.File]::WriteAllText((Join-Path $TestCase.Control "allow.$Gate"), '')
}

function Assert-CoordinatorResult($Run, [bool]$Success, [string]$Pattern = '') {
    $result = Get-CoordinatorResult $Run
    $message = ($result.Output + $result.Error) -replace '(?m)^[ \t]*\|[ \t]*','' -replace '\s+',' '
    if ((($result.Code -eq 0) -ne $Success) -or ($Pattern -and $message -notmatch $Pattern)) {
        throw "Unexpected coordinator exit $($result.Code). $($result.Output) $($result.Error)"
    }
}

function Assert-SerialRetries($TestCase, [int]$Attempt) {
    $events = @(Get-Content -LiteralPath (Join-Path $TestCase.Control 'events'))
    $amdStart = [array]::IndexOf($events, "upload amd64 $Attempt start")
    $amdEnd = [array]::IndexOf($events, "upload amd64 $Attempt finish")
    $armStart = [array]::IndexOf($events, "upload arm64 $Attempt start")
    $armEnd = [array]::IndexOf($events, "upload arm64 $Attempt finish")
    if (($amdStart -lt 0) -or ($armStart -lt 0) -or ($amdEnd -le $amdStart) -or ($armEnd -le $armStart) -or
        !($amdEnd -lt $armStart -or $armEnd -lt $amdStart)) {
        throw "Docker attempt $Attempt was not serialized for both architectures. $($events -join '; ')"
    }
}

try {
    foreach ($name in 'release-stage-docker.ps1','release-stage.ps1','build-cache.ps1','verify-release-assets.ps1') {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $tools
    }
    [IO.File]::WriteAllText((Join-Path $repo '.gitignore'), "/.rust-validation/" + [char]10)
    [IO.File]::WriteAllText((Join-Path $repo 'Cargo.toml'), ('[workspace.package]' + [char]10 + 'version = "1.2.3"' + [char]10))
    $fakeWorker = @'
#!/usr/bin/env bash
set -eu
fail() { printf '%s\n' "$*" >&2; exit 17; }
normalize() {
  case "$1" in
    [A-Za-z]:*) cygpath -u "$1" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
control=$(normalize "$AUDIOLIB_TEST_DOCKER_CONTROL")
scenario=$(cat "$control/scenario")
tag=$1
shift
mode=upload
resume=0
independent=0
timings=0
draft=''
notes=''
accepted=''
while [ $# -gt 0 ]; do
  case "$1" in
    --resume-draft) resume=1; shift ;;
    --stage-independent) independent=1; shift ;;
    --timings) timings=1; shift ;;
    --pair-status) mode=status; shift ;;
    --draft-id) draft=$2; shift 2 ;;
    --notes-file) notes=$2; shift 2 ;;
    --accepted-sha256) accepted=$2; shift 2 ;;
    --) shift; break ;;
    *) fail "Unexpected mock worker option $1" ;;
  esac
done
[ "$tag" = v1.2.3 ] && [ "$draft" = "$AUDIOLIB_TEST_DOCKER_DRAFT" ] && [ "$resume:$independent" = 1:1 ] &&
  [ $# -eq 2 ] && [ -f "$(normalize "$notes")" ] || fail 'Mock worker received wrong scope'
[ "$mode:$timings" = upload:1 ] || [ "$mode:$timings" = status:0 ] || fail 'Incompatible status/timing scope'
archive_path=$(normalize "$1")
checksum_path=$(normalize "$2")
archive=${archive_path##*/}
case "$archive" in
  "AudioLibServer-$tag-docker-linux-amd64.tar.gz") arch=amd64 ;;
  "AudioLibServer-$tag-docker-linux-arm64.tar.gz") arch=arm64 ;;
  "AudioLibExplorer-$tag-macos-arm64.zip") arch=mac-client ;;
  "AudioLibExplorer-$tag-win-x64-portable.zip") arch=win-client ;;
  "AudioLibServer-$tag-macos-arm64.tar.gz") arch=mac-server ;;
  "AudioLibServer-$tag-win-x64.zip") arch=win-server ;;
  "AudioLibServer-$tag-linux-x64.tar.gz") arch=linux-server ;;
  *) fail 'Mock worker received unrecognized archive' ;;
esac
[ -f "$archive_path" ] && [ "$checksum_path" = "$archive_path.sha256" ] || fail 'Mock worker received wrong asset pair'
read -r sidecar_hash sidecar_name < "$checksum_path"
[ "$accepted" = "$sidecar_hash" ] && [ "$archive" = "$sidecar_name" ] || fail 'Mock worker did not receive trusted receipt hash'
attempt=1
while [ -e "$control/$mode.$arch.$attempt.start" ]; do attempt=$((attempt+1)); done
record() {
  printf '%s %s %s %s\n' "$mode" "$arch" "$attempt" "$1" >> "$control/events"
  : > "$control/$mode.$arch.$attempt.$1"
}
record start
wait_gate() {
  i=0
  while [ ! -f "$control/allow.$1" ]; do
    i=$((i+1))
    [ "$i" -lt 240 ] || fail 'Timed out waiting for synthetic gate'
    sleep 0.1
  done
}
timing() {
  printf 'RELEASE_UPLOAD_TIMING {"schema_version":1,"tag":"%s","draft_id":"%s","archive":"%s","read_hash_ms":10,"intake_read_hash_ms":0,"local_read_hash_ms":10,"post_ms":15,"post_archive_ms":10,"post_checksum_ms":5,"remote_verify_ms":5,"post_attempts":%s,"post_failures":%s,"failed_post_asset":"%s","exit_code":%s}\n' \
    "$tag" "$draft" "$archive" "$1" "$2" "${4:-}" "$3"
}
reset_message() {
  file=$archive
  case "$scenario:$arch" in recovery:arm64|shared-recovery:arm64) file="$archive.sha256" ;; esac
  post_draft=$draft
  if [ "$scenario" = wrong-draft ]; then post_draft=$((draft+1)); fi
  printf 'gh: Post "https://uploads.github.com/repos/example/fixture/releases/%s/assets?name=%s": write tcp 192.0.2.1:443->192.0.2.2:443: connection reset by peer\n' "$post_draft" "$file" >&2
}
reset_upload() {
  reset_message
  timing 1 1 42 "$file"
  record finish
  exit 42
}
if [ "$mode" = status ]; then
  if [ "$arch" = amd64 ]; then
    case "$scenario:$attempt" in
      preexisting:*) fail 'release-create: Remote asset is incomplete: upload state starter' ;;
      starter:2) fail 'release-create: Remote asset is incomplete: upload state starter' ;;
      conflict:2) fail 'release-create: Remote asset size/SHA256 conflicts with accepted local asset' ;;
    esac
  fi
  missing=0
  if [ ! -f "$control/stored.$arch" ]; then
    if [ -f "$control/archive-stored.$arch" ]; then missing=1; else missing=2; fi
  fi
  printf 'RELEASE_PAIR_STATUS {"schema_version":1,"tag":"%s","draft_id":"%s","archive":"%s","missing":%s}\n' "$tag" "$draft" "$archive" "$missing"
  record finish
  exit 0
fi
case "$scenario:$arch:$attempt" in
  recovery:*:1)
    wait_gate "first.$arch"
    if [ "$arch" = arm64 ]; then : > "$control/archive-stored.$arch"; fi
    reset_upload ;;
  recovery:*:2)
    wait_gate "retry.$arch" ;;
  shared-recovery:amd64:1|shared-recovery:mac-server:1)
    wait_gate "first.$arch" ;;
  shared-recovery:arm64:1)
    wait_gate first.arm64
    : > "$control/archive-stored.$arch"
    reset_upload ;;
  shared-recovery:arm64:2)
    [ -f "$control/archive-stored.$arch" ] || fail 'Recovery lost the already uploaded archive'
    wait_gate retry.arm64 ;;
  persisted:*:1)
    reset_upload ;;
  persisted:*:2)
    if [ ! -f "$control/allow.relaunch" ]; then reset_upload; fi ;;
  stored:amd64:1)
    reset_message
    : > "$control/stored.$arch"
    timing 2 1 0
    record finish
    exit 0 ;;
  reconciled:amd64:1)
    : > "$control/stored.$arch"
    reset_upload ;;
  starter:amd64:1|conflict:amd64:1|wrong-draft:amd64:1)
    reset_upload ;;
  dispatch-starter:amd64:1|dispatch-conflict:amd64:1)
    wait_gate first.amd64
    reset_message
    timing 1 1 42 "$archive"
    record finish
    if [ "$scenario" = dispatch-starter ]; then
      fail 'release-create: Remote asset is incomplete: upload state starter'
    fi
    fail 'release-create: Remote asset size/SHA256 conflicts with accepted local asset' ;;
  dispatch-*:arm64:1)
    record post ;;
  mixed:amd64:1)
    reset_message
    printf 'gh: HTTP 422 while uploading %s.sha256\n' "$archive" >&2
    timing 2 2 22 "$archive.sha256"
    record finish
    exit 22 ;;
esac
: > "$control/stored.$arch"
if [ -f "$control/archive-stored.$arch" ]; then timing 1 0 0; else timing 2 0 0; fi
record finish
'@
    [IO.File]::WriteAllText((Join-Path $tools 'release-create.sh'), ($fakeWorker -replace '\r\n', [string][char]10))
    $gitGate = @'
$ErrorActionPreference = 'Stop'
function git {
    $control = $env:AUDIOLIB_TEST_DOCKER_CONTROL
    if ($args[-1] -ceq 'HEAD' -and (Test-Path -LiteralPath (Join-Path $control 'block.git.dispatch'))) {
        # This check runs inside the coordinator's launch preflight after its active-worker poll.
        if ($active.Count -ne 1) { throw 'Expected one live uploader before dispatch.' }
        $blocked = Join-Path $control 'git.dispatch.blocked'
        [IO.File]::WriteAllText("$blocked.tmp", [string]$active[0].Process.Id)
        [IO.File]::Move("$blocked.tmp", $blocked)
        $timer = [Diagnostics.Stopwatch]::StartNew()
        while (!(Test-Path -LiteralPath (Join-Path $control 'allow.git.dispatch'))) {
            if ($timer.Elapsed.TotalSeconds -gt 20) { throw 'Timed out waiting for the synthetic Git gate.' }
            Start-Sleep -Milliseconds 50
        }
    }
    $application = Get-Command git -CommandType Application | Select-Object -First 1
    & $application.Source @args
}
& (Join-Path $PSScriptRoot 'release-stage-docker.ps1') @args
'@
    [IO.File]::WriteAllText((Join-Path $tools 'coordinator-git-gate.ps1'), $gitGate)
    & git -C $repo init --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the synthetic release repository.' }
    & git -C $repo -c core.autocrlf=false add --all
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage the synthetic release repository.' }
    & git -C $repo -c user.name=AudioLib -c user.email=fixture@example.invalid -c commit.gpgSign=false commit --quiet -m 'Synthetic staging fixture'
    if ($LASTEXITCODE -ne 0) { throw 'Could not commit the synthetic release repository.' }
    & git -C $repo -c tag.gpgSign=false tag $tag
    if ($LASTEXITCODE -ne 0) { throw 'Could not tag the synthetic release repository.' }
    $commit = & git -C $repo rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Could not resolve the synthetic release commit.' }

    $handoff = New-TestCase 'handoff'
    $nativePairLock = Join-Path $repo ".rust-validation/release-upload/$tag/$($handoff.Draft)/AudioLibServer-$tag-macos-arm64.tar.gz.lock"
    $null = [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($nativePairLock))
    $lease = [IO.File]::Open($nativePairLock, 'OpenOrCreate', 'ReadWrite', 'None')
    try {
        $run = Start-Coordinator $handoff -AllPairs
        try { Assert-CoordinatorResult $run $false 'Release staging is busy.*macos-arm64' } finally { Close-Coordinator $run }
    } finally { $lease.Dispose() }
    $commandPath = Join-Path $handoff.Control 'command.json'
    $stageScript = Join-Path $handoff.Control 'stage.ps1'
    $started = (Get-Process -Id $PID).StartTime.ToUniversalTime().Ticks
    [IO.File]::WriteAllText($stageScript, 'Write-Output "Synthetic upload started"')
    @{
        root = $repo; lock_path = $nativePairLock; script = $stageScript; parameters = @{}
        upload_coordinator_pid = $PID; upload_coordinator_started = $started - 1
    } | ConvertTo-Json | Set-Content -LiteralPath $commandPath
    $stageOutput = & (Join-Path $PSHOME $(if ($IsWindows) { 'pwsh.exe' } else { 'pwsh' })) -NoProfile -File (Join-Path $tools 'release-stage.ps1') -CommandFile $commandPath 2>&1
    if ($LASTEXITCODE -eq 0 -or ($stageOutput -join ' ') -notmatch 'coordinator exited before its upload worker' -or
        $stageOutput -contains 'Synthetic upload started') {
        throw 'A delayed upload worker ran without its original coordinator.'
    }

    $recovery = New-TestCase 'recovery'
    Set-Ready $recovery 'amd64'
    $run = Start-Coordinator $recovery
    try {
        $null = Wait-WorkerMarker $recovery $run 'upload.amd64.1.start'
        if (Test-WorkerMarker $recovery 'status.arm64.1.start') { throw 'ARM64 started before its receipt was ready.' }
        Set-Ready $recovery 'arm64'
        $null = Wait-WorkerMarker $recovery $run 'upload.arm64.1.start'
        if (Test-WorkerMarker $recovery 'upload.amd64.1.finish') { throw 'The two initial uploaders did not overlap.' }
        Let-WorkerFinish $recovery 'first.amd64'
        $null = Wait-WorkerMarker $recovery $run 'upload.amd64.1.finish'
        Start-Sleep -Milliseconds 1500
        if ((Test-WorkerMarker $recovery 'upload.arm64.1.finish') -or (Test-WorkerMarker $recovery 'status.amd64.2.start') -or
            (Test-WorkerMarker $recovery 'upload.amd64.2.start') -or $run.Process.HasExited) {
            throw 'Recovery began before the still-live sibling finished.'
        }
        Let-WorkerFinish $recovery 'first.arm64'
        $firstRetry = Wait-WorkerMarker $recovery $run @('upload.amd64.2.start','upload.arm64.2.start')
        $otherRetry = if ($firstRetry -eq 'upload.amd64.2.start') { 'upload.arm64.2.start' } else { 'upload.amd64.2.start' }
        Start-Sleep -Milliseconds 1500
        if (Test-WorkerMarker $recovery $otherRetry) { throw 'The second retry overlapped the held retry.' }
        foreach ($arch in $arches) { Let-WorkerFinish $recovery "retry.$arch" }
        Assert-CoordinatorResult $run $true 'Switched to serial Docker staging'
        Assert-SerialRetries $recovery 2
        $events = @(Get-Content -LiteralPath (Join-Path $recovery.Control 'events'))
        $siblingFinished = [array]::IndexOf($events, 'upload arm64 1 finish')
        $retryStarted = [array]::IndexOf($events, $firstRetry.Replace('.', ' '))
        if ($siblingFinished -lt 0 -or $retryStarted -le ($siblingFinished + 1)) { throw 'Missing recovery check interval.' }
        $checks = @($events[($siblingFinished + 1)..($retryStarted - 1)])
        foreach ($arch in $arches) {
            if (!($checks -like "status $arch * finish")) { throw "Missing recovery pair verification for $arch before retry." }
        }
    } finally { Close-Coordinator $run }

    $shared = New-TestCase 'shared-recovery'
    Set-Ready $shared 'amd64'
    $run = Start-Coordinator $shared -AllPairs
    try {
        $null = Wait-WorkerMarker $shared $run 'upload.amd64.1.start'
        Set-Ready $shared 'arm64'
        $null = Wait-WorkerMarker $shared $run 'upload.arm64.1.start'
        Set-Ready $shared 'mac-server'
        Start-Sleep -Milliseconds 1500
        if (Test-WorkerMarker $shared 'upload.mac-server.1.start') {
            throw 'A native pair bypassed the two occupied Docker slots.'
        }

        Let-WorkerFinish $shared 'first.amd64'
        $null = Wait-WorkerMarker $shared $run 'upload.mac-server.1.start'
        if (!(Test-WorkerMarker $shared 'upload.amd64.1.finish') -or
            (Test-WorkerMarker $shared 'upload.arm64.1.finish')) {
            throw 'The queued native pair did not take the freed slot while ARM64 stayed active.'
        }
        Let-WorkerFinish $shared 'first.arm64'
        $null = Wait-WorkerMarker $shared $run 'upload.arm64.1.finish'
        Set-Ready $shared 'win-client'
        Start-Sleep -Milliseconds 1500
        if ((Test-WorkerMarker $shared 'status.arm64.2.start') -or
            (Test-WorkerMarker $shared 'upload.arm64.2.start') -or
            (Test-WorkerMarker $shared 'upload.win-client.1.start') -or $run.Process.HasExited) {
            throw 'A failed Docker POST triggered rechecks or dispatch before the native upload drained.'
        }
        Let-WorkerFinish $shared 'first.mac-server'
        $null = Wait-WorkerMarker $shared $run 'upload.arm64.2.start'
        foreach ($pair in 'mac-client','win-server','linux-server') { Set-Ready $shared $pair }
        Start-Sleep -Milliseconds 1500
        foreach ($pair in 'win-client','mac-client','win-server','linux-server') {
            if (Test-WorkerMarker $shared "upload.$pair.1.start") {
                throw "A waiting pair ($pair) bypassed single-lane recovery."
            }
        }
        Let-WorkerFinish $shared 'retry.arm64'
        Assert-CoordinatorResult $run $true 'All release pairs verified'
        $events = @(Get-Content -LiteralPath (Join-Path $shared.Control 'events'))
        $nativeFinished = [array]::IndexOf($events, 'upload mac-server 1 finish')
        $retryStarted = [array]::IndexOf($events, 'upload arm64 2 start')
        if ($nativeFinished -lt 0 -or $retryStarted -le ($nativeFinished + 1)) {
            throw 'Missing same-draft recovery check interval after the native worker drained.'
        }
        $checks = @($events[($nativeFinished + 1)..($retryStarted - 1)])
        foreach ($pair in 'amd64','arm64','mac-server') {
            if (!($checks -like "status $pair * finish")) { throw "$pair was not rechecked before recovery." }
        }
        foreach ($pair in 'amd64','mac-server') {
            if (Test-WorkerMarker $shared "upload.$pair.2.start") { throw "Recovery reposted a complete pair ($pair)." }
        }
        $evidence = Join-Path $repo ".rust-validation/release-upload/$tag/$($shared.Draft)"
        $retry = Get-ChildItem -LiteralPath $evidence -Directory -Filter "stage-AudioLibServer-$tag-docker-linux-arm64.tar.gz-*" |
            ForEach-Object {
                $stdout = Join-Path $_.FullName 'stdout.log'
                if (Test-Path -LiteralPath $stdout -PathType Leaf) { Get-Content -LiteralPath $stdout }
            } |
            Where-Object { $_ -clike 'RELEASE_UPLOAD_TIMING *' } |
            ForEach-Object { $_.Substring('RELEASE_UPLOAD_TIMING '.Length) | ConvertFrom-Json } |
            Where-Object exit_code -eq 0
        if (@($retry).Count -ne 1 -or $retry.post_attempts -ne 1) {
            throw 'Expected one POST for the missing checksum in the successful recovery.'
        }
    } finally { Close-Coordinator $run }

    $dispatchConflicts = [ordered]@{
        'dispatch-starter' = 'upload state starter'
        'dispatch-conflict' = 'size/SHA256 conflicts'
    }
    foreach ($scenario in $dispatchConflicts.Keys) {
        $testCase = New-TestCase $scenario
        Set-Ready $testCase 'amd64'
        $run = Start-Coordinator $testCase -BlockGitCheck
        try {
            $null = Wait-WorkerMarker $testCase $run 'upload.amd64.1.start'
            [IO.File]::WriteAllText((Join-Path $testCase.Control 'block.git.dispatch'), '')
            Set-Ready $testCase 'arm64'
            $null = Wait-WorkerMarker $testCase $run 'git.dispatch.blocked'
            $processId = [int](Get-Content -LiteralPath (Join-Path $testCase.Control 'git.dispatch.blocked') -Raw)
            $uploader = [Diagnostics.Process]::GetProcessById($processId)
            try {
                Let-WorkerFinish $testCase 'first.amd64'
                if (!$uploader.WaitForExit(10000)) { throw 'The first worker did not exit while Git verification was blocked.' }
            } finally { $uploader.Dispose() }
            Let-WorkerFinish $testCase 'git.dispatch'
            Assert-CoordinatorResult $run $false $dispatchConflicts[$scenario]
            if ((Test-WorkerMarker $testCase 'upload.arm64.1.start') -or (Test-WorkerMarker $testCase 'upload.arm64.1.post')) {
                throw "A new uploader reached dispatch after the first uploader had already stopped on $scenario."
            }
        } finally { Close-Coordinator $run }
    }

    $scenarios = [ordered]@{
        stored = ''
        reconciled = ''
        starter = 'Remote asset is incomplete'
        conflict = 'size/SHA256 conflicts'
        mixed = 'HTTP 422'
        'wrong-draft' = 'connection reset by peer'
        preexisting = 'Remote asset is incomplete'
    }
    foreach ($scenario in $scenarios.Keys) {
        $testCase = New-TestCase $scenario
        foreach ($arch in $arches) { Set-Ready $testCase $arch }
        $run = Start-Coordinator $testCase
        try {
            Assert-CoordinatorResult $run ($scenario -in 'stored','reconciled') $scenarios[$scenario]
            foreach ($arch in $arches) {
                if (Test-WorkerMarker $testCase "upload.$arch.2.start") { throw "$scenario caused an unverified or unnecessary retry for $arch." }
            }
            if ($scenario -in 'stored','reconciled') {
                $state = Get-Content -LiteralPath (Join-Path $repo ".rust-validation/release-upload/$tag/$($testCase.Draft)/state.json") -Raw | ConvertFrom-Json
                if ($state.mode -cne 'parallel') { throw 'A proven stored POST unnecessarily downgraded the draft.' }
            }
            if ($scenario -eq 'preexisting') {
                if (Test-WorkerMarker $testCase 'upload.amd64.1.start') { throw 'Preexisting starter reached POST.' }
            } elseif ($scenario -in 'mixed','wrong-draft') {
                if (Test-WorkerMarker $testCase 'status.amd64.2.start') { throw "$scenario incorrectly triggered automatic recovery." }
            } elseif ($scenario -ne 'stored' -and !(Test-WorkerMarker $testCase 'status.amd64.2.start')) {
                throw "$scenario did not inspect the failed worker's remote pair."
            }
        } finally { Close-Coordinator $run }
    }

    $persisted = New-TestCase 'persisted'
    foreach ($arch in $arches) { Set-Ready $persisted $arch }
    $run = Start-Coordinator $persisted
    try { Assert-CoordinatorResult $run $false 'Switched to serial Docker staging' } finally { Close-Coordinator $run }
    $state = Get-Content -LiteralPath (Join-Path $repo ".rust-validation/release-upload/$tag/$($persisted.Draft)/state.json") -Raw | ConvertFrom-Json
    if ($state.mode -cne 'serial') { throw 'The failed retry did not preserve serial recovery mode.' }
    $initialEventCount = @(Get-Content -LiteralPath (Join-Path $persisted.Control 'events')).Count
    Let-WorkerFinish $persisted 'relaunch'
    $run = Start-Coordinator $persisted
    try {
        Assert-CoordinatorResult $run $true
        $events = @(Get-Content -LiteralPath (Join-Path $persisted.Control 'events'))
        $resumed = @($events | Select-Object -Skip $initialEventCount)
        if ($resumed.Count -eq 0 -or $resumed[0] -notlike 'status * start') {
            throw 'Relaunch did not begin with a read-only check of the saved serial recovery.'
        }
        $starts = @($resumed | Where-Object { $_ -like 'upload * start' })
        if ($starts.Count -ne 2) { throw 'Relaunch did not recover exactly the two still-missing pairs.' }
        $firstEnd = [array]::IndexOf($resumed, $starts[0].Replace(' start',' finish'))
        if ($firstEnd -lt 0 -or $firstEnd -ge [array]::IndexOf($resumed, $starts[1])) {
            throw 'Relaunch replayed parallel upload after recording serial recovery.'
        }
    } finally { Close-Coordinator $run }

    'Release staging mock tests passed (readiness, shared two-slot queue, mixed-worker drain, draft identity, serial recovery, remote conflicts and coordinator restart).'
} finally {
    $absolute = [IO.Path]::GetFullPath($fixture)
    if (!$absolute.StartsWith((Join-Path $tempParent 'AudioLibDockerStageTest-'),[StringComparison]::Ordinal) -or
        (Get-Item -LiteralPath $absolute -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'Refusing mock cleanup outside the owned temporary directory.'
    }
    Remove-Item -LiteralPath $absolute -Recurse -Force
}
