# tools/test-release-assets.ps1
# Synthetic, offline regressions for exact platform assets and release digests.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$validator = Join-Path $PSScriptRoot 'verify-release-assets.ps1'
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar)
$testRoot = [IO.Path]::GetFullPath((Join-Path $tempParent ('AudioLibReleaseAssets-' + [guid]::NewGuid().ToString('N'))))
if (!$testRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Test root escaped its temporary parent.' }
New-Item -ItemType Directory -Path $testRoot | Out-Null
$script:caseNumber = 0
$script:assertionCount = 0

function Packages([string]$ReleaseTag, [bool]$MacClient) {
    "AudioLibExplorer-$ReleaseTag-win-x64-portable.zip"
    if ($MacClient) { "AudioLibExplorer-$ReleaseTag-macos-arm64.zip" }
    "AudioLibServer-$ReleaseTag-macos-arm64.tar.gz"
}
function AssetNames([string[]]$PackageNames) {
    foreach ($packageName in $PackageNames) { $packageName; "$packageName.sha256" }
}
function Manifest([string]$ReleaseTag, [string[]]$Names, [object]$Prerelease = $ReleaseTag.Contains('-'), [string]$Title = "AudioLib Explorer $ReleaseTag", [string]$ActualTag = $ReleaseTag) {
    $script:caseNumber++
    $path = Join-Path $testRoot ("release-$script:caseNumber.json")
    $metadata = [ordered]@{tagName=$ActualTag; name=$Title; isPrerelease=$Prerelease; assets=@($Names | ForEach-Object { [ordered]@{name=$_} })}
    [IO.File]::WriteAllText($path, ($metadata | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
    $path
}
function Accepted([string]$ReleaseTag, [string]$Metadata, [string[]]$ExpectedPackages, [string]$Directory) {
    $arguments = @{Tag=$ReleaseTag; ReleaseJsonPath=$Metadata}
    if ($Directory) { $arguments.AssetDirectory = $Directory }
    $actual = @(& $validator @arguments 6>$null)
    if ($actual.Count -ne $ExpectedPackages.Count -or ($actual -join '|') -cne ($ExpectedPackages -join '|')) { throw 'Verifier returned the wrong package set.' }
    $script:assertionCount++
}
function Rejected([string]$Case, [scriptblock]$Action, [string]$Message) {
    $rejected = $false
    try { & $Action 6>$null | Out-Null } catch {
        if ($_.Exception.Message -notmatch $Message) { throw "Wrong failure for ${Case}: $($_.Exception.Message)" }
        $rejected = $true
    }
    if (!$rejected) { throw "Invalid case was accepted: $Case" }
    $script:assertionCount++
}
function Payloads([string]$Directory, [string[]]$PackageNames) {
    New-Item -ItemType Directory -Force -Path $Directory | Out-Null
    foreach ($packageName in $PackageNames) {
        [IO.File]::WriteAllText((Join-Path $Directory $packageName), "synthetic $packageName", [Text.UTF8Encoding]::new($false))
        $hash = (Get-FileHash -LiteralPath (Join-Path $Directory $packageName) -Algorithm SHA256).Hash
        [IO.File]::WriteAllText((Join-Path $Directory "$packageName.sha256"), "$hash  $packageName`n", [Text.Encoding]::ASCII)
    }
}

try {
    foreach ($releaseTag in @('v0.7.8', 'v0.8.2', 'v0.8.2-beta.2')) {
        $packages = @(Packages $releaseTag $false)
        $metadata = Manifest $releaseTag @(AssetNames $packages)
        Accepted $releaseTag $metadata $packages
    }
    foreach ($releaseTag in @('v0.8.3', 'v0.8.3-beta.1', 'v0.8.3-rc.0', 'v0.9.0', 'v1.0.0')) {
        $packages = @(Packages $releaseTag $true)
        $metadata = Manifest $releaseTag @(AssetNames $packages)
        Accepted $releaseTag $metadata $packages
    }

    $tag = 'v0.8.3'
    $packages = @(Packages $tag $true)
    $names = @(AssetNames $packages)
    $metadata = Manifest $tag $names
    $missingMac = Manifest $tag @(AssetNames @(Packages $tag $false))
    Rejected 'new release missing Mac client' { & $validator -Tag $tag -ReleaseJsonPath $missingMac } 'Missing release assets'
    $oldExtra = Manifest 'v0.8.2' @(AssetNames @(Packages 'v0.8.2' $true))
    Rejected 'historical release with extra package' { & $validator -Tag 'v0.8.2' -ReleaseJsonPath $oldExtra } 'Unexpected release assets'
    $extra = Manifest $tag @($names + 'private-report.json')
    Rejected 'extra asset' { & $validator -Tag $tag -ReleaseJsonPath $extra } 'Unexpected release assets'
    $duplicate = Manifest $tag @($names + $names[0])
    Rejected 'duplicate asset' { & $validator -Tag $tag -ReleaseJsonPath $duplicate } 'Duplicate or case-colliding'
    $caseDuplicate = Manifest $tag @($names + $names[0].ToLowerInvariant())
    Rejected 'case-colliding asset' { & $validator -Tag $tag -ReleaseJsonPath $caseDuplicate } 'Duplicate or case-colliding'
    $wrongCaseNames = @($names)
    $wrongCaseNames[0] = $wrongCaseNames[0].ToLowerInvariant()
    $wrongCase = Manifest $tag $wrongCaseNames
    Rejected 'wrong-case asset' { & $validator -Tag $tag -ReleaseJsonPath $wrongCase } 'Missing release assets.*Unexpected release assets'
    $wrongTitle = Manifest $tag $names $false 'AudioLib Explorer 0.8.3'
    Rejected 'wrong title' { & $validator -Tag $tag -ReleaseJsonPath $wrongTitle } 'Release title'
    $wrongTag = Manifest $tag $names $false "AudioLib Explorer $tag" 'v0.8.2'
    Rejected 'wrong actual tag' { & $validator -Tag $tag -ReleaseJsonPath $wrongTag } 'Release tag'
    $wrongStable = Manifest $tag $names $true
    Rejected 'stable marked prerelease' { & $validator -Tag $tag -ReleaseJsonPath $wrongStable } 'prerelease flag'
    $betaTag = 'v0.8.3-beta.1'
    $wrongBeta = Manifest $betaTag @(AssetNames @(Packages $betaTag $true)) $false
    Rejected 'beta marked stable' { & $validator -Tag $betaTag -ReleaseJsonPath $wrongBeta } 'prerelease flag'
    $wrongType = Manifest $tag $names 'false'
    Rejected 'string prerelease flag' { & $validator -Tag $tag -ReleaseJsonPath $wrongType } 'prerelease flag'
    foreach ($badTag in @('0.8.3', 'v0.08.3', 'v0.8.3-beta.01', 'v0.8.3-preview.1', '../v0.8.3')) {
        Rejected "invalid tag $badTag" { & $validator -Tag $badTag -ReleaseJsonPath $metadata } 'Tag.*canonical'
    }
    $arrayMetadata = Join-Path $testRoot 'array-metadata.json'
    [IO.File]::WriteAllText($arrayMetadata, ('[' + [IO.File]::ReadAllText($metadata) + ']'), [Text.UTF8Encoding]::new($false))
    Rejected 'array-shaped metadata' { & $validator -Tag $tag -ReleaseJsonPath $arrayMetadata } 'one JSON object'

    $downloads = Join-Path $testRoot 'downloads'
    Payloads $downloads $packages
    Accepted $tag $metadata $packages $downloads
    $oldPackages = @(Packages 'v0.8.2' $false)
    $oldMetadata = Manifest 'v0.8.2' @(AssetNames $oldPackages)
    $oldDownloads = Join-Path $testRoot 'old-downloads'
    Payloads $oldDownloads $oldPackages
    Accepted 'v0.8.2' $oldMetadata $oldPackages $oldDownloads

    $chosenPackage = $packages[1]
    $chosenPath = Join-Path $downloads $chosenPackage
    $chosenChecksum = Join-Path $downloads "$chosenPackage.sha256"
    [IO.File]::AppendAllText($chosenPath, 'corrupted')
    Rejected 'corrupted archive' { & $validator -Tag $tag -ReleaseJsonPath $metadata -AssetDirectory $downloads } 'Checksum mismatch'
    Payloads $downloads $packages
    $digest = (Get-FileHash -LiteralPath $chosenPath -Algorithm SHA256).Hash
    [IO.File]::WriteAllText($chosenChecksum, "$($digest.ToLowerInvariant()) *$chosenPackage`n", [Text.Encoding]::ASCII)
    Accepted $tag $metadata $packages $downloads
    [IO.File]::WriteAllText($chosenChecksum, "$digest  WRONG.zip`n", [Text.Encoding]::ASCII)
    Rejected 'wrong checksum filename' { & $validator -Tag $tag -ReleaseJsonPath $metadata -AssetDirectory $downloads } 'Malformed checksum'
    [IO.File]::WriteAllText($chosenChecksum, "$digest  $chosenPackage`n$digest  $chosenPackage`n", [Text.Encoding]::ASCII)
    Rejected 'multiple checksums' { & $validator -Tag $tag -ReleaseJsonPath $metadata -AssetDirectory $downloads } 'Malformed checksum'
    [IO.File]::WriteAllText($chosenChecksum, "bad  $chosenPackage`n", [Text.Encoding]::ASCII)
    Rejected 'invalid digest' { & $validator -Tag $tag -ReleaseJsonPath $metadata -AssetDirectory $downloads } 'Malformed checksum'
    Payloads $downloads $packages
    Move-Item -LiteralPath $chosenPath -Destination ($chosenPath + '.held')
    Rejected 'missing downloaded archive' { & $validator -Tag $tag -ReleaseJsonPath $metadata -AssetDirectory $downloads } 'Downloaded asset is missing'
    Move-Item -LiteralPath ($chosenPath + '.held') -Destination $chosenPath
    Move-Item -LiteralPath $chosenChecksum -Destination ($chosenChecksum + '.held')
    Rejected 'missing checksum sidecar' { & $validator -Tag $tag -ReleaseJsonPath $metadata -AssetDirectory $downloads } 'Downloaded asset is missing'
    Move-Item -LiteralPath ($chosenChecksum + '.held') -Destination $chosenChecksum

    Write-Host "Release asset regression passed: $script:assertionCount checks; historical four/new six assets, tag/title/prerelease identity and SHA256 boundaries."
} finally {
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    if (!$resolvedRoot.StartsWith($tempParent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolvedRoot) -notmatch '^AudioLibReleaseAssets-[0-9a-f]{32}$') { throw 'Unsafe test cleanup target.' }
    if (Test-Path -LiteralPath $resolvedRoot) {
        if ((Get-Item -LiteralPath $resolvedRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint -or
            @(Get-ChildItem -LiteralPath $resolvedRoot -Recurse -Force -Attributes ReparsePoint).Count) { throw 'Linked test cleanup target.' }
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
}
