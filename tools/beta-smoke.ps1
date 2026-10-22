# tools/beta-smoke.ps1
# Scripted basic test of a beta server and the side-by-side beta install.
#   ./tools/beta-smoke.ps1 -BaseUrl <http://host:port> [-ExpectedCommit <sha>] [-Token <t>]
#       [-Sync [-KeepSyncData]] [-MinEntries <n>] [-StreamSamples 20] [-ReleaseDir <beta_release>]
# -BaseUrl (or AUDIOLIB_BETA_URL) is required; no server address is stored in the repo.
# Read-only against the server: GET/HEAD only, never starts, stops or scans anything.
# Stream reads time out after -StreamTimeoutSec (default 15).
# -Sync runs `AudioLibCli remote sync --force` from -ReleaseDir with AUDIOLIB_DATA_DIR set
# to a fresh directory under $env:TEMP; the stable v1 and the beta data directories are
# never read or written. A PASS/FAIL table is printed, a log is written to the temp run
# directory, and the exit code is 0 only when every check passes.
[CmdletBinding()]
param(
    [string]$BaseUrl = $env:AUDIOLIB_BETA_URL,
    [string]$ExpectedCommit,
    [string]$Token,
    [switch]$Sync,
    [int]$MinEntries = 1,
    [int]$StreamSamples = 20,
    [string]$ReleaseDir,
    [int]$SyncTimeoutSec = 900,
    [switch]$KeepSyncData,
    [int]$StreamTimeoutSec = 15
)
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($BaseUrl)) { throw 'Pass -BaseUrl <http://host:port> or set AUDIOLIB_BETA_URL.' }
$BaseUrl = $BaseUrl.Trim().TrimEnd('/')
if ($BaseUrl.EndsWith('/api/v1')) { $BaseUrl = $BaseUrl.Substring(0, $BaseUrl.Length - 7) }
$api = "$BaseUrl/api/v1"

function Test-V1Dir([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return $false }
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    return $full -match '[\\/]v1$'
}
if (Test-V1Dir $env:AUDIOLIB_DATA_DIR) { throw 'Refusing to run: AUDIOLIB_DATA_DIR points at a stable v1 data directory.' }

$runDir = Join-Path $env:TEMP ("audiolib-beta-smoke-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force -Path $runDir | Out-Null
$logPath = Join-Path $runDir 'beta-smoke.log'
$results = New-Object System.Collections.Generic.List[object]
function Log([string]$Text) { Write-Host $Text; Add-Content -LiteralPath $logPath -Value $Text -Encoding UTF8 }
function Record([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Check = $Name; Result = $(if ($Ok) { 'PASS' } else { 'FAIL' }); Detail = $Detail })
    Log ("[{0}] {1}: {2}" -f $(if ($Ok) { 'PASS' } else { 'FAIL' }), $Name, $Detail)
}

