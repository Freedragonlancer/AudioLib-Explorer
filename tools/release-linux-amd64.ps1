# tools/release-linux-amd64.ps1
# Select the persistent x86_64 Linux lane and delegate to the native release plan.
#Requires -Version 7.3
[CmdletBinding()]
param(
    [ValidateSet('Plan','Register','Run')][string]$Action = 'Plan',
    [ValidatePattern('^v\d+\.\d+\.\d+(?:-(?:beta|rc)\.(?:0|[1-9]\d*))?$')][string]$Tag,
    [Parameter(Mandatory)][string]$CacheRoot,
    [Parameter(Mandatory)][string]$ExpectedHostName,
    [ValidateRange('Positive')][double]$BuildCacheLimitGiB = 8,
    [string]$ChecksFile
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest

if ((& uname -s) -cne 'Linux' -or (& uname -m) -cne 'x86_64') {
    throw 'Use the Linux AMD64 host or its Linux/AMD64 builder container.'
}
if ([Environment]::MachineName -cne $ExpectedHostName) {
    throw 'Use the fixed Linux AMD64 host name registered for this cache.'
}
if (!$Tag -and $Action -ne 'Register') { throw 'Supply the release -Tag for Plan or Run.' }
if (![double]::IsFinite($BuildCacheLimitGiB) -or $BuildCacheLimitGiB -gt 100000) {
    throw 'BuildCacheLimitGiB must be finite and no greater than 100000.'
}

. (Join-Path $PSScriptRoot 'build-cache.ps1')
$root = Get-AudioLibCanonicalPath (Split-Path -Parent $PSScriptRoot)
if (![IO.Path]::IsPathFullyQualified($CacheRoot)) { throw 'CacheRoot must be an absolute mounted directory.' }
$cache = Get-AudioLibCanonicalPath $CacheRoot
Assert-AudioLibUnlinkedPath $cache
if (!(Test-Path -LiteralPath $cache -PathType Container) -or $cache -eq [IO.Path]::GetPathRoot($cache)) {
    throw 'Mount the dedicated persistent Linux AMD64 cache directory first.'
}
$separator = [IO.Path]::DirectorySeparatorChar
$protected = @($root, $env:CARGO_HOME, $env:RUSTUP_HOME, $env:AUDIOLIB_DATA_DIR, $env:AUDIOLIB_BETA_DATA_DIR)
$userProfile = [Environment]::GetFolderPath('UserProfile')
if ($userProfile) {
    $protected += @((Join-Path $userProfile '.cargo'), (Join-Path $userProfile '.rustup'))
}
foreach ($path in $protected) {
    if (!$path) { continue }
    $canonical = Get-AudioLibCanonicalPath $path
    if ($cache -ceq $canonical -or $cache.StartsWith($canonical + $separator,[StringComparison]::Ordinal) -or
        $canonical.StartsWith($cache + $separator,[StringComparison]::Ordinal)) {
        throw 'CacheRoot must be separate from the checkout, toolchain, Cargo registry and application data.'
    }
}
$target = Join-Path $cache 'target-linux-amd64'
$ownerPath = Join-Path $target '.audiolib-build-owner.json'
Assert-AudioLibUnlinkedPath $ownerPath
if ((Test-Path -LiteralPath $target) -and !(Test-Path -LiteralPath $target -PathType Container)) {
    throw 'Linux AMD64 target must be a directory.'
}
$checkout = & git -C $root rev-parse --show-toplevel
if ($LASTEXITCODE -ne 0 -or (Get-AudioLibCanonicalPath $checkout) -cne $root) {
    throw 'Use the fixed AudioLib release checkout as the source.'
}
$triple = 'x86_64-unknown-linux-gnu'
if ($Action -ne 'Register') {
    Push-Location -LiteralPath $root
    try {
        $compiler = & rustc -vV
        if ($LASTEXITCODE -ne 0 -or $compiler -cnotcontains "host: $triple") {
            throw "Select the pinned native $triple Rust toolchain before using this lane."
        }
    } finally { Pop-Location }
}

$stageLockPath = Join-Path $cache '.audiolib-linux-amd64-stage.lock'
$lease = $null
$owner = $null
$savedTarget = $env:CARGO_TARGET_DIR
$savedBuildTarget = $env:CARGO_BUILD_TARGET
try {
    if ($Action -ne 'Plan') {
        # Different container work volumes can have the same checkout path but
        # different Git lock files. Hold this shared-volume lock through cleanup.
        $lockPath = Join-Path $cache '.audiolib-linux-amd64-release.lock'
        Assert-AudioLibUnlinkedPath $lockPath
        try {
            $lease = [IO.File]::Open($lockPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
        } catch [IO.IOException] {
            throw 'The Linux AMD64 cache is busy or its lease cannot be opened. Check the existing runner first.'
        }
        # An interrupted supervisor releases its lock before its child necessarily exits.
        Assert-AudioLibUnlinkedPath $stageLockPath
        try {
            $stageProbe = [IO.File]::Open($stageLockPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
            $stageProbe.Dispose()
        } catch [IO.IOException] {
            throw 'A previous Linux AMD64 release stage is still active or its shared cache lease cannot be opened.'
        }
        Assert-AudioLibUnlinkedPath $ownerPath
        if (Test-Path -LiteralPath $ownerPath) {
            $ownerEntry = Get-Item -LiteralPath $ownerPath -Force
            if ($ownerEntry.PSIsContainer -or $ownerEntry.Length -gt 16384) { throw 'Invalid build cache owner marker.' }
            $owner = Get-Content -LiteralPath $ownerPath -Raw | ConvertFrom-Json -AsHashtable
            $identity = Get-AudioLibBuildPathHash (Get-AudioLibBuildCommonDirectory $root)
            if ($owner.schema_version -ne 1 -or $owner.owner_id -cne $identity -or $owner.host -cne $ExpectedHostName) {
                throw 'Linux AMD64 cache belongs to a different checkout path or host.'
            }
        } elseif ($Action -eq 'Run') {
            throw 'Register the empty dedicated Linux AMD64 target before Run.'
        }
        if ($Action -eq 'Register') {
            if ($owner) {
                @{status='registered'; reused=$true; target=$target} | ConvertTo-Json -Compress
                exit 0
            }
            & (Join-Path $PSScriptRoot 'clean-build-artifacts.ps1') -Action Register -Root $root -TargetDirectory $target
            exit $LASTEXITCODE
        }
    }

    $env:CARGO_TARGET_DIR = $target
    $env:CARGO_BUILD_TARGET = $triple
    $arguments = @{
        Action=$Action; Tag=$Tag; Root=$root
        PlanFile='tools/release-plans/server-unix.json'
        RunDirectory=".rust-validation/release-pipeline/$Tag/linux-x64-server"
        BuildCacheLimitGiB=$BuildCacheLimitGiB
        SharedStageLockPath=$stageLockPath
    }
    if ($ChecksFile) { $arguments.ChecksFile = $ChecksFile }
    & (Join-Path $PSScriptRoot 'release-pipeline.ps1') @arguments
    exit $LASTEXITCODE
} finally {
    $env:CARGO_TARGET_DIR = $savedTarget
    $env:CARGO_BUILD_TARGET = $savedBuildTarget
    if ($lease) { $lease.Dispose() }
}
