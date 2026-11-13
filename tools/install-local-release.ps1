# tools/install-local-release.ps1
# Install an already accepted portable payload into the fixed local_release/.
# Package files only; existing databases/preferences and unrelated files survive.
#Requires -Version 7.3
[CmdletBinding()]
param(
    [string]$Source = 'dist/latest/portable',
    [string]$Destination = 'local_release',
    [string]$VersionManifest = 'Cargo.toml'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (!$IsWindows) { throw 'Local Windows installation requires Windows.' }
. (Join-Path $PSScriptRoot 'build-cache.ps1')
. (Join-Path $PSScriptRoot 'native-desktop-window.ps1')
if (!('AudioLibLocalInstallNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class AudioLibLocalInstallNative {
    [DllImport("user32.dll", EntryPoint = "PostMessageW", SetLastError = true)]
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
}
'@
}
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$taskSource = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Source, $taskRoot))
$taskDestination = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Destination, $taskRoot))
$taskVersionManifest = [IO.Path]::GetFullPath($VersionManifest, $taskRoot)
if ([IO.Path]::GetFileName($taskDestination) -ine 'local_release') { throw 'The fixed installation destination must be named local_release.' }
$taskInstallationRoot = [IO.Path]::GetDirectoryName($taskDestination)
$taskCanonicalSource = Get-AudioLibCanonicalPath $taskSource
$taskCanonicalDestination = Get-AudioLibCanonicalPath $taskDestination
$taskCanonicalManifest = Get-AudioLibCanonicalPath $taskVersionManifest
if ($taskCanonicalSource -ieq $taskCanonicalDestination -or $taskCanonicalSource.StartsWith($taskCanonicalDestination+'\',[StringComparison]::OrdinalIgnoreCase) -or $taskCanonicalDestination.StartsWith($taskCanonicalSource+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Source must not overlap the fixed installation.' }
if ($taskCanonicalManifest -ieq $taskCanonicalDestination -or $taskCanonicalManifest.StartsWith($taskCanonicalDestination+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'The version manifest must not be inside the installation.' }
$taskEvidence = Join-Path $taskInstallationRoot ('.rust-validation/local-install/' + [guid]::NewGuid().ToString('N'))
$taskDirectories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
function Assert-Directory([string]$Path) {
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor -and $taskDirectories.Add($cursor)) {
        $item = Get-Item -LiteralPath $cursor -Force -ErrorAction SilentlyContinue
        if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Linked installation paths are not supported.' }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}
Assert-Directory $taskSource
Assert-Directory $taskDestination
Assert-Directory $taskEvidence
Assert-Directory $taskVersionManifest
$taskSourceEntry = Get-Item -LiteralPath $taskSource -Force -ErrorAction Stop
if (!$taskSourceEntry.PSIsContainer) { throw 'The accepted portable source must be a directory.' }
$taskDestinationEntry = Get-Item -LiteralPath $taskDestination -Force -ErrorAction SilentlyContinue
if ($taskDestinationEntry -and !$taskDestinationEntry.PSIsContainer) { throw 'The fixed installation destination must be a directory.' }
function Read-WorkspaceManifest([string]$Path) {
    Assert-Directory $Path
    $entry = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($entry.PSIsContainer) { throw 'The version manifest must be a regular Cargo workspace manifest.' }
    $bytes = [IO.File]::ReadAllBytes($Path)
    $text = [Text.Encoding]::UTF8.GetString($bytes).TrimStart([char]0xfeff)
    $sections = [regex]::Matches($text, '(?ms)^[ \t]*\[workspace\.package\][ \t]*(?:#[^\r\n]*)?\r?\n(?<body>.*?)(?=^[ \t]*\[|\z)')
    if ($sections.Count -ne 1) { throw 'The version manifest must contain one workspace.package table.' }
    $assignments = [regex]::Matches($sections[0].Groups['body'].Value, '(?m)^[ \t]*(?:version|"version"|''version'')[ \t]*=')
    if ($assignments.Count -ne 1) { throw 'The version manifest must contain one workspace.package version assignment.' }
    $versions = [regex]::Matches($sections[0].Groups['body'].Value, '(?m)^[ \t]*version[ \t]*=[ \t]*"(?<version>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))"[ \t]*(?:#[^\r\n]*)?\r?$')
    if ($versions.Count -ne 1) { throw 'The version manifest must contain one workspace.package version.' }
    [pscustomobject]@{version=$versions[0].Groups['version'].Value;text=$text;sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()}
}
# The destination remains a repository's fixed installation, even when its
# checkout is older/dirty. Only the explicit source manifest selects the version.
$taskInstallationManifest = Join-Path $taskInstallationRoot 'Cargo.toml'
$taskInstallationMetadata = Read-WorkspaceManifest $taskInstallationManifest
$taskWorkspace = [regex]::Match($taskInstallationMetadata.text, '(?ms)^[ \t]*\[workspace\][ \t]*(?:#[^\r\n]*)?\r?\n(?<body>.*?)(?=^[ \t]*\[|\z)')
if (!$taskWorkspace.Success -or $taskWorkspace.Groups['body'].Value -notmatch '"app/audiolib-desktop"') { throw 'The fixed installation parent must be an AudioLib workspace.' }
. (Join-Path $PSScriptRoot 'assert-portable-layout.ps1')
$taskFiles = @(Get-AudioLibPortableFiles $taskSource)
$taskManifest = Read-WorkspaceManifest $taskVersionManifest
$taskExpectedVersion = $taskManifest.version
$taskPackageVersion = ([IO.File]::ReadAllText((Join-Path $taskSource 'update-version.txt'))).Trim() -replace '-(?:rc|beta)\.\d+$',''
if ($taskPackageVersion -cne $taskExpectedVersion) { throw 'Accepted package version must match the selected workspace manifest before installation.' }
$taskLock = $null
$taskSavedData = $env:AUDIOLIB_DATA_DIR
$taskStreams = @()
try {
New-Item -ItemType Directory -Path (Join-Path $taskInstallationRoot '.rust-validation') -Force | Out-Null
$taskLockPath = Join-Path $taskInstallationRoot '.rust-validation/local-release-install.lock'
Assert-Directory $taskLockPath
$taskLock = [IO.File]::Open($taskLockPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
# Validate every destination before closing an application or copying anything.
$taskCopyPlan = @($taskFiles | ForEach-Object {
    $taskRelative = [IO.Path]::GetRelativePath($taskSource,$_.FullName)
    $taskTarget = [IO.Path]::GetFullPath((Join-Path $taskDestination $taskRelative))
    if (!$taskTarget.StartsWith($taskDestination+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Installation destination escaped local_release.' }
    Assert-Directory ([IO.Path]::GetDirectoryName($taskTarget))
    $taskExisting = Get-Item -LiteralPath $taskTarget -Force -ErrorAction SilentlyContinue
    if ($taskExisting -and ($taskExisting.PSIsContainer -or ($taskExisting.Attributes -band [IO.FileAttributes]::ReparsePoint))) { throw 'Invalid package-owned destination.' }
    [pscustomobject]@{source=$_.FullName;relative=$taskRelative;target=$taskTarget;expected=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}
})
foreach ($taskProcess in @(Get-Process -Name AudioLibExplorer,AudioLibCli -ErrorAction SilentlyContinue)) {
    $taskProcessPath = if ($taskProcess.Path) { Get-AudioLibCanonicalPath $taskProcess.Path } else { '' }
    if ($taskProcessPath -and $taskProcessPath.StartsWith($taskCanonicalDestination + '\', [StringComparison]::OrdinalIgnoreCase)) {
        $taskHandle = [AudioLibDesktopWindowSelector]::FindApplicationWindow($taskProcess.Id)
        if ($taskHandle -eq [IntPtr]::Zero) {
            $taskProcess.Refresh()
            if ($taskProcess.HasExited) { continue }
            throw 'Installed process has no visible application window; leave it running and close it gracefully before retrying.'
        }
        [AudioLibDesktopWindowSelector]::AssertApplicationWindow($taskHandle,$taskProcess.Id)
        if (![AudioLibLocalInstallNative]::PostMessage($taskHandle,0x0010,[IntPtr]::Zero,[IntPtr]::Zero) -or !$taskProcess.WaitForExit(30000)) { throw 'Installed application is still running/draining writes; close it gracefully before retrying.' }
    }
}
New-Item -ItemType Directory -Path $taskEvidence -Force | Out-Null
$taskRecords = [Collections.Generic.List[object]]::new()
$taskCopied = 0
foreach ($taskFile in $taskCopyPlan) {
    $taskRelative = $taskFile.relative
    $taskTarget = $taskFile.target
    $taskParent = [IO.Path]::GetDirectoryName($taskTarget)
    Assert-Directory $taskParent
    $taskExisting = Get-Item -LiteralPath $taskTarget -Force -ErrorAction SilentlyContinue
    if ($taskExisting -and ($taskExisting.PSIsContainer -or ($taskExisting.Attributes -band [IO.FileAttributes]::ReparsePoint))) { throw 'Invalid package-owned destination.' }
    $taskExpected = $taskFile.expected
    $taskActual = if ($taskExisting) { (Get-FileHash -LiteralPath $taskTarget -Algorithm SHA256).Hash } else { '' }
    if ($taskActual -cne $taskExpected) {
        New-Item -ItemType Directory -Path $taskParent -Force | Out-Null
        Copy-Item -LiteralPath $taskFile.source -Destination $taskTarget -Force
        $taskActual = (Get-FileHash -LiteralPath $taskTarget -Algorithm SHA256).Hash
        $taskCopied++
    }
    if ($taskActual -cne $taskExpected) { throw "Installed file identity failed: $taskRelative" }
    $taskRecords.Add(@{file=$taskRelative;sha256=$taskActual.ToLowerInvariant()})
}
$taskRecords | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $taskEvidence 'files.json') -Encoding utf8
$taskTemporary = Join-Path ([IO.Path]::GetTempPath()) ('AudioLibLocalInstall-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $taskTemporary | Out-Null
    $env:AUDIOLIB_DATA_DIR = $taskTemporary
    @{active_database=(Join-Path $taskTemporary 'library.db');language='cn';playback=@{volume=0.0};allowed_scan_roots=@()} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $taskTemporary 'settings.json') -Encoding utf8
    $taskVersion = & (Join-Path $taskDestination 'AudioLibCli.exe') --version
    if ($LASTEXITCODE -ne 0 -or $taskVersion.Trim() -cne ('AudioLibCli ' + $taskExpectedVersion)) { throw 'Installed CLI version mismatch.' }
    $taskStart = [Diagnostics.ProcessStartInfo]::new()
    $taskStart.FileName = Join-Path $taskDestination 'AudioLibExplorer.exe'
    $taskStart.UseShellExecute=$false; $taskStart.CreateNoWindow=$true; $taskStart.RedirectStandardOutput=$true; $taskStart.RedirectStandardError=$true
    foreach ($taskArgument in @('--smoke-ui','--smoke-language','cn','--screenshot',(Join-Path $taskEvidence 'installed.png'))) { $taskStart.ArgumentList.Add($taskArgument) }
    $taskStreams = @([IO.File]::Create((Join-Path $taskEvidence 'startup.stdout.log')), [IO.File]::Create((Join-Path $taskEvidence 'startup.stderr.log')))
    $taskDesktop = [Diagnostics.Process]::Start($taskStart)
    $taskCopies = @($taskDesktop.StandardOutput.BaseStream.CopyToAsync($taskStreams[0]), $taskDesktop.StandardError.BaseStream.CopyToAsync($taskStreams[1]))
    if (!$taskDesktop.WaitForExit(45000)) { throw 'Owned installed startup did not finish; no process was forcibly stopped.' }
    foreach ($taskCopy in $taskCopies) { $null = $taskCopy.GetAwaiter().GetResult() }
    if ($taskDesktop.ExitCode -ne 0 -or !(Test-Path -LiteralPath (Join-Path $taskEvidence 'installed.png'))) { throw 'Installed desktop startup failed.' }
    $taskDesktop.Dispose()
    $taskResult = @{success=$true;version=$taskVersion.Trim();source=$taskSource;installation=$taskDestination;installation_root=$taskInstallationRoot;version_manifest=$taskVersionManifest;version_manifest_sha256=$taskManifest.sha256;verified_files=$taskRecords.Count;copied_files=$taskCopied;startup='passed';evidence=$taskEvidence}
    $taskResult | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $taskEvidence 'report.json') -Encoding utf8
    $taskResult | ConvertTo-Json -Compress | Write-Output
} finally {
    foreach ($taskStream in $taskStreams) { $taskStream.Dispose() }
    $env:AUDIOLIB_DATA_DIR = $taskSavedData
    if ($taskLock) { $taskLock.Dispose() }
}
