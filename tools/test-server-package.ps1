# tools/test-server-package.ps1
# Isolated package-boundary regression. Uses a tiny --version fixture executable,
# synthetic Cargo/Git metadata, and public placeholders; never builds/starts a server.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$testParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$testRoot = [IO.Path]::GetFullPath((Join-Path $testParent ('AudioLibServerPackage-' + [guid]::NewGuid().ToString('N'))))
$comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
function Assert-TestPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if (!$full.StartsWith($testRoot + [IO.Path]::DirectorySeparatorChar, $comparison)) { throw 'Server regression escaped its owned subtree.' }
}
function Expect-Rejection([scriptblock]$Action, [string]$Message) {
    $rejected = $false
    try { & $Action } catch { $rejected = $_.Exception.Message -match $Message }
    if (!$rejected) { throw 'Server package failed to reject a boundary regression.' }
}
$target = if ($IsWindows) { 'x86_64-pc-windows-msvc' } elseif ($IsLinux) { 'x86_64-unknown-linux-gnu' } else { 'aarch64-apple-darwin' }
$platformName = if ($IsWindows) { 'win-x64' } elseif ($IsLinux) { 'linux-x64' } else { 'macos-arm64' }
$binaryName = if ($IsWindows) { 'audiolib-server.exe' } else { 'audiolib-server' }
$extension = if ($IsWindows) { 'zip' } else { 'tar.gz' }
New-Item -ItemType Directory -Path (Join-Path $testRoot 'tools') | Out-Null
try {
    foreach ($file in @('package-server.ps1', 'server-files.json')) { Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination (Join-Path $testRoot 'tools') }
    $files = @(Get-Content -LiteralPath (Join-Path $testRoot 'tools/server-files.json') -Raw | ConvertFrom-Json)
    foreach ($file in $files) {
        $destination = Join-Path $testRoot $file
        Assert-TestPath $destination
        New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
        Set-Content -LiteralPath $destination -Value 'Synthetic public documentation.' -Encoding utf8NoBOM
    }
    $binaryRoot = Join-Path $testRoot "target/$target/release"
    New-Item -ItemType Directory -Path $binaryRoot -Force | Out-Null
    $binary = Join-Path $binaryRoot $binaryName
    if ($IsWindows) {
        $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
        if (!(Test-Path -LiteralPath $compiler -PathType Leaf)) { throw 'Windows regression requires the installed .NET Framework C# compiler.' }
        $source = Join-Path $testRoot 'VersionFixture.cs'
        Set-Content -LiteralPath $source -Encoding utf8NoBOM -Value 'using System; class VersionFixture { static int Main(string[] args) { if(args.Length != 1 || args[0] != "--version") return 7; Console.WriteLine("audiolib-server 0.4.4"); return 0; } }'
        & $compiler /nologo /target:exe /optimize+ "/out:$binary" $source
        if ($LASTEXITCODE -ne 0) { throw 'Cannot compile the version-only package fixture.' }
    } else {
        [IO.File]::WriteAllText($binary, "#!/bin/sh`n[ `"`$#`" -eq 1 ] && [ `"`$1`" = '--version' ] || exit 7`nprintf 'audiolib-server 0.4.4\n'`n", [Text.UTF8Encoding]::new($false))
        [IO.File]::SetUnixFileMode($binary, [IO.UnixFileMode]493)
    }
    $licenseRoot = Join-Path $testRoot 'dependency'
    New-Item -ItemType Directory -Path $licenseRoot | Out-Null
    Set-Content -LiteralPath (Join-Path $licenseRoot 'LICENSE-MIT') -Value 'Synthetic MIT license notice.' -Encoding utf8NoBOM
    $fakeMetadata = [ordered]@{
        packages=@(
            @{ id='server'; name='audiolib-server'; version='0.4.4'; source=$null; license='GPL-3.0-only'; license_file=$null; repository=$null; manifest_path=(Join-Path $testRoot 'server/audiolib-server/Cargo.toml') },
            @{ id='normal'; name='normal-dependency'; version='1.0.0'; source='registry+public'; license='MIT'; license_file='LICENSE-MIT'; repository='https://example.com/normal'; manifest_path=(Join-Path $licenseRoot 'Cargo.toml') },
            @{ id='dev'; name='private-dev-dependency'; version='1.0.0'; source='registry+public'; license='MIT'; license_file=$null; repository=$null; manifest_path=(Join-Path $testRoot 'must-not-be-opened/Cargo.toml') }
        )
        resolve=@{ nodes=@(
            @{ id='server'; deps=@(@{pkg='normal'; dep_kinds=@(@{kind=$null})}, @{pkg='dev'; dep_kinds=@(@{kind='dev'})}) },
            @{ id='normal'; deps=@() }, @{ id='dev'; deps=@() }
        ) }
    }
    # These functions mask only the external commands in this invocation's child scope.
    $metadataCommand = {
        if ($args -notcontains '--locked' -or $args -notcontains '--filter-platform') { throw 'Package metadata must use the locked target filter.' }
        $fakeMetadata | ConvertTo-Json -Depth 8 -Compress
        $global:LASTEXITCODE = 0
    }.GetNewClosure()
    Set-Item -LiteralPath Function:cargo -Value $metadataCommand
    function git { 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; $global:LASTEXITCODE = 0 }
    $latest = Join-Path $testRoot 'dist/latest'
    $payload = Join-Path $latest "server-$platformName"
    New-Item -ItemType Directory -Path $payload -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $payload 'settings.json') -Value 'Stale private runtime data.'
    foreach ($protected in @('dist/latest/portable/preserve.txt', 'local_release/preserve.txt', 'doc/unlisted-private-report.json')) {
        $path = Join-Path $testRoot $protected
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        Set-Content -LiteralPath $path -Value 'Owned sentinel.'
    }
    $packager = Join-Path $testRoot 'tools/package-server.ps1'
    & $packager -Tag v0.4.4 -Target $target
    if (Test-Path -LiteralPath (Join-Path $payload 'settings.json')) { throw 'Stale server runtime file survived packaging.' }
    foreach ($protected in @('dist/latest/portable/preserve.txt', 'local_release/preserve.txt', 'doc/unlisted-private-report.json')) {
        if ((Get-Content -LiteralPath (Join-Path $testRoot $protected) -Raw).Trim() -ne 'Owned sentinel.') { throw 'Packaging changed an unrelated output.' }
    }
    if (Test-Path -LiteralPath (Join-Path $payload 'doc/unlisted-private-report.json')) { throw 'Unlisted private report was packaged.' }
    $dependencyText = Get-Content -LiteralPath (Join-Path $payload 'licenses/dependencies.json') -Raw
    if ($dependencyText.Contains($testRoot) -or $dependencyText.Contains('manifest_path') -or $dependencyText.Contains('private-dev-dependency')) { throw 'Dependency metadata leaked private fields or dev-only dependencies.' }
    $archive = Join-Path $latest "AudioLibServer-v0.4.4-$platformName.$extension"
    $archiveHash = (Get-FileHash -LiteralPath $archive).Hash
    if ((Get-Content -LiteralPath ($archive + '.sha256') -Raw).Trim() -cne ($archiveHash + '  ' + [IO.Path]::GetFileName($archive))) { throw 'Server archive checksum differs.' }
    $reportText = Get-Content -LiteralPath (Join-Path $latest "AudioLibServer-v0.4.4-$platformName-report.json") -Raw
    $report = $reportText | ConvertFrom-Json
    if (@($report.PSObject.Properties.Name | Sort-Object) -join ',' -cne 'archive_sha256,binary_sha256,checks,commit,success,tag,target,version' -or !$report.success -or
        $report.archive_sha256 -cne $archiveHash -or $report.binary_sha256 -cne (Get-FileHash -LiteralPath $binary).Hash -or
        $reportText.Contains($testRoot) -or @($report.checks.PSObject.Properties.Value | Where-Object { $_ -ne $true }).Count) { throw 'Server public report schema/identity failed.' }
    if (!$IsWindows) {
        $extracted = Join-Path $testRoot 'extracted'
        New-Item -ItemType Directory -Path $extracted | Out-Null
        & tar -xzf $archive -C $extracted
        if ($LASTEXITCODE -ne 0 -or ([int][IO.File]::GetUnixFileMode((Join-Path $extracted $binaryName)) -band 73) -ne 73 -or
            ([int][IO.File]::GetUnixFileMode((Join-Path $extracted 'server/deploy/redeploy-server.sh')) -band 73) -ne 73) { throw 'Unix archive did not preserve executable permissions.' }
    }
    Expect-Rejection { & $packager -Tag v0.4.5 -Target $target } 'version does not match'
    $document = Join-Path $testRoot $files[0]
    Set-Content -LiteralPath $document -Value $testRoot
    Expect-Rejection { & $packager -Tag v0.4.4 -Target $target -BinaryPath $binary } 'Local build/user path'
    Set-Content -LiteralPath $document -Value 'Synthetic public documentation.'
    $manifest = Join-Path $testRoot 'tools/server-files.json'
    $files + @('../outside.txt') | ConvertTo-Json | Set-Content -LiteralPath $manifest -Encoding utf8NoBOM
    Expect-Rejection { & $packager -Tag v0.4.4 -Target $target } 'Unsafe relative'
    ConvertTo-Json -InputObject $files | Set-Content -LiteralPath $manifest -Encoding utf8NoBOM
    if ((Get-FileHash -LiteralPath $archive).Hash -cne $archiveHash) { throw 'A rejected package replaced the last verified archive.' }
    $dist = Join-Path $testRoot 'dist'
    $redirected = Join-Path $testRoot 'redirected'
    Assert-TestPath $dist; Assert-TestPath $redirected
    Move-Item -LiteralPath $dist -Destination (Join-Path $testRoot 'prior-output')
    New-Item -ItemType Directory -Path $redirected | Out-Null
    Set-Content -LiteralPath (Join-Path $redirected 'preserve.txt') -Value 'Owned sentinel.'
    try {
        if ($IsWindows) { New-Item -ItemType Junction -Path $dist -Target $redirected | Out-Null } else { $null = [IO.Directory]::CreateSymbolicLink($dist, $redirected) }
        Expect-Rejection { & $packager -Tag v0.4.4 -Target $target } 'Linked server package path'
        if (Test-Path -LiteralPath (Join-Path $redirected 'latest')) { throw 'Packaging followed a linked output ancestor.' }
    } finally {
        $link = Get-Item -LiteralPath $dist -Force -ErrorAction SilentlyContinue
        if ($null -ne $link -and ($link.Attributes -band [IO.FileAttributes]::ReparsePoint)) { Assert-TestPath $dist; Remove-Item -LiteralPath $dist -Force }
    }
    Write-Host 'Server package boundary regression passed: allowlist, dependency closure, safe report/checksums, rejection preservation and linked-path guard.'
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        if (!(Split-Path -Leaf $testRoot).StartsWith('AudioLibServerPackage-', [StringComparison]::Ordinal) -or
            !$testRoot.StartsWith($testParent.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar, $comparison) -or
            @(Get-ChildItem -LiteralPath $testRoot -Recurse -Force -Attributes ReparsePoint).Count) { throw 'Unsafe server regression cleanup path.' }
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
