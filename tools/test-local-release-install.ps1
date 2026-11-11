# tools/test-local-release-install.ps1
# Test the installer with owned fake CLI/desktop executables and optional already
# accepted runtime payloads. Never install into this checkout's real local_release.
#Requires -Version 7.3
param([string]$RuntimePayload)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$false
if (!$IsWindows) { throw 'Windows installation regression requires Windows.' }
$taskRoot=Join-Path ([IO.Path]::GetTempPath()) ('AudioLibInstallRegression-'+[guid]::NewGuid().ToString('N'))
$taskTools=Join-Path $taskRoot 'tools'
$taskSource=Join-Path $taskRoot 'payload'
$taskInstalled=Join-Path $taskRoot 'local_release'
New-Item -ItemType Directory -Path $taskTools,$taskSource,$taskInstalled | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install-local-release.ps1') -Destination $taskTools
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'build-cache.ps1') -Destination $taskTools
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'native-desktop-window.ps1') -Destination $taskTools
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'assert-portable-layout.ps1') -Destination $taskTools
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'portable-files.json') -Destination $taskTools
Copy-Item -LiteralPath (Join-Path (Split-Path -Parent $PSScriptRoot) 'Cargo.toml') -Destination $taskRoot
$taskFixtureVersion=[regex]::Match([IO.File]::ReadAllText((Join-Path $taskRoot 'Cargo.toml')),'(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"').Groups[1].Value
$taskOriginalManifest=[IO.File]::ReadAllText((Join-Path $taskRoot 'Cargo.toml'))
$taskSentinel=Join-Path $taskInstalled 'user-file.keep'
[IO.File]::WriteAllText($taskSentinel,'unowned file must survive')
$taskPwsh=Join-Path $PSHOME 'pwsh.exe'
$taskInstaller=Join-Path $taskTools 'install-local-release.ps1'
& $taskPwsh -NoProfile -File $taskInstaller -Source $taskSource *> (Join-Path $taskRoot 'incomplete.log')
if ($LASTEXITCODE -eq 0 -or [IO.File]::ReadAllText($taskSentinel) -cne 'unowned file must survive') { throw 'Incomplete source was accepted or installation data was touched.' }
Write-Host 'PASS: incomplete package rejected before installation; unrelated file preserved.'
$taskRequired=@('AudioLibExplorer.exe','AudioLibCli.exe','Start-AudioLibExplorer.cmd','update-channel.txt','update-version.txt') + @(Get-Content -LiteralPath (Join-Path $taskTools 'portable-files.json') -Raw | ConvertFrom-Json)
foreach($taskName in $taskRequired){
    $taskPath=Join-Path $taskSource $taskName
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($taskPath)) -Force | Out-Null
    [IO.File]::WriteAllText($taskPath,'fixture')
}
[IO.File]::WriteAllText((Join-Path $taskSource 'update-channel.txt'),'stable')
[IO.File]::WriteAllText((Join-Path $taskSource 'update-version.txt'),$taskFixtureVersion)
$taskOldExe=Join-Path $taskInstalled 'AudioLibExplorer.exe'
[IO.File]::WriteAllText($taskOldExe,'original installed executable')
function Assert-Rejected([string]$Name,[string]$Candidate=$taskSource,[string]$Destination,[string]$VersionManifest,[string]$Installer=$taskInstaller,[string]$ExpectedMessage){
    $arguments=@('-NoProfile','-File',$Installer,'-Source',$Candidate)
    $installed=if($Destination){$Destination}else{$taskInstalled}
    if($Destination){$arguments+=@('-Destination',$Destination)}
    if($VersionManifest){$arguments+=@('-VersionManifest',$VersionManifest)}
    $protected=if(Test-Path -LiteralPath $installed -PathType Leaf){$installed}else{Join-Path $installed 'AudioLibExplorer.exe'}
    $before=(Get-FileHash -LiteralPath $protected).Hash
    & $taskPwsh @arguments *> (Join-Path $taskRoot ($Name+'.log'))
    if($LASTEXITCODE -eq 0 -or (Get-FileHash -LiteralPath $protected).Hash -cne $before){throw "Preflight $Name accepted or copied files before rejection."}
    if($ExpectedMessage-and![IO.File]::ReadAllText((Join-Path $taskRoot ($Name+'.log'))).Contains($ExpectedMessage)){throw "Preflight $Name did not report its intended rejection."}
}
[IO.File]::WriteAllText((Join-Path $taskSource 'settings.json'),'must not replace user state')
Assert-Rejected 'unexpected-state'
Remove-Item -LiteralPath (Join-Path $taskSource 'settings.json')
$taskLink=Join-Path $taskInstalled 'config'
$taskOutside=Join-Path $taskRoot 'outside-install'
New-Item -ItemType Directory -Path $taskOutside | Out-Null
[IO.File]::WriteAllText((Join-Path $taskOutside 'keep.txt'),'outside untouched')
New-Item -ItemType Junction -Path $taskLink -Target $taskOutside | Out-Null
Assert-Rejected 'late-linked-directory'
if([IO.File]::ReadAllText((Join-Path $taskOutside 'keep.txt')) -cne 'outside untouched'){throw 'Linked destination was modified.'}
if(![IO.Path]::GetFullPath($taskLink).StartsWith($taskRoot+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Fixture link cleanup escaped owned root.'}
[IO.Directory]::Delete($taskLink)
New-Item -ItemType Directory -Path (Join-Path $taskRoot '.rust-validation') -Force | Out-Null
$taskInstallLock=[IO.File]::Open((Join-Path $taskRoot '.rust-validation/local-release-install.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
try{Assert-Rejected 'concurrent-install'}finally{$taskInstallLock.Dispose()}
Assert-Rejected 'overlap' (Join-Path $taskInstalled 'payload')
$taskSourceFile=Join-Path $taskRoot 'not-a-payload-directory.zip'
[IO.File]::WriteAllText($taskSourceFile,'a file is not a portable directory')
Assert-Rejected 'source-is-a-file' $taskSourceFile
[IO.File]::WriteAllText((Join-Path $taskSource 'update-version.txt'),$(if($taskFixtureVersion -eq '0.0.0'){'0.0.1'}else{'0.0.0'}))
Assert-Rejected 'wrong-version'
[IO.File]::WriteAllText((Join-Path $taskSource 'update-version.txt'),$taskFixtureVersion)
Write-Host 'PASS: allowlist, version, every destination, overlap and installation lock checked before copying.'

# Separate script, final version-manifest and older installation checkouts.
# These are owned filesystem fixtures, not application working directories.
$taskPreviousRoot=Join-Path $taskRoot 'previous-checkout'
$taskOtherCaller=Join-Path $taskRoot 'other-caller'
$taskFinalRoot=Join-Path $taskRoot 'final-source'
$taskRoutedDestination=Join-Path $taskPreviousRoot 'local_release'
$taskRoutedSentinel=Join-Path $taskRoutedDestination 'user-file.keep'
$taskPreviousVersion=if($taskFixtureVersion -ceq '0.0.0'){'0.0.1'}else{'0.0.0'}
function Write-FixtureManifest([string]$Path,[string]$Version){
    [IO.File]::WriteAllText($Path,"[workspace]`nmembers = [`"app/audiolib-desktop`"]`n[workspace.package]`nversion = `"$Version`"`n")
}
New-Item -ItemType Directory -Path $taskRoutedDestination,(Join-Path $taskOtherCaller 'tools'),$taskFinalRoot | Out-Null
Write-FixtureManifest (Join-Path $taskPreviousRoot 'Cargo.toml') $taskPreviousVersion
Write-FixtureManifest (Join-Path $taskOtherCaller 'Cargo.toml') $taskPreviousVersion
Write-FixtureManifest (Join-Path $taskFinalRoot 'Cargo.toml') $taskFixtureVersion
$taskFinalManifest=Join-Path $taskFinalRoot 'Cargo.toml'
$taskFinalManifestHash=(Get-FileHash -LiteralPath $taskFinalManifest).Hash.ToLowerInvariant()
$taskPreviousManifestHash=(Get-FileHash -LiteralPath (Join-Path $taskPreviousRoot 'Cargo.toml')).Hash
$taskOtherCallerManifestHash=(Get-FileHash -LiteralPath (Join-Path $taskOtherCaller 'Cargo.toml')).Hash
[IO.File]::WriteAllText($taskRoutedSentinel,'routed unrelated file must survive')
[IO.File]::WriteAllText((Join-Path $taskRoutedDestination 'AudioLibExplorer.exe'),'original installed executable')
foreach($taskHelper in @('install-local-release.ps1','build-cache.ps1','native-desktop-window.ps1','assert-portable-layout.ps1','portable-files.json')){
    Copy-Item -LiteralPath (Join-Path $taskTools $taskHelper) -Destination (Join-Path $taskOtherCaller 'tools')
}
$taskOtherInstaller=Join-Path $taskOtherCaller 'tools/install-local-release.ps1'
Assert-Rejected 'older-selected-manifest' -Destination $taskRoutedDestination -VersionManifest (Join-Path $taskPreviousRoot 'Cargo.toml')
Assert-Rejected 'missing-manifest' -Destination $taskRoutedDestination -VersionManifest (Join-Path $taskFinalRoot 'missing.toml')
$taskMalformed=Join-Path $taskFinalRoot 'malformed.toml'
[IO.File]::WriteAllText($taskMalformed,"[workspace.package]`nname = `"fixture`"`n[package]`nversion = `"$taskFixtureVersion`"`n")
Assert-Rejected 'version-outside-workspace-table' -Destination $taskRoutedDestination -VersionManifest $taskMalformed
[IO.File]::WriteAllText($taskMalformed,"[workspace.package]`nversion = `"$taskFixtureVersion`"`nversion = `"$taskFixtureVersion`"`n")
Assert-Rejected 'duplicate-workspace-version' -Destination $taskRoutedDestination -VersionManifest $taskMalformed -ExpectedMessage 'one workspace.package version assignment'
[IO.File]::WriteAllText($taskMalformed,"[workspace.package]`nversion = `"$taskFixtureVersion`"`nversion = `"not-semver`"`n")
Assert-Rejected 'duplicate-invalid-workspace-version' -Destination $taskRoutedDestination -VersionManifest $taskMalformed -ExpectedMessage 'one workspace.package version assignment'
[IO.File]::WriteAllText($taskMalformed,"[workspace.package]`nversion = `"$taskFixtureVersion`"`nversion = '$taskFixtureVersion'`n")
Assert-Rejected 'duplicate-singlequoted-workspace-version' -Destination $taskRoutedDestination -VersionManifest $taskMalformed -ExpectedMessage 'one workspace.package version assignment'
[IO.File]::WriteAllText($taskMalformed,"[workspace.package]`nversion = `"$taskFixtureVersion`"`n`"version`" = `"not-semver`"`n")
Assert-Rejected 'duplicate-quoted-workspace-key' -Destination $taskRoutedDestination -VersionManifest $taskMalformed -ExpectedMessage 'one workspace.package version assignment'
Assert-Rejected 'manifest-directory' -Destination $taskRoutedDestination -VersionManifest $taskFinalRoot
$taskInsideManifest=Join-Path $taskRoutedDestination 'Cargo.toml'
Write-FixtureManifest $taskInsideManifest $taskFixtureVersion
Assert-Rejected 'manifest-overlaps-installation' -Destination $taskRoutedDestination -VersionManifest $taskInsideManifest
Remove-Item -LiteralPath $taskInsideManifest
$taskManifestLink=Join-Path $taskRoot 'manifest-link'
New-Item -ItemType Junction -Path $taskManifestLink -Target $taskFinalRoot | Out-Null
try{Assert-Rejected 'linked-manifest-ancestor' -Destination $taskRoutedDestination -VersionManifest (Join-Path $taskManifestLink 'Cargo.toml')}finally{[IO.Directory]::Delete($taskManifestLink)}
$taskDestinationLink=Join-Path $taskRoot 'destination-link'
New-Item -ItemType Junction -Path $taskDestinationLink -Target $taskPreviousRoot | Out-Null
try{Assert-Rejected 'linked-destination-ancestor' -Destination (Join-Path $taskDestinationLink 'local_release') -VersionManifest $taskFinalManifest}finally{[IO.Directory]::Delete($taskDestinationLink)}
$taskWrongDestination=Join-Path $taskRoot 'wrong-destination-name'
New-Item -ItemType Directory -Path $taskWrongDestination | Out-Null
[IO.File]::WriteAllText((Join-Path $taskWrongDestination 'AudioLibExplorer.exe'),'original installed executable')
Assert-Rejected 'not-fixed-local-release' -Destination $taskWrongDestination -VersionManifest $taskFinalManifest
$taskNonWorkspace=Join-Path $taskRoot 'non-workspace/local_release'
New-Item -ItemType Directory -Path $taskNonWorkspace | Out-Null
[IO.File]::WriteAllText((Join-Path $taskNonWorkspace 'AudioLibExplorer.exe'),'original installed executable')
Assert-Rejected 'destination-parent-not-workspace' -Destination $taskNonWorkspace -VersionManifest $taskFinalManifest
$taskFileInstallationRoot=Join-Path $taskRoot 'file-installation'
New-Item -ItemType Directory -Path $taskFileInstallationRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $taskRoot 'Cargo.toml') -Destination $taskFileInstallationRoot
$taskFileDestination=Join-Path $taskFileInstallationRoot 'local_release'
[IO.File]::WriteAllText($taskFileDestination,'a file is not an installation directory')
Assert-Rejected 'destination-is-a-file' -Destination $taskFileDestination -VersionManifest $taskFinalManifest
New-Item -ItemType Directory -Path (Join-Path $taskPreviousRoot '.rust-validation') -Force | Out-Null
$taskRoutedLock=[IO.File]::Open((Join-Path $taskPreviousRoot '.rust-validation/local-release-install.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
try{
    Assert-Rejected 'same-destination-lock-first-caller' -Destination $taskRoutedDestination -VersionManifest $taskFinalManifest
    Assert-Rejected 'same-destination-lock-other-caller' -Destination $taskRoutedDestination -VersionManifest $taskFinalManifest -Installer $taskOtherInstaller
}finally{$taskRoutedLock.Dispose()}
if([IO.File]::ReadAllText($taskRoutedSentinel)-cne'routed unrelated file must survive'){throw 'Routed preflight modified an unowned file.'}
Write-Host 'PASS: routed manifest/destination preflights and cross-checkout destination lock reject before copying.'

# Simulate a mapped-drive DisplayRoot without changing OS drive mappings or
# requiring a network share. All provider paths stay in the owned fixture.
$taskAliasDrive=@('Z','Y','W','V','U','T','S','R','Q','P'|Where-Object{!(Get-PSDrive -Name $_ -ErrorAction SilentlyContinue)})[0]
if(!$taskAliasDrive){throw 'No unused process-local fixture drive is available.'}
$taskAliasInstaller=Join-Path $taskRoot 'alias-installer.ps1'
$taskAliasCode=@'
param([string]$Source,[string]$Destination,[string]$VersionManifest)
$ErrorActionPreference='Stop'
$driveName=$env:AUDIOLIB_INSTALL_FIXTURE_ALIAS_DRIVE
if(Get-PSDrive -Name $driveName -ErrorAction SilentlyContinue){throw 'Fixture alias drive already exists.'}
$script:fixtureAliasRoot=Join-Path $PSScriptRoot 'previous-checkout'
New-PSDrive -Name $driveName -PSProvider FileSystem -Root $script:fixtureAliasRoot -Scope Script|Out-Null
function Get-PSDrive {
    [CmdletBinding()]param([string]$Name)
    if($Name-ceq$env:AUDIOLIB_INSTALL_FIXTURE_ALIAS_DRIVE){[pscustomobject]@{DisplayRoot=(Microsoft.PowerShell.Management\Get-PSDrive @PSBoundParameters).Root}}
    else{Microsoft.PowerShell.Management\Get-PSDrive @PSBoundParameters}
}
if($env:AUDIOLIB_INSTALL_FIXTURE_PROCESS_PATH){
    function Get-Process {
        [CmdletBinding()]param([string[]]$Name)
        $process=[pscustomobject]@{Path=$env:AUDIOLIB_INSTALL_FIXTURE_PROCESS_PATH;Id=[int]::MaxValue;HasExited=$false}
        $process|Add-Member ScriptMethod Refresh {}
        $process
    }
}
try{& (Join-Path $PSScriptRoot 'tools/install-local-release.ps1') -Source $Source -Destination $Destination -VersionManifest $VersionManifest}
finally{Remove-PSDrive -Name $driveName}
'@
[IO.File]::WriteAllText($taskAliasInstaller,$taskAliasCode)
$taskSavedAliasDrive=$env:AUDIOLIB_INSTALL_FIXTURE_ALIAS_DRIVE
$taskSavedProcessPath=$env:AUDIOLIB_INSTALL_FIXTURE_PROCESS_PATH
$env:AUDIOLIB_INSTALL_FIXTURE_ALIAS_DRIVE=$taskAliasDrive
New-PSDrive -Name $taskAliasDrive -PSProvider FileSystem -Root $taskPreviousRoot | Out-Null
try{
    $taskAliasedDestination=$taskAliasDrive+':\local_release'
    Assert-Rejected 'source-alias-overlap' -Candidate $taskAliasedDestination -Destination $taskRoutedDestination -VersionManifest $taskFinalManifest -Installer $taskAliasInstaller -ExpectedMessage 'Source must not overlap the fixed installation.'
    Assert-Rejected 'destination-alias-overlap' -Candidate $taskRoutedDestination -Destination $taskAliasedDestination -VersionManifest $taskFinalManifest -Installer $taskAliasInstaller -ExpectedMessage 'Source must not overlap the fixed installation.'
    Assert-Rejected 'manifest-alias-inside-install' -Destination $taskRoutedDestination -VersionManifest ($taskAliasedDestination+'\Cargo.toml') -Installer $taskAliasInstaller -ExpectedMessage 'The version manifest must not be inside the installation.'
    $env:AUDIOLIB_INSTALL_FIXTURE_PROCESS_PATH=$taskAliasedDestination+'\AudioLibExplorer.exe'
    Assert-Rejected 'installed-process-alias' -Destination $taskRoutedDestination -VersionManifest $taskFinalManifest -Installer $taskAliasInstaller -ExpectedMessage 'Installed process has no visible application window'
}finally{
    Remove-PSDrive -Name $taskAliasDrive
    $env:AUDIOLIB_INSTALL_FIXTURE_ALIAS_DRIVE=$taskSavedAliasDrive
    $env:AUDIOLIB_INSTALL_FIXTURE_PROCESS_PATH=$taskSavedProcessPath
}
Write-Host 'PASS: mapped-drive canonical overlap, manifest containment and installed-process selection reject at the intended preflight.'

# Compile tiny test processes with the Windows Framework compiler. These fakes
# exercise actual CLI invocation/process startup; no AudioLib binary is used.
$taskCompiler=Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
if(!(Test-Path -LiteralPath $taskCompiler)){throw 'The owned fake-executable regression requires the Windows Framework C# compiler.'}
$taskFakeSource=Join-Path $taskRoot 'fake-runtime.cs'
$taskFakeExe=Join-Path $taskRoot 'fake-runtime.exe'
$taskFakeCode=@'
using System;
using System.IO;
using System.Reflection;
public static class AudioLibInstallerFixture {
    public static int Main(string[] args) {
        string location = Assembly.GetExecutingAssembly().Location;
        string name = Path.GetFileName(location);
        string data = Environment.GetEnvironmentVariable("AUDIOLIB_DATA_DIR");
        if (String.IsNullOrEmpty(data) || !data.StartsWith(Path.Combine(Path.GetTempPath(), "AudioLibLocalInstall-"), StringComparison.OrdinalIgnoreCase) || !File.Exists(Path.Combine(data, "settings.json"))) return 21;
        string trace = Environment.GetEnvironmentVariable("AUDIOLIB_INSTALL_FIXTURE_TRACE");
        if (!String.IsNullOrEmpty(trace)) File.AppendAllText(trace, name + "|" + location + "|" + data + Environment.NewLine);
        if (name.Equals("AudioLibCli.exe", StringComparison.OrdinalIgnoreCase)) {
            if (args.Length != 1 || args[0] != "--version") return 22;
            Console.WriteLine("AudioLibCli __VERSION__");
            return 0;
        }
        if (args.Length != 5 || args[0] != "--smoke-ui" || args[1] != "--smoke-language" || args[2] != "cn" || args[3] != "--screenshot") return 23;
        File.WriteAllBytes(args[4], Convert.FromBase64String("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a4J8AAAAASUVORK5CYII="));
        return 0;
    }
}
'@
[IO.File]::WriteAllText($taskFakeSource,$taskFakeCode.Replace('__VERSION__',$taskFixtureVersion))
& $taskCompiler /nologo /target:exe ('/out:'+$taskFakeExe) $taskFakeSource *> (Join-Path $taskRoot 'fixture-compile.log')
if($LASTEXITCODE-ne0){throw 'Owned fake CLI/desktop fixture compilation failed.'}
Copy-Item -LiteralPath $taskFakeExe -Destination (Join-Path $taskSource 'AudioLibExplorer.exe') -Force
Copy-Item -LiteralPath $taskFakeExe -Destination (Join-Path $taskSource 'AudioLibCli.exe') -Force
$taskTrace=Join-Path $taskRoot 'fixture-startup.trace'
$taskSavedTrace=$env:AUDIOLIB_INSTALL_FIXTURE_TRACE
$taskOriginalData=$env:AUDIOLIB_DATA_DIR
$taskPrivateData=Join-Path $taskRoot 'normal-user-data'
New-Item -ItemType Directory -Path $taskPrivateData | Out-Null
[IO.File]::WriteAllText((Join-Path $taskPrivateData 'settings.json'),'normal user data must survive')
$env:AUDIOLIB_INSTALL_FIXTURE_TRACE=$taskTrace
$env:AUDIOLIB_DATA_DIR=$taskPrivateData
function Assert-SyntheticIdentity([string]$Installed,$Result){
    $files=@(Get-ChildItem -LiteralPath $taskSource -Recurse -File)
    if($Result.verified_files-ne$files.Count){throw 'Installed file manifest has the wrong coverage.'}
    foreach($file in $files){
        $relative=[IO.Path]::GetRelativePath($taskSource,$file.FullName)
        if((Get-FileHash -LiteralPath (Join-Path $Installed $relative)).Hash-cne(Get-FileHash -LiteralPath $file.FullName).Hash){throw "Installed synthetic payload differs: $relative"}
    }
}
try{
    $taskDefaultLog=Join-Path $taskRoot 'synthetic-default.log'
    & $taskPwsh -NoProfile -File $taskInstaller -Source $taskSource *> $taskDefaultLog
    if($LASTEXITCODE-ne0){throw "Default synthetic installation failed; inspect $taskDefaultLog"}
    $taskDefaultResult=Get-Content -LiteralPath $taskDefaultLog -Raw|ConvertFrom-Json
    if(!$taskDefaultResult.success-or$taskDefaultResult.installation-cne$taskInstalled-or$taskDefaultResult.version_manifest-cne(Join-Path $taskRoot 'Cargo.toml')-or$taskDefaultResult.startup-cne'passed'){throw 'Default installation routing changed.'}
    Assert-SyntheticIdentity $taskInstalled $taskDefaultResult
    Write-FixtureManifest (Join-Path $taskRoot 'Cargo.toml') $taskPreviousVersion
    $taskCallerManifestHash=(Get-FileHash -LiteralPath (Join-Path $taskRoot 'Cargo.toml')).Hash
    foreach($taskAttempt in 1..2){
        $taskRoutedLog=Join-Path $taskRoot ('synthetic-routed-'+$taskAttempt+'.log')
        $taskCaller=if($taskAttempt-eq1){$taskInstaller}else{$taskOtherInstaller}
        & $taskPwsh -NoProfile -File $taskCaller -Source $taskSource -Destination $taskRoutedDestination -VersionManifest $taskFinalManifest *> $taskRoutedLog
        if($LASTEXITCODE-ne0){throw "Routed synthetic installation failed; inspect $taskRoutedLog"}
        $taskRoutedResult=Get-Content -LiteralPath $taskRoutedLog -Raw|ConvertFrom-Json
        if(!$taskRoutedResult.success-or$taskRoutedResult.installation-cne$taskRoutedDestination-or$taskRoutedResult.installation_root-cne$taskPreviousRoot-or$taskRoutedResult.version_manifest-cne$taskFinalManifest-or$taskRoutedResult.version_manifest_sha256-cne$taskFinalManifestHash-or$taskRoutedResult.startup-cne'passed'-or($taskAttempt-eq2-and$taskRoutedResult.copied_files-ne0)){throw 'Explicit destination/manifest/startup/idempotence identity failed.'}
        if(!$taskRoutedResult.evidence.StartsWith((Join-Path $taskPreviousRoot '.rust-validation/local-install')+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Routed evidence is not anchored to the fixed installation root.'}
        Assert-SyntheticIdentity $taskRoutedDestination $taskRoutedResult
    }
    if((Get-FileHash -LiteralPath (Join-Path $taskPreviousRoot 'Cargo.toml')).Hash-cne$taskPreviousManifestHash-or(Get-FileHash -LiteralPath (Join-Path $taskRoot 'Cargo.toml')).Hash-cne$taskCallerManifestHash-or(Get-FileHash -LiteralPath (Join-Path $taskOtherCaller 'Cargo.toml')).Hash-cne$taskOtherCallerManifestHash-or(Get-FileHash -LiteralPath $taskFinalManifest).Hash.ToLowerInvariant()-cne$taskFinalManifestHash-or[IO.File]::ReadAllText($taskRoutedSentinel)-cne'routed unrelated file must survive'-or[IO.File]::ReadAllText($taskSentinel)-cne'unowned file must survive'-or[IO.File]::ReadAllText((Join-Path $taskPrivateData 'settings.json'))-cne'normal user data must survive'){throw 'Installation changed a source/target manifest or user files/preferences.'}
    $taskTraceLines=@(Get-Content -LiteralPath $taskTrace)
    if($taskTraceLines.Count-ne6-or@($taskTraceLines|Where-Object{$_-like('*|'+$taskRoutedDestination+'\*')}).Count-ne4){throw 'Synthetic CLI/desktop processes did not run at the expected destinations.'}
    Write-Host 'PASS: synthetic default/explicit routing, external version manifest, shared destination, all-file identity, idempotence and isolated CLI/desktop startup.'
}finally{
    $env:AUDIOLIB_DATA_DIR=$taskOriginalData
    $env:AUDIOLIB_INSTALL_FIXTURE_TRACE=$taskSavedTrace
    [IO.File]::WriteAllText((Join-Path $taskRoot 'Cargo.toml'),$taskOriginalManifest)
}
if (!$RuntimePayload) { Write-Host 'Real AudioLib runtime: not-run (synthetic test processes only).'; Write-Host "Local installer regression passed. Owned evidence: $taskRoot"; exit 0 }
$RuntimePayload=[IO.Path]::GetFullPath($RuntimePayload)
$taskOriginalData=$env:AUDIOLIB_DATA_DIR
$taskPrivateData=Join-Path $taskRoot 'normal-user-data'
New-Item -ItemType Directory -Path $taskPrivateData -Force | Out-Null
[IO.File]::WriteAllText((Join-Path $taskPrivateData 'settings.json'),'normal user data must survive')
$env:AUDIOLIB_DATA_DIR=$taskPrivateData
try {
    # Reuse the accepted payload in the owned fixture; never touch the real install.
    foreach ($taskFile in Get-ChildItem -LiteralPath $RuntimePayload -Recurse -File) {
        $taskRelative=[IO.Path]::GetRelativePath($RuntimePayload,$taskFile.FullName)
        $taskTarget=Join-Path $taskSource $taskRelative
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($taskTarget)) -Force | Out-Null
        Copy-Item -LiteralPath $taskFile.FullName -Destination $taskTarget
    }
    foreach ($taskAttempt in 1..2) {
        $taskLog=Join-Path $taskRoot ('attempt-'+$taskAttempt+'.log')
        & $taskPwsh -NoProfile -File $taskInstaller -Source $taskSource *> $taskLog
        if ($LASTEXITCODE -ne 0) { throw "Owned installation fixture failed; inspect $taskLog" }
        $taskResult=Get-Content -LiteralPath $taskLog -Raw | ConvertFrom-Json
        if (!$taskResult.success -or $taskResult.startup -cne 'passed' -or ($taskAttempt -eq 2 -and $taskResult.copied_files -ne 0)) { throw 'Installation identity/startup/idempotence failed.' }
    }
    if ([IO.File]::ReadAllText($taskSentinel) -cne 'unowned file must survive' -or [IO.File]::ReadAllText((Join-Path $taskPrivateData 'settings.json')) -cne 'normal user data must survive') { throw 'User data was modified.' }
    Write-Host 'PASS: owned runtime fixture identity and startup; identical files not recopied; user files/preferences preserved.'
} finally { $env:AUDIOLIB_DATA_DIR=$taskOriginalData }
Write-Host "Local installer regression passed. Owned evidence: $taskRoot"
