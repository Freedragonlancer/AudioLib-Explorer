# tools/validate-playback-ipc.ps1
[CmdletBinding()]
param([string]$BinaryDirectory = 'target/x86_64-pc-windows-msvc/release')
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$binaryRoot = if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRoot $BinaryDirectory }
$cli = Join-Path $binaryRoot 'AudioLibCli.exe'
$desktop = Join-Path $binaryRoot 'AudioLibExplorer.exe'
$previousRoot = $env:AUDIOLIB_DATA_DIR
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar)
$testRoot = [IO.Path]::GetFullPath((Join-Path $temporaryRoot ('AudioLibPlayback-' + [guid]::NewGuid().ToString('N'))))
$ownedProcess = $null
$checks = [Collections.Generic.List[string]]::new()
function Invoke-Control([string[]]$Arguments) {
    $reply = (& $cli @Arguments | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$reply.success) { throw "Playback command failed: $($reply.error)" }
    return $reply.data
}
try {
    $absent = (& $cli status | Out-String) | ConvertFrom-Json
    if ($absent.success) { throw 'Close the existing desktop instance before this isolated playback test.' }
    $checks.Add('absent desktop returns explicit failure')
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $env:AUDIOLIB_DATA_DIR = Join-Path $testRoot 'userdata'
    $wavPath = Join-Path $testRoot 'silent-192k.wav'
    $stream = [IO.File]::Create($wavPath)
    $writer = [IO.BinaryWriter]::new($stream)
    try {
        $rate = 192000
        $dataSize = $rate * 5 * 8
        $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([uint32]($dataSize + 36))
        $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
        $writer.Write([uint32]16)
        $writer.Write([uint16]3)
        $writer.Write([uint16]2)
        $writer.Write([uint32]$rate)
        $writer.Write([uint32]($rate * 8))
        $writer.Write([uint16]8)
        $writer.Write([uint16]32)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([uint32]$dataSize)
        $writer.Write([byte[]]::new($dataSize))
    } finally { $writer.Dispose() }
    $screenshot = Join-Path $testRoot 'ipc-ui.png'
    $ownedProcess = Start-Process -FilePath $desktop -WorkingDirectory $taskRoot -WindowStyle Hidden -ArgumentList @('--smoke-ui','--smoke-duration-ms','24000','--screenshot',('"' + $screenshot + '"')) -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    do {
        Start-Sleep -Milliseconds 100
        $ready = (& $cli status | Out-String) | ConvertFrom-Json
        if ($ownedProcess.HasExited) { throw 'Desktop exited before the local control server became available.' }
    } while (!$ready.success -and [DateTime]::UtcNow -lt $deadline)
    if (!$ready.success) { throw 'Local control server did not become ready.' }
    # Selection in UI smoke happens at 900 ms; finish it before this IPC load.
    Start-Sleep -Milliseconds 1000
    $libraryPath = Join-Path $testRoot 'userdata/library.db'
    $settingsPath = Join-Path $testRoot 'userdata/settings.json'
    $settingsHash = (Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash
    $secondError = Join-Path $testRoot 'second-desktop.stderr'
    $second = Start-Process -FilePath $desktop -WorkingDirectory $taskRoot -WindowStyle Hidden -ArgumentList @('--smoke-ui') -RedirectStandardError $secondError -PassThru
    try {
        if (!$second.WaitForExit(5000)) { throw 'A second desktop instance was allowed to keep running.' }
        if ($second.ExitCode -eq 0 -or (Get-Content -LiteralPath $secondError -Raw) -notmatch 'already running' -or
            (Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash -ne $settingsHash) {
            throw 'Duplicate desktop startup did not protect the first instance preferences.'
        }
    } finally {
        if (!$second.HasExited) { $second.Kill(); $second.WaitForExit() }
        $second.Dispose()
    }
    $checks.Add('duplicate desktop startup fails before modifying shared preferences')
    $blockedUse = (& $cli db use $libraryPath | Out-String) | ConvertFrom-Json
    $blockedUseExit = $LASTEXITCODE
    $blockedRestore = (& $cli db restore $libraryPath --replace | Out-String) | ConvertFrom-Json
    $blockedRestoreExit = $LASTEXITCODE
    if ($blockedUseExit -eq 0 -or $blockedUse.success -or $blockedUse.error -notmatch 'desktop library manager' -or
        $blockedRestoreExit -eq 0 -or $blockedRestore.success -or $blockedRestore.error -notmatch 'desktop library manager' -or
        (Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash -ne $settingsHash) {
        throw 'CLI selection/restore did not protect the running desktop and its preferences.'
    }
    $checks.Add('CLI library selection and restore reject a running desktop without changing preferences')
    $null = Invoke-Control @('volume','0')
    $playing = Invoke-Control @('play',$wavPath)
    if ($playing.state -ne 'playing' -or $playing.sample_rate -ne 192000) { throw 'Play did not return confirmed engine state.' }
    $checks.Add('192k float WAV starts through confirmed local control')
    Start-Sleep -Milliseconds 120
    $paused = Invoke-Control @('pause')
    Start-Sleep -Milliseconds 120
    $frozen = Invoke-Control @('status')
    if ($frozen.state -ne 'paused' -or [Math]::Abs($frozen.position - $paused.position) -gt 0.002) { throw 'Paused position continued advancing.' }
    $checks.Add('pause freezes audio position')
    $resumed = Invoke-Control @('play')
    if ($resumed.state -ne 'playing' -or $resumed.position -lt $paused.position -or $resumed.generation -ne $paused.generation) { throw 'Resume reloaded or rewound the source.' }
    $checks.Add('resume preserves source generation and position')
    $null = Invoke-Control @('pause')
    $seeked = Invoke-Control @('seek','2')
    if ([Math]::Abs($seeked.position - 2) -gt 0.002 -or $seeked.state -ne 'paused') { throw 'Seek lost the paused transport state or accurate position.' }
    $gain = Invoke-Control @('volume','0.37')
    if ([Math]::Abs($gain.volume - 0.37) -gt 0.0001) { throw 'Volume did not update confirmed engine state.' }
    $checks.Add('seek and volume use confirmed state')
    $changed = Invoke-Control @('audition','--pitch','12','--rate','0.5','--mono')
    if ($changed.state -ne 'paused' -or [Math]::Abs($changed.position - 2) -gt 0.002 -or
        $changed.audition.pitch_semitones -ne 12 -or $changed.audition.rate -ne 0.5 -or !$changed.audition.mono) { throw 'Pitch/Rate change lost paused source position or confirmed options.' }
    $invalid = (& $cli audition --rate 0 | Out-String) | ConvertFrom-Json
    if ($invalid.success -or $LASTEXITCODE -eq 0) { throw 'Invalid audition speed was accepted.' }
    $retained = Invoke-Control @('status')
    if ($retained.audition.rate -ne 0.5 -or $retained.state -ne 'paused') { throw 'Invalid audition speed modified the active transport.' }
    $checks.Add('independent Pitch/Rate and mono preserve paused seek; invalid input leaves transport unchanged')
    $loop = Invoke-Control @('audition','--pitch','7','--rate','4','--repeat')
    $null = Invoke-Control @('seek','4.95')
    $null = Invoke-Control @('play')
    Start-Sleep -Milliseconds 700
    $looped = Invoke-Control @('status')
    if ($looped.state -ne 'playing' -or $looped.position -ge 4.9 -or $looped.generation -ne $loop.generation) { throw 'Repeat did not wrap in source time while preserving the source generation.' }
    $null = Invoke-Control @('pause')
    $null = Invoke-Control @('audition')
    $checks.Add('repeat wraps at end with pitch-preserving 4x speed and unchanged source identity')

    $largest = Get-ChildItem -LiteralPath (Join-Path $taskRoot 'test_audiolib/P-51 Mustang') -Filter '*.wav' -Recurse -File | Sort-Object Length -Descending | Select-Object -First 1
    $memoryDelta = $null
    if ($largest) {
        $baselineMemory = (Get-Process -Id $ownedProcess.Id).WorkingSet64
        $null = Invoke-Control @('volume','0')
        $largePlayback = Invoke-Control @('play',$largest.FullName)
        Start-Sleep -Milliseconds 100
        $null = Invoke-Control @('pause')
        $largeMemory = (Get-Process -Id $ownedProcess.Id).WorkingSet64
        if ($baselineMemory -le 0 -or $largeMemory -le 0) { throw 'Windows process memory counters are unavailable.' }
        $memoryDelta = $largeMemory - $baselineMemory
        if ($largePlayback.duration -lt 100 -or $memoryDelta -gt 64MB) { throw 'Large WAV did not remain within the bounded streaming memory budget.' }
        $historyDeadline = [DateTime]::UtcNow.AddSeconds(2)
        do {
            $history = @(Invoke-Control @('play-history','--limit','100'))
            $actualHistory = @($history | Where-Object {
                [IO.Path]::GetFullPath($_.file_path).Replace('\\?\','') -eq $largest.FullName
            })
            if ($actualHistory.Count -eq 0) { Start-Sleep -Milliseconds 65 }
        } while ($actualHistory.Count -eq 0 -and [DateTime]::UtcNow -lt $historyDeadline)
        if ($history.Count -ne 1 -or $actualHistory.Count -ne 1) {
            throw 'Playback history did not resolve the actual IPC source or recorded the GUI-selected/unindexed file.'
        }
        $checks.Add('playback history resolves actual indexed IPC source while GUI selection differs')
        # The real fixture remains muted throughout these replay/history checks.
        $null = Invoke-Control @('play')
        Start-Sleep -Milliseconds 120
        $null = Invoke-Control @('pause')
        $continuedHistory = @(Invoke-Control @('play-history','--limit','100'))
        if ($continuedHistory.Count -ne 1) { throw 'Pause/resume duplicated the playback history session.' }
        $null = Invoke-Control @('stop')
        $replayed = Invoke-Control @('play')
        if ($replayed.generation -ne $largePlayback.generation) { throw 'Stop/replay unexpectedly reloaded the source.' }
        $replayDeadline = [DateTime]::UtcNow.AddSeconds(2)
        do {
            $replayedHistory = @(Invoke-Control @('play-history','--limit','100'))
            if ($replayedHistory.Count -lt 2) { Start-Sleep -Milliseconds 65 }
        } while ($replayedHistory.Count -lt 2 -and [DateTime]::UtcNow -lt $replayDeadline)
        if ($replayedHistory.Count -ne 2) { throw 'Stop/replay did not create exactly one new playback history session.' }
        $null = Invoke-Control @('pause')
        $null = Invoke-Control @('seek',[string]($largePlayback.duration - 0.02))
        $null = Invoke-Control @('play')
        $endDeadline = [DateTime]::UtcNow.AddSeconds(3)
        do {
            $ended = Invoke-Control @('status')
            if ($ended.state -ne 'ended') { Start-Sleep -Milliseconds 65 }
        } while ($ended.state -ne 'ended' -and [DateTime]::UtcNow -lt $endDeadline)
        if ($ended.state -ne 'ended') { throw 'The muted near-end source did not finish naturally.' }
        $null = Invoke-Control @('play')
        $replayDeadline = [DateTime]::UtcNow.AddSeconds(2)
        do {
            $replayedHistory = @(Invoke-Control @('play-history','--limit','100'))
            if ($replayedHistory.Count -lt 3) { Start-Sleep -Milliseconds 65 }
        } while ($replayedHistory.Count -lt 3 -and [DateTime]::UtcNow -lt $replayDeadline)
        if ($replayedHistory.Count -ne 3) { throw 'Natural end/replay did not create exactly one new playback history session.' }
        $null = Invoke-Control @('pause')
        $checks.Add('pause/resume and seek preserve history; stop/replay and natural end/replay each add one session')
        $null = Invoke-Control @('volume','0.37')
        $checks.Add('largest 192k fixture decodes muted without full-file PCM memory')
    }
    $stopped = Invoke-Control @('stop')
    if ($stopped.state -ne 'stopped' -or $stopped.position -ne 0) { throw 'Stop did not rewind transport.' }
    $checks.Add('stop rewinds transport')
    $null = Invoke-Control @('audition','--pitch','7','--rate','0.75','--mono')
    if (!$ownedProcess.WaitForExit(25000)) { throw 'Desktop did not complete the smoke shutdown.' }
    if ($ownedProcess.ExitCode -ne 0 -or !(Test-Path -LiteralPath $screenshot)) { throw 'Desktop screenshot/shutdown failed.' }
    $preferences = Get-Content -LiteralPath (Join-Path $testRoot 'userdata/settings.json') -Raw | ConvertFrom-Json
    if ([Math]::Abs($preferences.playback.volume - 0.37) -gt 0.0001) { throw 'Confirmed IPC volume was lost on desktop shutdown.' }
    $checks.Add('confirmed CLI volume survives desktop shutdown')
    if ($preferences.playback.audition.pitch_semitones -ne 7 -or $preferences.playback.audition.rate -ne 0.75 -or !$preferences.playback.audition.mono) { throw 'Confirmed audition settings were lost on desktop shutdown.' }
    $checks.Add('confirmed independent Pitch/Rate and mono survive desktop shutdown')
    $checks.Add('desktop snapshot and clean shutdown')
    [PSCustomObject]@{ success=$true; checks=$checks; output_rate=$playing.device_sample_rate; actual_buffer_frames=$playing.actual_buffer_frames; synthetic_audio_is_silent=$true; real_fixture_gain_is_zero=$true; large_fixture_bytes=$largest.Length; large_fixture_duration=$largePlayback.duration; working_set_before_bytes=$baselineMemory; working_set_large_bytes=$largeMemory; large_fixture_working_set_delta_bytes=$memoryDelta; acoustic_latency_measured=$false } | ConvertTo-Json -Depth 4
} finally {
    if ($ownedProcess -and !$ownedProcess.HasExited) { Stop-Process -Id $ownedProcess.Id -Force }
    $env:AUDIOLIB_DATA_DIR = $previousRoot
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    if ($resolvedRoot.StartsWith($temporaryRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolvedRoot)) {
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
}
