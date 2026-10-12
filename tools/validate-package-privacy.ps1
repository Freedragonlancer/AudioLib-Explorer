# tools/validate-package-privacy.ps1
# Release boundary check: explicit payload files, public report schema and local
# build-path removal. This is not a scan of historical Git objects or releases.
[CmdletBinding()]
param([string]$BinaryDirectory = 'dist/latest/portable', [string]$ReleaseReport)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$payloadRoot = [IO.Path]::GetFullPath($(if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRoot $BinaryDirectory }))
$allowedFiles = @('AudioLibExplorer.exe', 'AudioLibCli.exe', 'Start-AudioLibExplorer.cmd') +
    @(Get-Content -LiteralPath (Join-Path $PSScriptRoot 'portable-files.json') -Raw | ConvertFrom-Json)
$privatePrefixes = [Collections.Generic.List[string]]::new()
foreach ($prefix in @($taskRoot, $env:USERPROFILE, $env:CARGO_HOME, $env:RUSTUP_HOME, $env:CARGO_TARGET_DIR)) {
    if ($prefix) {
        $privatePrefixes.Add([IO.Path]::GetFullPath($prefix).TrimEnd('\', '/'))
        $privatePrefixes.Add([IO.Path]::GetFullPath($prefix).Replace('\', '/').TrimEnd('/'))
    }
}
$workspaceDrive = Get-PSDrive -Name ([IO.Path]::GetPathRoot($taskRoot).TrimEnd(':', '\')) -ErrorAction SilentlyContinue
if ($workspaceDrive.DisplayRoot) {
    $privatePrefixes.Add($workspaceDrive.DisplayRoot.TrimEnd('\') + '\' + $taskRoot.Substring(3))
}
function Assert-NoPrivatePath([string]$Text, [string]$Name) {
    foreach ($prefix in $privatePrefixes) {
        if ($Text.IndexOf($prefix, [StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $Text.IndexOf($prefix.Replace('\', '\\'), [StringComparison]::OrdinalIgnoreCase) -ge 0) { throw "Local build/user path found in public payload: $Name" }
    }
}
if (!(Test-Path -LiteralPath $payloadRoot -PathType Container)) { throw 'Portable payload is missing.' }
if ((Get-Item -LiteralPath $payloadRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint -or
    @(Get-ChildItem -LiteralPath $payloadRoot -Recurse -Force -Attributes ReparsePoint).Count -gt 0) { throw 'Linked payload is not supported.' }
foreach ($file in (Get-ChildItem -LiteralPath $payloadRoot -Recurse -Force -File)) {
    $relative = [IO.Path]::GetRelativePath($payloadRoot, $file.FullName).Replace('\', '/')
    if ($relative -notin $allowedFiles -and !$relative.StartsWith('licenses/', [StringComparison]::Ordinal)) { throw "File is outside the public payload allowlist: $relative" }
    if ($file.Extension -in @('.db', '.sqlite', '.sqlite3', '.log') -or $file.Name -in @('settings.json', '.env')) { throw "Runtime state found in payload: $relative" }
    $bytes = [IO.File]::ReadAllBytes($file.FullName)
    Assert-NoPrivatePath ([Text.Encoding]::UTF8.GetString($bytes)) $relative
    if ($file.Extension -eq '.exe') {
        Assert-NoPrivatePath ([Text.Encoding]::Unicode.GetString($bytes)) $relative
        if ($bytes.Length -gt 1) { Assert-NoPrivatePath ([Text.Encoding]::Unicode.GetString($bytes, 1, $bytes.Length - 1)) $relative }
    }
}
foreach ($file in $allowedFiles) {
    if (!(Test-Path -LiteralPath (Join-Path $payloadRoot $file) -PathType Leaf)) { throw "Required public payload file missing: $file" }
}
if ($ReleaseReport) {
    $reportText = Get-Content -LiteralPath $ReleaseReport -Raw -Encoding UTF8
    Assert-NoPrivatePath $reportText 'release report'
    $report = $reportText | ConvertFrom-Json
    $fields = @('success','tag','commit','version','checked_at','checks','binaries','portable')
    if (@($report.PSObject.Properties.Name | Where-Object { $_ -notin $fields }).Count -or
        @($fields | Where-Object { $_ -notin $report.PSObject.Properties.Name }).Count) { throw 'Public release report schema mismatch.' }
    $portableFields = @('success','mode','records','source_unchanged','python_runtime_required')
    if (@($report.portable.PSObject.Properties.Name | Where-Object { $_ -notin $portableFields }).Count -or
        @($portableFields | Where-Object { $_ -notin $report.portable.PSObject.Properties.Name }).Count) { throw 'Public portable report schema mismatch.' }
    if (!$report.success -or !$report.portable.success -or !$report.portable.source_unchanged -or
        $report.portable.mode -ne 'clean-checkout-synthetic' -or $report.portable.records -ne 1 -or
        $report.portable.python_runtime_required -or $report.commit -notmatch '^[0-9a-f]{40}$' -or
        $report.version -notmatch '^\d+\.\d+\.\d+$' -or $report.tag -notmatch ('^v' + [regex]::Escape($report.version) + '(?:-rc\.\d+)?$')) { throw 'Public release report has invalid build/validation identity.' }
    if (@($report.binaries.PSObject.Properties.Name | Sort-Object) -join ',' -ne 'AudioLibCli.exe,AudioLibExplorer.exe') { throw 'Public binary report schema mismatch.' }
    foreach ($binary in @('AudioLibExplorer.exe', 'AudioLibCli.exe')) {
        if ($report.binaries.$binary -cne (Get-FileHash -LiteralPath (Join-Path $payloadRoot $binary) -Algorithm SHA256).Hash) { throw "Release report binary identity mismatch: $binary" }
    }
}
Write-Host 'Portable publication boundary passed: allowlisted files, no local build/user paths, public report schema (when provided).'
