# tools/build-cache.ps1
# Shared build/cleanup coordination. Compatible with Windows PowerShell 5.1.
function Get-AudioLibCanonicalPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT -and $full -match '^[A-Za-z]:\\') {
        $drive = Get-PSDrive -Name $full.Substring(0,1) -ErrorAction SilentlyContinue
        if ($drive -and $drive.DisplayRoot) { $full = [IO.Path]::GetFullPath((Join-Path $drive.DisplayRoot $full.Substring(3))) }
    }
    if ($full.Length -gt [IO.Path]::GetPathRoot($full).Length) { $full = $full.TrimEnd([IO.Path]::DirectorySeparatorChar) }
    return $full
}
function Assert-AudioLibUnlinkedPath([string]$Path) {
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor) {
        $entry = Get-Item -LiteralPath $cursor -Force -ErrorAction SilentlyContinue
        if ($entry -and ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Linked build/cache path is not supported.' }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}
function Get-AudioLibBuildCommonDirectory([string]$Root) {
    $savedExitVariable = Get-Variable LASTEXITCODE -Scope Global -ErrorAction SilentlyContinue
    $savedExit = if ($savedExitVariable) { $savedExitVariable.Value } else { $null }
    $savedPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $common = & git -C $Root rev-parse --git-common-dir 2>$null
        if ($LASTEXITCODE -eq 0 -and $common) {
            if (![IO.Path]::IsPathRooted($common)) { $common = Join-Path $Root $common }
            return Get-AudioLibCanonicalPath $common
        }
        return Get-AudioLibCanonicalPath (Join-Path $Root '.rust-validation')
    } finally { $global:LASTEXITCODE = $savedExit; $ErrorActionPreference = $savedPreference }
}
function Enter-AudioLibBuildLease([string]$Root, [switch]$Exclusive) {
    $common = Get-AudioLibBuildCommonDirectory $Root
    Assert-AudioLibUnlinkedPath $common
    $null = [IO.Directory]::CreateDirectory($common)
    $path = Join-Path $common 'audiolib-build-cache.lock'
    Assert-AudioLibUnlinkedPath $path
    $share = if ($Exclusive) { [IO.FileShare]::None } else { [IO.FileShare]::ReadWrite }
    return [IO.File]::Open($path,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,$share)
}

# Use the same path hash on Windows PowerShell 5.1 and PowerShell 7. Keep these
# caches outside AudioLibExplorer's protected persistent application data.
function Get-AudioLibBuildPathHash([string]$Path) {
    $identity = Get-AudioLibCanonicalPath $Path
    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) { $identity = $identity.ToLowerInvariant() }
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($algorithm.ComputeHash([Text.Encoding]::UTF8.GetBytes($identity))).Replace('-', '') }
    finally { $algorithm.Dispose() }
}

# Resolve only: Plan, packaging and diagnostic commands must not create caches.
function Get-AudioLibTargetDirectory([string]$Root) {
    $workspaceRoot = Get-AudioLibCanonicalPath $Root
    if ($env:CARGO_TARGET_DIR) {
        $selected = $env:CARGO_TARGET_DIR
        if (![IO.Path]::IsPathRooted($selected)) { $selected = Join-Path $workspaceRoot $selected }
        $selected = Get-AudioLibCanonicalPath $selected
        Assert-AudioLibUnlinkedPath $selected
        return $selected
    }
    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) {
        if (!$env:LOCALAPPDATA) { throw 'LOCALAPPDATA is required for the local Windows build directory.' }
        $localStorage = Get-AudioLibCanonicalPath $env:LOCALAPPDATA
        $volumeRoot = [IO.Path]::GetPathRoot($localStorage)
        if ($volumeRoot -notmatch '^[A-Za-z]:\\$' -or ([IO.DriveInfo]::new($volumeRoot)).DriveType -ne [IO.DriveType]::Fixed) {
            throw 'Automatic Windows build storage requires LOCALAPPDATA on a fixed local volume; choose an explicit CARGO_TARGET_DIR for another build location.'
        }
        $checkoutId = (Get-AudioLibBuildPathHash $workspaceRoot).Substring(0,16).ToLowerInvariant()
        $selected = Get-AudioLibCanonicalPath (Join-Path (Join-Path (Join-Path $localStorage 'AudioLibExplorerBuild') $checkoutId) 'target')
        Assert-AudioLibUnlinkedPath $selected
        return $selected
    }
    $selected = Get-AudioLibCanonicalPath (Join-Path $workspaceRoot 'target')
    Assert-AudioLibUnlinkedPath $selected
    return $selected
}