Add-Type -AssemblyName System.Net.Http
# Audio records carry free-form attribute keys that can differ only in case, which
# ConvertFrom-Json (Windows PowerShell 5) rejects; parse those with a case-sensitive reader.
Add-Type -AssemblyName System.Web.Extensions
$json = New-Object System.Web.Script.Serialization.JavaScriptSerializer
$json.MaxJsonLength = [int]::MaxValue
$handler = New-Object System.Net.Http.HttpClientHandler
$handler.AllowAutoRedirect = $false
$http = New-Object System.Net.Http.HttpClient($handler)
$http.Timeout = [TimeSpan]::FromMinutes(10)
function Send([string]$Method, [string]$Url, [hashtable]$Headers = @{}, [switch]$NoAuth, [switch]$HeadersOnly, [int]$TimeoutSec = 600) {
    $request = New-Object System.Net.Http.HttpRequestMessage((New-Object System.Net.Http.HttpMethod($Method)), $Url)
    if ($Token -and !$NoAuth) { [void]$request.Headers.TryAddWithoutValidation('Authorization', "Bearer $Token") }
    foreach ($key in $Headers.Keys) { [void]$request.Headers.TryAddWithoutValidation($key, [string]$Headers[$key]) }
    $option = if ($HeadersOnly) { [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead } else { [System.Net.Http.HttpCompletionOption]::ResponseContentRead }
    $cancel = New-Object System.Threading.CancellationTokenSource([TimeSpan]::FromSeconds($TimeoutSec))
    try { return $http.SendAsync($request, $option, $cancel.Token).GetAwaiter().GetResult() }
    catch { if (!$cancel.IsCancellationRequested) { throw }; throw "$Method $($Url.Substring($BaseUrl.Length)) timed out after $TimeoutSec s" }
}
function Body($Response) { $Response.Content.ReadAsStringAsync().GetAwaiter().GetResult() }
function Try-Check([string]$Name, [scriptblock]$Block) {
    try { & $Block } catch { Record $Name $false ("error: " + $_.Exception.Message) }
}

Log "AudioLib beta smoke test $(Get-Date -Format s) against $BaseUrl"
Log "Run directory: $runDir"

# 1. Health, version and commit.
$health = $null
Try-Check 'health' {
    $response = Send GET "$api/health" -NoAuth
    $script:health = (Body $response) | ConvertFrom-Json
    Record 'health' (([int]$response.StatusCode -eq 200) -and $health.status -eq 'ok') ("HTTP {0} status={1} version={2} api={3} schema={4} commit={5}" -f [int]$response.StatusCode, $health.status, $health.version, $health.api_version, $health.schema_version, $health.commit)
}
if ($ExpectedCommit) {
    $commit = if ($health) { [string]$health.commit } else { '' }
    $match = $commit -and $commit -ne 'unknown' -and ($commit.StartsWith($ExpectedCommit, [StringComparison]::OrdinalIgnoreCase) -or $ExpectedCommit.StartsWith($commit, [StringComparison]::OrdinalIgnoreCase))
    Record 'commit' ([bool]$match) "server=$commit expected=$ExpectedCommit"
}

# Auth: doc/api §2.2 default `external` mode ignores Authorization for LAN peers, so an
# unauthenticated /library is either 200 (LAN direct) or 401 + WWW-Authenticate (token required).
$library = $null
Try-Check 'auth' {
    $response = Send GET "$api/library" -NoAuth
    $code = [int]$response.StatusCode
    if ($code -eq 200) {
        $script:library = (Body $response) | ConvertFrom-Json
        Record 'auth' $true 'unauthenticated /library -> 200 (LAN peer, token not required)'
    } elseif ($code -eq 401) {
        $challenge = [string]$response.Headers.WwwAuthenticate
        Record 'auth' ($challenge -match 'Bearer') "unauthenticated /library -> 401 ($challenge)"
    } else { Record 'auth' $false "unauthenticated /library -> HTTP $code" }
}
if (!$library) {
    Try-Check 'library' {
        $response = Send GET "$api/library"
        if ([int]$response.StatusCode -eq 200) { $script:library = (Body $response) | ConvertFrom-Json }
        Record 'library' ([int]$response.StatusCode -eq 200) "HTTP $([int]$response.StatusCode)"
    }
}
$serverFiles = if ($library -and $library.stats) { [int64]$library.stats.audio_files } else { 0 }
if ($library) { Log ("Library {0} revision {1}: {2} audio files, {3} missing" -f $library.library_id, $library.revision, $serverFiles, $library.stats.missing_files) }

# 2. Waveform bundle: 200 + quoted ETag, then 304 for HEAD and GET with If-None-Match.
$bundleUrl = "$api/waveforms/bundle?encoding=i8-minmax-base64"
$etag = $null
Try-Check 'bundle GET' {
    $response = Send GET $bundleUrl -HeadersOnly
    $code = [int]$response.StatusCode
    $script:etag = if ($response.Headers.ETag) { $response.Headers.ETag.ToString() } else { $null }
    $length = $response.Content.Headers.ContentLength
    $response.Dispose()
    Record 'bundle GET' ($code -eq 200 -and $etag -and $etag.StartsWith('"')) "HTTP $code ETag=$etag length=$length"
}
if ($etag) {
    Try-Check 'bundle HEAD If-None-Match' {
        $response = Send HEAD $bundleUrl @{ 'If-None-Match' = $etag }
        Record 'bundle HEAD If-None-Match' ([int]$response.StatusCode -eq 304) "HTTP $([int]$response.StatusCode)"
    }
    Try-Check 'bundle GET If-None-Match' {
        $response = Send GET $bundleUrl @{ 'If-None-Match' = $etag } -HeadersOnly
        $code = [int]$response.StatusCode; $response.Dispose()
        Record 'bundle GET If-None-Match' ($code -eq 304) "HTTP $code"
    }
} else { Record 'bundle 304' $false 'skipped: no ETag from GET' }

# Random tracks: probe ids, keep present records, then a Range request on /audio/{id}/stream.
if ($StreamSamples -gt 0) {
    Try-Check 'stream samples' {
        # Only records under a configured root can stream (doc/api §8: others are 404).
        $rootsResponse = Send GET "$api/roots"
        $roots = @((Body $rootsResponse) | ConvertFrom-Json | ForEach-Object { $_ })
        $prefixes = @($roots | ForEach-Object { ([string]$_.server_path).TrimEnd('/', '\') + '/' })
        $covered = [int64]0; foreach ($root in $roots) { if ($root.file_count) { $covered += [int64]$root.file_count } }
        Log ("Roots: {0} ({1} files under configured roots, library has {2})" -f (($roots | ForEach-Object { "$($_.name)=$($_.status)" }) -join ', '), $covered, $serverFiles)
        if ($serverFiles -and $covered -lt $serverFiles) { Log ("  WARNING: {0} library records lie outside the configured roots and return 404 from /stream" -f ($serverFiles - $covered)) }
        $outside = 0
        $random = New-Object System.Random
        $upper = [int][Math]::Max(100, $serverFiles * 1.5 + 100)
        $picked = New-Object System.Collections.Generic.List[object]
        $seen = @{}
        for ($attempt = 0; $attempt -lt ($StreamSamples * 200) -and $picked.Count -lt $StreamSamples; $attempt++) {
            $id = $random.Next(1, $upper)
            if ($seen.ContainsKey($id)) { continue }
            $seen[$id] = $true
            $response = Send GET "$api/audio/$id"
            if ([int]$response.StatusCode -ne 200) { continue }
            $record = $json.DeserializeObject((Body $response))
            $file = [pscustomobject]@{ id = $record['id']; missing = $record['missing']; file_path = $record['file_path'] }
            if ($file.missing) { continue }
            $path = ([string]$file.file_path).Replace('\', '/')
            if (!($prefixes | Where-Object { $path.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) })) { $outside++; continue }
            $picked.Add($file)
        }
        Log ("  picked {0} present records under configured roots" -f $picked.Count)
        $failures = 0
        foreach ($file in $picked) {
            try { $response = Send GET "$api/audio/$($file.id)/stream" @{ 'Range' = 'bytes=0-4095' } -HeadersOnly -TimeoutSec $StreamTimeoutSec }
            catch {
                $failures++
                Log ("  stream id={0} FAIL: {1}" -f $file.id, $_.Exception.Message)
                continue
            }
            $code = [int]$response.StatusCode
            $type = [string]$response.Content.Headers.ContentType
            $bytes = $response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
            $magic = if ($bytes.Length -ge 4) { [Text.Encoding]::ASCII.GetString($bytes, 0, 4) } else { '' }
            $errorCode = if ($code -ge 400) { try { [string]$json.DeserializeObject([Text.Encoding]::UTF8.GetString($bytes))['error_code'] } catch { '' } } else { '' }
            $ok = ($code -eq 206) -and $bytes.Length -gt 0 -and ($type -like 'audio/*') -and ($type -ne 'audio/wav' -or $magic -in @('RIFF', 'RF64', 'BW64'))
            if (!$ok) { $failures++ }
            Log ("  stream id={0} HTTP {1} {2} {3} bytes magic={4} {5} {6}" -f $file.id, $code, $type, $bytes.Length, $magic, $errorCode, $(if ($ok) { 'ok' } else { 'FAIL' }))
        }
        Record 'stream samples' ($picked.Count -eq $StreamSamples -and $failures -eq 0) "$($picked.Count)/$StreamSamples random tracks sampled, $failures failed ($outside probed records outside configured roots skipped)"
    }
}

# 3. Optional full sync with the beta CLI into a fresh temp data directory.
if ($Sync) {
    Try-Check 'sync' {
        if (!$ReleaseDir) { throw '-Sync needs -ReleaseDir <beta install directory>' }
        $cli = Join-Path $ReleaseDir 'AudioLibCli.exe'
        if (!(Test-Path -LiteralPath $cli -PathType Leaf)) { throw "No AudioLibCli.exe in $ReleaseDir" }
        $dataDir = Join-Path $runDir 'data'
        if (Test-V1Dir $dataDir) { throw 'Refusing to sync into a v1 data directory.' }
        New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
        $settings = @{ data_source = @{ mode = 'server'; server_url = $BaseUrl } } | ConvertTo-Json -Depth 4
        [IO.File]::WriteAllText((Join-Path $dataDir 'settings.json'), $settings, (New-Object Text.UTF8Encoding($false)))
        $previous = $env:AUDIOLIB_DATA_DIR
        $env:AUDIOLIB_DATA_DIR = $dataDir
        try {
            $out = Join-Path $runDir 'sync.out.txt'; $err = Join-Path $runDir 'sync.err.txt'
            $watch = [Diagnostics.Stopwatch]::StartNew()
            $process = Start-Process -FilePath $cli -ArgumentList 'remote', 'sync', '--force' -NoNewWindow -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
            $null = $process.Handle  # cache the handle so ExitCode is available after WaitForExit
            if (!$process.WaitForExit($SyncTimeoutSec * 1000)) { throw "sync did not finish within $SyncTimeoutSec s (left running, pid $($process.Id))" }
            $watch.Stop()
            $text = (Get-Content -LiteralPath $out -Raw)
            Add-Content -LiteralPath $logPath -Value $text -Encoding UTF8
            $reply = $text | ConvertFrom-Json
            $data = if ($reply.PSObject.Properties['data']) { $reply.data } else { $reply }
            $entries = [int64]$data.audio_files
            $ok = ($process.ExitCode -eq 0) -and ($reply.success -ne $false) -and $data.downloaded -and ($entries -ge $MinEntries) -and (!$serverFiles -or $entries -eq $serverFiles)
            Record 'sync' $ok ("exit {0}, downloaded={1}, entries={2} (server {3}, min {4}), snapshot {5:N1} MiB, {6:N1} s" -f $process.ExitCode, $data.downloaded, $entries, $serverFiles, $MinEntries, ($data.snapshot_bytes / 1MB), $watch.Elapsed.TotalSeconds)
        } finally {
            $env:AUDIOLIB_DATA_DIR = $previous
            # The synced snapshot can be many GB; keep it only on request.
            if (!$KeepSyncData -and $process -and $process.HasExited) { Remove-Item -LiteralPath $dataDir -Recurse -Force -ErrorAction SilentlyContinue }
        }
    }
}

# 4. Optional binary hashes of the beta install.
if ($ReleaseDir) {
    foreach ($binary in @('AudioLibExplorer.exe', 'AudioLibCli.exe')) {
        Try-Check "sha256 $binary" {
            $hash = (Get-FileHash -LiteralPath (Join-Path $ReleaseDir $binary) -Algorithm SHA256).Hash
            Record "sha256 $binary" $true $hash
        }
    }
}

# 5. Manual GUI checklist (not scored).
Log ''
Log 'Manual GUI checklist (launch through AudioLibExplorer-beta.cmd):'
Log '  [ ] Drag the volume bar: the dB readout updates live while dragging (and shows -inf dB at 0).'
Log '  [ ] Connect the desktop app to the beta server: sync completes and list waveforms import without errors.'

# 6. Summary.
Log ''
$table = ($results | Format-Table -AutoSize -Wrap | Out-String).TrimEnd()
Log $table
$failed = @($results | Where-Object { $_.Result -ne 'PASS' }).Count
Log ("Summary: {0} passed, {1} failed -> {2}" -f ($results.Count - $failed), $failed, $(if ($failed -eq 0) { 'PASS' } else { 'FAIL' }))
Log "Log: $logPath"
$http.Dispose()
if ($failed -ne 0 -or $results.Count -eq 0) { exit 1 }
exit 0
