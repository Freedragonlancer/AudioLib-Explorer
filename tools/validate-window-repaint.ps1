# tools/validate-window-repaint.ps1
# Native Windows exposure regression; only the launched isolated test window is captured.
param([Parameter(Mandatory=$true)][string]$BinaryDirectory)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$binaryRoot = (Resolve-Path -LiteralPath $BinaryDirectory).Path
$desktop = Join-Path $binaryRoot 'AudioLibExplorer.exe'
$cli = Join-Path $binaryRoot 'AudioLibCli.exe'
if (!(Test-Path -LiteralPath $desktop) -or !(Test-Path -LiteralPath $cli)) { throw 'Both application binaries are required.' }
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/')
$testRoot = Join-Path $tempParent ('audiolib-window-repaint-' + [Guid]::NewGuid().ToString('N'))
$artifacts = Join-Path $taskRoot '.rust-validation/window-repaint'
New-Item -ItemType Directory -Force -Path $testRoot,$artifacts | Out-Null
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
if (!('AudioLibRepaintNative' -as [type])) {
    $nativeReferences = if ($PSVersionTable.PSEdition -eq 'Core') {
        @('System.Windows.Forms','System.Drawing.Common','System.Drawing.Primitives','System.ComponentModel.Primitives','System.Private.Windows.GdiPlus','System.Private.Windows.Core')
    } else { @('System.Windows.Forms','System.Drawing') }
    Add-Type -ReferencedAssemblies $nativeReferences -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Drawing;
using System.Windows.Forms;
public sealed class AudioLibRepaintCover : Form {
    public bool ActivateCover;
    protected override bool ShowWithoutActivation { get { return !ActivateCover; } }
    protected override CreateParams CreateParams {
        get { var p = base.CreateParams; if (!ActivateCover) p.ExStyle |= 0x08000000; return p; }
    }
}
public static class AudioLibRepaintNative {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int z, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint message, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr GetThreadDpiAwarenessContext();
    [DllImport("user32.dll")] public static extern bool AreDpiAwarenessContextsEqual(IntPtr first, IntPtr second);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32.dll", SetLastError=true)] public static extern bool BitBlt(IntPtr destination, int x, int y, int width, int height, IntPtr source, int sourceX, int sourceY, uint operation);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr desktop);
    public static bool IsOwnedWindow(IntPtr window, uint expectedProcess) {
        uint owner;
        return IsWindow(window) && GetWindowThreadProcessId(window,out owner)!=0 && owner==expectedProcess;
    }
    public static void AssertOwnedWindow(IntPtr window, uint expectedProcess) {
        if (!IsOwnedWindow(window,expectedProcess)) throw new Exception("Target is not an owned test window");
    }
    public static bool FocusOwnedWindow(IntPtr window, uint expectedProcess) {
        AssertOwnedWindow(window,expectedProcess);
        return SetForegroundWindow(window);
    }
    public static bool CloseOwnedWindow(IntPtr window, uint expectedProcess) {
        if (!IsOwnedWindow(window,expectedProcess)) return false;
        return PostMessage(window,0x0010,IntPtr.Zero,IntPtr.Zero);
    }
    public static string CaptureEnvironment() {
        IntPtr desktop = OpenInputDesktop(0,false,1);
        if (desktop==IntPtr.Zero) return "Input-desktop READOBJECTS unavailable; Win32="+Marshal.GetLastWin32Error();
        CloseDesktop(desktop);
        return "Input-desktop READOBJECTS available";
    }
    public static bool InputDesktopAvailable() {
        IntPtr desktop = OpenInputDesktop(0,false,1);
        if (desktop==IntPtr.Zero) return false;
        CloseDesktop(desktop);
        return true;
    }
    public static Rectangle Client(IntPtr h) {
        RECT r; if (!GetClientRect(h, out r)) throw new Exception("GetClientRect failed");
        POINT p = new POINT(); if (!ClientToScreen(h, ref p)) throw new Exception("ClientToScreen failed");
        return new Rectangle(p.X, p.Y, r.Right-r.Left, r.Bottom-r.Top);
    }
    public static Bitmap Capture(IntPtr h) {
        if (!IsWindow(h)) throw new Exception("Capture HWND is no longer valid: " + h);
        var r = Client(h);
        if (r.Width<=0 || r.Height<=0) throw new Exception("Capture client has no area: " + r);
        IntPtr source = GetDC(h);
        if (source==IntPtr.Zero) throw new Exception("GetDC(owned HWND) failed: " + Marshal.GetLastWin32Error());
        Bitmap bitmap = null;
        try {
            bitmap = new Bitmap(r.Width, r.Height);
            using (var graphics = Graphics.FromImage(bitmap)) {
                IntPtr destination = graphics.GetHdc();
                try {
                    // Read only the already presented owned client pixels. No
                    // PrintWindow/WM_PRINT or redraw may repair the frame first.
                    if (!BitBlt(destination,0,0,r.Width,r.Height,source,0,0,0x00CC0020))
                        throw new Exception("BitBlt SRCCOPY failed: HWND="+h+", client="+r+", Win32="+Marshal.GetLastWin32Error());
                } finally { graphics.ReleaseHdc(destination); }
            }
            return bitmap;
        } catch (Exception error) {
            if (bitmap!=null) bitmap.Dispose();
            throw new Exception("Owned client capture failed: HWND="+h+", valid="+IsWindow(h)+", client="+r+", sourceDC="+source,error);
        } finally { ReleaseDC(h,source); }
    }
    public static double Difference(Bitmap before, Bitmap after) {
        if (before.Size != after.Size) throw new Exception("Client size changed");
        long changed = 0, total = 0;
        // Sample every fourth pixel across the entire client, not a local button.
        for (int y=0; y<before.Height; y+=4) for (int x=0; x<before.Width; x+=4) {
            Color a=before.GetPixel(x,y), b=after.GetPixel(x,y); total++;
            if (Math.Abs(a.R-b.R)+Math.Abs(a.G-b.G)+Math.Abs(a.B-b.B)>24) changed++;
        }
        return (double)changed/total;
    }
    public static bool HasContent(Bitmap bitmap) {
        int dark=765, bright=0, lightPixels=0;
        for (int y=0; y<bitmap.Height; y+=4) for (int x=0; x<bitmap.Width; x+=4) {
            Color p=bitmap.GetPixel(x,y); int level=p.R+p.G+p.B;
            dark=Math.Min(dark,level); bright=Math.Max(bright,level);
            if (level>384) lightPixels++;
        }
        return bright-dark>180 && lightPixels>100;
    }
}
'@
}
$savedData = $env:AUDIOLIB_DATA_DIR
$savedBackend = $env:SLINT_BACKEND
$savedLog = $env:RUST_LOG
$process = $null
$before = $null
$savedDpiContext = [IntPtr]::Zero
try {
    # Window/client DC coordinates and cover bounds must all use physical pixels.
    # PowerShell does not guarantee a DPI-aware thread; keep this local to the
    # validation thread and restore its previous context even after a failure.
    $perMonitorV2 = [IntPtr](-4)
    $savedDpiContext = [AudioLibRepaintNative]::SetThreadDpiAwarenessContext($perMonitorV2)
    if ($savedDpiContext -eq [IntPtr]::Zero -or ![AudioLibRepaintNative]::AreDpiAwarenessContextsEqual([AudioLibRepaintNative]::GetThreadDpiAwarenessContext(),$perMonitorV2)) {
        throw 'Unable to establish per-monitor DPI awareness for native capture.'
    }
    $env:AUDIOLIB_DATA_DIR = $testRoot
    $env:SLINT_BACKEND = 'winit-software'
    $env:RUST_LOG = 'info'
    # Persist the owned library before any CLI/GUI startup can discover legacy inputs.
    $settings = @{ active_database=(Join-Path $testRoot 'library.db'); language='en'; playback=@{volume=0.0}; allowed_scan_roots=@(); data_source=@{mode='local';server_url=''} } | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText((Join-Path $testRoot 'settings.json'),$settings,[Text.UTF8Encoding]::new($false))
    $created = (& $cli db create (Join-Path $testRoot 'library.db') | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$created.success) { throw 'Isolated empty library creation failed.' }
    $initial = (& $cli stats | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$initial.success -or $initial.data.audio_files -ne 0) { throw 'Isolated empty library initialization failed.' }
    $process = Start-Process -FilePath $desktop -ArgumentList @('--smoke-ui','--smoke-view','window-repaint','--smoke-delay-ms','58000','--smoke-duration-ms','60000','--screenshot',('"' + (Join-Path $artifacts 'final-software.png') + '"')) -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    do {
        Start-Sleep -Milliseconds 100
        $process.Refresh()
        if ($process.HasExited) { throw "Test desktop exited early: $($process.ExitCode)" }
    } while ($process.MainWindowHandle -eq 0 -and [DateTime]::UtcNow -lt $deadline)
    $handle = $process.MainWindowHandle
    if ($handle -eq 0) { throw 'Isolated desktop window did not open.' }
    [AudioLibRepaintNative]::AssertOwnedWindow($handle,$process.Id)
    [AudioLibRepaintNative]::ShowWindow($handle,9) | Out-Null
    [AudioLibRepaintNative]::AssertOwnedWindow($handle,$process.Id)
    [AudioLibRepaintNative]::SetWindowPos($handle,[IntPtr](-1),30,30,1120,740,0x0040) | Out-Null
    $initialFocusRequested = [AudioLibRepaintNative]::FocusOwnedWindow($handle,$process.Id)
    Start-Sleep -Milliseconds 1800
    $inputDesktopAvailable = [AudioLibRepaintNative]::InputDesktopAvailable()
    $initialForeground = [AudioLibRepaintNative]::GetForegroundWindow() -eq $handle
    if ($inputDesktopAvailable -and !$initialForeground) {
        $foregroundHandle = [AudioLibRepaintNative]::GetForegroundWindow()
        [uint32]$foregroundOwner = 0
        $foregroundThread = [AudioLibRepaintNative]::GetWindowThreadProcessId($foregroundHandle,[ref]$foregroundOwner)
        throw "The available input desktop did not give the owned test window foreground focus: requested=$initialFocusRequested; owned HWND=$handle; foreground HWND=$foregroundHandle; foreground PID=$foregroundOwner; foreground TID=$foregroundThread; owned visible=$([AudioLibRepaintNative]::IsWindowVisible($handle))."
    }
    $before = [AudioLibRepaintNative]::Capture($handle)
    if (![AudioLibRepaintNative]::HasContent($before)) { throw 'The baseline client did not contain visible application controls.' }
    $before.Save((Join-Path $artifacts 'before.png'))
    $dpi = [AudioLibRepaintNative]::GetDpiForWindow($handle)
    $cases = @()
    foreach ($action in @('refocus','occlusion','minimize-restore')) {
      foreach ($cycle in 1..3) {
        $inputBefore = [AudioLibRepaintNative]::InputDesktopAvailable()
        $appForegroundBefore = [AudioLibRepaintNative]::GetForegroundWindow() -eq $handle
        $coverVisible = $null
        $coverFocusRequested = $null
        $coverForeground = $null
        $appForegroundWhileCovered = $null
        $minimizeLostForeground = $null
        if ($action -eq 'minimize-restore') {
            [AudioLibRepaintNative]::AssertOwnedWindow($handle,$process.Id)
            [AudioLibRepaintNative]::ShowWindow($handle,6) | Out-Null
            Start-Sleep -Milliseconds 250
            $minimizeLostForeground = [AudioLibRepaintNative]::GetForegroundWindow() -ne $handle
            [AudioLibRepaintNative]::AssertOwnedWindow($handle,$process.Id)
            [AudioLibRepaintNative]::ShowWindow($handle,9) | Out-Null
        } else {
            $cover = [AudioLibRepaintCover]::new()
            try {
                $cover.ActivateCover = $action -eq 'refocus'
                $cover.FormBorderStyle = 'None'
                $cover.Bounds = [AudioLibRepaintNative]::Client($handle)
                $cover.TopMost = $true
                $cover.BackColor = [Drawing.Color]::Black
                $cover.Show()
                [Windows.Forms.Application]::DoEvents()
                if ($action -eq 'refocus') {
                    $coverFocusRequested = [AudioLibRepaintNative]::FocusOwnedWindow($cover.Handle,[Diagnostics.Process]::GetCurrentProcess().Id)
                    [Windows.Forms.Application]::DoEvents()
                }
                Start-Sleep -Milliseconds 250
                $coverVisible = [AudioLibRepaintNative]::IsWindowVisible($cover.Handle)
                $coverForeground = [AudioLibRepaintNative]::GetForegroundWindow() -eq $cover.Handle
                $appForegroundWhileCovered = [AudioLibRepaintNative]::GetForegroundWindow() -eq $handle
                $cover.Close()
                [Windows.Forms.Application]::DoEvents()
            } finally { $cover.Dispose() }
        }
        $appFocusRequested = [AudioLibRepaintNative]::FocusOwnedWindow($handle,$process.Id)
        Start-Sleep -Milliseconds 400
        $appForegroundAfter = [AudioLibRepaintNative]::GetForegroundWindow() -eq $handle
        $caseInputAvailable = $inputBefore -and [AudioLibRepaintNative]::InputDesktopAvailable()
        $foregroundVerified = $caseInputAvailable -and $appForegroundBefore -and $appForegroundAfter
        if ($action -eq 'refocus') { $foregroundVerified = $foregroundVerified -and $coverVisible -and $coverForeground }
        elseif ($action -eq 'occlusion') { $foregroundVerified = $foregroundVerified -and $coverVisible -and $appForegroundWhileCovered }
        else { $foregroundVerified = $foregroundVerified -and $minimizeLostForeground }
        if ($caseInputAvailable -and !$foregroundVerified) { throw "$action cycle $cycle did not perform the required owned-window foreground transition on the available input desktop." }
        $after = [AudioLibRepaintNative]::Capture($handle)
        try {
            $after.Save((Join-Path $artifacts ($action + '-' + $cycle + '.png')))
            $difference = [AudioLibRepaintNative]::Difference($before,$after)
            if ($difference -gt 0.02) { throw "$action lost window contents: $difference changed pixels." }
            $cases += [PSCustomObject]@{ action=$action; cycle=$cycle; changed_fraction=$difference; full_client_preserved=$true; input_desktop_available=$caseInputAvailable; foreground_exposure_verified=$foregroundVerified; app_foreground_before=$appForegroundBefore; cover_visible=$coverVisible; cover_focus_request_succeeded=$coverFocusRequested; cover_was_foreground=$coverForeground; app_foreground_while_covered=$appForegroundWhileCovered; minimize_lost_foreground=$minimizeLostForeground; app_focus_request_succeeded=$appFocusRequested; app_foreground_after=$appForegroundAfter }
        } finally { $after.Dispose() }
      }
    }
    # WM_CLOSE uses the application's drain path; never terminate a user instance.
    if (![AudioLibRepaintNative]::CloseOwnedWindow($handle,$process.Id)) { throw 'Could not close the owned test window.' }
    if (!$process.WaitForExit(12000)) { throw 'The isolated test window did not close gracefully; its data is retained.' }
    if ($process.ExitCode -ne 0) { throw "The isolated desktop failed: $($process.ExitCode)" }
    $foregroundExposureVerified = @($cases | Where-Object { !$_.foreground_exposure_verified }).Count -eq 0
    $limitations = if ($foregroundExposureVerified) { @() } else { @('Input desktop was inaccessible for one or more cycles; retained client pixels were verified, but foreground exposure and focus restoration were not verified.') }
    $report = [PSCustomObject]@{ success=$true; binary_sha256=(Get-FileHash -LiteralPath $desktop -Algorithm SHA256).Hash; native_client_size=@($before.Width,$before.Height); window_dpi=$dpi; capture_thread_dpi_context='per-monitor-aware-v2'; capture_method='owned-client-DC BitBlt SRCCOPY'; input_desktop_available=$inputDesktopAvailable; foreground_exposure_verified=$foregroundExposureVerified; initial_focus_request_succeeded=$initialFocusRequested; initial_app_foreground=$initialForeground; limitations=@($limitations); isolated_empty_library=$true; synthetic_unsaved_form=$true; cases=$cases }
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $artifacts 'report.json') -Encoding UTF8
    $report | ConvertTo-Json -Depth 5
} catch {
    if (Test-Path -LiteralPath (Join-Path $testRoot 'logs')) {
        foreach ($logFile in @(Get-ChildItem -LiteralPath (Join-Path $testRoot 'logs') -File)) { Copy-Item -LiteralPath $logFile.FullName -Destination (Join-Path $artifacts ('failed-' + $logFile.Name)) -Force }
    }
    Write-Warning ('Native exposure regression failed: ' + $_.Exception.ToString())
    Write-Warning ([AudioLibRepaintNative]::CaptureEnvironment())
    throw
} finally {
    if ($savedDpiContext -ne [IntPtr]::Zero) {
        [AudioLibRepaintNative]::SetThreadDpiAwarenessContext($savedDpiContext) | Out-Null
    }
    if ($before) { $before.Dispose() }
    if ($process -and !$process.HasExited -and $process.MainWindowHandle -ne 0) {
        if ([AudioLibRepaintNative]::CloseOwnedWindow($process.MainWindowHandle,$process.Id)) { $process.WaitForExit(12000) | Out-Null }
    }
    $env:AUDIOLIB_DATA_DIR = $savedData
    $env:SLINT_BACKEND = $savedBackend
    $env:RUST_LOG = $savedLog
    if (!$process -or $process.HasExited) {
        $resolvedTest = [IO.Path]::GetFullPath($testRoot)
        if (!$resolvedTest.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup outside the owned temporary root.' }
        $pending = [Collections.Generic.Stack[string]]::new()
        $pending.Push($resolvedTest)
        while ($pending.Count -gt 0) {
            $candidate = $pending.Pop()
            $item = Get-Item -LiteralPath $candidate -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing cleanup of a temporary subtree containing a reparse point.' }
            if ($item.PSIsContainer) { foreach ($child in Get-ChildItem -LiteralPath $candidate -Force) { $pending.Push($child.FullName) } }
        }
        Remove-Item -LiteralPath $resolvedTest -Recurse -Force
    }
}
