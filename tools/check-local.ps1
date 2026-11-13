# tools/check-local.ps1
<#
.SYNOPSIS
Run local Rust validation once with the selected build features.
.DESCRIPTION
By default runs format, strict Clippy and selected tests once, failing at the first error.
Select Package or explicitly select Workspace. Stable, unaffected modules do not
need a new test run. Lib/Bin/Test narrow tests; Clippy still covers package targets.
Asio selects the release feature configuration after verified SDK preparation;
it does not add a second pass. Omit Asio to validate a non-ASIO build.
Logs stay in .rust-validation/local-checks/<run>/.
This does not build a release, install binaries or run subsystem validators.
Windows checks require the MSVC toolchain. If the default host is GNU, save the
current RUSTUP_TOOLCHAIN value, set $env:RUSTUP_TOOLCHAIN to
'1.97.1-x86_64-pc-windows-msvc' in this shell before invoking the script, and
restore the saved value afterward. Do not change the global Rustup toolchain.
.PARAMETER Asio
Enable desktop/CLI ASIO features for the single Clippy/test pass.
.PARAMETER Package
Packages affected by the change, including relevant direct consumers.
.PARAMETER Workspace
Explicitly check the whole workspace for broad or uncertain impact.
.PARAMETER Stage
All (default) runs format, Clippy and tests. Clippy or Tests runs only that stage
when other required evidence is still valid. Omitted stages are not-requested,
not passed or automatically reused. This does not infer which checks are needed.
.PARAMETER Lib
Test only the library target of one Package.
.PARAMETER Bin
Test only the named binary target of one Package; no target/package wildcards.
.PARAMETER Test
Run only the named integration test target of one Package; no target/package wildcards.
.PARAMETER TestFilter
Substring filter within the selected Lib/Bin/Test target (for example a module
name). A targeted run must report at least one passed test; zero matches or only
ignored tests are not acceptance evidence. Use the standard Rust test harness.
.PARAMETER NoDefaultFeatures
Match a release built with --no-default-features (for example the headless server).
.PARAMETER Target
Override the repository Cargo target for Clippy/tests (for example,
aarch64-apple-darwin or x86_64-unknown-linux-gnu). Format is target-independent.
ASIO requires a Windows MSVC target; omitting Target retains repository defaults.
.PARAMETER CleanBuildArtifacts
After checks pass, prune debug caches above BuildCacheLimitGiB (default 40).
Requires pwsh 7.3+; do not nest this cleanup inside an active release pipeline.
.EXAMPLE
./tools/check-local.ps1 -Package audiolib-db -Test search_fragments
.EXAMPLE
./tools/check-local.ps1 -Package audiolib-desktop -Bin AudioLibExplorer -TestFilter controller::search_tasks::tests:: -Asio
.EXAMPLE
./tools/check-local.ps1 -Package audiolib-db -Test search_fragments -Stage Tests
.EXAMPLE
./tools/check-local.ps1 -Package audiolib-desktop -Stage Clippy -Asio
.EXAMPLE
./tools/check-local.ps1 -Package audiolib-desktop,audiolib-cli -Asio
.EXAMPLE
./tools/check-local.ps1 -Package audiolib-server -NoDefaultFeatures -Target x86_64-unknown-linux-gnu
.EXAMPLE
./tools/check-local.ps1 -Workspace -Target aarch64-apple-darwin
#>
[CmdletBinding()]
param(
    [switch]$Asio,
    [ValidateNotNullOrEmpty()][string[]]$Package,
    [switch]$Workspace,
    [ValidateSet('All','Clippy','Tests')][string]$Stage = 'All',
    [switch]$Lib,
    [ValidateNotNullOrEmpty()][string]$Bin,
    [ValidateNotNullOrEmpty()][string]$Test,
    [ValidateNotNullOrEmpty()][string]$TestFilter,
    [switch]$NoDefaultFeatures,
    [ValidateNotNullOrEmpty()][string]$Target,
    [switch]$CleanBuildArtifacts,
    [ValidateRange(0,100000)][double]$BuildCacheLimitGiB = 40
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if ([bool]$Package -eq [bool]$Workspace) {
    throw 'Select -Package <packages> or -Workspace explicitly, not both. No checks were started.'
}
if ($Package) { $Package = @($Package | Select-Object -Unique) }
$testTargetCount = [int][bool]$Lib + [int][bool]$Bin + [int][bool]$Test
if ($Stage -eq 'Clippy' -and ($testTargetCount -or $TestFilter)) {
    throw '-Stage Clippy does not run tests; omit -Lib, -Bin, -Test and -TestFilter.'
}
if ($testTargetCount -gt 1) { throw 'Select only one of -Lib, -Bin or -Test.' }
if ($TestFilter -and !$testTargetCount) { throw '-TestFilter requires -Lib, -Bin or -Test.' }
if ($testTargetCount -and ($Workspace -or @($Package).Count -ne 1)) {
    throw 'Targeted tests require exactly one -Package; check direct consumers separately.'
}
if ($testTargetCount -and (@($Package[0], $Bin, $Test) -match '[*?\[\]]').Count) {
    throw 'Targeted tests require literal package and target names; wildcards can select multiple targets.'
}
foreach ($testSelection in @($Bin, $Test, $TestFilter)) {
    if ($testSelection -and ($testSelection.StartsWith('-') -or [string]::IsNullOrWhiteSpace($testSelection))) {
        throw 'Test names and filters must be nonblank names, not options.'
    }
}
[string[]]$testTargetArguments = if ($Lib) { @('--lib') } elseif ($Bin) { @('--bin', $Bin) } elseif ($Test) { @('--test', $Test) } else { @('--all-targets') }
$effectiveTarget = if ($Target) { $Target } elseif ($env:CARGO_BUILD_TARGET) { $env:CARGO_BUILD_TARGET } else { 'x86_64-pc-windows-msvc' }
if ($Asio -and $effectiveTarget -notmatch '-windows-msvc$') {
    throw 'ASIO checks require a Windows MSVC target; choose a matching -Target.'
}
$selectionArguments = @('--workspace')
$featureArguments = @()
if ($Package) {
    $selectionArguments = @($Package | ForEach-Object { '-p'; $_ })
}
if ($Asio) {
    $asioPackages = if ($Package) { @($Package | Where-Object { $_ -in @('audiolib-desktop','audiolib-cli','audiolib-audio','audiolib-windows') }) } else { @('audiolib-desktop','audiolib-cli') }
    if (!$asioPackages.Count) { throw '-Asio requires selecting an ASIO-capable package; unrelated packages do not need a second feature configuration.' }
    $featureArguments = @('--features', (($asioPackages | ForEach-Object { "$_/asio" }) -join ','))
}
if ($NoDefaultFeatures) { $featureArguments += '--no-default-features' }
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
. (Join-Path $PSScriptRoot 'build-cache.ps1')
if ($CleanBuildArtifacts -and $PSVersionTable.PSVersion -lt [version]'7.3') { throw 'Use pwsh 7.3+ for post-check build cache maintenance.' }
$buildLease = $null
$runId = [guid]::NewGuid().ToString('N')
$logs = Join-Path (Join-Path $taskRoot '.rust-validation/local-checks') $runId
$targetArguments = if ($Target) { @('--target', $Target) } else { @() }
$savedAsioDirectory = [Environment]::GetEnvironmentVariable('CPAL_ASIO_DIR', 'Process')
$savedClangDirectory = [Environment]::GetEnvironmentVariable('LIBCLANG_PATH', 'Process')
$checkResults = [ordered]@{ format='not-requested'; clippy='not-requested'; tests='not-requested' }

function Invoke-LoggedCheck([string]$Name, [scriptblock]$Command) {
    $logPath = Join-Path $logs ($Name + '.log')
    Write-Host "Running $Name"
    try {
        & $Command *> $logPath
    } catch {
        $_ | Out-String | Add-Content -LiteralPath $logPath
        Get-Content -LiteralPath $logPath -Tail 60 | ForEach-Object { Write-Host $_ }
        throw "$Name failed. Log: $logPath. $($_.Exception.Message)"
    }
}

function Invoke-CargoCheck([string]$Name, [string[]]$Arguments) {
    Invoke-LoggedCheck $Name {
        $cargoCommand = Get-Command cargo -ErrorAction Stop
        $global:LASTEXITCODE = $null
        try {
            # Windows PowerShell treats redirected native stderr as errors.
            # Cargo progress goes to stderr; the process exit code decides success.
            $ErrorActionPreference = 'Continue'
            & $cargoCommand @Arguments
            $cargoExitCode = $global:LASTEXITCODE
        } finally {
            $ErrorActionPreference = 'Stop'
        }
        if ($null -eq $cargoExitCode) { throw 'cargo did not report an exit code.' }
        if ($cargoExitCode -ne 0) { throw "cargo exited with code $cargoExitCode." }
    }
}

Push-Location -LiteralPath $taskRoot
try {
    $buildLease = Enter-AudioLibBuildLease $taskRoot
    New-Item -ItemType Directory -Path $logs -Force | Out-Null
    if ($Stage -eq 'All') {
        Invoke-CargoCheck 'format' @('fmt', '--all', '--', '--check')
        $checkResults.format = 'passed'
    }
    if ($Asio) {
        Invoke-LoggedCheck 'prepare-asio' { . (Join-Path $PSScriptRoot 'prepare-asio.ps1') }
    }
    $configuration = if ($Asio) { 'asio' } else { 'default' }
    $checkArguments = $selectionArguments + @('--locked') + $targetArguments + $featureArguments
    if ($Stage -in @('All','Clippy')) {
        Invoke-CargoCheck "$configuration-clippy" (@('clippy') + $checkArguments + @('--all-targets', '--', '-D', 'warnings'))
        $checkResults.clippy = 'passed'
    }
    if ($Stage -in @('All','Tests')) {
        $testArguments = @('test') + $checkArguments + $testTargetArguments
        if ($TestFilter) { $testArguments += @('--', $TestFilter) }
        Invoke-CargoCheck "$configuration-tests" $testArguments
        if ($testTargetCount) {
            $testLog = Join-Path $logs "$configuration-tests.log"
            # One target uses one top-level libtest summary, after any child summaries.
            $testSummary = Select-String -LiteralPath $testLog -Pattern '^test result: ok\. (\d+) passed;' | Select-Object -Last 1
            if (!$testSummary -or [long]$testSummary.Matches[0].Groups[1].Value -eq 0) {
                throw "Targeted tests did not report any passed tests; verify the target/filter and ignored status. Log: $testLog"
            }
        }
        $checkResults.tests = 'passed'
    }
    $selection = if ($Package) { $Package -join ', ' } else { 'workspace' }
    $testScope = if ($Stage -eq 'Clippy') { 'not-requested' } elseif ($testTargetCount) { ($testTargetArguments -join ' ') + $(if ($TestFilter) { "; filter=$TestFilter" }) } else { 'all targets' }
    $scope = "$selection, $configuration configuration only; no-default-features=$([bool]$NoDefaultFeatures); test-scope=$testScope"
    $results = ($checkResults.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join '; '
    Write-Host "Selected local checks passed ($scope). $results. Logs: $logs"
} finally {
    if ($buildLease) { $buildLease.Dispose() }
    [Environment]::SetEnvironmentVariable('CPAL_ASIO_DIR', $savedAsioDirectory, 'Process')
    [Environment]::SetEnvironmentVariable('LIBCLANG_PATH', $savedClangDirectory, 'Process')
    Pop-Location
}
if ($CleanBuildArtifacts) {
    & (Join-Path $PSScriptRoot 'clean-build-artifacts.ps1') -Action Run -Mode Debug -MaxGiB $BuildCacheLimitGiB
    if ($LASTEXITCODE -ne 0) { throw 'Checks passed; build cache maintenance did not complete. Inspect its status; no process was stopped.' }
}
