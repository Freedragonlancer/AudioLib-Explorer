# tools/validate-remote-resume.ps1
# Synthetic loopback snapshot interruption/restart and native desktop-close regression.
# Requires current CLI, desktop and server builds; never contacts a configured server.
[CmdletBinding()]
param(
    [string]$BinaryDirectory = 'target/x86_64-pc-windows-msvc/release',
    [string]$ServerBinary,
    [switch]$SkipDesktop,
    # Reuse a passing CLI fault report when only the desktop/CLI lease needs checking.
    [switch]$DesktopOnly
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'console-utf8.ps1')
try {
Set-StrictMode -Version Latest
if ($PSVersionTable.PSVersion -lt [version]'7.3' -or !$IsWindows) { throw 'Remote resume validation requires Windows and PowerShell 7.3 or newer.' }
if ($SkipDesktop -and $DesktopOnly) { throw '-DesktopOnly cannot be combined with -SkipDesktop.' }
$taskRepo = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$taskBinary = [IO.Path]::GetFullPath($(if ([IO.Path]::IsPathRooted($BinaryDirectory)) { $BinaryDirectory } else { Join-Path $taskRepo $BinaryDirectory }))
$taskCli = Join-Path $taskBinary 'AudioLibCli.exe'
$taskDesktop = Join-Path $taskBinary 'AudioLibExplorer.exe'
if (!$ServerBinary) { $ServerBinary = Join-Path $taskBinary 'audiolib-server.exe' }
$ServerBinary = [IO.Path]::GetFullPath($ServerBinary)
foreach ($file in @($taskCli,$ServerBinary) + $(if (!$SkipDesktop) { @($taskDesktop) } else { @() })) {
    if (!(Test-Path -LiteralPath $file -PathType Leaf)) { throw "Required current application build is missing: $file" }
}
if (!('AudioLibResumeFixture' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot 'remote-resume-fixture.cs') }
$taskTemp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/')
$taskRoot = Join-Path $taskTemp ('AudioLibRemoteResume-' + [Guid]::NewGuid().ToString('N'))
$taskArtifacts = Join-Path $taskRepo '.rust-validation/remote-resume'
$taskSavedEnvironment = @{}
foreach ($key in @('AUDIOLIB_DATA_DIR','SLINT_BACKEND','RUST_LOG')) { $taskSavedEnvironment[$key] = [Environment]::GetEnvironmentVariable($key) }
$taskChildren = [Collections.Generic.List[object]]::new()
$taskChecks = [Collections.Generic.List[object]]::new()
$taskFixture = $null
$taskHttp = $null
$taskKeepRoot = $false

function Assert-OwnedTree([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if (!$full.StartsWith($taskTemp + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path -Leaf $full) -cnotmatch '^AudioLibRemoteResume-[0-9a-f]{32}$') { throw 'Refusing cleanup outside this validation owned temporary tree.' }
    if ((Get-Item -LiteralPath $full -Force).Attributes -band [IO.FileAttributes]::ReparsePoint -or
        @(Get-ChildItem -LiteralPath $full -Recurse -Force -Attributes ReparsePoint).Count -gt 0) { throw 'Refusing cleanup of a linked temporary tree.' }
}

function Start-Owned([string]$Executable, [string[]]$Arguments, [string]$Name, [bool]$Desktop = $false) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Executable; $info.WorkingDirectory = $taskRoot
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    $info.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($info)
    $child = [PSCustomObject]@{Process=$process;Name=$Name;Desktop=$Desktop;Out=$process.StandardOutput.ReadToEndAsync();Err=$process.StandardError.ReadToEndAsync()}
    $taskChildren.Add($child)
    return $child
}

function Wait-Owned($Child, [int]$Milliseconds = 60000) {
    if (!$Child.Process.WaitForExit($Milliseconds)) { throw "Owned $($Child.Name) exceeded its validation deadline; isolated data retained on failure." }
    $Child.Process.WaitForExit()
    $stdout = $Child.Out.GetAwaiter().GetResult(); $stderr = $Child.Err.GetAwaiter().GetResult()
    [IO.File]::WriteAllText((Join-Path $taskRoot ($Child.Name + '.stdout')),$stdout)
    [IO.File]::WriteAllText((Join-Path $taskRoot ($Child.Name + '.stderr')),$stderr)
    return [PSCustomObject]@{ExitCode=$Child.Process.ExitCode;Output=$stdout;Error=$stderr}
}

function Invoke-Cli([string[]]$Arguments, [string]$Name, [bool]$ExpectFailure = $false) {
    $reply = Wait-Owned (Start-Owned $taskCli $Arguments $Name)
    if ($ExpectFailure) {
        if ($reply.ExitCode -eq 0) { throw "$Name unexpectedly succeeded during injected disconnections." }
        return $reply
    }
    if ($reply.ExitCode -ne 0) { throw "$Name failed: $($reply.Error) $($reply.Output)" }
    $json = $reply.Output | ConvertFrom-Json
    if (!$json.success) { throw "$Name returned an unsuccessful CLI envelope." }
    return $json.data
}

function Set-IsolatedSettings([string]$Name, [string]$ServerUrl = '') {
    $data = Join-Path $taskRoot $Name
    New-Item -ItemType Directory -Force -Path $data | Out-Null
    $env:AUDIOLIB_DATA_DIR = $data
    $mode = if ($ServerUrl) { 'server' } else { 'local' }
    $settings = @{language='en';active_database=(Join-Path $data 'unused-local.db');allowed_scan_roots=@();playback=@{volume=0.0};data_source=@{mode=$mode;server_url=$ServerUrl;auto_sync=$true;poll_interval_secs=0;webdav=@{username=''}}}
    [IO.File]::WriteAllText((Join-Path $data 'settings.json'),($settings | ConvertTo-Json -Depth 6),[Text.UTF8Encoding]::new($false))
    return $data
}

function Wait-Until([scriptblock]$Predicate, [string]$Message, [int]$Seconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        if (& $Predicate) { return }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    throw $Message
}

function Close-Desktop($Child) {
    Wait-Until { $Child.Process.Refresh(); $Child.Process.HasExited -or [AudioLibResumeWindow]::FindApplicationWindow($Child.Process.Id) -ne [IntPtr]::Zero } 'Owned desktop did not create its application window.'
    if ($Child.Process.HasExited) { throw 'Owned desktop exited before the requested native close.' }
    $window = [AudioLibResumeWindow]::FindApplicationWindow($Child.Process.Id)
    [uint32]$owner = 0
    $thread = [AudioLibResumeWindow]::GetWindowThreadProcessId($window,[ref]$owner)
    if ($thread -eq 0 -or $owner -ne $Child.Process.Id -or ![AudioLibResumeWindow]::IsWindow($window)) { throw 'Close target is not the owned desktop window.' }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    if (![AudioLibResumeWindow]::PostMessage($window,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)) { throw 'Could not send the owned WM_CLOSE request.' }
    $result = Wait-Owned $Child 12000
    if ($result.ExitCode -ne 0) { throw "Owned desktop failed to close normally: $($result.Error)" }
    return $timer.ElapsedMilliseconds
}

function Assert-Resumed($Scenario, [int]$After = 0) {
    $requests = @($Scenario.Requests.ToArray() | Select-Object -Skip $After)
    $resumed = @($requests | Where-Object { $_.Offset -gt 0 })
    if ($resumed.Count -eq 0) { throw 'No positive-offset snapshot Range request was observed.' }
    foreach ($request in $resumed) {
        if ($request.IfRange -cne $Scenario.Headers['ETag'] -or $request.SnapshotId -cne $Scenario.Headers['X-Snapshot-Id']) { throw 'Resume did not bind the same strong ETag and snapshot ID.' }
    }
    return $resumed[0].Offset
}

try {
    New-Item -ItemType Directory -Path $taskRoot -ErrorAction Stop | Out-Null
    Assert-OwnedTree $taskRoot
    New-Item -ItemType Directory -Force -Path $taskArtifacts | Out-Null
    $env:SLINT_BACKEND = 'winit-software'; $env:RUST_LOG = 'info'
    $null = Set-IsolatedSettings 'seed'
    $database = Join-Path $taskRoot 'fixture.db'
    $null = Invoke-Cli @('db','create',$database) 'create'
    # A small real library supplies valid SQLite/FTS5 and nontrivial zstd prefixes.
    $sounds = Join-Path $taskRoot 'sounds'
    New-Item -ItemType Directory -Path $sounds | Out-Null
    $random = [Random]::new(1729)
    foreach ($index in 0..31) {
        $randomBytes = [byte[]]::new(1536); $random.NextBytes($randomBytes)
        $comment = [Text.Encoding]::ASCII.GetBytes([Convert]::ToBase64String($randomBytes) + [char]0)
        $writer = [IO.BinaryWriter]::new([IO.File]::Create((Join-Path $sounds ('resume-' + $index + '.wav'))))
        try {
            $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $writer.Write([uint32](36 + 256 + 20 + $comment.Length + ($comment.Length % 2)))
            $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $writer.Write([uint32]16)
            $writer.Write([uint16]1); $writer.Write([uint16]1); $writer.Write([uint32]48000); $writer.Write([uint32]96000); $writer.Write([uint16]2); $writer.Write([uint16]16)
            $writer.Write([Text.Encoding]::ASCII.GetBytes('LIST')); $writer.Write([uint32](12 + $comment.Length + ($comment.Length % 2)))
            $writer.Write([Text.Encoding]::ASCII.GetBytes('INFOICMT')); $writer.Write([uint32]$comment.Length); $writer.Write($comment)
            if ($comment.Length % 2) { $writer.Write([byte]0) }
            $writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([uint32]256); $writer.Write([byte[]]::new(256))
        } finally { $writer.Dispose() }
    }
    $null = Invoke-Cli @('--database',$database,'scan',$sounds) 'scan'
    $portProbe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
    try { $portProbe.Start(); $serverPort = $portProbe.LocalEndpoint.Port } finally { $portProbe.Stop() }
    $config = @(
        ('bind = "127.0.0.1:' + $serverPort + '"'), ('library_db = ' + ($database | ConvertTo-Json -Compress)),
        'auth_mode = "off"','manage_mode = "off"','roots = []',
        ('token_env = "AUDIOLIB_RESUME_FIXTURE_' + [Guid]::NewGuid().ToString('N') + '"'),
        ('snapshot_dir = ' + ((Join-Path $taskRoot 'snapshots') | ConvertTo-Json -Compress))
    )
    $configPath = Join-Path $taskRoot 'server.toml'
    [IO.File]::WriteAllLines($configPath,$config,[Text.UTF8Encoding]::new($false))
    $server = Start-Owned $ServerBinary @('--config',$configPath) 'server'
    $handler = [Net.Http.HttpClientHandler]::new(); $handler.UseProxy = $false; $handler.AutomaticDecompression = [Net.DecompressionMethods]::None
    $taskHttp = [Net.Http.HttpClient]::new($handler); $taskHttp.Timeout = [TimeSpan]::FromSeconds(15)
    $base = "http://127.0.0.1:$serverPort/api/v1"
    Wait-Until {
        if ($server.Process.HasExited) { throw 'Owned seed server exited before health became ready.' }
        try { (($taskHttp.GetStringAsync($base + '/health').GetAwaiter().GetResult() | ConvertFrom-Json).status -eq 'ok') } catch { $false }
    } 'Owned server did not open the synthetic library.' 45
    $health = $taskHttp.GetStringAsync($base + '/health').GetAwaiter().GetResult()
    $library = $taskHttp.GetStringAsync($base + '/library').GetAwaiter().GetResult()
    $roots = $taskHttp.GetStringAsync($base + '/roots').GetAwaiter().GetResult()
    $capabilities = $taskHttp.GetStringAsync($base + '/capabilities').GetAwaiter().GetResult()
    $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::Get,$base + '/library/snapshot')
    $request.Headers.TryAddWithoutValidation('Accept-Encoding','zstd') | Out-Null
    # The seed server and fixture client use this same build. Negotiate its actual
    # format before capturing the immutable representation for fault injection.
    $seedFormat = ($library | ConvertFrom-Json).snapshot_format
    $seedCatalog = ($capabilities | ConvertFrom-Json).compatibility
    if (!$seedFormat -or !$seedCatalog) { throw 'The owned seed server must expose its modern snapshot format and compatibility catalog.' }
    $request.Headers.TryAddWithoutValidation('x-audiolib-client-compatibility',($seedCatalog | ConvertTo-Json -Depth 20 -Compress)) | Out-Null
    $request.Headers.TryAddWithoutValidation('x-audiolib-accept-snapshot-formats',[string]$seedFormat.id) | Out-Null
    $request.Headers.TryAddWithoutValidation('x-audiolib-snapshot-schema-max',[string]$seedFormat.format.schema_version) | Out-Null
    $request.Headers.TryAddWithoutValidation('x-audiolib-search-storage-max',[string]$seedFormat.format.features.search_storage) | Out-Null
    $request.Headers.TryAddWithoutValidation('x-audiolib-tokenization-max',[string]$seedFormat.format.features.tokenization) | Out-Null
    try {
        $response = $taskHttp.SendAsync($request).GetAwaiter().GetResult()
        try {
            $response.EnsureSuccessStatusCode() | Out-Null
            $body = $response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
            $headers = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::OrdinalIgnoreCase)
            foreach ($header in @($response.Headers) + @($response.Content.Headers)) {
                if ($header.Key -notin @('Connection','Content-Length','Transfer-Encoding','Date','Content-Range')) { $headers[$header.Key] = $header.Value -join ', ' }
            }
        } finally { $response.Dispose() }
    } finally { $request.Dispose() }
    if (!$headers.ContainsKey('X-Snapshot-Id') -or !$headers.ContainsKey('ETag') -or $headers['ETag'].StartsWith('W/') -or $headers['Content-Encoding'] -cne 'zstd' -or $body.Length -lt 1024) { throw 'Build a current resume-capable server returning a strong ETag, snapshot ID and zstd body.' }
    $taskFixture = [AudioLibResumeFixture]::new($health,$roots,$capabilities)
    $fixtureBase = 'http://127.0.0.1:' + $taskFixture.Port
    if (!$DesktopOnly) {
        Write-Host 'Checking automatic resume after a truncated compressed response...'
        $automatic = [AudioLibResumeScenario]::new($body,$headers,$library); $automatic.CutsLeft = 1
        $taskFixture.Add('automatic',$automatic)
        $null = Set-IsolatedSettings 'automatic' ($fixtureBase + '/automatic')
        $result = Invoke-Cli @('remote','sync') 'automatic'
        if ($result.audio_files -ne 32 -or !$result.downloaded) { throw 'Automatic resume did not publish the valid synthetic snapshot.' }
        $offset = Assert-Resumed $automatic
        $taskChecks.Add(@{case='automatic compressed resume';resume_offset=$offset;requests=$automatic.Requests.Count})

        Write-Host 'Checking the production 30-second silent-read timeout, then automatic Range resume...'
        $readStall = [AudioLibResumeScenario]::new($body,$headers,$library); $readStall.StallsLeft = 1
        $taskFixture.Add('read-stall',$readStall)
        $null = Set-IsolatedSettings 'read-stall' ($fixtureBase + '/read-stall')
        $stallTimer = [Diagnostics.Stopwatch]::StartNew()
        $result = Invoke-Cli @('remote','sync') 'read-stall-resumed'
        $stallTimer.Stop()
        if ($result.audio_files -ne 32 -or !$result.downloaded -or $readStall.Stalled -ne 1) { throw 'A silent body did not recover to a valid snapshot after one injected stall.' }
        if ($stallTimer.ElapsedMilliseconds -lt 28000 -or $stallTimer.ElapsedMilliseconds -ge 60000) { throw 'Silent-read recovery did not exercise the production 30-second timeout within its bounded deadline.' }
        $offset = Assert-Resumed $readStall
        $taskChecks.Add(@{case='production 30-second silent-read timeout and automatic compressed resume';elapsed_ms=$stallTimer.ElapsedMilliseconds;resume_offset=$offset;requests=$readStall.Requests.Count})

        Write-Host 'Checking a new CLI process resumes the persisted checkpoint...'
        $persistent = [AudioLibResumeScenario]::new($body,$headers,$library); $persistent.CutsLeft = 4
        $taskFixture.Add('persistent',$persistent)
        $persistentData = Set-IsolatedSettings 'persistent' ($fixtureBase + '/persistent')
        $null = Invoke-Cli @('remote','sync') 'persistent-interrupted' $true
        if (Test-Path -LiteralPath (Join-Path $persistentData 'remote/active.json')) { throw 'Failed download published active cache state.' }
        $before = $persistent.Requests.Count
        $persistent.CutsLeft = 0
        $result = Invoke-Cli @('remote','sync') 'persistent-resumed'
        if ($result.audio_files -ne 32) { throw 'Cross-process resume did not validate the synthetic snapshot.' }
        $offset = Assert-Resumed $persistent $before
        $taskChecks.Add(@{case='CLI restart retains compressed bytes';resume_offset=$offset;requests=$persistent.Requests.Count})

        Write-Host 'Checking explicit restart discards the old transfer...'
        $restart = [AudioLibResumeScenario]::new($body,$headers,$library)
        $taskFixture.Add('restart',$restart)
        $restartData = Set-IsolatedSettings 'restart' ($fixtureBase + '/restart')
        $seeded = Invoke-Cli @('remote','sync') 'restart-seed'
        $oldState = [IO.File]::ReadAllText((Join-Path $restartData 'remote/active.json'))
        $oldHash = (Get-FileHash -LiteralPath $seeded.database -Algorithm SHA256).Hash
        $restart.CutsLeft = 4
        $null = Invoke-Cli @('remote','sync','--force') 'restart-interrupted' $true
        if ([IO.File]::ReadAllText((Join-Path $restartData 'remote/active.json')) -cne $oldState -or
            (Get-FileHash -LiteralPath $seeded.database -Algorithm SHA256).Hash -cne $oldHash) { throw 'An interrupted replacement damaged the previous validated cache.' }
        $before = $restart.Requests.Count; $restart.CutsLeft = 0
        $null = Invoke-Cli @('remote','sync','--restart') 'restart-full'
        $newRequests = @($restart.Requests.ToArray() | Select-Object -Skip $before)
        if (!$newRequests.Count -or $null -ne $newRequests[0].Range -or $newRequests[0].Offset -ne 0) { throw '--restart reused the old Range checkpoint.' }
        $taskChecks.Add(@{case='CLI explicit restart and old cache preservation';first_offset=$newRequests[0].Offset})
    }

    if (!$SkipDesktop) {
        Write-Host 'Checking native desktop close during a silent response, then restart/resume...'
        $desktopCase = [AudioLibResumeScenario]::new($body,$headers,$library); $desktopCase.Stall = $true
        $taskFixture.Add('desktop',$desktopCase)
        $desktopData = Set-IsolatedSettings 'desktop' ($fixtureBase + '/desktop')
        $desktop = Start-Owned $taskDesktop @() 'desktop-interrupted' $true
        Wait-Until { $desktop.Process.Refresh(); if ($desktop.Process.HasExited) { throw 'Owned desktop exited before the stalled response.' }; $desktopCase.Stalled -gt 0 } 'Desktop did not begin its isolated snapshot transfer.' 30
        $requestsBefore = $desktopCase.Requests.Count
        $checkpoint = @(Get-ChildItem -LiteralPath (Join-Path $desktopData 'remote/.transfers') -Filter checkpoint.json -Recurse -File)
        if ($checkpoint.Count -ne 1) { throw 'The owned desktop did not create its endpoint checkpoint.' }
        $checkpointBefore = (Get-FileHash -LiteralPath $checkpoint[0].FullName -Algorithm SHA256).Hash
        $busy = Invoke-Cli @('remote','sync') 'desktop-cli-busy' $true
        if (($busy.Output + $busy.Error) -notmatch 'Another process is syncing this server' -or
            $desktopCase.Requests.Count -ne $requestsBefore -or
            (Get-FileHash -LiteralPath $checkpoint[0].FullName -Algorithm SHA256).Hash -cne $checkpointBefore) {
            throw 'A second CLI must report busy without changing the desktop transfer or making a snapshot request.'
        }
        $taskChecks.Add(@{case='desktop and CLI endpoint lease';second_process_rejected=$true;checkpoint_unchanged=$true;extra_snapshot_requests=0})
        $closeMs = Close-Desktop $desktop
        if (Test-Path -LiteralPath (Join-Path $desktopData 'remote/active.json')) { throw 'Closing a partial first sync published active cache state.' }
        $before = $desktopCase.Requests.Count; $desktopCase.Stall = $false
        $desktop = Start-Owned $taskDesktop @() 'desktop-resumed' $true
        Wait-Until { $desktop.Process.Refresh(); if ($desktop.Process.HasExited) { throw 'Owned desktop exited before resumed cache publication.' }; Test-Path -LiteralPath (Join-Path $desktopData 'remote/active.json') } 'Restarted desktop did not publish its resumed snapshot.' 45
        $offset = Assert-Resumed $desktopCase $before
        $null = Close-Desktop $desktop
        $logs = @(Get-ChildItem -LiteralPath (Join-Path $desktopData 'logs') -File | ForEach-Object { [IO.File]::ReadAllText($_.FullName) }) -join "`n"
        if ($logs -notmatch 'Desktop shutdown complete') { throw 'Owned desktop did not complete its normal shutdown path.' }
        $taskChecks.Add(@{case='native close during silent compressed body then desktop restart';close_ms=$closeMs;resume_offset=$offset})
    }
    if (!$taskFixture.Errors.IsEmpty) { throw ('Synthetic fixture handler failed: ' + ($taskFixture.Errors.ToArray() -join '; ')) }
    $report = [ordered]@{success=$true;loopback_only=$true;synthetic_records=32;compressed_bytes=$body.Length;cli_sha256=(Get-FileHash -LiteralPath $taskCli -Algorithm SHA256).Hash;server_sha256=(Get-FileHash -LiteralPath $ServerBinary -Algorithm SHA256).Hash;desktop_tested=(!$SkipDesktop);desktop_sha256=$(if (!$SkipDesktop) { (Get-FileHash -LiteralPath $taskDesktop -Algorithm SHA256).Hash } else { $null });checks=$taskChecks;limits=@('Synthetic loopback transport only; no real WAN measurement.','Server graceful shutdown and CLI Ctrl+C are not asserted by this script.')}
    $reportName = if ($DesktopOnly) { 'desktop-only-report.json' } else { 'report.json' }
    $report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $taskArtifacts $reportName) -Encoding utf8NoBOM
    $report | ConvertTo-Json -Depth 6
} catch {
    $taskKeepRoot = $true
    [ordered]@{success=$false;owned_root=$taskRoot;error=$_.Exception.Message} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $taskArtifacts 'failure-private.json') -Encoding utf8NoBOM
    throw
} finally {
    try {
        if ($null -ne $taskFixture) { $taskFixture.Dispose() }
        if ($null -ne $taskHttp) { $taskHttp.Dispose() }
        foreach ($child in $taskChildren) {
            if (!$child.Process.HasExited) {
                if ($child.Desktop) {
                    try { $null = Close-Desktop $child } catch { $taskKeepRoot = $true; Write-Warning 'Owned desktop remains alive; its isolated data is preserved without forced termination.' }
                } else {
                    # Only the synthetic server or CLI process created above can be stopped.
                    # This cleanup is not a server graceful-shutdown assertion.
                    $child.Process.Kill($true); $null = $child.Process.WaitForExit(10000)
                }
            }
            if ($child.Process.HasExited) { $child.Process.Dispose() }
        }
    } catch {
        $taskKeepRoot = $true
        throw
    } finally {
        foreach ($key in $taskSavedEnvironment.Keys) { [Environment]::SetEnvironmentVariable($key,$taskSavedEnvironment[$key]) }
        if (!$taskKeepRoot -and (Test-Path -LiteralPath $taskRoot)) { Assert-OwnedTree $taskRoot; Remove-Item -LiteralPath $taskRoot -Recurse -Force }
        elseif ($taskKeepRoot) { Write-Warning "Failed validation retained only its isolated fixture: $taskRoot" }
    }
}
} finally {
    Restore-AudioLibConsoleEncoding
}
