# tools/validate-access-denial.ps1
[CmdletBinding()]
param([string]$BinaryDirectory = 'target/x86_64-pc-windows-msvc/release', [switch]$CleanCheckout)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'console-utf8.ps1')
try {
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$binaryRoot = if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRoot $BinaryDirectory }
$cli = Join-Path $binaryRoot 'AudioLibCli.exe'
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar)
$testRoot = [IO.Path]::GetFullPath((Join-Path $tempParent ('AudioLibAcl-' + [guid]::NewGuid().ToString('N'))))
$libraryRoot = Join-Path $testRoot 'audio'
$deniedRoot = Join-Path $libraryRoot 'denied'
$testDatabase = Join-Path $testRoot 'library.db'
$savedDataRoot = $env:AUDIOLIB_DATA_DIR
$originalAcl = $null
function Invoke-AclLibrary([string[]]$Arguments) {
    $reply = (& $cli --database $testDatabase @Arguments | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$reply.success) { throw "ACL library operation failed: $($reply.error)" }
    return $reply.data
}
try {
    if (!$testRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'ACL test path escaped its owned temporary root.' }
    New-Item -ItemType Directory -Path $deniedRoot -Force | Out-Null
    $env:AUDIOLIB_DATA_DIR = Join-Path $testRoot 'userdata'
    New-Item -ItemType Directory -Path $env:AUDIOLIB_DATA_DIR | Out-Null
    Copy-Item -LiteralPath (Join-Path $taskRoot 'config/rust_settings.example.json') -Destination (Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json')
    $null = Invoke-AclLibrary @('db','create',$testDatabase)
    if ($CleanCheckout) {
        $fixturePath = Join-Path $testRoot 'synthetic.wav'
        $writer = [IO.BinaryWriter]::new([IO.File]::Create($fixturePath))
        try {
            $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $writer.Write([uint32]292)
            $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $writer.Write([uint32]16)
            $writer.Write([uint16]1); $writer.Write([uint16]1); $writer.Write([uint32]48000)
            $writer.Write([uint32]96000); $writer.Write([uint16]2); $writer.Write([uint16]16)
            $writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([uint32]256)
            $writer.Write([byte[]]::new(256))
        } finally { $writer.Dispose() }
    } else {
        $fixture = Get-ChildItem -LiteralPath (Join-Path $taskRoot 'test_audiolib/P-51 Mustang') -Filter '*.wav' -Recurse -File | Sort-Object Length | Select-Object -First 1
        if (!$fixture) { throw 'Legacy WAV fixture unavailable; use -CleanCheckout for the isolated synthetic fixture.' }
        $fixturePath = $fixture.FullName
    }
    $fixtureHash = (Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash
    Copy-Item -LiteralPath $fixturePath -Destination (Join-Path $libraryRoot 'visible.wav')
    Copy-Item -LiteralPath $fixturePath -Destination (Join-Path $deniedRoot 'protected.wav')
    $initial = Invoke-AclLibrary @('scan',$libraryRoot,'--full')
    if ($initial.errors -ne 0 -or $initial.added -ne 2) { throw 'ACL fixture did not index both files.' }
    Remove-Item -LiteralPath (Join-Path $libraryRoot 'visible.wav')
    $originalAcl = Get-Acl -LiteralPath $deniedRoot
    $restrictedAcl = Get-Acl -LiteralPath $deniedRoot
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $deny = [Security.AccessControl.FileSystemAccessRule]::new($identity,[Security.AccessControl.FileSystemRights]::ReadAndExecute,[Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit,[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Deny)
    $restrictedAcl.AddAccessRule($deny)
    # This ACL mutation is confined to a fresh test-owned directory, never source fixtures.
    Set-Acl -LiteralPath $deniedRoot -AclObject $restrictedAcl
    $accessDenied = $false
    try { Get-ChildItem -LiteralPath $deniedRoot -ErrorAction Stop | Out-Null } catch { $accessDenied = $true }
    if (!$accessDenied) { throw 'The test environment bypassed the directory ACL; denial was not reproduced.' }
    $partial = Invoke-AclLibrary @('scan',$libraryRoot,'--full')
    $retained = Invoke-AclLibrary @('search')
    if ($partial.errors -lt 1 -or $retained.total_count -ne 2) { throw 'A partially inaccessible scan incorrectly reconciled missing files.' }
    Set-Acl -LiteralPath $deniedRoot -AclObject $originalAcl
    $originalAcl = $null
    $successful = Invoke-AclLibrary @('scan',$libraryRoot,'--full')
    $after = Invoke-AclLibrary @('search')
    if ($successful.errors -ne 0 -or $after.total_count -ne 1 -or $after.results[0].filename -ne 'protected.wav') { throw 'Successful enumeration did not reconcile only the absent file.' }
    if ((Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash -cne $fixtureHash) { throw 'ACL validation changed its source fixture.' }
    [PSCustomObject]@{ success=$true; fixture_mode=$(if ($CleanCheckout) { 'synthetic' } else { 'legacy' }); windows_acl_denial_reproduced=$true; partial_scan_errors=$partial.errors; records_retained_during_denial=$retained.total_count; records_after_successful_scan=$after.total_count; source_fixture_unchanged=$true } | ConvertTo-Json
} finally {
    $env:AUDIOLIB_DATA_DIR = $savedDataRoot
    if ($originalAcl) { Set-Acl -LiteralPath $deniedRoot -AclObject $originalAcl }
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    if ($resolvedRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolvedRoot)) {
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
}
} finally {
    Restore-AudioLibConsoleEncoding
}