# Register only a freshly selected, empty local cache. Existing explicit caches
# retain the cleaner's separate Register/AdoptExisting ownership contract.
function Enter-AudioLibBuildEnvironment([string]$Root) {
    $context = [pscustomobject]@{TargetDirectory=(Get-AudioLibTargetDirectory $Root);SavedEnvironment=@{}}
    foreach ($name in @('CARGO_TARGET_DIR','TEMP','TMP')) {
        $context.SavedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    try {
        Assert-AudioLibUnlinkedPath $context.TargetDirectory
        $windowsHost = [Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT
        if ($windowsHost -and !$env:CARGO_TARGET_DIR) {
            # Shared build leases intentionally allow concurrent Cargo starts.
            # Serialize target registration separately so no starter can read a
            # partial owner marker or mistake another starter's files for an
            # unregistered cache. Leave the lock file in place to avoid replacing
            # a pathname while another starter still holds its original handle.
            $initializationParent = [IO.Path]::GetDirectoryName($context.TargetDirectory)
            Assert-AudioLibUnlinkedPath $initializationParent
            $null = [IO.Directory]::CreateDirectory($initializationParent)
            $initializationPath = Join-Path $initializationParent '.audiolib-target-init.lock'
            $initializationLock = $null
            $initializationTimer = [Diagnostics.Stopwatch]::StartNew()
            try {
                while (!$initializationLock) {
                    Assert-AudioLibUnlinkedPath $initializationPath
                    try { $initializationLock = [IO.File]::Open($initializationPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None) }
                    catch [IO.IOException] {
                        if ($initializationTimer.Elapsed.TotalSeconds -ge 10) { throw 'Local build target initialization is busy; retry after the current starter completes.' }
                        Start-Sleep -Milliseconds 100
                    }
                }
                Assert-AudioLibUnlinkedPath $context.TargetDirectory
                $ownerId = Get-AudioLibBuildPathHash (Get-AudioLibBuildCommonDirectory $Root)
                $ownerPath = Join-Path $context.TargetDirectory '.audiolib-build-owner.json'
                if (Test-Path -LiteralPath $context.TargetDirectory) {
                    if (!(Test-Path -LiteralPath $context.TargetDirectory -PathType Container)) { throw 'Local build target must be a directory.' }
                    if (!(Test-Path -LiteralPath $ownerPath) -and @(Get-ChildItem -LiteralPath $context.TargetDirectory -Force).Count) {
                        throw 'Existing local build target is not registered; inspect it before explicit registration.'
                    }
                }
                $null = [IO.Directory]::CreateDirectory($context.TargetDirectory)
                if (!(Test-Path -LiteralPath $ownerPath)) {
                    $marker = @{schema_version=1;owner_id=$ownerId;host=[Environment]::MachineName} | ConvertTo-Json
                    $stream = $null
                    try {
                        $stream = [IO.File]::Open($ownerPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
                        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($marker)
                        $stream.Write($bytes,0,$bytes.Length)
                    } finally { if ($stream) { $stream.Dispose() } }
                }
                Assert-AudioLibUnlinkedPath $ownerPath
                $ownerEntry = Get-Item -LiteralPath $ownerPath -Force
                if ($ownerEntry.PSIsContainer -or $ownerEntry.Length -gt 16384) { throw 'Invalid local build cache ownership marker.' }
                $owner = Get-Content -LiteralPath $ownerPath -Raw | ConvertFrom-Json
                if ($owner.schema_version -ne 1 -or $owner.owner_id -cne $ownerId -or $owner.host -cne [Environment]::MachineName) {
                    throw 'Local build cache belongs to a different workspace or host.'
                }
            } finally { if ($initializationLock) { $initializationLock.Dispose() } }
        }
        [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $context.TargetDirectory, 'Process')
        if ($windowsHost) {
            $temporaryDirectory = Join-Path $context.TargetDirectory 'tmp'
            Assert-AudioLibUnlinkedPath $temporaryDirectory
            $null = [IO.Directory]::CreateDirectory($temporaryDirectory)
            foreach ($name in @('TEMP','TMP')) { [Environment]::SetEnvironmentVariable($name, $temporaryDirectory, 'Process') }
        }
        return $context
    } catch {
        Exit-AudioLibBuildEnvironment $context
        throw
    }
}

function Exit-AudioLibBuildEnvironment($Context) {
    if (!$Context) { return }
    foreach ($name in $Context.SavedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $Context.SavedEnvironment[$name], 'Process')
    }
}

# Call only after releasing the build lease and after all consumers accepted the
# outputs. An explicit target keeps cleanup correct after environment restoration.
function Invoke-AudioLibPostBuildCleanup([string]$Root,[string]$TargetDirectory,[double]$MaxGiB=0) {
    if ($PSVersionTable.PSVersion -lt [version]'7.3') { throw 'Use pwsh 7.3+ for automatic post-build cleanup.' }
    & (Join-Path $PSScriptRoot 'clean-build-artifacts.ps1') -Root $Root -TargetDirectory $TargetDirectory -Action Run -Mode Intermediate -MaxGiB $MaxGiB
    if ($LASTEXITCODE -ne 0) { throw 'Build/checks passed; build cache cleanup did not complete. Inspect its status; no process was stopped.' }
}
