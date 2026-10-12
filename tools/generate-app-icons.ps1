# tools/generate-app-icons.ps1
# Windows authoring utility. Checked-in assets are used by ordinary Cargo builds.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$taskRoot = Split-Path -Parent $PSScriptRoot
$iconRoot = Join-Path $taskRoot 'resources/icons'
$source = [Drawing.Bitmap]::new((Join-Path $iconRoot 'AudioLibExplorer-source.png'))
$pngs = @{}
try {
    if ($source.Width -ne $source.Height -or $source.Width -lt 1024) {
        throw 'Icon source must be square and at least 1024 pixels.'
    }
    # Include intermediate Windows scales, plus all macOS 1x/2x representations.
    $windowsSizes = @(16,20,24,30,32,36,40,48,60,64,72,80,96,128,192,256)
    foreach ($size in @($windowsSizes + @(512,1024) | Sort-Object -Unique)) {
        $bitmap = [Drawing.Bitmap]::new($size, $size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        $stream = [IO.MemoryStream]::new()
        try {
            $graphics.CompositingMode = [Drawing.Drawing2D.CompositingMode]::SourceCopy
            $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $graphics.DrawImage($source, [Drawing.Rectangle]::new(0, 0, $size, $size))
            $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
            $pngs[$size] = $stream.ToArray()
        } finally { $stream.Dispose(); $graphics.Dispose(); $bitmap.Dispose() }
    }
} finally { $source.Dispose() }
[IO.File]::WriteAllBytes((Join-Path $iconRoot 'AudioLibExplorer.png'), $pngs[256])
[IO.File]::WriteAllBytes((Join-Path $iconRoot 'AudioLibExplorer-1024.png'), $pngs[1024])

# ICO directory uses little-endian integers and PNG-compressed 32-bit RGBA entries.
$writer = [IO.BinaryWriter]::new([IO.File]::Create((Join-Path $iconRoot 'AudioLibExplorer.ico')))
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$windowsSizes.Count)
    $offset = 6 + 16 * $windowsSizes.Count
    foreach ($size in $windowsSizes) {
        $dimension = [byte]$(if ($size -eq 256) { 0 } else { $size })
        $writer.Write($dimension); $writer.Write($dimension)
        $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$pngs[$size].Length); $writer.Write([uint32]$offset)
        $offset += $pngs[$size].Length
    }
    foreach ($size in $windowsSizes) { $writer.Write([byte[]]$pngs[$size]) }
} finally { $writer.Dispose() }

$iconset = Join-Path $iconRoot 'AudioLibExplorer.iconset'
New-Item -ItemType Directory -Force -Path $iconset | Out-Null
foreach ($size in @(16,32,128,256,512)) {
    [IO.File]::WriteAllBytes((Join-Path $iconset "icon_${size}x${size}.png"), $pngs[$size])
    [IO.File]::WriteAllBytes((Join-Path $iconset "icon_${size}x${size}@2x.png"), $pngs[($size * 2)])
}

# Modern ICNS chunks contain PNGs. Retina type codes preserve logical size and scale.
$chunks = [ordered]@{ icp4=16; icp5=32; ic07=128; ic08=256; ic09=512; ic10=1024; ic11=32; ic12=64; ic13=256; ic14=512 }
function Write-BigEndianUInt32([IO.BinaryWriter]$Writer, [uint32]$Value) {
    $bytes = [BitConverter]::GetBytes($Value)
    if ([BitConverter]::IsLittleEndian) { [Array]::Reverse($bytes) }
    $Writer.Write($bytes)
}
$writer = [IO.BinaryWriter]::new([IO.File]::Create((Join-Path $iconRoot 'AudioLibExplorer.icns')))
try {
    $total = 8
    foreach ($size in $chunks.Values) { $total += 8 + $pngs[$size].Length }
    $writer.Write([Text.Encoding]::ASCII.GetBytes('icns'))
    Write-BigEndianUInt32 $writer $total
    foreach ($chunk in $chunks.GetEnumerator()) {
        $bytes = $pngs[$chunk.Value]
        $writer.Write([Text.Encoding]::ASCII.GetBytes($chunk.Key))
        Write-BigEndianUInt32 $writer ($bytes.Length + 8)
        $writer.Write([byte[]]$bytes)
    }
} finally { $writer.Dispose() }
Write-Host "Generated Windows ICO ($($windowsSizes.Count) sizes), macOS ICNS/iconset and PNG assets in $iconRoot"
