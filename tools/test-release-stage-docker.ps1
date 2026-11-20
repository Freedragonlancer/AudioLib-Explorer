# tools/test-release-stage-docker.ps1
# Run the Docker staging coordinator against synthetic readiness receipts and Bash workers.
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

function Set-Ready($TestCase, [string]$Architecture) {
    $archive = "AudioLibServer-$tag-docker-linux-$Architecture.tar.gz"
    $path = Join-Path $TestCase.Assets $archive
    [IO.File]::WriteAllText($path, "Synthetic payload for $Architecture")
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

function Start-Coordinator($TestCase) {
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME $(if ($IsWindows) { 'pwsh.exe' } else { 'pwsh' })))
    $start.WorkingDirectory = $repo
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.Environment['AUDIOLIB_TEST_DOCKER_CONTROL'] = $TestCase.Control
    foreach ($argument in '-NoProfile','-File',(Join-Path $tools 'release-stage-docker.ps1'),
        '-Tag',$tag,'-DraftId',"$($TestCase.Draft)",'-NotesFile',$TestCase.Notes,'-AssetDirectory',$TestCase.Assets,
        '-ReadyDirectory',$TestCase.Ready,'-Bash',$Bash,'-WaitSeconds','40') { $start.ArgumentList.Add($argument) }
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
[ "$tag" = v1.2.3 ] && [[ "$draft" =~ ^[1-9][0-9]*$ ]] && [ "$resume:$independent" = 1:1 ] &&
  [ $# -eq 2 ] && [ -f "$(normalize "$notes")" ] || fail 'Mock worker received wrong scope'
[ "$mode:$timings" = upload:1 ] || [ "$mode:$timings" = status:0 ] || fail 'Incompatible status/timing scope'
archive_path=$(normalize "$1")
checksum_path=$(normalize "$2")
archive=${archive_path##*/}
case "$archive" in
  "AudioLibServer-$tag-docker-linux-amd64.tar.gz") arch=amd64 ;;
  "AudioLibServer-$tag-docker-linux-arm64.tar.gz") arch=arm64 ;;
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
  if [ "$scenario:$arch" = recovery:arm64 ]; then file="$archive.sha256"; fi
  printf 'gh: Post "https://uploads.github.com/repos/example/fixture/releases/%s/assets?name=%s": write tcp 192.0.2.1:443->192.0.2.2:443: connection reset by peer\n' "$draft" "$file" >&2
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
  starter:amd64:1|conflict:amd64:1)
    reset_upload ;;
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

    $scenarios = [ordered]@{
        stored = ''
        reconciled = ''
        starter = 'Remote asset is incomplete'
        conflict = 'size/SHA256 conflicts'
        mixed = 'HTTP 422'
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
            } elseif ($scenario -eq 'mixed') {
                if (Test-WorkerMarker $testCase 'status.amd64.2.start') { throw 'An earlier recovered archive reset triggered recovery for a later HTTP error.' }
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

    'Docker staging mock tests passed (readiness, live sibling drain, serial recovery, proven assets, hard conflicts and persisted recovery).'
} finally {
    $absolute = [IO.Path]::GetFullPath($fixture)
    if (!$absolute.StartsWith((Join-Path $tempParent 'AudioLibDockerStageTest-'),[StringComparison]::Ordinal) -or
        (Get-Item -LiteralPath $absolute -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'Refusing mock cleanup outside the owned temporary directory.'
    }
    Remove-Item -LiteralPath $absolute -Recurse -Force
}
