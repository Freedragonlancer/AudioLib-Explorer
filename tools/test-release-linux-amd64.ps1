# tools/test-release-linux-amd64.ps1
# Offline Linux-lane routing test: synthetic Git/cache, mocked host and pipeline.
#Requires -Version 7.3
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-ExpectedFailure([scriptblock]$Command, [string]$Pattern) {
    try { $message = & $Command | Out-String; $code = $LASTEXITCODE }
    catch { $message = $_.Exception.Message; $code = 1 }
    if (!$code -or $message -notmatch $Pattern) { throw "Unexpected result for failure matching $Pattern. $message" }
}

$linux = 'Linux'
$architecture = 'x86_64'
$compilerHost = 'x86_64-unknown-linux-gnu'
function uname {
    $global:LASTEXITCODE = 0
    switch ($args[0]) {
        '-s' { $linux }
        '-m' { $architecture }
        default { throw "Unexpected uname arguments." }
    }
}
function rustc {
    if ($args.Count -ne 1 -or $args[0] -cne '-vV') { throw 'Unexpected rustc arguments.' }
    $global:LASTEXITCODE = 0
    'rustc 1.97.1'
    "host: $compilerHost"
}

$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$fixture = Join-Path $tempParent ('AudioLibLinuxReleaseTest-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
try {
    $source = Join-Path $fixture 'source'
    $tools = Join-Path $source 'tools'
    $cache = Join-Path $fixture 'cache'
    $otherCache = Join-Path $fixture 'other-cache'
    $null = New-Item -ItemType Directory -Path $tools,$cache,$otherCache
    foreach ($fileName in 'release-linux-amd64.ps1','build-cache.ps1','clean-build-artifacts.ps1') {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $fileName) -Destination $tools
    }
    Set-Content -LiteralPath (Join-Path $source 'Cargo.toml') -Value '[workspace]', 'members = ["crates/audiolib-core"]'
    & git -C $source init --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Could not initialize owned fixture repo.' }
    Set-Content -LiteralPath (Join-Path $tools 'release-pipeline.ps1') -Value @(
        'param($Action,$Tag,$Root,$PlanFile,$RunDirectory,$BuildCacheLimitGiB,$ChecksFile)',
        '@{action=$Action;tag=$Tag;root=$Root;plan=$PlanFile;run=$RunDirectory;budget=$BuildCacheLimitGiB;checks=$ChecksFile;target=$env:CARGO_TARGET_DIR;triple=$env:CARGO_BUILD_TARGET}|ConvertTo-Json -Compress',
        'exit 0'
    )

    $route = Join-Path $tools 'release-linux-amd64.ps1'
    $hostName = [Environment]::MachineName
    $target = Join-Path $cache 'target-linux-amd64'
    $marker = Join-Path $target '.audiolib-build-owner.json'
    $lease = Join-Path $cache '.audiolib-linux-amd64-release.lock'
    $priorTarget = $env:CARGO_TARGET_DIR
    $priorTriple = $env:CARGO_BUILD_TARGET

    Assert-ExpectedFailure { & $route -Action Run -Tag v1.2.3 -CacheRoot $cache -ExpectedHostName $hostName } 'Register the empty dedicated'
    if (Test-Path -LiteralPath $target) { throw 'Unregistered run created a target.' }
    Assert-ExpectedFailure { & $route -Tag v1.2.3 -CacheRoot $source -ExpectedHostName $hostName } 'separate from the checkout'
    Assert-ExpectedFailure { & $route -Tag v1.2.3 -CacheRoot $cache -ExpectedHostName "$hostName-wrong" } 'fixed Linux AMD64 host name'
    $architecture = 'aarch64'
    Assert-ExpectedFailure { & $route -Tag v1.2.3 -CacheRoot $cache -ExpectedHostName $hostName } 'Linux AMD64 host'
    $architecture = 'x86_64'
    $compilerHost = 'aarch64-unknown-linux-gnu'
    Assert-ExpectedFailure { & $route -Tag v1.2.3 -CacheRoot $cache -ExpectedHostName $hostName } 'native x86_64-unknown-linux-gnu'
    $compilerHost = 'x86_64-unknown-linux-gnu'

    $preview = & $route -Tag v1.2.3 -CacheRoot $otherCache -ExpectedHostName $hostName -BuildCacheLimitGiB 6.5 -ChecksFile 'private-checks.json' | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $preview.action -cne 'Plan' -or $preview.plan -cne 'tools/release-plans/server-unix.json' -or
        $preview.run -cne '.rust-validation/release-pipeline/v1.2.3/linux-x64-server' -or $preview.root -cne $source -or
        $preview.target -cne (Join-Path $otherCache 'target-linux-amd64') -or $preview.triple -cne $compilerHost -or
        $preview.budget -ne 6.5 -or $preview.checks -cne 'private-checks.json' -or @(Get-ChildItem -LiteralPath $otherCache -Force).Count) {
        throw 'Plan did not select the native isolated lane and remain read-only.'
    }

    $register = & $route -Action Register -CacheRoot $cache -ExpectedHostName $hostName | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $register.status -cne 'registered' -or !(Test-Path -LiteralPath $marker -PathType Leaf)) {
        throw 'The existing cleaner did not register the empty synthetic cache.'
    }
    $retainedMarker = [IO.File]::ReadAllText($marker)
    $again = & $route -Action Register -CacheRoot $cache -ExpectedHostName $hostName | ConvertFrom-Json
    if (!$again.reused -or [IO.File]::ReadAllText($marker) -cne $retainedMarker) { throw 'Repeat registration changed the owner marker.' }

    $nonempty = Join-Path $otherCache 'target-linux-amd64'
    $null = New-Item -ItemType Directory -Path $nonempty
    $existingCargoTag = Join-Path $nonempty 'CACHEDIR.TAG'
    Set-Content -LiteralPath $existingCargoTag -Value 'Signature: 8a477f597d28d172789f06886806bc55'
    Assert-ExpectedFailure { & $route -Action Register -CacheRoot $otherCache -ExpectedHostName $hostName } 'explicit -AdoptExisting'
    if ((Test-Path -LiteralPath (Join-Path $nonempty '.audiolib-build-owner.json')) -or !(Test-Path -LiteralPath $existingCargoTag)) {
        throw 'Registration claimed or changed the unowned cache.'
    }

    $owner = $retainedMarker | ConvertFrom-Json
    $owner.host = "$hostName-other"
    $owner | ConvertTo-Json | Set-Content -LiteralPath $marker
    Assert-ExpectedFailure { & $route -Action Run -Tag v1.2.3 -CacheRoot $cache -ExpectedHostName $hostName } 'different checkout path or host'
    [IO.File]::WriteAllText($marker,$retainedMarker)

    $held = [IO.File]::Open($lease,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    try {
        Assert-ExpectedFailure { & $route -Action Run -Tag v1.2.4 -CacheRoot $cache -ExpectedHostName $hostName } 'cache is busy'
    } finally { $held.Dispose() }
    foreach ($tag in 'v1.2.3','v1.2.4') {
        $run = & $route -Action Run -Tag $tag -CacheRoot $cache -ExpectedHostName $hostName | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or $run.action -cne 'Run' -or $run.tag -cne $tag -or $run.run -cne ".rust-validation/release-pipeline/$tag/linux-x64-server" -or
            $run.target -cne $target -or $run.budget -ne 8 -or $run.triple -cne $compilerHost -or
            $env:CARGO_TARGET_DIR -cne $priorTarget -or $env:CARGO_BUILD_TARGET -cne $priorTriple) {
            throw 'Run did not use the stable target, tag-specific receipts, and positive budget.'
        }
    }
    'Linux AMD64 persistent release lane mock tests passed.'
} finally {
    $absolute = [IO.Path]::GetFullPath($fixture)
    if (!$absolute.StartsWith((Join-Path $tempParent 'AudioLibLinuxReleaseTest-'),[StringComparison]::Ordinal) -or
        (Get-Item -LiteralPath $absolute -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'Refusing mock cleanup outside the owned temporary directory.'
    }
    Remove-Item -LiteralPath $absolute -Recurse -Force
}
