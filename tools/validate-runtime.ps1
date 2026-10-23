# tools/validate-runtime.ps1
[CmdletBinding()]
param([string]$BinaryDirectory = 'target/x86_64-pc-windows-msvc/release')
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'console-utf8.ps1')
try {
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$binaryRoot = if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRoot $BinaryDirectory }
$cli = Join-Path $binaryRoot 'AudioLibCli.exe'
if (!(Test-Path -LiteralPath $cli)) { throw "CLI executable missing: $cli" }
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$validationRoot = [IO.Path]::GetFullPath((Join-Path $temporaryRoot ('AudioLibValidation-' + [guid]::NewGuid().ToString('N'))))
if (!$validationRoot.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Test path escaped the temporary directory.' }
New-Item -ItemType Directory -Path $validationRoot | Out-Null
$testDatabase = Join-Path $validationRoot 'library.db'
$sourceDatabase = Join-Path $taskRoot 'data/audiolib.db'
$fixtureRoot = Join-Path $taskRoot 'test_audiolib/P-51 Mustang'
$originalHash = (Get-FileHash -LiteralPath $sourceDatabase -Algorithm SHA256).Hash
$checks = [Collections.Generic.List[string]]::new()
$savedDataRoot = $env:AUDIOLIB_DATA_DIR
function Invoke-Library([string[]]$Arguments) {
    $reply = (& $cli --database $testDatabase @Arguments | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$reply.success) { throw "CLI failure: $($reply.error)" }
    return $reply.data
}
try {
    $env:AUDIOLIB_DATA_DIR = Join-Path $validationRoot 'userdata'
    New-Item -ItemType Directory -Path $env:AUDIOLIB_DATA_DIR | Out-Null
    Copy-Item -LiteralPath (Join-Path $taskRoot 'config/rust_settings.example.json') -Destination (Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json')
    $imported = Invoke-Library @('db','import',$sourceDatabase)
    if ($imported.audio_files -ne 34 -or !$imported.source_preserved) { throw 'Legacy import did not preserve the 34-file library.' }
    $checks.Add('legacy import and original preservation')
    $page = Invoke-Library @('search','Mustang','--limit','200')
    if ($page.total_count -ne 34 -or $page.results.Count -ne 34) { throw 'Search did not return all fixture records.' }
    $checks.Add('English search and total count')
    $filtered = Invoke-Library @('search','Mustang','--sample-rate','192000','--bit-depth','24','--channels','2','--duration-min','0')
    if ($filtered.total_count -ne 34) { throw 'Filter predicates/count differ from fixture Metadata.' }
    $checks.Add('combined filters including zero duration boundary')
    $id = [string]$page.results[0].id
    $edited = Invoke-Library @('edit',$id,'--tags','rustprobealpha 喷气发动机','--notes','中文航空 Metadata 验证','--color','#66DDBB','--favorite','true')
    if (!$edited.favorite) { throw 'Annotation update did not persist.' }
    $chinese = Invoke-Library @('search','喷气发动机')
    if ($chinese.total_count -ne 1) { throw 'Chinese audio terminology failed to recall the annotated file.' }
    $favorites = Invoke-Library @('search','--favorites')
    if ($favorites.total_count -ne 1) { throw 'Favorite filter failed.' }
    $annotationFilter = Invoke-Library @('search','--tag','喷气发动机','--color','#66ddbb','--favorites')
    if ($annotationFilter.total_count -ne 1) { throw 'Annotation filters/count failed.' }
    $checks.Add('Chinese recall, Metadata persistence and favorites')
    $null = Invoke-Library @('edit',$id,'--tags','rustprobebeta 喷气发动机')
    $stale = Invoke-Library @('search','rustprobealpha')
    $current = Invoke-Library @('search','rustprobebeta')
    if ($stale.total_count -ne 0 -or $current.total_count -ne 1) { throw 'FTS5 update retained stale terms.' }
    $checks.Add('FTS5 update removes old terms')
    $scanTimer = [Diagnostics.Stopwatch]::StartNew()
    $first = Invoke-Library @('scan',$fixtureRoot,'--full')
    $scanTimer.Stop()
    if ($first.errors -ne 0 -or $first.processed -ne 34 -or $first.state -ne 'completed') { throw 'Full fixture scan failed.' }
    $second = Invoke-Library @('scan',$fixtureRoot)
    if ($second.added -ne 0 -or $second.updated -ne 0 -or $second.unchanged -ne 34) { throw 'Unchanged incremental scan incorrectly rewrote records.' }
    $reopened = Invoke-Library @('info',$id)
    if ($reopened.attributes.wav_metadata.fields.category -ne 'Vehicles' -or $reopened.attributes.wav_metadata.fields.subcategory -ne 'Aircraft' -or $reopened.attributes.wav_metadata.fields.manufacturer -ne 'BOOM Library') { throw 'Real BOOM SMED Metadata was not decoded.' }
    $aircraft = Invoke-Library @('search','Aircraft')
    if ($aircraft.total_count -ne 34) { throw 'Real SMED categories are not searchable for all 34 fixtures.' }
    $checks.Add('34 real BOOM SMED records decode professional fields and index their aircraft category')
    if (!$reopened.favorite -or $reopened.notes -ne '中文航空 Metadata 验证') { throw 'Rescan overwrote library annotations.' }
    $checks.Add('full/incremental scan and annotation preservation')
    $stats = Invoke-Library @('stats')
    if ($stats.audio_files -ne 34) { throw 'Rescan introduced duplicate source paths.' }
    $checks.Add('Windows canonical path identity avoids duplicates')
    if ((Get-FileHash -LiteralPath $sourceDatabase -Algorithm SHA256).Hash -ne $originalHash) { throw 'Original library was modified.' }
    [PSCustomObject]@{ success=$true; checks=$checks; records=$stats.audio_files; original_database_unchanged=$true; full_scan_ms=$scanTimer.Elapsed.TotalMilliseconds; full_scan_files_per_second=34/$scanTimer.Elapsed.TotalSeconds; scan_timing_includes_cli_startup=$true } | ConvertTo-Json -Depth 4
} finally {
    $env:AUDIOLIB_DATA_DIR = $savedDataRoot
    # Test-owned paths only, resolved and constrained before recursive deletion.
    $resolvedTestRoot = [IO.Path]::GetFullPath($validationRoot)
    if ($resolvedTestRoot.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolvedTestRoot)) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}
} finally {
    Restore-AudioLibConsoleEncoding
}
