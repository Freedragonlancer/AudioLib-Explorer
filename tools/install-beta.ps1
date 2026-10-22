# tools/install-beta.ps1
# Install a validated portable package as the side-by-side beta build.
#   ./tools/install-beta.ps1 [-Source dist/latest/portable] [-Destination beta_release] [-WhatIf]
# Relative paths resolve against the repository root. The beta installation is
# launched through AudioLibExplorer-beta.cmd, which sets AUDIOLIB_DATA_DIR to a
# separate directory (default %LOCALAPPDATA%\AudioLibExplorer\beta) so a beta
# never shares settings, database, caches or logs with the stable v1 data root.
# Only package-owned files are overwritten; nothing is deleted and no running
# process is stopped.
[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$Source = 'dist/latest/portable',
    [string]$Destination = 'beta_release'
)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
function Resolve-RepoPath([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { [IO.Path]::GetFullPath($Path) } else { [IO.Path]::GetFullPath((Join-Path $taskRoot $Path)) }
}
$sourceRoot = (Resolve-RepoPath $Source).TrimEnd('\', '/')
$destinationRoot = (Resolve-RepoPath $Destination).TrimEnd('\', '/')
if (!(Test-Path -LiteralPath (Join-Path $sourceRoot 'AudioLibExplorer.exe') -PathType Leaf)) { throw "No portable package at $sourceRoot (run package-portable.ps1 first)." }
$leaf = [IO.Path]::GetFileName($destinationRoot)
if ($leaf -ieq 'local_release' -or $destinationRoot -ieq $taskRoot -or $destinationRoot -ieq $sourceRoot) { throw 'Beta destination must be a dedicated directory, not the stable installation, repository root or package source.' }
if ((Test-Path -LiteralPath $destinationRoot) -and ((Get-Item -LiteralPath $destinationRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Linked beta destination is not supported.' }
$running = @(Get-Process -Name AudioLibExplorer, AudioLibCli -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($destinationRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) })
if ($running.Count -gt 0) { throw 'The beta build is running; close it before installing (processes are never stopped by this script).' }

$files = @(Get-ChildItem -LiteralPath $sourceRoot -Recurse -File)
if ($PSCmdlet.ShouldProcess($destinationRoot, "Install $($files.Count) package files and AudioLibExplorer-beta.cmd")) {
    New-Item -ItemType Directory -Force -Path $destinationRoot | Out-Null
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($sourceRoot.Length + 1)
        $target = Join-Path $destinationRoot $relative
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
        Copy-Item -LiteralPath $file.FullName -Destination $target -Force
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'AudioLibExplorer-beta.cmd') -Destination $destinationRoot -Force
    foreach ($binary in @('AudioLibExplorer.exe', 'AudioLibCli.exe')) {
        $expected = (Get-FileHash -LiteralPath (Join-Path $sourceRoot $binary) -Algorithm SHA256).Hash
        $actual = (Get-FileHash -LiteralPath (Join-Path $destinationRoot $binary) -Algorithm SHA256).Hash
        if ($expected -ne $actual) { throw "Installed $binary does not match the package." }
    }
    Write-Host "Beta installed to $destinationRoot ($($files.Count) files). Launch with AudioLibExplorer-beta.cmd."
}
