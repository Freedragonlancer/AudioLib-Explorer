# tools/test-development-pipeline.ps1
# Verify check/build command routing and failure propagation with a fake Cargo.
# No Rust build, SDK download, application process or working library is used.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskArtifacts = Join-Path $taskRoot ('.rust-validation/development-pipeline-tests/' + [guid]::NewGuid().ToString('N'))
$taskTools = Join-Path $taskArtifacts 'tools'
New-Item -ItemType Directory -Path $taskTools -Force | Out-Null
foreach ($taskScript in @('check-local.ps1', 'build-release.ps1')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $taskScript) -Destination $taskTools
}
@'
$taskState.Prepared++
$env:CPAL_ASIO_DIR = 'synthetic-sdk'
$env:LIBCLANG_PATH = 'synthetic-clang'
if ($taskState.FailPreparation) { throw 'Synthetic SDK preparation failed.' }
'@ | Set-Content -LiteralPath (Join-Path $taskTools 'prepare-asio.ps1') -Encoding UTF8

$taskCalls = [Collections.Generic.List[object]]::new()
$taskState = @{ Prepared=0; FailPreparation=$false; FailAt=0 }
$taskEnvironmentKeys = @('CPAL_ASIO_DIR', 'LIBCLANG_PATH', 'CARGO_ENCODED_RUSTFLAGS', 'CXXFLAGS_x86_64_pc_windows_msvc', 'CXXFLAGS_x86_64-pc-windows-msvc', 'CARGO_BUILD_TARGET')
$taskOriginalEnvironment = @{}
foreach ($taskKey in $taskEnvironmentKeys) {
    $taskOriginalEnvironment[$taskKey] = [Environment]::GetEnvironmentVariable($taskKey, 'Process')
}
$taskOriginalExitCode = $global:LASTEXITCODE

# Functions shadow executables only in this test and scripts invoked beneath it.
function cargo {
    $taskCalls.Add([pscustomobject]@{
        Arguments=@($args)
        Prepared=$taskState.Prepared
        Flags=$env:CARGO_ENCODED_RUSTFLAGS
    })
    $global:LASTEXITCODE = if ($taskState.FailAt -eq $taskCalls.Count) { 17 } else { 0 }
}
if ((Get-Command cargo).CommandType -ne 'Function') { throw 'Synthetic Cargo is not active.' }

function Invoke-PipelineCase([string]$Script, [hashtable]$Parameters, [string]$Failure = '') {
    $taskCalls.Clear()
    $taskState.Prepared = 0
    $taskBeforeLocation = (Get-Location).Path
    $taskBeforeEnvironment = @{}
    foreach ($taskKey in $taskEnvironmentKeys) {
        $taskBeforeEnvironment[$taskKey] = [Environment]::GetEnvironmentVariable($taskKey, 'Process')
    }
    $taskCaught = $null
    $taskCaseLog = Join-Path $taskArtifacts ([guid]::NewGuid().ToString('N') + '.log')
    try { & (Join-Path $taskTools $Script) @Parameters *> $taskCaseLog } catch { $taskCaught = $_ }
    if (!$Failure -and $taskCaught) { throw "Unexpected $Script failure: $taskCaught. Log: $taskCaseLog" }
    if ($Failure -and (!$taskCaught -or ($taskCaught.ToString() + ' ' + $taskCaught.FullyQualifiedErrorId) -notmatch $Failure)) { throw "Expected $Script failure ($Failure), got: $taskCaught. Log: $taskCaseLog" }
    if ((Get-Location).Path -cne $taskBeforeLocation) { throw "$Script did not restore the working directory." }
    foreach ($taskKey in $taskEnvironmentKeys) {
        if ([string][Environment]::GetEnvironmentVariable($taskKey, 'Process') -cne [string]$taskBeforeEnvironment[$taskKey]) { throw "$Script did not restore $taskKey." }
    }
}

function Get-SelectedPackages($Call) {
    for ($taskIndex = 0; $taskIndex -lt $Call.Arguments.Count; $taskIndex++) {
        if ($Call.Arguments[$taskIndex] -eq '-p') { $Call.Arguments[$taskIndex + 1] }
    }
}
function Assert-FullDefaultChecks {
    if ($taskCalls.Count -lt 3 -or $taskCalls[0].Arguments[0] -ne 'fmt' -or
        $taskCalls[1].Arguments[0] -ne 'clippy' -or $taskCalls[2].Arguments[0] -ne 'test') { throw 'Default validation stages are missing or reordered.' }
    foreach ($taskCall in @($taskCalls[1], $taskCalls[2])) {
        if ($taskCall.Arguments -notcontains '--workspace' -or $taskCall.Arguments -notcontains '--all-targets' -or
            $taskCall.Arguments -notcontains '--locked' -or $taskCall.Arguments -contains '--features') { throw 'Default workspace coverage changed.' }
    }
    if (($taskCalls[1].Arguments -join ' ') -notmatch '-- -D warnings$') { throw 'Default Clippy must remain strict.' }
}

