# tools/validate-portable-zip.ps1
# Regression guard for portable ZIP entry names. The updater only accepts
# forward-slash relative paths, so any `\` separator, directory entry, duplicate
# or absolute/parent name fails the package.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Archive,
    [string[]]$ExpectedEntries
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($Archive))
try { $names = @($zip.Entries | ForEach-Object { $_.FullName }) } finally { $zip.Dispose() }
if ($names.Count -eq 0) { throw 'Portable ZIP is empty.' }
$backslash = @($names | Where-Object { $_.Contains('\') })
if ($backslash.Count) { throw "Portable ZIP has $($backslash.Count) entry name(s) with '\' separators, e.g. $($backslash[0])" }
$invalid = @($names | Where-Object { $_.EndsWith('/') -or $_.StartsWith('/') -or $_ -match '(^|/)\.\.?(/|$)' -or $_ -match '^[A-Za-z]:' })
if ($invalid.Count) { throw "Portable ZIP has invalid entry name: $($invalid[0])" }
$seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($name in $names) { if (!$seen.Add($name)) { throw "Portable ZIP has duplicate entry: $name" } }
if ($PSBoundParameters.ContainsKey('ExpectedEntries')) {
    $expected = [Collections.Generic.HashSet[string]]::new([string[]]$ExpectedEntries, [StringComparer]::Ordinal)
    if ($names.Count -ne $expected.Count -or @($names | Where-Object { !$expected.Contains($_) }).Count) { throw 'Portable ZIP contents differ from the staged package files.' }
}
Write-Host "Portable ZIP entry names verified: $($names.Count) files, forward slashes only."
