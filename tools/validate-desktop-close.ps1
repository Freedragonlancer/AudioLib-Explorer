# tools/validate-desktop-close.ps1
# Exercise native title-bar close messages with owned empty libraries, never user data.
param(
    [Parameter(Mandatory=$true)][string]$BinaryDirectory,
    [ValidateRange(1,20)][int]$Cycles = 3
)
$ErrorActionPreference = 'Stop'
$taskRepo = Split-Path -Parent $PSScriptRoot
$taskBinary = (Resolve-Path -LiteralPath $BinaryDirectory).Path
$taskDesktop = Join-Path $taskBinary 'AudioLibExplorer.exe'
$taskCli = Join-Path $taskBinary 'AudioLibCli.exe'
if (!(Test-Path -LiteralPath $taskDesktop) -or !(Test-Path -LiteralPath $taskCli)) { throw 'Both application binaries are required.' }
$taskTempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/')
$taskArtifacts = Join-Path $taskRepo '.rust-validation/desktop-close'
New-Item -ItemType Directory -Force -Path $taskArtifacts | Out-Null
if (!('AudioLibDesktopCloseNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class AudioLibDesktopCloseNative {
    private delegate bool EnumWindowCallback(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowCallback callback, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder text, int count);

    // MainWindowHandle may select winit's utility target, whose close destroys
    // event-loop wake delivery. Select only this process's visible application UI.
    public static IntPtr FindApplicationWindow(uint process)
    {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, parameter) => {
            uint owner;
            if (GetWindowThreadProcessId(window, out owner) == 0 || owner != process || !IsWindowVisible(window)) return true;
            var title = new StringBuilder(256);
            var kind = new StringBuilder(256);
            GetWindowText(window, title, title.Capacity);
            GetClassName(window, kind, kind.Capacity);
            if (!String.Equals(title.ToString(), "AudioLib Explorer", StringComparison.Ordinal) ||
                !String.Equals(kind.ToString(), "Window Class", StringComparison.Ordinal)) return true;
            found = window;
            return false;
        }, IntPtr.Zero);
        return found;
    }

    [DllImport("user32.dll",SetLastError=true)] public static extern bool PostMessage(IntPtr window,uint message,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window,out uint process);
}
'@
}
$taskSavedData = $env:AUDIOLIB_DATA_DIR
$taskSavedBackend = $env:SLINT_BACKEND
$taskSavedLog = $env:RUST_LOG
$taskSavedUserName = $env:USERNAME
$taskCases = @()