try {
    $env:CARGO_BUILD_TARGET = 'x86_64-pc-windows-msvc'
    Invoke-PipelineCase 'check-local.ps1' @{}
    Assert-FullDefaultChecks
    if ($taskCalls.Count -ne 3 -or $taskState.Prepared -ne 0) { throw 'Default checks unexpectedly add ASIO work.' }
    Write-Host 'PASS: default full workspace checks run once without SDK preparation.'

    Invoke-PipelineCase 'check-local.ps1' @{Asio=$true; Target='x86_64-pc-windows-msvc'}
    Assert-FullDefaultChecks
    if ($taskCalls.Count -ne 5 -or $taskState.Prepared -ne 1) { throw 'ASIO validation stages are missing or repeated.' }
    $taskAsioPackages = 'audiolib-audio,audiolib-cli,audiolib-desktop,audiolib-windows'
    foreach ($taskCall in @($taskCalls[3], $taskCalls[4])) {
        if ($taskCall.Arguments -contains '--workspace' -or
            (@(Get-SelectedPackages $taskCall | Sort-Object) -join ',') -cne $taskAsioPackages -or
            $taskCall.Prepared -ne 1 -or $taskCall.Arguments -notcontains 'audiolib-desktop/asio,audiolib-cli/asio' -or
            $taskCall.Arguments -notcontains '--all-targets' -or $taskCall.Arguments -notcontains '--locked') { throw 'ASIO coverage must include only feature-affected packages after verified preparation.' }
    }
    if ($taskCalls[3].Arguments[0] -ne 'clippy' -or ($taskCalls[3].Arguments -join ' ') -notmatch '-- -D warnings$' -or
        $taskCalls[4].Arguments[0] -ne 'test') { throw 'ASIO strict Clippy/tests were not preserved.' }
    foreach ($taskCall in @($taskCalls | Select-Object -Skip 1)) {
        $taskTargetIndex = [array]::IndexOf($taskCall.Arguments, '--target')
        if ($taskTargetIndex -lt 0 -or $taskCall.Arguments[$taskTargetIndex + 1] -cne 'x86_64-pc-windows-msvc') { throw 'Explicit target was not forwarded.' }
    }
    Write-Host 'PASS: ASIO adds checks for four packages and forwards the selected target.'

    Invoke-PipelineCase 'check-local.ps1' @{AsioOnly=$true}
    if ($taskCalls.Count -ne 3 -or $taskState.Prepared -ne 1 -or $taskCalls[0].Arguments[0] -ne 'fmt') { throw 'ASIO-only checks must run format and one prepared ASIO pass.' }
    foreach ($taskCall in @($taskCalls[1], $taskCalls[2])) {
        if ($taskCall.Arguments -contains '--workspace' -or
            (@(Get-SelectedPackages $taskCall | Sort-Object) -join ',') -cne $taskAsioPackages -or
            $taskCall.Arguments -notcontains 'audiolib-desktop/asio,audiolib-cli/asio' -or
            $taskCall.Prepared -ne 1) { throw 'ASIO-only mode must preserve all four packages without repeating the default pass.' }
    }
    if ($taskCalls[1].Arguments[0] -ne 'clippy' -or ($taskCalls[1].Arguments -join ' ') -notmatch '-- -D warnings$' -or
        $taskCalls[2].Arguments[0] -ne 'test') { throw 'ASIO-only strict Clippy/tests were not preserved.' }
    Invoke-PipelineCase 'check-local.ps1' @{Asio=$true; AsioOnly=$true} 'AmbiguousParameterSet'
    if ($taskCalls.Count -or $taskState.Prepared) { throw 'Conflicting modes started checks.' }
    Write-Host 'PASS: ASIO-only mode avoids the default pass and rejects conflicting modes.'

    Invoke-PipelineCase 'check-local.ps1' @{Target='aarch64-apple-darwin'}
    Assert-FullDefaultChecks
    if ($taskCalls.Count -ne 3 -or $taskCalls[2].Arguments -notcontains 'aarch64-apple-darwin') { throw 'Native non-Windows routing changed.' }
    Invoke-PipelineCase 'check-local.ps1' @{Asio=$true; Target='aarch64-apple-darwin'} 'Windows MSVC'
    if ($taskCalls.Count -or $taskState.Prepared) { throw 'Invalid ASIO target started work.' }
    Invoke-PipelineCase 'check-local.ps1' @{AsioOnly=$true; Target='aarch64-apple-darwin'} 'Windows MSVC'
    if ($taskCalls.Count -or $taskState.Prepared) { throw 'Invalid ASIO-only target started work.' }
    $env:CARGO_BUILD_TARGET = 'aarch64-apple-darwin'
    Invoke-PipelineCase 'check-local.ps1' @{AsioOnly=$true} 'Windows MSVC'
    if ($taskCalls.Count -or $taskState.Prepared) { throw 'An incompatible environment target started ASIO work.' }
    $env:CARGO_BUILD_TARGET = 'x86_64-pc-windows-msvc'
    Write-Host 'PASS: native target checks work and incompatible ASIO targets fail before starting.'

    foreach ($taskFailAt in 1..5) {
        $taskState.FailAt = $taskFailAt
        Invoke-PipelineCase 'check-local.ps1' @{Asio=$true} 'cargo exited with code 17'
        if ($taskCalls.Count -ne $taskFailAt) { throw 'Validation continued after a failed stage.' }
    }
    $taskState.FailAt = 0
    $taskState.FailPreparation = $true
    Invoke-PipelineCase 'check-local.ps1' @{Asio=$true} 'Synthetic SDK preparation failed'
    if ($taskCalls.Count -ne 3) { throw 'ASIO checks continued after SDK preparation failed.' }
    Invoke-PipelineCase 'check-local.ps1' @{AsioOnly=$true} 'Synthetic SDK preparation failed'
    if ($taskCalls.Count -ne 1) { throw 'ASIO-only checks continued after SDK preparation failed.' }
    Invoke-PipelineCase 'build-release.ps1' @{} 'Synthetic SDK preparation failed'
    if ($taskCalls.Count) { throw 'Build continued after SDK preparation failed.' }
    $taskState.FailPreparation = $false
    Write-Host 'PASS: every failed check or SDK preparation stops later stages and restores the environment.'

    $env:CARGO_BUILD_TARGET = 'aarch64-apple-darwin'
    foreach ($taskWithoutAsio in @($true, $false)) {
        Invoke-PipelineCase 'build-release.ps1' @{WithoutAsio=$taskWithoutAsio}
        $taskBuild = $taskCalls[0]
        if ($taskCalls.Count -ne 1 -or $taskBuild.Arguments[0] -ne 'build' -or
            (@(Get-SelectedPackages $taskBuild | Sort-Object) -join ',') -cne 'audiolib-cli,audiolib-desktop' -or
            $taskBuild.Arguments -contains '--workspace' -or $taskBuild.Arguments -notcontains '--bins' -or
            $taskBuild.Arguments -notcontains '--release' -or $taskBuild.Arguments -notcontains '--locked') { throw 'Client release must build only its two executable packages.' }
        $taskTargetIndex = [array]::IndexOf($taskBuild.Arguments, '--target')
        if ($taskTargetIndex -lt 0 -or $taskBuild.Arguments[$taskTargetIndex + 1] -cne 'x86_64-pc-windows-msvc') { throw 'Build must match the portable packager target regardless of caller overrides.' }
        if (($taskBuild.Arguments -contains '--features') -eq $taskWithoutAsio -or
            $taskState.Prepared -ne [int](!$taskWithoutAsio) -or
            (!$taskWithoutAsio -and $taskBuild.Arguments -notcontains 'audiolib-desktop/asio,audiolib-cli/asio')) { throw 'Build and SDK feature selection differ.' }
        if ($taskBuild.Flags -notmatch 'target-feature=\+crt-static' -or $taskBuild.Flags -notmatch '--remap-path-prefix=') { throw 'Release privacy/static CRT flags were lost.' }
    }
    $taskState.FailAt = 1
    foreach ($taskWithoutAsio in @($true, $false)) {
        Invoke-PipelineCase 'build-release.ps1' @{WithoutAsio=$taskWithoutAsio} 'Rust release build failed'
    }
    Write-Host 'PASS: client-only builds preserve ASIO selection, release flags and build failures.'
    Write-Host "Development pipeline routing passed. Synthetic artifacts: $taskArtifacts"
} finally {
    foreach ($taskKey in $taskEnvironmentKeys) {
        [Environment]::SetEnvironmentVariable($taskKey, $taskOriginalEnvironment[$taskKey], 'Process')
    }
    $global:LASTEXITCODE = $taskOriginalExitCode
}
