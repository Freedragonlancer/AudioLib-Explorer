# tools/test-package-boundary.ps1
# Regression for stale-output and ignored-file leaks. Runs the real packager in
# an owned synthetic checkout with fake build outputs and dependency metadata.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$testParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = [IO.Path]::GetFullPath((Join-Path $testParent ('AudioLibPackageBoundary-' + [guid]::NewGuid().ToString('N'))))
if (!$testRoot.StartsWith($testParent + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Package regression escaped owned test parent.' }
try {
    New-Item -ItemType Directory -Path (Join-Path $testRoot 'tools') | Out-Null
    foreach ($script in @('package-portable.ps1','validate-package-privacy.ps1','portable-files.json','Start-AudioLibExplorer.cmd')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $script) -Destination (Join-Path $testRoot 'tools')
    }
    $files = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'portable-files.json') -Raw | ConvertFrom-Json
    foreach ($file in $files) {
        $destination = Join-Path $testRoot $file
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
        Set-Content -LiteralPath $destination -Value 'synthetic public document' -Encoding UTF8
    }
    $buildRoot = Join-Path $testRoot 'target/x86_64-pc-windows-msvc/release'
    New-Item -ItemType Directory -Force -Path $buildRoot | Out-Null
    foreach ($binary in @('AudioLibExplorer.exe','AudioLibCli.exe')) { Set-Content -LiteralPath (Join-Path $buildRoot $binary) -Value 'synthetic build' -Encoding ASCII }
    $staleRoot = Join-Path $testRoot 'dist/latest/portable'
    New-Item -ItemType Directory -Force -Path (Join-Path $staleRoot 'doc/assets/issues') | Out-Null
    Set-Content -LiteralPath (Join-Path $staleRoot 'settings.json') -Value 'stale private state'
    Set-Content -LiteralPath (Join-Path $staleRoot 'doc/Rust_Old_Report.json') -Value 'stale private report'
    Set-Content -LiteralPath (Join-Path $staleRoot 'doc/assets/issues/private.png') -Value 'private image'
    Set-Content -LiteralPath (Join-Path $testRoot 'doc/Rust_CI_Release.json') -Value 'ignored private report'
    New-Item -ItemType Directory -Force -Path (Join-Path $testRoot 'resources/private') | Out-Null
    Set-Content -LiteralPath (Join-Path $testRoot 'resources/private/settings.json') -Value 'untracked private state'
    # SMB can mark dot-prefixed staging directories hidden. Existing hidden
    # ancestors must still be inspected rather than silently skipped or failing.
    $hiddenAncestor = Get-Item -LiteralPath (Split-Path -Parent $staleRoot) -Force
    $hiddenAncestor.Attributes = $hiddenAncestor.Attributes -bor [IO.FileAttributes]::Hidden
    # Function scope masks Cargo only in this synthetic regression invocation.
    function cargo { '{"packages":[]}' ; $global:LASTEXITCODE = 0 }
    & (Join-Path $testRoot 'tools/package-portable.ps1') -SkipBuild -WithoutAsio
    if (Test-Path -LiteralPath (Join-Path $staleRoot 'settings.json')) { throw 'Stale runtime state survived clean staging.' }
    foreach ($file in @('doc/Rust_Old_Report.json','doc/Rust_CI_Release.json','doc/assets/issues/private.png','resources/private/settings.json')) {
        if (Test-Path -LiteralPath (Join-Path $staleRoot $file)) { throw "Private file leaked through package: $file" }
    }
    $hiddenLeak = Join-Path $staleRoot '.private-report.json'
    Set-Content -LiteralPath $hiddenLeak -Value 'hidden private report'
    (Get-Item -LiteralPath $hiddenLeak -Force).Attributes = [IO.FileAttributes]::Hidden
    $rejected = $false
    try { & (Join-Path $testRoot 'tools/validate-package-privacy.ps1') -BinaryDirectory $staleRoot } catch { $rejected = $_.Exception.Message -match 'outside the public payload allowlist' }
    if (!$rejected) { throw 'Hidden payload leak was not rejected.' }
    Remove-Item -LiteralPath $hiddenLeak -Force
    # The privacy gate must reject actual machine paths embedded in a build.
    Set-Content -LiteralPath (Join-Path $staleRoot 'AudioLibCli.exe') -Value $env:USERPROFILE -Encoding ASCII
    $rejected = $false
    try { & (Join-Path $testRoot 'tools/validate-package-privacy.ps1') -BinaryDirectory $staleRoot } catch { $rejected = $_.Exception.Message -match 'Local build/user path' }
    if (!$rejected) { throw 'Machine-path regression was not rejected.' }
    Set-Content -LiteralPath (Join-Path $staleRoot 'AudioLibCli.exe') -Value 'synthetic build' -Encoding ASCII
    $report = @{
        success=$true; tag='v0.4.4'; version='0.4.4'; commit=('a' * 40)
        checked_at=[DateTime]::UtcNow.ToString('o'); checks=@('synthetic validation')
        binaries=@{}
        portable=@{ success=$true; mode='clean-checkout-synthetic'; records=1; source_unchanged=$true; python_runtime_required=$false }
    }
    foreach ($binary in @('AudioLibExplorer.exe','AudioLibCli.exe')) { $report.binaries[$binary] = (Get-FileHash -LiteralPath (Join-Path $staleRoot $binary)).Hash }
    $reportPath = Join-Path $testRoot 'public-report.json'
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding UTF8
    & (Join-Path $testRoot 'tools/validate-package-privacy.ps1') -BinaryDirectory $staleRoot -ReleaseReport $reportPath
    foreach ($leak in @('unknown-field','escaped-path','wrong-binary')) {
        $bad = $report | ConvertTo-Json -Depth 5 | ConvertFrom-Json
        switch ($leak) {
            'unknown-field' { $bad.portable | Add-Member -NotePropertyName screenshot -NotePropertyValue 'private.png' }
            'escaped-path' { $bad.checks = @($env:USERPROFILE) }
            'wrong-binary' { $bad.binaries.'AudioLibCli.exe' = ('0' * 64) }
        }
        $bad | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding UTF8
        $rejected = $false
        try { & (Join-Path $testRoot 'tools/validate-package-privacy.ps1') -BinaryDirectory $staleRoot -ReleaseReport $reportPath } catch { $rejected = $true }
        if (!$rejected) { throw "Public report leak was not rejected: $leak" }
    }
    # Cloud releases keep the report schema but leave runtime validation unrun.
    # These nulls must never be accepted as measured success, counts or flags.
    $notRun = $report | ConvertTo-Json -Depth 5 | ConvertFrom-Json
    $notRun.portable.mode = 'not-run'
    foreach ($field in @('success','records','source_unchanged','python_runtime_required')) { $notRun.portable.$field = $null }
    $notRun | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding UTF8
    & (Join-Path $testRoot 'tools/validate-package-privacy.ps1') -BinaryDirectory $staleRoot -ReleaseReport $reportPath
    foreach ($invalid in @('success','records','source_unchanged','python_runtime_required','mode')) {
        $bad = $notRun | ConvertTo-Json -Depth 5 | ConvertFrom-Json
        switch ($invalid) {
            'success' { $bad.portable.success = $true }
            'records' { $bad.portable.records = 0 }
            'source_unchanged' { $bad.portable.source_unchanged = $false }
            'python_runtime_required' { $bad.portable.python_runtime_required = $false }
            'mode' { $bad.portable.mode = 'unknown' }
        }
        $bad | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding UTF8
        $rejected = $false
        try { & (Join-Path $testRoot 'tools/validate-package-privacy.ps1') -BinaryDirectory $staleRoot -ReleaseReport $reportPath } catch { $rejected = $true }
        if (!$rejected) { throw "Unrun portable report accepted invalid state: $invalid" }
    }
    $dist = [IO.Path]::GetFullPath((Join-Path $testRoot 'dist'))
    $redirected = [IO.Path]::GetFullPath((Join-Path $testRoot 'redirected-output'))
    if (!$dist.StartsWith($testRoot + '\', [StringComparison]::OrdinalIgnoreCase) -or
        !$redirected.StartsWith($testRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Junction regression escaped test root.' }
    Move-Item -LiteralPath $dist -Destination (Join-Path $testRoot 'previous-output')
    New-Item -ItemType Directory -Path $redirected | Out-Null
    Set-Content -LiteralPath (Join-Path $redirected 'preserve.txt') -Value 'owned sentinel'
    try {
        New-Item -ItemType Junction -Path $dist -Target $redirected | Out-Null
        $rejected = $false
        try { & (Join-Path $testRoot 'tools/package-portable.ps1') -SkipBuild -WithoutAsio } catch { $rejected = $_.Exception.Message -match 'Linked package path' }
        if (!$rejected -or (Test-Path -LiteralPath (Join-Path $redirected 'latest')) -or
            (Get-Content -LiteralPath (Join-Path $redirected 'preserve.txt') -Raw).Trim() -ne 'owned sentinel') { throw 'Junction ancestor was followed or changed.' }
    } finally {
        if ((Test-Path -LiteralPath $dist) -and ((Get-Item -LiteralPath $dist -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            # Unlink only the exact test-created junction, without recursive deletion.
            if ($dist -ne [IO.Path]::GetFullPath((Join-Path $testRoot 'dist'))) { throw 'Unexpected junction cleanup target.' }
            Remove-Item -LiteralPath $dist -Force
        }
    }
    Write-Host 'Package boundary regression passed: stale state/ignored reports/untracked assets excluded; completed and unrun reports accepted; local paths, unsafe reports, false runtime results, wrong binary identities and junction ancestors rejected.'
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        if (!$testRoot.StartsWith($testParent + '\', [StringComparison]::OrdinalIgnoreCase) -or
            (Split-Path -Leaf $testRoot) -notmatch '^AudioLibPackageBoundary-[0-9a-f]{32}$' -or
            @(Get-ChildItem -LiteralPath $testRoot -Recurse -Force -Attributes ReparsePoint).Count -gt 0) { throw 'Unexpected regression cleanup path.' }
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
