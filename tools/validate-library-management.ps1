# tools/validate-library-management.ps1
[CmdletBinding()]
param([string]$BinaryDirectory = 'target/x86_64-pc-windows-msvc/release')
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$binaryRoot = [IO.Path]::GetFullPath($(if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRoot $BinaryDirectory }))
$cli = Join-Path $binaryRoot 'AudioLibCli.exe'
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar)
$ownedName = 'AudioLibManagement-' + [guid]::NewGuid().ToString('N')
$testRoot = [IO.Path]::GetFullPath((Join-Path $tempParent $ownedName))
$database = Join-Path $testRoot 'library.db'
$savedDataRoot = $env:AUDIOLIB_DATA_DIR
$checks = [Collections.Generic.List[string]]::new()
$ownsDirectory = $false

function Assert-OwnedPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if ($absolute -ne $testRoot -and !$absolute.StartsWith($testRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Validation path escaped its owned directory: $absolute"
    }
}
function Assert-Check([bool]$Condition, [string]$Message) {
    if (!$Condition) { throw $Message }
}
function New-SilentWav([string]$Path) {
    Assert-OwnedPath $Path
    $stream = [IO.MemoryStream]::new()
    $writer = [IO.BinaryWriter]::new($stream)
    try {
        $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([uint32]292)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
        $writer.Write([uint32]16)
        $writer.Write([uint16]1)
        $writer.Write([uint16]1)
        $writer.Write([uint32]48000)
        $writer.Write([uint32]96000)
        $writer.Write([uint16]2)
        $writer.Write([uint16]16)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([uint32]256)
        $writer.Write([byte[]]::new(256))
        $writer.Flush()
        [IO.File]::WriteAllBytes($Path, $stream.ToArray())
    } finally { $writer.Dispose() }
}
function Invoke-Library([string[]]$Arguments, [string]$Library = $database, [switch]$FailureExpected, [switch]$WithoutDatabase) {
    if (!$WithoutDatabase) { Assert-OwnedPath $Library }
    $stderrPath = Join-Path $testRoot ('cli-' + [guid]::NewGuid().ToString('N') + '.stderr')
    $commandArguments = @('--format', 'json')
    if (!$WithoutDatabase) { $commandArguments += @('--database', $Library) }
    $commandArguments += $Arguments
    $stdout = & $cli @commandArguments 2>$stderrPath | Out-String
    $exitCode = $LASTEXITCODE
    try { $reply = $stdout | ConvertFrom-Json } catch { throw "CLI returned invalid JSON for $($Arguments -join ' '): $stdout" }
    if ($FailureExpected) {
        Assert-Check ($exitCode -ne 0 -and !$reply.success) "Expected command rejection: $($Arguments -join ' ')"
        return $reply
    }
    if ($exitCode -ne 0 -or !$reply.success) {
        throw "Library management failed: $($Arguments -join ' '): $($reply.error); $(Get-Content -LiteralPath $stderrPath -Raw)"
    }
    return $reply.data
}
function Assert-Annotations($File) {
    Assert-Check ($File.tags -match 'beta' -and $File.tags -notmatch 'alpha') 'Append/remove tags did not survive this operation.'
    Assert-Check ($File.notes -eq 'private, notes' -and $File.favorite -and $File.color -eq '#123456') 'Annotations were not preserved.'
}
function Normalize-Path([string]$Path) { return [IO.Path]::GetFullPath($Path.Replace('\\?\', '')) }

try {
    Assert-Check ($testRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) 'Owned temporary directory escaped the system temporary parent.'
    Assert-Check ((Split-Path -Leaf $testRoot) -eq $ownedName) 'Owned directory identity changed.'
    Assert-Check (!(Test-Path -LiteralPath $testRoot)) 'Owned temporary directory already exists.'
    Assert-Check (Test-Path -LiteralPath $cli -PathType Leaf) 'Build the release AudioLibCli.exe before running this validation.'
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $ownsDirectory = $true
    $env:AUDIOLIB_DATA_DIR = Join-Path $testRoot 'userdata'
    New-Item -ItemType Directory -Path $env:AUDIOLIB_DATA_DIR | Out-Null
    # Saved preferences suppress legacy discovery, including the repository's real database.
    @{ active_database=$database; language='en'; playback=@{ backend='wasapi'; volume=0.8 }; allowed_scan_roots=@() } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json') -Encoding UTF8
    $null = Invoke-Library @('db', 'create', $database)
    Assert-Check ((Invoke-Library @('stats')).audio_files -eq 0) 'New library was not empty.'
    $checks.Add('create empty library with isolated saved preferences')

    $audioRoot = Join-Path $testRoot 'ÉCHO'
    $relocatedRoot = Join-Path $testRoot 'relocated'
    New-Item -ItemType Directory -Path $audioRoot,$relocatedRoot | Out-Null
    $fileA = Join-Path $audioRoot 'first.wav'
    $fileB = Join-Path $audioRoot 'second.wav'
    $relocated = Join-Path $relocatedRoot 'second-renamed.wav'
    New-SilentWav $fileA
    New-SilentWav $fileB
    $sourceHashA = (Get-FileHash -LiteralPath $fileA -Algorithm SHA256).Hash
    $sourceHashB = (Get-FileHash -LiteralPath $fileB -Algorithm SHA256).Hash
    $initial = Invoke-Library @('scan', $audioRoot, '--full', '--no-recursive')
    Assert-Check ($initial.added -eq 2 -and $initial.errors -eq 0) 'Initial two-file Unicode scan failed.'
    $all = Invoke-Library @('search', '--visibility', 'all')
    $idA = [string]($all.results | Where-Object { $_.filename -eq 'first.wav' }).id
    $idB = [string]($all.results | Where-Object { $_.filename -eq 'second.wav' }).id
    Assert-Check ($idA -and $idB -and $idA -ne $idB) 'Initial files did not receive distinct IDs.'
    $ids = $idA + ',' + $idB
    $batch = Invoke-Library @('edit-batch', '--ids', $ids, '--tags', 'alpha', '--color', '#123456', '--notes', 'private, notes', '--favorite', 'true')
    Assert-Check ($batch.updated -eq 2) 'Batch annotation update did not affect both records.'
    $null = Invoke-Library @('edit-batch', '--ids', $ids, '--tags', 'beta', '--tag-mode', 'append')
    $null = Invoke-Library @('edit-batch', '--ids', $ids, '--tags', 'alpha', '--tag-mode', 'remove')
    Assert-Annotations (Invoke-Library @('info', $idA))
    Assert-Annotations (Invoke-Library @('info', $idB))
    Assert-Check ((Invoke-Library @('search', 'beta')).total_count -eq 2 -and (Invoke-Library @('search', 'alpha')).total_count -eq 0) 'Batch tag edits did not synchronize FTS5.'
    $checks.Add('atomic batch annotations and tag replace/append/remove')

    Assert-OwnedPath $fileB
    Assert-OwnedPath $relocated
    Move-Item -LiteralPath $fileB -Destination $relocated
    $missingScan = Invoke-Library @('scan', $audioRoot, '--full', '--no-recursive')
    $repeatScan = Invoke-Library @('scan', $audioRoot, '--full', '--no-recursive')
    Assert-Check ($missingScan.removed -eq 1 -and $repeatScan.removed -eq 0) 'Unicode nonrecursive reconciliation did not count new missing files exactly once.'
    Assert-Check ((Invoke-Library @('search', '--visibility', 'present')).total_count -eq 1) 'Present-file view is wrong.'
    $missing = Invoke-Library @('search', '--visibility', 'missing')
    Assert-Check ($missing.total_count -eq 1 -and $missing.results[0].id -eq $idB -and $missing.results[0].missing) 'Missing-file view lost its original ID.'
    Assert-Annotations $missing.results[0]
    $scanHistory = @(Invoke-Library @('scan-history'))
    Assert-Check (@($scanHistory | Where-Object { $_.id -eq $missingScan.session_id -and $_.removed -eq 1 }).Count -eq 1) 'Scan history did not persist removed count.'
    Assert-Check (@(Invoke-Library @('play-history')).Count -eq 0) 'A library-only validation unexpectedly recorded playback.'
    $checks.Add('Unicode full reconciliation, present/missing/all visibility, scan and empty play histories')

    $backupPath = Join-Path $testRoot 'backup.db'
    $backup = Invoke-Library @('db', 'backup', $backupPath)
    Assert-Check ($backup.total_records -eq 2 -and (Test-Path -LiteralPath $backupPath)) 'Backup did not contain the entire library.'
    $backupRows = Invoke-Library @('search', '--visibility', 'all') -Library $backupPath
    Assert-Check ($backupRows.total_count -eq 2 -and @($backupRows.results | Where-Object { $_.missing }).Count -eq 1) 'Backup lost missing state.'
    foreach ($row in $backupRows.results) { Assert-Annotations $row }
    $null = Invoke-Library @('edit', $idA, '--tags', 'changed')
    $null = Invoke-Library @('remove', $idB, '--confirm')
    $restore = Invoke-Library @('db', 'restore', $backupPath, '--replace')
    Assert-OwnedPath $restore.rollback_backup
    Assert-Check (Test-Path -LiteralPath $restore.rollback_backup) 'Restore did not create its automatic rollback backup.'
    $restoredRows = Invoke-Library @('search', '--visibility', 'all')
    Assert-Check ($restoredRows.total_count -eq 2 -and @($restoredRows.results | Where-Object { $_.missing }).Count -eq 1) 'Restore did not restore missing records.'
    foreach ($row in $restoredRows.results) { Assert-Annotations $row }
    $rollbackRows = Invoke-Library @('search', '--visibility', 'all') -Library $restore.rollback_backup
    Assert-Check ($rollbackRows.total_count -eq 1 -and $rollbackRows.results[0].tags -eq 'changed') 'Automatic rollback backup does not contain the state before restore.'
    $corrupt = Join-Path $testRoot 'corrupt.db'
    [IO.File]::WriteAllBytes($corrupt, [Text.Encoding]::ASCII.GetBytes('not a SQLite database'))
    $null = Invoke-Library @('db', 'restore', $corrupt, '--replace') -FailureExpected
    $afterRejectedRestore = Invoke-Library @('search', '--visibility', 'all')
    Assert-Check ($afterRejectedRestore.total_count -eq 2) 'Rejected corrupt restore damaged the active library.'
    foreach ($row in $afterRejectedRestore.results) { Assert-Annotations $row }
    $checks.Add('backup includes missing/annotations, restore creates a usable rollback, corrupt restore preserves current library')

    $integrity = Invoke-Library @('db', 'check')
    Assert-Check ($integrity.sqlite_ok -and $integrity.fts_ok -and $integrity.foreign_key_violations -eq 0 -and @($integrity.errors).Count -eq 0) 'Library integrity check failed.'
    foreach ($action in @('optimize', 'rebuild-search')) {
        $maintenance = Invoke-Library @('db', $action)
        Assert-OwnedPath $maintenance.rollback_backup
        Assert-Check ((Test-Path -LiteralPath $maintenance.rollback_backup) -and $maintenance.integrity.sqlite_ok -and $maintenance.integrity.fts_ok) "Maintenance $action failed integrity or rollback checks."
    }
    Assert-Check ((Invoke-Library @('search', 'beta', '--visibility', 'all')).total_count -eq 2 -and (Invoke-Library @('search', 'beta', '--visibility', 'present')).total_count -eq 1) 'Maintenance damaged indexed annotations or visibility filtering.'
    $checks.Add('integrity check and rollback-protected optimize/rebuild-search')

    $jsonl = Join-Path $testRoot 'library.jsonl'
    $csv = Join-Path $testRoot 'library.csv'
    Assert-Check ((Invoke-Library @('export', $jsonl, '--export-format', 'jsonl')).records -eq 2) 'JSONL export was not a whole-library export.'
    Assert-Check ((Invoke-Library @('export', $csv, '--export-format', 'csv')).records -eq 2) 'CSV export was not a whole-library export.'
    $exportRows = @(Get-Content -LiteralPath $jsonl | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
    Assert-Check ($exportRows.Count -eq 2 -and @($exportRows | Where-Object { $_.missing }).Count -eq 1) 'JSONL export omitted missing records.'
    foreach ($row in $exportRows) { Assert-Annotations $row }
    $csvRows = @(Import-Csv -LiteralPath $csv)
    Assert-Check ($csvRows.Count -eq 2 -and @($csvRows | Where-Object { $_.missing -eq 'true' }).Count -eq 1) 'CSV export omitted missing records.'
    foreach ($row in $csvRows) {
        Assert-Check ($row.notes -eq 'private, notes' -and $row.favorite -eq 'true') 'CSV export lost annotation columns or quoting.'
    }
    foreach ($annotationFile in @($jsonl, $csv)) {
        $null = Invoke-Library @('edit-batch', '--ids', $ids, '--tags', 'changed', '--notes', 'changed', '--favorite', 'false')
        $imported = Invoke-Library @('import-annotations', $annotationFile, '--replace-annotations')
        Assert-Check ($imported.records -eq 2 -and $imported.skipped -eq 0) 'Annotation import did not restore both paths.'
        Assert-Annotations (Invoke-Library @('info', $idA))
        Assert-Annotations (Invoke-Library @('info', $idB))
    }
    $checks.Add('whole-library JSONL/CSV export and path-matched annotation import')

    $relinked = Invoke-Library @('relink', $idB, $relocated)
    Assert-Check ($relinked.id -eq $idB -and !$relinked.missing -and (Normalize-Path $relinked.file_path) -eq (Normalize-Path $relocated)) 'Relink did not retain ID or activate the relocated WAV.'
    Assert-Annotations $relinked
    Assert-Check ((Invoke-Library @('search', '--visibility', 'present')).total_count -eq 2) 'Relink did not restore search visibility.'
    $checks.Add('relink moved WAV preserves ID and annotations')
    $removed = Invoke-Library @('remove', $idB, '--confirm')
    Assert-Check ($removed.removed -eq 1 -and (Test-Path -LiteralPath $relocated)) 'Record removal deleted its source WAV or returned the wrong count.'
    $folder = @(Invoke-Library @('folders')) | Where-Object { $_.name -eq 'ÉCHO' } | Select-Object -First 1
    Assert-Check ($null -ne $folder) 'Unicode root folder disappeared before explicit folder removal.'
    $folderRemoval = Invoke-Library @('remove-folder', [string]$folder.id, '--confirm')
    Assert-Check ($folderRemoval.removed -eq 1 -and (Test-Path -LiteralPath $fileA)) 'Folder removal deleted its source WAV or returned the wrong count.'
    Assert-Check ((Invoke-Library @('search', '--visibility', 'all')).total_count -eq 0) 'Confirmed record/folder removal left library records behind.'
    Assert-Check ((Get-FileHash -LiteralPath $fileA -Algorithm SHA256).Hash -eq $sourceHashA -and (Get-FileHash -LiteralPath $relocated -Algorithm SHA256).Hash -eq $sourceHashB) 'Management operations changed source WAV bytes.'
    $checks.Add('record/folder removals affect only the library; source WAV bytes remain intact')

    $badRoot = Join-Path $testRoot 'broken'
    New-Item -ItemType Directory -Path $badRoot | Out-Null
    [IO.File]::WriteAllBytes((Join-Path $badRoot 'wrong-header.wav'), [Text.Encoding]::ASCII.GetBytes('not a WAV payload'))
    [IO.File]::WriteAllBytes((Join-Path $badRoot 'truncated.wav'), [byte[]](82,73,70,70,36,0,0,0,87,65,86,69,102,109,116,32,16,0,0,0))
    $errors = Invoke-Library @('scan', $badRoot, '--full')
    $issuePaths = @($errors.error_details | ForEach-Object { $_.path } | Select-Object -Unique)
    $issueReasons = @($errors.error_details | ForEach-Object { $_.message } | Select-Object -Unique)
    Assert-Check ($errors.errors -eq 2 -and $errors.errors_truncated -eq 0 -and $issuePaths.Count -eq 2 -and $issueReasons.Count -eq 2) 'Two corrupt WAVs did not retain distinct paths and reasons.'
    $history = @(Invoke-Library @('scan-history')) | Where-Object { $_.id -eq $errors.session_id }
    Assert-Check ($history.errors -eq 2 -and @($history.error_details).Count -eq 2) 'Corrupt-file error details did not persist in scan history.'
    $checks.Add('two corrupt WAVs retain path/reason details in live status and scan history')

    $selectedDb = Join-Path $testRoot 'selected.db'
    $null = Invoke-Library @('db', 'create', $selectedDb)
    $selected = Invoke-Library @('db', 'use', $selectedDb)
    Assert-Check ((Normalize-Path $selected.database) -eq (Normalize-Path $selectedDb)) 'CLI db use did not select the requested library.'
    $preferences = Get-Content -LiteralPath (Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json') -Raw | ConvertFrom-Json
    Assert-Check ((Normalize-Path $preferences.active_database) -eq (Normalize-Path $selectedDb)) 'Selected library was not persisted in isolated settings.'
    $status = Invoke-Library @('db', 'status') -WithoutDatabase
    Assert-Check ((Normalize-Path $status.database) -eq (Normalize-Path $selectedDb) -and $status.stats.audio_files -eq 0 -and (Invoke-Library @('stats') -WithoutDatabase).audio_files -eq 0) 'Fresh CLI process did not reuse the selected empty library.'
    $checks.Add('db create/use persist the active library for subsequent CLI launches')

    $report = [PSCustomObject]@{
        success=$true
        checked_at_utc=[DateTime]::UtcNow.ToString('o')
        checks=$checks
        fixture_records=2
        source_wav_bytes_unchanged=$true
        real_library_opened=$false
        audio_output_opened=$false
        cli_sha256=(Get-FileHash -LiteralPath $cli -Algorithm SHA256).Hash
    }
    $report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $taskRoot 'doc/Rust_Library_Management.json') -Encoding UTF8
    $report | ConvertTo-Json -Depth 6
} finally {
    $env:AUDIOLIB_DATA_DIR = $savedDataRoot
    if ($ownsDirectory -and (Test-Path -LiteralPath $testRoot)) {
        $resolved = [IO.Path]::GetFullPath((Get-Item -LiteralPath $testRoot).FullName)
        if ($resolved -ne $testRoot -or (Split-Path -Leaf $resolved) -ne $ownedName -or !$resolved.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Refusing cleanup outside the verified owned validation directory.'
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
