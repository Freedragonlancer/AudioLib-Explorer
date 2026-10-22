# tools/ci-release.ps1
[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')][string]$Tag)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
Push-Location -LiteralPath $taskRoot
try {
    $metadata = cargo metadata --format-version 1 --no-deps --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Workspace metadata failed.' }
    $desktop = $metadata.packages | Where-Object name -eq 'audiolib-desktop'
    # Pre-release tags (vX.Y.Z-rc.N / vX.Y.Z-beta.N) build the X.Y.Z workspace version.
    $tagCore = $Tag -replace '-(?:rc|beta)\.\d+$', ''
    if (!$desktop -or ($Tag -cne ('v' + $desktop.version) -and $tagCore -cne ('v' + $desktop.version))) { throw 'Tag and Cargo version must match.' }
    $commit = (git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Unable to identify checkout commit.' }
    $tagCommit = (git rev-parse "$Tag`^{commit}").Trim()
    if ($LASTEXITCODE -ne 0 -or $tagCommit -ne $commit) { throw 'Build must check out the tagged commit.' }
    # Full Rust suites and data-safety regressions run locally via
    # tools/check-local.ps1 -Asio; cloud release checks cover the shipped payload.
    & (Join-Path $PSScriptRoot 'build-release.ps1')
    & (Join-Path $PSScriptRoot 'package-portable.ps1') -SkipBuild
    $binaries = @{}
    foreach ($name in @('AudioLibExplorer.exe', 'AudioLibCli.exe')) {
        $binaries[$name] = (Get-FileHash -LiteralPath (Join-Path $taskRoot ('dist/latest/portable/' + $name)) -Algorithm SHA256).Hash
    }
    # Public reports have an explicit schema. Never publish raw local reports,
    # screenshots, runtime configuration or machine-specific artifact paths.
    $report = @{
        success = $true; tag = $Tag; commit = $commit; version = $desktop.version
        checked_at = [DateTime]::UtcNow.ToString('o')
        checks = @('tag/version/commit identity','ASIO release build','public portable payload allowlist/privacy','publication report schema and binary identity')
        binaries = $binaries
        portable = @{
            success = $null
            mode = 'not-run'
            records = $null
            source_unchanged = $null
            python_runtime_required = $null
        }
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $taskRoot 'doc/Rust_CI_Release.json') -Encoding UTF8
    & (Join-Path $PSScriptRoot 'validate-package-privacy.ps1') -ReleaseReport (Join-Path $taskRoot 'doc/Rust_CI_Release.json')
    Write-Host "Release build/package checks passed for $Tag ($commit); runtime validation was not run."
} finally { Pop-Location }