function Get-CanonicalSettingsPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
    if ($Path.StartsWith('\\?\UNC\',[StringComparison]::OrdinalIgnoreCase)) {
        $Path = '\\' + $Path.Substring(8)
    } elseif ($Path.StartsWith('\\?\',[StringComparison]::OrdinalIgnoreCase)) {
        $Path = $Path.Substring(4)
    }
    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    return [IO.Path]::GetFullPath($resolved)
}

try {
    # The playback pipe is per USERNAME, independent of AUDIOLIB_DATA_DIR. Give
    # this invocation its own pipe without contacting another running desktop.
    $env:USERNAME = 'AudioLibClose_' + [Guid]::NewGuid().ToString('N')
    foreach ($taskMessage in @('WM_CLOSE','SC_CLOSE')) {
        foreach ($taskCycle in 1..$Cycles) {
            $taskRoot = Join-Path $taskTempParent ('audiolib-desktop-close-' + [Guid]::NewGuid().ToString('N'))
            New-Item -ItemType Directory -Path $taskRoot | Out-Null
            $taskProcess = $null
            try {
                $env:AUDIOLIB_DATA_DIR = $taskRoot
                $env:SLINT_BACKEND = 'winit-software'
                $env:RUST_LOG = 'info'
                $taskDatabase = Join-Path $taskRoot 'library.db'
                $taskSettings = @{active_database=$taskDatabase;language='en';playback=@{volume=0.0};allowed_scan_roots=@();data_source=@{mode='local';server_url=''}} | ConvertTo-Json -Depth 5
                [IO.File]::WriteAllText((Join-Path $taskRoot 'settings.json'),$taskSettings,[Text.UTF8Encoding]::new($false))
                $taskCreated = (& $taskCli db create $taskDatabase | Out-String) | ConvertFrom-Json
                if ($LASTEXITCODE -ne 0 -or !$taskCreated.success) { throw 'Owned library creation failed.' }
                $taskProcess = Start-Process -FilePath $taskDesktop -WorkingDirectory $taskBinary -WindowStyle Hidden -PassThru
                $taskDeadline = [DateTime]::UtcNow.AddSeconds(15)
                do {
                    Start-Sleep -Milliseconds 100
                    $taskProcess.Refresh()
                    if ($taskProcess.HasExited) { throw "Owned desktop exited during startup: $($taskProcess.ExitCode)" }
                    $taskHandle = [AudioLibDesktopCloseNative]::FindApplicationWindow($taskProcess.Id)
                } while ($taskHandle -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $taskDeadline)
                if ($taskHandle -eq [IntPtr]::Zero) { throw 'Owned desktop did not create its application window.' }
                # IPC starts only after the main controller and close handler bind.
                # Verify its settings identity as well as the isolated pipe name.
                $taskExpectedSettings = Get-CanonicalSettingsPath (Join-Path $taskRoot 'settings.json')
                $taskReadyMatches = $false
                do {
                    $taskReady = (& $taskCli status | Out-String) | ConvertFrom-Json
                    if ($taskReady.success) {
                        $taskActualSettings = Get-CanonicalSettingsPath ([string]$taskReady.metadata.settings_path)
                        $taskReadyMatches = $null -ne $taskActualSettings -and $taskActualSettings.Equals($taskExpectedSettings,[StringComparison]::OrdinalIgnoreCase)
                    }
                    if (!$taskReadyMatches) { Start-Sleep -Milliseconds 100 }
                } while (!$taskReadyMatches -and [DateTime]::UtcNow -lt $taskDeadline)
                if (!$taskReadyMatches) { throw 'Owned main controller did not become ready with the exact isolated settings identity.' }
                Start-Sleep -Milliseconds (100 * $taskCycle)
                $taskProcess.Refresh()
                $taskHandle = [AudioLibDesktopCloseNative]::FindApplicationWindow($taskProcess.Id)
                [uint32]$taskOwner = 0
                $taskThread = [AudioLibDesktopCloseNative]::GetWindowThreadProcessId($taskHandle,[ref]$taskOwner)
                if ($taskHandle -eq 0 -or $taskThread -eq 0 -or $taskOwner -ne $taskProcess.Id -or ![AudioLibDesktopCloseNative]::IsWindow($taskHandle)) { throw 'Close target is not the owned test window.' }
                $taskStarted = [DateTime]::UtcNow
                $taskNativeMessage = if ($taskMessage -eq 'WM_CLOSE') { [uint32]0x0010 } else { [uint32]0x0112 }
                $taskWParam = if ($taskMessage -eq 'WM_CLOSE') { [IntPtr]::Zero } else { [IntPtr]0xF060 }
                # SC_CLOSE is the title bar X command; repeat it while draining.
                foreach ($taskRequest in 1..2) {
                    [uint32]$taskOwner = 0
                    if (![AudioLibDesktopCloseNative]::IsWindow($taskHandle) -or [AudioLibDesktopCloseNative]::GetWindowThreadProcessId($taskHandle,[ref]$taskOwner) -eq 0 -or $taskOwner -ne $taskProcess.Id) { throw 'Owned close HWND changed before the native request.' }
                    if (![AudioLibDesktopCloseNative]::PostMessage($taskHandle,$taskNativeMessage,$taskWParam,[IntPtr]::Zero)) { throw 'Could not post the owned native close request.' }
                }
                if (!$taskProcess.WaitForExit(12000)) { throw 'Owned desktop did not close gracefully; retained its isolated data and logs.' }
                $taskElapsed = ([DateTime]::UtcNow - $taskStarted).TotalMilliseconds
                if ($taskProcess.ExitCode -ne 0) { throw "Owned desktop failed during close: $($taskProcess.ExitCode)" }
                if ([AudioLibDesktopCloseNative]::IsWindow($taskHandle)) { throw 'Saving changes window remained after process exit.' }
                $taskLogFiles = @(Get-ChildItem -LiteralPath (Join-Path $taskRoot 'logs') -File)
                $taskLogText = ($taskLogFiles | ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw }) -join "`n"
                if ($taskLogText -notmatch 'Desktop shutdown complete') { throw 'Close did not complete the normal shutdown path.' }
                foreach ($taskLogFile in $taskLogFiles) { Copy-Item -LiteralPath $taskLogFile.FullName -Destination (Join-Path $taskArtifacts ($taskMessage + '-' + $taskCycle + '-' + $taskLogFile.Name)) -Force }
                $taskStats = (& $taskCli stats | Out-String) | ConvertFrom-Json
                if ($LASTEXITCODE -ne 0 -or !$taskStats.success -or $taskStats.data.audio_files -ne 0) { throw 'Closed owned library could not be reopened unchanged.' }
                $taskCases += [PSCustomObject]@{message=$taskMessage;cycle=$taskCycle;duplicate_requests=2;elapsed_ms=[Math]::Round($taskElapsed,2);exit_code=$taskProcess.ExitCode;normal_shutdown=$true;library_reopened=$true}
            } catch {
                $taskFailure = [PSCustomObject]@{message=$taskMessage;cycle=$taskCycle;root=$taskRoot;process_id=if($taskProcess){$taskProcess.Id}else{$null};error=$_.Exception.Message}
                $taskFailure | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $taskArtifacts 'failure-private.json') -Encoding UTF8
                if (Test-Path -LiteralPath (Join-Path $taskRoot 'logs')) {
                    foreach ($taskLogFile in @(Get-ChildItem -LiteralPath (Join-Path $taskRoot 'logs') -File)) { Copy-Item -LiteralPath $taskLogFile.FullName -Destination (Join-Path $taskArtifacts ('failed-' + $taskMessage + '-' + $taskCycle + '-' + $taskLogFile.Name)) -Force }
                }
                throw
            } finally {
                if (!$taskProcess -or $taskProcess.HasExited) {
                    $taskResolved = [IO.Path]::GetFullPath($taskRoot)
                    if (!$taskResolved.StartsWith($taskTempParent + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup outside the owned temporary parent.' }
                    if ((Get-Item -LiteralPath $taskResolved -Force).Attributes -band [IO.FileAttributes]::ReparsePoint -or @(Get-ChildItem -LiteralPath $taskResolved -Recurse -Force -Attributes ReparsePoint).Count -gt 0) { throw 'Refusing cleanup of a linked temporary subtree.' }
                    Remove-Item -LiteralPath $taskResolved -Recurse -Force
                } else { Write-Warning 'Owned test process remains alive; its data is preserved. No process was terminated.' }
            }
        }
    }
    $taskReport = [PSCustomObject]@{success=$true;binary_sha256=(Get-FileHash -LiteralPath $taskDesktop -Algorithm SHA256).Hash;isolated_empty_libraries=$true;native_close_cases=$taskCases}
    $taskReport | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $taskArtifacts 'report.json') -Encoding UTF8
    $taskReport | ConvertTo-Json -Depth 5
} finally { $env:AUDIOLIB_DATA_DIR=$taskSavedData; $env:SLINT_BACKEND=$taskSavedBackend; $env:RUST_LOG=$taskSavedLog; $env:USERNAME=$taskSavedUserName }
