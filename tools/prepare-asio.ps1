# tools/prepare-asio.ps1
[CmdletBinding()]
param([string]$ClangDirectory = 'C:\Program Files\LLVM\bin')
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$sdkRoot = Join-Path $taskRoot 'tools/.build-tools/asio-2.3.4'
$sdkDirectory = Join-Path $sdkRoot 'ASIOSDK'
$archivePath = Join-Path $taskRoot 'tools/.build-tools/ASIO-SDK_2.3.4_2025-10-15.zip'
$expectedHash = 'D5EBF0C20DD2C5F43771FD0C1418F4B361BF52434EE670097CFA6B3A335E2ECA'
$sdkUrl = 'https://download.steinberg.net/sdk_downloads/ASIO-SDK_2.3.4_2025-10-15.zip'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $archivePath) | Out-Null
if (!(Test-Path -LiteralPath $archivePath)) {
    Invoke-WebRequest -Uri $sdkUrl -OutFile $archivePath
}
if ((Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -ne $expectedHash) {
    throw 'ASIO SDK checksum mismatch. The unverified archive will not be used.'
}
if (!(Test-Path -LiteralPath (Join-Path $sdkDirectory 'common/asio.h'))) {
    Expand-Archive -LiteralPath $archivePath -DestinationPath $sdkRoot -Force
}
if (!(Test-Path -LiteralPath (Join-Path $ClangDirectory 'libclang.dll'))) {
    throw "LLVM libclang.dll is missing at $ClangDirectory. Install LLVM or pass -ClangDirectory."
}
$env:CPAL_ASIO_DIR = $sdkDirectory
$env:LIBCLANG_PATH = $ClangDirectory
Write-Host "Using verified Steinberg ASIO SDK 2.3.4: $sdkDirectory"

