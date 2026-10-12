# tools/package-portable.ps1
[CmdletBinding()]
param([switch]$SkipBuild, [switch]$WithoutAsio)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
if (!$SkipBuild) { & (Join-Path $PSScriptRoot 'build-release.ps1') -WithoutAsio:$WithoutAsio }
$latestRoot = [IO.Path]::GetFullPath((Join-Path $taskRoot 'dist/latest'))
$portableRoot = [IO.Path]::GetFullPath((Join-Path $latestRoot 'portable'))
if (!$portableRoot.StartsWith($taskRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Portable path escaped the intended workspace.'
}
$binaryRoot = Join-Path $taskRoot 'target/x86_64-pc-windows-msvc/release'
foreach ($binary in @('AudioLibExplorer.exe', 'AudioLibCli.exe')) {
    if (!(Test-Path -LiteralPath (Join-Path $binaryRoot $binary))) { throw "Missing release executable: $binary" }
}
# Only replace this task's fixed output after the release build has succeeded.
New-Item -ItemType Directory -Force -Path $portableRoot | Out-Null
foreach ($binary in @('AudioLibExplorer.exe', 'AudioLibCli.exe')) {
    Copy-Item -LiteralPath (Join-Path $binaryRoot $binary) -Destination (Join-Path $portableRoot $binary) -Force
}
foreach ($file in @('README.md', 'PORTABLE_README.md', 'THIRD_PARTY.md')) {
    Copy-Item -LiteralPath (Join-Path $taskRoot $file) -Destination $portableRoot -Force
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Start-AudioLibExplorer.cmd') -Destination $portableRoot -Force
New-Item -ItemType Directory -Force -Path (Join-Path $portableRoot 'doc') | Out-Null
Get-ChildItem -LiteralPath (Join-Path $taskRoot 'doc') -File -Filter 'Rust_*' |
    ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $portableRoot 'doc') -Force }
$documentAssets = Join-Path $taskRoot 'doc/assets'
if (Test-Path -LiteralPath $documentAssets -PathType Container) {
    Copy-Item -LiteralPath $documentAssets -Destination (Join-Path $portableRoot 'doc') -Recurse -Force
}
Copy-Item -LiteralPath (Join-Path $taskRoot 'resources') -Destination $portableRoot -Recurse -Force
New-Item -ItemType Directory -Force -Path (Join-Path $portableRoot 'config') | Out-Null
Copy-Item -LiteralPath (Join-Path $taskRoot 'config/rust_settings.example.json') -Destination (Join-Path $portableRoot 'config') -Force
$licenseRoot = Join-Path $portableRoot 'licenses'
New-Item -ItemType Directory -Force -Path $licenseRoot | Out-Null
Push-Location -LiteralPath $taskRoot
try {
    $metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata generation failed.' }
    $manifest = @()
    foreach ($package in $metadata.packages) {
        $manifest += [PSCustomObject]@{ name=$package.name; version=$package.version; license=$package.license; repository=$package.repository }
        $sourceRoot = Split-Path -Parent $package.manifest_path
        if ($package.name -eq 'soundtouch-ffi') {
            Copy-Item -LiteralPath (Join-Path $sourceRoot 'soundtouch-2_3_2/COPYING.TXT') -Destination (Join-Path $licenseRoot 'SoundTouch-2.3.2-LGPL.txt') -Force
        }
        $notices = Get-ChildItem -LiteralPath $sourceRoot -File | Where-Object { $_.Name -match '^(LICENSE|COPYING|NOTICE)' }
        if ($notices) {
            $destination = Join-Path $licenseRoot ($package.name + '-' + $package.version)
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
            $notices | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $destination -Force }
        }
        $noticeDirectories = Get-ChildItem -LiteralPath $sourceRoot -Directory | Where-Object { $_.Name -match '^(LICENSES?|COPYING|NOTICE)$' }
        foreach ($noticeDirectory in $noticeDirectories) {
            $destination = Join-Path $licenseRoot ($package.name + '-' + $package.version)
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
            Copy-Item -LiteralPath $noticeDirectory.FullName -Destination $destination -Recurse -Force
        }
        if ($package.license_file -and (Test-Path -LiteralPath $package.license_file -PathType Leaf)) {
            $destination = Join-Path $licenseRoot ($package.name + '-' + $package.version)
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
            Copy-Item -LiteralPath $package.license_file -Destination $destination -Force
        }
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $licenseRoot 'dependencies.json') -Encoding UTF8
} finally { Pop-Location }
if (!$WithoutAsio) {
    Copy-Item -LiteralPath (Join-Path $taskRoot 'tools/.build-tools/asio-2.3.4/ASIOSDK/LICENSE.txt') -Destination (Join-Path $licenseRoot 'Steinberg-ASIO-SDK-2.3.4.txt') -Force
}
$archivePath = Join-Path $latestRoot 'AudioLibExplorer-latest-win-x64-portable.zip'
# Cargo archives may give notices pre-1980 timestamps, which ZIP cannot represent.
# Adjust only the copied package files; registry and project sources stay untouched.
$zipEpoch = [datetime]::new(1980, 1, 1)
Get-ChildItem -LiteralPath $portableRoot -Recurse -File |
    Where-Object { $_.LastWriteTime -lt $zipEpoch } |
    ForEach-Object { $_.LastWriteTime = $zipEpoch }
Compress-Archive -Path (Join-Path $portableRoot '*') -DestinationPath $archivePath -Force
$hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
Set-Content -LiteralPath ($archivePath + '.sha256') -Value ($hash + '  ' + [IO.Path]::GetFileName($archivePath)) -Encoding ASCII
Write-Host "Portable package ready: $archivePath"

