# tools/check-local.ps1
<#
.SYNOPSIS
Run the full local Rust validation matrix when the change warrants it.
.DESCRIPTION
Runs format, default-feature strict Clippy and workspace tests, failing at the
first error. Optional ASIO checks cover the four feature-affected packages after
verified SDK preparation; unrelated workspace tests are not repeated.
Logs stay in .rust-validation/local-checks/<run>/.
This does not build a release, install binaries or run subsystem validators.
Windows checks require the MSVC toolchain. If the default host is GNU, save the
current RUSTUP_TOOLCHAIN value, set $env:RUSTUP_TOOLCHAIN to
'1.97.1-x86_64-pc-windows-msvc' in this shell before invoking the script, and
restore the saved value afterward. Do not change the global Rustup toolchain.
.PARAMETER Asio
Also run strict Clippy and tests for audio, Windows, desktop and CLI with ASIO.
.PARAMETER AsioOnly
Run format and only the ASIO package checks. Reuse still-valid default results;
this mode alone does not complete the full release validation matrix.
.PARAMETER Target
Override the repository Cargo target for Clippy/tests (for example,
aarch64-apple-darwin or x86_64-unknown-linux-gnu). Format is target-independent.
ASIO requires a Windows MSVC target; omitting Target retains repository defaults.
.EXAMPLE
./tools/check-local.ps1
.EXAMPLE
./tools/check-local.ps1 -Asio
.EXAMPLE
./tools/check-local.ps1 -AsioOnly
.EXAMPLE
./tools/check-local.ps1 -Target aarch64-apple-darwin
#>
[CmdletBinding(DefaultParameterSetName = 'Default')]
param(
    [Parameter(ParameterSetName = 'Default')]
    [switch]$Asio,
    [Parameter(Mandatory, ParameterSetName = 'AsioOnly')]
    [switch]$AsioOnly,
    [ValidateNotNullOrEmpty()][string]$Target
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$withAsio = $Asio -or $AsioOnly
$effectiveTarget = if ($Target) { $Target } elseif ($env:CARGO_BUILD_TARGET) { $env:CARGO_BUILD_TARGET } else { 'x86_64-pc-windows-msvc' }
if ($withAsio -and $effectiveTarget -notmatch '-windows-msvc$') {
    throw 'ASIO checks require a Windows MSVC target; choose a matching -Target.'
}
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$runId = [guid]::NewGuid().ToString('N')
$logs = Join-Path (Join-Path $taskRoot '.rust-validation/local-checks') $runId
$targetArguments = if ($Target) { @('--target', $Target) } else { @() }
$savedAsioDirectory = [Environment]::GetEnvironmentVariable('CPAL_ASIO_DIR', 'Process')
$savedClangDirectory = [Environment]::GetEnvironmentVariable('LIBCLANG_PATH', 'Process')

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
    New-Item -ItemType Directory -Path $logs -Force | Out-Null
    Invoke-CargoCheck 'format' @('fmt', '--all', '--', '--check')
    if (!$AsioOnly) {
        Invoke-CargoCheck 'default-clippy' (@('clippy', '--workspace', '--all-targets', '--locked') + $targetArguments + @('--', '-D', 'warnings'))
        Invoke-CargoCheck 'default-tests' (@('test', '--workspace', '--all-targets', '--locked') + $targetArguments)
    }
    if ($withAsio) {
        Invoke-LoggedCheck 'prepare-asio' { . (Join-Path $PSScriptRoot 'prepare-asio.ps1') }
        # ASIO propagates desktop/CLI -> audio -> Windows. Keep both backend
        # configurations covered without repeating core/db/remote/IPC/update/server.
        $asioPackages = @('-p', 'audiolib-audio', '-p', 'audiolib-windows', '-p', 'audiolib-desktop', '-p', 'audiolib-cli')
        $asioArguments = @('--features', 'audiolib-desktop/asio,audiolib-cli/asio')
        Invoke-CargoCheck 'asio-clippy' (@('clippy', '--all-targets', '--locked') + $asioPackages + $targetArguments + $asioArguments + @('--', '-D', 'warnings'))
        Invoke-CargoCheck 'asio-tests' (@('test', '--all-targets', '--locked') + $asioPackages + $targetArguments + $asioArguments)
    }
    $scope = if ($AsioOnly) { 'ASIO packages only; default workspace checks not run' } elseif ($Asio) { 'default workspace and ASIO packages' } else { 'default workspace' }
    Write-Host "Local checks passed ($scope). Logs: $logs"
} finally {
    [Environment]::SetEnvironmentVariable('CPAL_ASIO_DIR', $savedAsioDirectory, 'Process')
    [Environment]::SetEnvironmentVariable('LIBCLANG_PATH', $savedClangDirectory, 'Process')
    Pop-Location
}
