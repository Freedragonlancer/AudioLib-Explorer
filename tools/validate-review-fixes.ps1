# tools/validate-review-fixes.ps1
[CmdletBinding()]
param([string]$BinaryDirectory = 'target/x86_64-pc-windows-msvc/release')
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$binaryRoot = [IO.Path]::GetFullPath($(if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRoot $BinaryDirectory }))
$cli = Join-Path $binaryRoot 'AudioLibCli.exe'
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar)
$testRoot = [IO.Path]::GetFullPath((Join-Path $tempParent ('AudioLibReviewFixes-' + [guid]::NewGuid().ToString('N'))))
$database = Join-Path $testRoot 'library.db'
$checks = [Collections.Generic.List[string]]::new()
$ownedProcesses = [Collections.Generic.List[Diagnostics.Process]]::new()
$savedDataRoot = $env:AUDIOLIB_DATA_DIR
$sourceDatabase = Join-Path $taskRoot 'data/audiolib.db'
$sourceHash = (Get-FileHash -LiteralPath $sourceDatabase -Algorithm SHA256).Hash
function New-ReviewWav([string]$Path, [string]$Description = '') {
    $stream = [IO.MemoryStream]::new()
    $writer = [IO.BinaryWriter]::new($stream)
    try {
        $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([uint32]0)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
        $writer.Write([uint32]16)
        $writer.Write([uint16]1)
        $writer.Write([uint16]1)
        $writer.Write([uint32]48000)
        $writer.Write([uint32]96000)
        $writer.Write([uint16]2)
        $writer.Write([uint16]16)
        if ($Description) {
            $writer.Write([Text.Encoding]::ASCII.GetBytes('bext'))
            $writer.Write([uint32]602)
            $bext = [byte[]]::new(602)
            [Text.Encoding]::ASCII.GetBytes($Description).CopyTo($bext,0)
            $writer.Write($bext)
        }
        $writer.Write([Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([uint32]256)
        $writer.Write([byte[]]::new(256))
        if ($Description) {
            # Real-world libraries often append public Metadata after the audio data.
            $infoStream = [IO.MemoryStream]::new()
            $infoWriter = [IO.BinaryWriter]::new($infoStream)
            try {
                $infoWriter.Write([Text.Encoding]::ASCII.GetBytes('INFOICMT'))
                $comment = [Text.Encoding]::UTF8.GetBytes("reviewinfodescription 木门碰撞`0")
                $infoWriter.Write([uint32]$comment.Length)
                $infoWriter.Write($comment)
                if ($comment.Length % 2) { $infoWriter.Write([byte]0) }
                $infoWriter.Flush()
                $info = $infoStream.ToArray()
                $writer.Write([Text.Encoding]::ASCII.GetBytes('LIST'))
                $writer.Write([uint32]$info.Length)
                $writer.Write($info)
            } finally { $infoWriter.Dispose() }
            $xml = [Text.Encoding]::UTF8.GetBytes('<BWFXML><USER><CATID>DOORWood</CATID><DESIGNER>reviewfoleydesigner</DESIGNER></USER></BWFXML>')
            $writer.Write([Text.Encoding]::ASCII.GetBytes('iXML'))
            $writer.Write([uint32]$xml.Length)
            $writer.Write($xml)
            if ($xml.Length % 2) { $writer.Write([byte]0) }
        }
        $writer.Flush()
        $stream.Position = 4
        $writer.Write([uint32]($stream.Length - 8))
        $writer.Flush()
        [IO.File]::WriteAllBytes($Path,$stream.ToArray())
    } finally { $writer.Dispose() }
}
function Invoke-ReviewLibrary([string[]]$Arguments) {
    $reply = (& $cli --database $database @Arguments | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$reply.success) { throw "Review regression failed: $($reply.error)" }
    return $reply.data
}
function Assert-RejectedReview([string[]]$Arguments) {
    $reply = (& $cli @Arguments | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -eq 0 -or $reply.success) { throw "Expected safe CLI rejection: $($Arguments -join ' ')" }
}
try {
    if (!$testRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Review test path escaped its owned temporary root.' }
    New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
    $env:AUDIOLIB_DATA_DIR = Join-Path $testRoot 'userdata'
    New-Item -ItemType Directory -Force -Path $env:AUDIOLIB_DATA_DIR | Out-Null
    Copy-Item -LiteralPath (Join-Path $taskRoot 'config/rust_settings.example.json') -Destination (Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json')
    $null = Invoke-ReviewLibrary @('db','create',$database)
    $legacyCopy = Join-Path $testRoot 'legacy-source.db'
    Copy-Item -LiteralPath $sourceDatabase -Destination $legacyCopy
    $legacyCopyHash = (Get-FileHash -LiteralPath $legacyCopy -Algorithm SHA256).Hash
    Assert-RejectedReview @('--database',$legacyCopy,'stats')
    if ((Get-FileHash -LiteralPath $legacyCopy -Algorithm SHA256).Hash -ne $legacyCopyHash) { throw 'Ordinary CLI statistics migrated an existing legacy source in place.' }
    $absentExplicit = Join-Path $testRoot 'explicit-absent.db'
    Assert-RejectedReview @('--database',$absentExplicit,'stats')
    if (Test-Path -LiteralPath $absentExplicit) { throw 'Ordinary CLI statistics created an explicit missing database.' }
    $settingsPath = Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json'
    $originalSettings = [IO.File]::ReadAllBytes($settingsPath)
    $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    $settings.active_database = Join-Path $testRoot 'selected-absent.db'
    try {
        $settings | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $settingsPath -Encoding UTF8
        Assert-RejectedReview @('stats')
        if (Test-Path -LiteralPath $settings.active_database) { throw 'Ordinary CLI statistics recreated a missing selected database.' }
    } finally { [IO.File]::WriteAllBytes($settingsPath,$originalSettings) }
    $checks.Add('ordinary CLI rejects legacy and explicit/selected missing databases without modifying or creating them')
    $parent = Join-Path $testRoot 'parent'
    $child = Join-Path $parent 'child'
    New-Item -ItemType Directory -Force -Path $child | Out-Null
    $wav = Join-Path $child 'one.wav'
    New-ReviewWav $wav 'review_stale_bwf_description'
    $null = Invoke-ReviewLibrary @('scan',$child,'--full')
    $null = Invoke-ReviewLibrary @('scan',$parent,'--full')
    $folders = Invoke-ReviewLibrary @('folders')
    $parentFolder = $folders | Where-Object { $_.name -eq 'parent' } | Select-Object -First 1
    if (!$parentFolder) { throw 'Parent directory did not become a root after child-first import.' }
    $recursive = Invoke-ReviewLibrary @('search','--folder',[string]$parentFolder.id)
    if ($recursive.total_count -ne 1) { throw 'Child-first import still loses the parent recursive result.' }
    $checks.Add('child-first then parent scan repairs parent and ancestor links')
    $id = [string]$recursive.results[0].id
    $embedded = Invoke-ReviewLibrary @('info',$id)
    if ($embedded.attributes.wav_metadata.fields.cat_id -ne 'DOORWood' -or $embedded.attributes.wav_metadata.fields.description -ne 'reviewinfodescription 木门碰撞') { throw 'Professional source Metadata was not extracted from INFO/iXML.' }
    foreach ($keyword in @('reviewinfodescription','reviewfoleydesigner','DOORWood','木门')) {
        $found = Invoke-ReviewLibrary @('search',$keyword)
        if ($found.total_count -ne 1) { throw "Embedded Metadata is not searchable: $keyword" }
    }
    $checks.Add('INFO/iXML professional fields after audio are extracted and searchable in English/Chinese')
    $null = Invoke-ReviewLibrary @('edit',$id,'--tags','ÉCHO 航空','--notes','preserve user annotations','--favorite','true')
    $unicode = Invoke-ReviewLibrary @('search','--tag','écho')
    if ($unicode.total_count -ne 1) { throw 'Unicode case-insensitive tag filter failed.' }
    $checks.Add('Unicode literal tag case-insensitive filter')
    New-ReviewWav $wav
    $null = Invoke-ReviewLibrary @('scan',$parent,'--full')
    $cleared = Invoke-ReviewLibrary @('info',$id)
    $stale = Invoke-ReviewLibrary @('search','review_stale_bwf_description')
    if ($cleared.bext_description -or $stale.total_count -ne 0) { throw 'Removed BWF description remains in Metadata/FTS5.' }
    foreach ($keyword in @('reviewinfodescription','reviewfoleydesigner','DOORWood','木门')) {
        $staleEmbedded = Invoke-ReviewLibrary @('search',$keyword)
        if ($staleEmbedded.total_count -ne 0) { throw "Removed embedded Metadata remains searchable: $keyword" }
    }
    if (!$cleared.favorite -or $cleared.notes -ne 'preserve user annotations') { throw 'Source Metadata refresh damaged user annotations.' }
    $checks.Add('removed BWF Metadata leaves FTS5 while annotations survive')
    $outsideRoot = Join-Path $testRoot 'outside'
    New-Item -ItemType Directory -Path $outsideRoot | Out-Null
    $outsideWav = Join-Path $outsideRoot 'outside.wav'
    New-ReviewWav $outsideWav
    $forbiddenLink = Join-Path $child 'forbidden-link.wav'
    $permittedLink = Join-Path $child 'permitted-link.wav'
    New-Item -ItemType SymbolicLink -Path $forbiddenLink -Target $outsideWav | Out-Null
    New-Item -ItemType SymbolicLink -Path $permittedLink -Target $wav | Out-Null
    $settings.allowed_scan_roots = @($parent)
    $settings.active_database = $null
    try {
        $settings | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $settingsPath -Encoding UTF8
        Assert-RejectedReview @('--database',$database,'relink',$id,$outsideWav)
        Assert-RejectedReview @('--database',$database,'relink',$id,$forbiddenLink)
        $same = Invoke-ReviewLibrary @('info',$id)
        if ($same.file_path -ne $cleared.file_path -or $same.tags -ne $cleared.tags -or $same.notes -ne $cleared.notes) { throw 'Rejected relink changed the original record.' }
        $permitted = Invoke-ReviewLibrary @('relink',$id,$permittedLink)
        if ($permitted.file_path -ne $cleared.file_path -or $permitted.tags -ne $cleared.tags) { throw 'Allowed symlink relink did not preserve the real target and annotations.' }
    } finally { [IO.File]::WriteAllBytes($settingsPath,$originalSettings) }
    $checks.Add('relink validates real symlink targets against allowed roots and retains annotations')
    $badConfig = Join-Path $testRoot 'config'
    New-Item -ItemType Directory -Force -Path $badConfig | Out-Null
    Set-Content -LiteralPath (Join-Path $badConfig 'audiolib_config.yaml') -Value 'broken: [' -Encoding UTF8
    Push-Location -LiteralPath $testRoot
    try { $null = Invoke-ReviewLibrary @('stats') } finally { Pop-Location }
    $checks.Add('saved Rust settings ignore malformed legacy YAML')
    $overlapRoot = Join-Path $testRoot 'overlap'
    New-Item -ItemType Directory -Force -Path $overlapRoot | Out-Null
    $fixtureCount = 2000
    for ($index=0;$index -lt $fixtureCount;$index++) {
        [IO.File]::Copy($wav,(Join-Path $overlapRoot ('fixture-' + $index.ToString('D4') + '.wav')))
    }
    for ($index=0;$index -lt 2;$index++) {
        $process = Start-Process -FilePath $cli -WorkingDirectory $testRoot -WindowStyle Hidden -ArgumentList @('--database',('"' + $database + '"'),'scan',('"' + $overlapRoot + '"'),'--full') -RedirectStandardOutput (Join-Path $testRoot ('scan-' + $index + '.json')) -RedirectStandardError (Join-Path $testRoot ('scan-' + $index + '.stderr')) -PassThru
        $ownedProcesses.Add($process)
    }
    $replies = @()
    foreach ($process in $ownedProcesses) {
        if (!$process.WaitForExit(30000)) { throw 'Concurrent scan regression timed out.' }
    }
    for ($index=0;$index -lt 2;$index++) {
        $replies += Get-Content -LiteralPath (Join-Path $testRoot ('scan-' + $index + '.json')) -Raw | ConvertFrom-Json
    }
    $succeeded = @($replies | Where-Object { $_.success })
    $rejected = @($replies | Where-Object { !$_.success })
    if ($succeeded.Count -ne 1 -or $rejected.Count -ne 1 -or $rejected[0].error -notmatch 'scan|扫描') { throw 'Concurrent CLI scans did not return one completion and one explicit scan-busy failure.' }
    $visible = Invoke-ReviewLibrary @('search','fixture','--limit','1')
    if ($visible.total_count -ne $fixtureCount) { throw 'Overlapping scan attempts hid existing WAV records.' }
    $checks.Add('cross-process overlapping full scan is rejected without hiding records')
    $followup = Invoke-ReviewLibrary @('scan',$overlapRoot)
    if ($followup.state -ne 'completed' -or $followup.unchanged -ne $fixtureCount) { throw 'Scan lock was not released after completion.' }
    $checks.Add('OS scan lock is released for the next incremental scan')
    $firstLaunch = Join-Path $testRoot 'first-launch'
    $firstConfig = Join-Path $firstLaunch 'config'
    New-Item -ItemType Directory -Force -Path $firstConfig | Out-Null
    $firstYaml = Join-Path $firstConfig 'audiolib_config.yaml'
    $legacyFixture = "application:`n  language: cn`ndatabase:`n  path: '" + $database.Replace('\','/') + "'`nplayback:`n  default_volume: 0.25`n"
    Set-Content -LiteralPath $firstYaml -Value $legacyFixture -Encoding UTF8
    $env:AUDIOLIB_DATA_DIR = Join-Path $testRoot 'first-launch-userdata'
    Push-Location -LiteralPath $firstLaunch
    try {
        $firstReply = (& $cli stats | Out-String) | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or !$firstReply.success) { throw 'First CLI startup did not import the isolated legacy fixture.' }
        $saved = Get-Content -LiteralPath (Join-Path $env:AUDIOLIB_DATA_DIR 'settings.json') -Raw | ConvertFrom-Json
        if ($saved.language -ne 'cn' -or [Math]::Abs($saved.playback.volume - 0.25) -gt 0.0001) { throw 'One-time CLI legacy preferences were not persisted.' }
        Set-Content -LiteralPath $firstYaml -Value 'broken: [' -Encoding UTF8
        $repeat = (& $cli stats | Out-String) | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or !$repeat.success) { throw 'Initialized CLI library still depends on old YAML validity.' }
    } finally { Pop-Location }
    $checks.Add('first CLI launch persists inherited preferences and later ignores legacy YAML')
    if ((Get-FileHash -LiteralPath $sourceDatabase -Algorithm SHA256).Hash -ne $sourceHash) { throw 'Original database changed during review regression.' }
    $report = [PSCustomObject]@{ success=$true; checks=$checks; overlapping_fixture_records=$fixtureCount; source_database_unchanged=$true; audio_output_opened=$false }
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $taskRoot 'doc/Rust_Review_Regression.json') -Encoding UTF8
    $report | ConvertTo-Json -Depth 5
} finally {
    foreach ($process in $ownedProcesses) {
        if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
        $process.Dispose()
    }
    $env:AUDIOLIB_DATA_DIR = $savedDataRoot
    if ((Test-Path -LiteralPath $testRoot) -and $testRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
