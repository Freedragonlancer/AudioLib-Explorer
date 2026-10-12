# tools/validate-portable.ps1
[CmdletBinding()]
param([string]$BinaryDirectory = 'dist/latest/portable', [switch]$CleanCheckout)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$binaryRoot = [IO.Path]::GetFullPath($(if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRoot $BinaryDirectory }))
$cli = Join-Path $binaryRoot 'AudioLibCli.exe'
$desktop = Join-Path $binaryRoot 'AudioLibExplorer.exe'
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar)
$testRoot = [IO.Path]::GetFullPath((Join-Path $tempParent ('AudioLibPortable-' + [guid]::NewGuid().ToString('N'))))
$artifactRoot = [IO.Path]::GetFullPath((Join-Path $taskRoot '.rust-validation/portable-smoke'))
$savedDataRoot = $env:AUDIOLIB_DATA_DIR
$legacyPath = Join-Path $taskRoot 'data/audiolib.db'
$originalHash = if (!$CleanCheckout) { (Get-FileHash -LiteralPath $legacyPath -Algorithm SHA256).Hash } else { $null }
function Invoke-DesktopSmoke([string[]]$Arguments, [string]$Name) {
    $stdout = Join-Path $testRoot ($Name + '.stdout')
    $stderr = Join-Path $testRoot ($Name + '.stderr')
    $process = Start-Process -FilePath $desktop -ArgumentList $Arguments -WorkingDirectory $binaryRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    # Windows PowerShell 5 reports no ExitCode unless the handle is opened before exit.
    $null = $process.Handle
    try {
        if (!$process.WaitForExit(30000)) { throw "Portable desktop $Name timed out." }
        return [PSCustomObject]@{ ExitCode=$process.ExitCode; Output=(Get-Content -LiteralPath $stdout -Raw); Error=(Get-Content -LiteralPath $stderr -Raw) }
    } finally {
        if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
        $process.Dispose()
    }
}
try {
    if (!$testRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Portable smoke path escaped its owned temporary root.' }
    if (!$artifactRoot.StartsWith($taskRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Portable artifacts escaped the workspace.' }
    New-Item -ItemType Directory -Force -Path $testRoot,$artifactRoot | Out-Null
    $env:AUDIOLIB_DATA_DIR = Join-Path $testRoot 'userdata'
    if ($CleanCheckout) {
        # Explicit isolated preferences prevent discovery/import of a developer's
        # legacy database, even when this mode is invoked from their checkout.
        New-Item -ItemType Directory -Force -Path $env:AUDIOLIB_DATA_DIR | Out-Null
        # Without a BOM: Windows PowerShell 5's `-Encoding UTF8` writes one, which the
        # settings parser rejects.
        $settingsJson = @{active_database=(Join-Path $env:AUDIOLIB_DATA_DIR 'library.db');language='en';playback=@{volume=0.0};allowed_scan_roots=@()} |
            ConvertTo-Json -Depth 4
        [IO.File]::WriteAllText((Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json'), $settingsJson, [Text.UTF8Encoding]::new($false))
    }
    foreach ($binary in @('AudioLibExplorer.exe','AudioLibCli.exe')) {
        $sourceHash = (Get-FileHash -LiteralPath (Join-Path $taskRoot ('target/x86_64-pc-windows-msvc/release/' + $binary)) -Algorithm SHA256).Hash
        $packageHash = (Get-FileHash -LiteralPath (Join-Path $binaryRoot $binary) -Algorithm SHA256).Hash
        if ($sourceHash -ne $packageHash) { throw "Packaged binary differs from release: $binary" }
    }
    $initial = Invoke-DesktopSmoke @('--headless-smoke') 'headless'
    if ($initial.ExitCode -ne 0) { throw "Portable headless startup failed: $($initial.Error)" }
    $library = $initial.Output | ConvertFrom-Json
    $initialCount = if ($CleanCheckout) { 0 } else { 34 }
    if (!$library.success -or $library.total_count -ne $initialCount) { throw "Portable startup record count differs from expected $initialCount." }
    $expectedCount = $initialCount
    if ($CleanCheckout) {
        $fixtureRoot = Join-Path $testRoot 'fixtures'
        New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
        $fixture = Join-Path $fixtureRoot 'CI synthetic tone.wav'
        $writer = [IO.BinaryWriter]::new([IO.File]::Create($fixture))
        try {
            $rate = 48000; $bytes = $rate * 2
            $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $writer.Write([uint32]($bytes + 36))
            $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $writer.Write([uint32]16)
            $writer.Write([uint16]1); $writer.Write([uint16]1); $writer.Write([uint32]$rate)
            $writer.Write([uint32]($rate * 2)); $writer.Write([uint16]2); $writer.Write([uint16]16)
            $writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([uint32]$bytes)
            for ($frame = 0; $frame -lt $rate; $frame++) {
                $writer.Write([int16](12000 * [Math]::Sin(2 * [Math]::PI * 619 * $frame / $rate)))
            }
        } finally { $writer.Dispose() }
        $fixtureHash = (Get-FileHash -LiteralPath $fixture -Algorithm SHA256).Hash
        $scan = (& $cli scan $fixtureRoot | Out-String) | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or !$scan.success) { throw 'Packaged CLI synthetic fixture scan failed.' }
        $expectedCount = 1
    }
    $stats = (& $cli stats | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$stats.success -or $stats.data.audio_files -ne $expectedCount) { throw 'Packaged CLI statistics failed.' }
    $snapshot = Join-Path $artifactRoot 'chinese.png'
    $ui = Invoke-DesktopSmoke @('--smoke-ui','--smoke-language','cn','--screenshot',('"' + $snapshot + '"')) 'chinese'
    if ($ui.ExitCode -ne 0 -or !(Test-Path -LiteralPath $snapshot)) { throw "Portable Chinese UI smoke failed: $($ui.Error)" }
    $badRoot = Join-Path $testRoot 'bad-preferences'
    New-Item -ItemType Directory -Force -Path $badRoot | Out-Null
    Set-Content -LiteralPath (Join-Path $badRoot 'settings.json') -Value '{invalid JSON' -Encoding UTF8
    $env:AUDIOLIB_DATA_DIR = $badRoot
    $failure = Invoke-DesktopSmoke @('--headless-smoke') 'expected-failure'
    $fatalLog = Join-Path $badRoot 'logs/AudioLibExplorer-startup-error.log'
    if ($failure.ExitCode -ne 1 -or !(Test-Path -LiteralPath $fatalLog)) { throw 'Fatal preference failure did not return code 1 and persist a startup diagnostic.' }
    if ((Get-Content -LiteralPath $fatalLog -Raw) -notmatch 'Load AudioLib preferences') { throw 'Fatal log does not contain the startup context.' }
    $notices = Get-Content -LiteralPath (Join-Path $binaryRoot 'licenses/dependencies.json') -Raw | ConvertFrom-Json
    if (!($notices | Where-Object { $_.name -eq 'slint' -and $_.version -eq '1.17.1' })) { throw 'Slint dependency notice missing.' }
    if (!(Test-Path -LiteralPath (Join-Path $binaryRoot 'licenses/slint-1.17.1/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md'))) { throw 'Selected Slint license text missing.' }
    if (!(Test-Path -LiteralPath (Join-Path $binaryRoot 'licenses/Steinberg-ASIO-SDK-2.3.4.txt'))) { throw 'ASIO SDK license missing.' }
    if (!($notices | Where-Object { $_.name -eq 'soundtouch' -and $_.version -eq '0.5.4' }) -or
        !(Test-Path -LiteralPath (Join-Path $binaryRoot 'licenses/SoundTouch-2.3.2-LGPL.txt'))) { throw 'SoundTouch dependency or license notice missing.' }
    $unchanged = if ($CleanCheckout) {
        $fixtureHash -eq (Get-FileHash -LiteralPath $fixture -Algorithm SHA256).Hash
    } else { $originalHash -eq (Get-FileHash -LiteralPath $legacyPath -Algorithm SHA256).Hash }
    if (!$unchanged) { throw 'Source changed during portable smoke.' }
    $report = [PSCustomObject]@{
        success=$true
        mode=$(if ($CleanCheckout) { 'clean-checkout-synthetic' } else { 'legacy-import' })
        checks=@('packaged EXE hashes match release',$(if ($CleanCheckout) { 'isolated empty first launch and synthetic WAV scan' } else { 'headless first launch imports 34 records' }),'packaged CLI statistics','Chinese UI snapshot and clean exit','invalid preferences return failure and synchronous diagnostic','Slint, SoundTouch and ASIO license materials','source unchanged')
        records=$expectedCount
        screenshot=$snapshot
        original_database_unchanged=$(if ($CleanCheckout) { $null } else { $unchanged })
        source_unchanged=$unchanged
        python_runtime_required=$false
    }
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $taskRoot 'doc/Rust_Portable.json') -Encoding UTF8
    $report | ConvertTo-Json -Depth 5
} finally {
    $env:AUDIOLIB_DATA_DIR = $savedDataRoot
    if (Test-Path -LiteralPath $testRoot) {
        if (!$testRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup outside owned portable test root.' }
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
