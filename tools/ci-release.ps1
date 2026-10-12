# tools/ci-release.ps1
[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')][string]$Tag)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$logs = Join-Path $taskRoot '.rust-validation/ci-release'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
function Invoke-Check([string]$Name, [scriptblock]$Command) {
    Write-Host "Running $Name"
    & $Command *> (Join-Path $logs ($Name + '.log'))
    if ($LASTEXITCODE -ne 0) {
        Get-Content -LiteralPath (Join-Path $logs ($Name + '.log')) -Tail 80
        throw "$Name failed with exit code $LASTEXITCODE."
    }
}
Push-Location -LiteralPath $taskRoot
try {
    $metadata = cargo metadata --format-version 1 --no-deps --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Workspace metadata failed.' }
    $desktop = $metadata.packages | Where-Object name -eq 'audiolib-desktop'
    if (!$desktop -or $Tag -cne ('v' + $desktop.version)) { throw 'Tag and Cargo version must match.' }
    $commit = (git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Unable to identify checkout commit.' }
    $tagCommit = (git rev-parse "$Tag`^{commit}").Trim()
    if ($LASTEXITCODE -ne 0 -or $tagCommit -ne $commit) { throw 'Build must check out the tagged commit.' }
    Invoke-Check 'format' { cargo fmt --all -- --check }
    Invoke-Check 'clippy' { cargo clippy --workspace --all-targets --locked -- -D warnings }
    # Hosted runners have no audio output device; substring skip also covers the child_ variant.
    $skipArgs = @('--skip', 'delayed_ipc_reload_play_is_cancelled_by_stop')
    Invoke-Check 'tests' { cargo test --workspace --all-targets --locked -- @skipArgs }
    . (Join-Path $PSScriptRoot 'prepare-asio.ps1')
    Invoke-Check 'asio-clippy' { cargo clippy --workspace --all-targets --locked --features 'audiolib-desktop/asio,audiolib-cli/asio' -- -D warnings }
    Invoke-Check 'asio-tests' { cargo test --workspace --all-targets --locked --features 'audiolib-desktop/asio,audiolib-cli/asio' -- @skipArgs }
    & (Join-Path $PSScriptRoot 'build-release.ps1')
    & (Join-Path $PSScriptRoot 'package-portable.ps1') -SkipBuild
    & (Join-Path $PSScriptRoot 'validate-portable.ps1') -CleanCheckout
    $binaries = @{}
    foreach ($name in @('AudioLibExplorer.exe', 'AudioLibCli.exe')) {
        $binaries[$name] = (Get-FileHash -LiteralPath (Join-Path $taskRoot ('dist/latest/portable/' + $name)) -Algorithm SHA256).Hash
    }
    $report = @{
        success = $true; tag = $Tag; commit = $commit; version = $desktop.version
        checked_at = [DateTime]::UtcNow.ToString('o')
        checks = @('format','default strict Clippy','default workspace tests','ASIO strict Clippy','ASIO workspace tests','ASIO release build','clean-checkout portable validation')
        binaries = $binaries
        portable = Get-Content -LiteralPath (Join-Path $taskRoot 'doc/Rust_Portable.json') -Raw | ConvertFrom-Json
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $taskRoot 'doc/Rust_CI_Release.json') -Encoding UTF8
    # Include the report from this exact build in the downloadable package.
    & (Join-Path $PSScriptRoot 'package-portable.ps1') -SkipBuild
    Write-Host "Release checks passed for $Tag ($commit)."
} finally { Pop-Location }
