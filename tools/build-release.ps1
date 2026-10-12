# tools/build-release.ps1
[CmdletBinding()]
param([switch]$WithoutAsio)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $taskRoot
try {
    if (!$WithoutAsio) {
        . (Join-Path $PSScriptRoot 'prepare-asio.ps1')
        cargo build --workspace --release --locked --features 'audiolib-desktop/asio,audiolib-cli/asio'
    } else {
        cargo build --workspace --release --locked
    }
    if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed.' }
} finally { Pop-Location }

