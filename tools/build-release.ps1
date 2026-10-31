# tools/build-release.ps1
# Build only the portable client's two executables and their dependencies.
# Servers use build-server-release.ps1; benchmarks are built on demand.
[CmdletBinding()]
param([switch]$WithoutAsio)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$savedEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$nativeFlagKey = 'CXXFLAGS_x86_64_pc_windows_msvc'
$savedNativeFlags = [Environment]::GetEnvironmentVariable($nativeFlagKey)
$savedHyphenNativeFlags = [Environment]::GetEnvironmentVariable('CXXFLAGS_x86_64-pc-windows-msvc')
$flagSeparator = [char]31
# Cargo's encoded form preserves paths containing spaces. Keep caller flags and
# the workspace's static CRT choice when overriding target rustflags.
$releaseFlags = [Collections.Generic.List[string]]::new()
$releaseFlags.Add('-C')
$releaseFlags.Add('target-feature=+crt-static')
if ($savedEncodedFlags) {
    foreach ($flag in $savedEncodedFlags.Split($flagSeparator)) { $releaseFlags.Add($flag) }
} elseif ($env:RUSTFLAGS) {
    foreach ($flag in ($env:RUSTFLAGS -split '\s+' | Where-Object { $_ })) { $releaseFlags.Add($flag) }
}
$pathMappings = @{}
$pathMappings[[IO.Path]::GetFullPath($taskRoot)] = '/audiolib/source'
if ($env:CARGO_TARGET_DIR) { $pathMappings[[IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)] = '/audiolib/build' }
if ($env:USERPROFILE) { $pathMappings[$env:USERPROFILE] = '/audiolib/user' }
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$pathMappings[[IO.Path]::GetFullPath($cargoHome)] = '/audiolib/cargo'
$pathMappings[[IO.Path]::GetFullPath($rustupHome)] = '/audiolib/rustup'
$workspaceDrive = Get-PSDrive -Name ([IO.Path]::GetPathRoot($taskRoot).TrimEnd(':', '\')) -ErrorAction SilentlyContinue
if ($workspaceDrive.DisplayRoot) {
    $mappedRoot = $workspaceDrive.DisplayRoot.TrimEnd('\') + '\' + $taskRoot.Substring(3)
    $pathMappings[$mappedRoot] = '/audiolib/source'
}
foreach ($sourcePrefix in ($pathMappings.Keys | Sort-Object Length -Descending)) {
    $releaseFlags.Add('--remap-path-prefix=' + $sourcePrefix + '=' + $pathMappings[$sourcePrefix])
    $releaseFlags.Add('--remap-path-prefix=' + $sourcePrefix.Replace('\', '/') + '=' + $pathMappings[$sourcePrefix])
}
$env:CARGO_ENCODED_RUSTFLAGS = $releaseFlags -join $flagSeparator
# The bundled SoundTouch cc build does not define NDEBUG from Cargo's release
# profile. Its C++ debug assertions otherwise embed absolute wide __FILE__ paths
# that Rust remapping cannot change. Use the native release configuration;
# Rust/domain validation and SoundTouch's explicit runtime checks remain active.
$nativeFlags = if ($savedHyphenNativeFlags) { $savedHyphenNativeFlags } elseif ($savedNativeFlags) { $savedNativeFlags } else { $env:CXXFLAGS }
[Environment]::SetEnvironmentVariable($nativeFlagKey, ($nativeFlags + ' /DNDEBUG').Trim())
[Environment]::SetEnvironmentVariable('CXXFLAGS_x86_64-pc-windows-msvc', ($nativeFlags + ' /DNDEBUG').Trim())
Push-Location -LiteralPath $taskRoot
try {
    if (!$WithoutAsio) {
        . (Join-Path $PSScriptRoot 'prepare-asio.ps1')
        cargo build -p audiolib-desktop -p audiolib-cli --bins --release --locked --features 'audiolib-desktop/asio,audiolib-cli/asio'
    } else {
        cargo build -p audiolib-desktop -p audiolib-cli --bins --release --locked
    }
    if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed.' }
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedFlags
    [Environment]::SetEnvironmentVariable($nativeFlagKey, $savedNativeFlags)
    [Environment]::SetEnvironmentVariable('CXXFLAGS_x86_64-pc-windows-msvc', $savedHyphenNativeFlags)
    Pop-Location
}

