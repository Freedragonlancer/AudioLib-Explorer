# tools/check-local.ps1
<#
.SYNOPSIS
Run local Rust validation once with the selected build features.
.DESCRIPTION
Runs format, strict Clippy and workspace tests once, failing at the first error.
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
.PARAMETER Target
Override the repository Cargo target for Clippy/tests (for example,
aarch64-apple-darwin or x86_64-unknown-linux-gnu). Format is target-independent.
ASIO requires a Windows MSVC target; omitting Target retains repository defaults.
.EXAMPLE
./tools/check-local.ps1
.EXAMPLE
./tools/check-local.ps1 -Asio
.EXAMPLE
./tools/check-local.ps1 -Target aarch64-apple-darwin
#>
[CmdletBinding()]
param(
    [switch]$Asio,
    [ValidateNotNullOrEmpty()][string]$Target
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$effectiveTarget = if ($Target) { $Target } elseif ($env:CARGO_BUILD_TARGET) { $env:CARGO_BUILD_TARGET } else { 'x86_64-pc-windows-msvc' }
if ($Asio -and $effectiveTarget -notmatch '-windows-msvc$') {
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
    $featureArguments = @()
    if ($Asio) {
        Invoke-LoggedCheck 'prepare-asio' { . (Join-Path $PSScriptRoot 'prepare-asio.ps1') }
        $featureArguments = @('--features', 'audiolib-desktop/asio,audiolib-cli/asio')
    }
    $configuration = if ($Asio) { 'asio' } else { 'default' }
    $checkArguments = @('--workspace', '--all-targets', '--locked') + $targetArguments + $featureArguments
    Invoke-CargoCheck "$configuration-clippy" (@('clippy') + $checkArguments + @('--', '-D', 'warnings'))
    Invoke-CargoCheck "$configuration-tests" (@('test') + $checkArguments)
    $scope = "workspace, $configuration configuration only"
    Write-Host "Local checks passed ($scope). Logs: $logs"
} finally {
    [Environment]::SetEnvironmentVariable('CPAL_ASIO_DIR', $savedAsioDirectory, 'Process')
    [Environment]::SetEnvironmentVariable('LIBCLANG_PATH', $savedClangDirectory, 'Process')
    Pop-Location
}
