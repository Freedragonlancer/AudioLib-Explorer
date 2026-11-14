# tools/test-build-environment.ps1
# Synthetic path/ownership/environment tests. Never touches a real build cache.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'build-cache.ps1')
$taskFixture = Join-Path ([IO.Path]::GetTempPath()) ('AudioLibBuildEnvironmentTest-' + [guid]::NewGuid().ToString('N'))
$taskRoot = Join-Path $taskFixture 'checkout with spaces'
$taskOtherRoot = Join-Path $taskFixture 'other checkout'
$taskLocalData = Join-Path $taskFixture 'local-app-data'
$taskSaved = @{}
foreach ($name in @('CARGO_TARGET_DIR','TEMP','TMP','LOCALAPPDATA')) { $taskSaved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$taskWindows = [Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT
function Assert-TestEnvironment([hashtable]$Expected) {
    foreach ($name in $Expected.Keys) {
        if ([string][Environment]::GetEnvironmentVariable($name, 'Process') -cne [string]$Expected[$name]) { throw "Build environment did not restore $name." }
    }
}
function Assert-EnterRejected([string]$Root,[string]$Pattern) {
    $caught = $null
    try {
        $context = Enter-AudioLibBuildEnvironment $Root
        Exit-AudioLibBuildEnvironment $context
    } catch { $caught = $_ }
    if (!$caught -or $caught.ToString() -notmatch $Pattern) { throw "Expected build environment refusal ($Pattern), got: $caught" }
}
try {
    New-Item -ItemType Directory -Path $taskRoot,$taskOtherRoot -Force | Out-Null
    foreach ($root in @($taskRoot,$taskOtherRoot)) {
        & git -C $root init -q
        if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the owned build environment fixture.' }
    }
    [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $null, 'Process')
    [Environment]::SetEnvironmentVariable('LOCALAPPDATA', $taskLocalData, 'Process')
    $taskBaseline = @{}
    foreach ($name in @('CARGO_TARGET_DIR','TEMP','TMP','LOCALAPPDATA')) { $taskBaseline[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
    $target = Get-AudioLibTargetDirectory $taskRoot
    if (Test-Path -LiteralPath $target) { throw 'Read-only target resolution created a directory.' }
    if ((Get-AudioLibTargetDirectory ($taskRoot + [IO.Path]::DirectorySeparatorChar)) -cne $target) { throw 'Checkout path normalization changed its target.' }
    if ((Get-AudioLibTargetDirectory $taskOtherRoot) -ceq $target) { throw 'Different checkouts share a default build target.' }
    if ($taskWindows) {
        $expectedPrefix = (Get-AudioLibCanonicalPath (Join-Path $taskLocalData 'AudioLibExplorerBuild')) + [IO.Path]::DirectorySeparatorChar
        if (!$target.StartsWith($expectedPrefix,[StringComparison]::OrdinalIgnoreCase)) { throw 'Windows default build cache did not use local application storage.' }
    } elseif ($target -cne (Get-AudioLibCanonicalPath (Join-Path $taskRoot 'target'))) { throw 'Unix default target changed.' }
    Assert-TestEnvironment $taskBaseline
    Write-Host 'PASS: read-only resolution is deterministic, checkout-specific and platform-aware.'

    if ($taskWindows) {
        $env:LOCALAPPDATA = '\\127.0.0.1\AudioLibBuildEnvironmentTest'
        $networkError = $null
        try { $null = Get-AudioLibTargetDirectory $taskRoot } catch { $networkError = $_ }
        if (!$networkError -or $networkError.ToString() -notmatch 'fixed local volume') { throw 'Automatic Windows target accepted network-backed application storage.' }
        $env:LOCALAPPDATA = $taskLocalData
        Assert-TestEnvironment $taskBaseline
        Write-Host 'PASS: automatic Windows build storage refuses network volumes before creation.'
    }

    $context = Enter-AudioLibBuildEnvironment $taskRoot
    try {
        if ($env:CARGO_TARGET_DIR -cne $target -or $context.TargetDirectory -cne $target) { throw 'Entered build environment selected another target.' }
        if ($taskWindows) {
            $temporary = Join-Path $target 'tmp'
            if ($env:TEMP -cne $temporary -or $env:TMP -cne $temporary -or !(Test-Path -LiteralPath $temporary -PathType Container)) { throw 'Compiler temporary paths did not move into the local target.' }
            $owner = Get-Content -LiteralPath (Join-Path $target '.audiolib-build-owner.json') -Raw | ConvertFrom-Json
            if ($owner.schema_version -ne 1 -or $owner.host -cne [Environment]::MachineName -or $owner.owner_id -cne (Get-AudioLibBuildPathHash (Get-AudioLibBuildCommonDirectory $taskRoot))) { throw 'Fresh local target has incompatible ownership.' }
        }
    } finally { Exit-AudioLibBuildEnvironment $context }
    Assert-TestEnvironment $taskBaseline
    $context = Enter-AudioLibBuildEnvironment $taskRoot
    Exit-AudioLibBuildEnvironment $context
    Assert-TestEnvironment $taskBaseline
    Write-Host 'PASS: fresh local caches get matching ownership; entry/reentry restore caller environment.'

    if ($taskWindows) {
        $concurrentRoot = Join-Path $taskFixture 'concurrent checkout'
        New-Item -ItemType Directory -Path $concurrentRoot -Force | Out-Null
        & git -C $concurrentRoot init -q
        if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the concurrent starter fixture.' }
        $concurrentTarget = Get-AudioLibTargetDirectory $concurrentRoot
        $initializationParent = [IO.Path]::GetDirectoryName($concurrentTarget)
        New-Item -ItemType Directory -Path $initializationParent -Force | Out-Null
        $initializationPath = Join-Path $initializationParent '.audiolib-target-init.lock'
        $heldInitializationLock = [IO.File]::Open($initializationPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
        $childScript = Join-Path $taskFixture 'concurrent-starter.ps1'
        @'
param($Helper,$Root,$Ready)
$ErrorActionPreference='Stop'
. $Helper
[IO.File]::WriteAllText($Ready,'ready')
$context=Enter-AudioLibBuildEnvironment $Root
try {
    $owner=Get-Content -LiteralPath (Join-Path $context.TargetDirectory '.audiolib-build-owner.json') -Raw | ConvertFrom-Json
    if($owner.owner_id -cne (Get-AudioLibBuildPathHash (Get-AudioLibBuildCommonDirectory $Root))){throw 'Concurrent starter read incorrect ownership.'}
    $context.TargetDirectory
} finally {Exit-AudioLibBuildEnvironment $context}
'@ | Set-Content -LiteralPath $childScript -Encoding UTF8
        $children = @()
        try {
            $pwsh = (Get-Command pwsh -ErrorAction Stop).Source
            foreach ($number in 1..2) {
                $ready = Join-Path $taskFixture ("starter-$number.ready")
                $start = [Diagnostics.ProcessStartInfo]::new()
                $start.FileName = $pwsh
                $start.Arguments = '-NoProfile -ExecutionPolicy Bypass -File "' + $childScript + '" -Helper "' + (Join-Path $PSScriptRoot 'build-cache.ps1') + '" -Root "' + $concurrentRoot + '" -Ready "' + $ready + '"'
                $start.UseShellExecute = $false
                $start.CreateNoWindow = $true
                $start.RedirectStandardOutput = $true
                $start.RedirectStandardError = $true
                $null = $start.EnvironmentVariables.Remove('CARGO_TARGET_DIR')
                $start.EnvironmentVariables['LOCALAPPDATA'] = $taskLocalData
                $process = [Diagnostics.Process]::Start($start)
                $children += [pscustomobject]@{Process=$process;Ready=$ready;Output=$process.StandardOutput.ReadToEndAsync();Error=$process.StandardError.ReadToEndAsync()}
            }
            $readyTimer = [Diagnostics.Stopwatch]::StartNew()
            while (@($children | Where-Object { !(Test-Path -LiteralPath $_.Ready) }).Count) {
                if ($readyTimer.Elapsed.TotalSeconds -ge 10) { throw 'Concurrent starters did not reach the initialization fixture.' }
                Start-Sleep -Milliseconds 50
            }
            Start-Sleep -Milliseconds 250
            if ((Test-Path -LiteralPath $concurrentTarget) -or @($children | Where-Object { $_.Process.HasExited }).Count) { throw 'Concurrent starter bypassed the held initialization lock.' }
            $heldInitializationLock.Dispose()
            $heldInitializationLock = $null
            foreach ($child in $children) {
                if (!$child.Process.WaitForExit(15000)) { throw 'Concurrent starter did not finish after initialization unlocked.' }
                $output = $child.Output.GetAwaiter().GetResult().Trim()
                $errorOutput = $child.Error.GetAwaiter().GetResult()
                if ($child.Process.ExitCode -ne 0 -or $output -cne $concurrentTarget) { throw "Concurrent cache initialization failed: $errorOutput" }
            }
            if (!(Test-Path -LiteralPath $initializationPath -PathType Leaf)) { throw 'Initialization lock pathname was removed after unlocking.' }
            Assert-TestEnvironment $taskBaseline
        } finally {
            if ($heldInitializationLock) { $heldInitializationLock.Dispose() }
            foreach ($child in $children) {
                if (!$child.Process.HasExited) { $null = $child.Process.WaitForExit(15000) }
                if ($child.Process.HasExited) { $child.Process.Dispose() }
            }
        }
        Write-Host 'PASS: two concurrent first-use starters wait for initialization and read complete matching ownership.'
    }

    $env:CARGO_TARGET_DIR = 'explicit build artifacts'
    $explicitBaseline = @{}
    foreach ($name in @('CARGO_TARGET_DIR','TEMP','TMP','LOCALAPPDATA')) { $explicitBaseline[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
    $explicitTarget = Get-AudioLibCanonicalPath (Join-Path $taskRoot $env:CARGO_TARGET_DIR)
    if ((Get-AudioLibTargetDirectory $taskRoot) -cne $explicitTarget -or (Test-Path -LiteralPath $explicitTarget)) { throw 'Explicit relative target was not resolved read-only against the workspace.' }
    $context = Enter-AudioLibBuildEnvironment $taskRoot
    try {
        if ($env:CARGO_TARGET_DIR -cne $explicitTarget) { throw 'Explicit target was not made absolute.' }
        if (Test-Path -LiteralPath (Join-Path $explicitTarget '.audiolib-build-owner.json')) { throw 'Explicit cache was silently adopted.' }
    } finally { Exit-AudioLibBuildEnvironment $context }
    Assert-TestEnvironment $explicitBaseline
    [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $null, 'Process')
    Write-Host 'PASS: explicit relative targets stay authoritative and are never silently adopted.'

    if ($taskWindows) {
        $unregistered = Get-AudioLibTargetDirectory $taskOtherRoot
        New-Item -ItemType Directory -Path $unregistered -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $unregistered 'keep.txt'),'unregistered cache retained')
        Assert-EnterRejected $taskOtherRoot 'not registered'
        if ([IO.File]::ReadAllText((Join-Path $unregistered 'keep.txt')) -cne 'unregistered cache retained' -or (Test-Path -LiteralPath (Join-Path $unregistered '.audiolib-build-owner.json'))) { throw 'Unregistered target was modified.' }
        Assert-TestEnvironment $taskBaseline
        $marker = Join-Path $target '.audiolib-build-owner.json'
        $savedMarker = [IO.File]::ReadAllText($marker)
        try {
            [IO.File]::WriteAllText($marker,(@{schema_version=1;owner_id='foreign';host=[Environment]::MachineName} | ConvertTo-Json))
            Assert-EnterRejected $taskRoot 'different workspace or host'
            Assert-TestEnvironment $taskBaseline
        } finally { [IO.File]::WriteAllText($marker,$savedMarker) }
        Write-Host 'PASS: existing unregistered/foreign-owned local caches are refused without adoption.'
    }
    $linkParent = Join-Path $taskFixture 'linked target'
    $linkDestination = Join-Path $taskFixture 'link destination'
    New-Item -ItemType Directory -Path $linkDestination -Force | Out-Null
    if ($taskWindows) { New-Item -ItemType Junction -Path $linkParent -Target $linkDestination | Out-Null }
    else { $null = [IO.Directory]::CreateSymbolicLink($linkParent,$linkDestination) }
    $env:CARGO_TARGET_DIR = Join-Path $linkParent 'target'
    $linkedBaseline = @{CARGO_TARGET_DIR=$env:CARGO_TARGET_DIR;TEMP=$taskBaseline.TEMP;TMP=$taskBaseline.TMP;LOCALAPPDATA=$taskBaseline.LOCALAPPDATA}
    Assert-EnterRejected $taskRoot 'Linked build/cache path'
    Assert-TestEnvironment $linkedBaseline
    if (@(Get-ChildItem -LiteralPath $linkDestination -Force).Count) { throw 'Linked path refusal wrote outside the selected cache.' }
    Write-Host 'PASS: linked ancestors are refused before target creation and preserve caller environment.'

    $fixtureTools = Join-Path $taskFixture 'tools'
    New-Item -ItemType Directory -Path $fixtureTools -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'build-cache.ps1') -Destination $fixtureTools
    @'
param($Root,$TargetDirectory,$Action,$Mode,$MaxGiB)
if ($Action -cne 'Run' -or $Mode -cne 'Intermediate') { throw 'Wrong automatic cleanup mode.' }
@{root=$Root;target=$TargetDirectory;max_gib=$MaxGiB} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $Root 'cleanup-called.json')
if (Test-Path -LiteralPath (Join-Path $Root 'fail-cleanup')) { exit 17 }
exit 0
'@ | Set-Content -LiteralPath (Join-Path $fixtureTools 'clean-build-artifacts.ps1') -Encoding UTF8
    . (Join-Path $fixtureTools 'build-cache.ps1')
    $cleanupError = $null
    try { $null = Invoke-AudioLibPostBuildCleanup $taskRoot $explicitTarget 1.5 } catch { $cleanupError = $_ }
    if ($PSVersionTable.PSVersion -ge [version]'7.3') {
        if ($cleanupError) { throw $cleanupError }
        $cleanupCall = Get-Content -LiteralPath (Join-Path $taskRoot 'cleanup-called.json') -Raw | ConvertFrom-Json
        if ($cleanupCall.root -cne $taskRoot -or $cleanupCall.target -cne $explicitTarget -or $cleanupCall.max_gib -ne 1.5) { throw 'Automatic cleanup lost its explicit target or budget.' }
        [IO.File]::WriteAllText((Join-Path $taskRoot 'fail-cleanup'),'fail')
        $cleanupError = $null
        try { $null = Invoke-AudioLibPostBuildCleanup $taskRoot $explicitTarget } catch { $cleanupError = $_ }
        if (!$cleanupError -or $cleanupError.ToString() -notmatch 'cleanup did not complete') { throw 'Automatic cleanup ignored its failed exit code.' }
    } elseif (!$cleanupError -or $cleanupError.ToString() -notmatch 'pwsh 7.3') { throw 'Unsupported automatic cleanup did not report its required PowerShell version.' }
    Assert-TestEnvironment $linkedBaseline
    Write-Host 'PASS: post-build cleanup forwards the explicit target/budget and propagates failure or unsupported runtime.'
    Write-Host "Build environment regressions passed. Owned evidence: $taskFixture"
} finally {
    foreach ($name in $taskSaved.Keys) { [Environment]::SetEnvironmentVariable($name, $taskSaved[$name], 'Process') }
}
