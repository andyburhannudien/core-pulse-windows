# Generates installer\core-pulse.ico without any external image dependency.
#
# The mark is a dark rounded tile with a cyan pulse trace: readable at 16 px
# and consistent with the UI's instrument-panel palette.

param(
    [string]$Output = (Join-Path (Split-Path -Parent $PSScriptRoot) "installer\core-pulse.ico")
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

function New-IconPng {
    param([int]$Size)

    $bmp = [System.Drawing.Bitmap]::new($Size, $Size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    $pad = [single]($Size * 0.04)
    $rect = [System.Drawing.RectangleF]::new($pad, $pad, [single]($Size - 2 * $pad), [single]($Size - 2 * $pad))
    $radius = [single]($Size * 0.22)
    $d = [single]($radius * 2)

    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $path.AddArc($rect.X, $rect.Y, $d, $d, 180, 90)
    $path.AddArc(($rect.Right - $d), $rect.Y, $d, $d, 270, 90)
    $path.AddArc(($rect.Right - $d), ($rect.Bottom - $d), $d, $d, 0, 90)
    $path.AddArc($rect.X, ($rect.Bottom - $d), $d, $d, 90, 90)
    $path.CloseFigure()

    $brush = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
        $rect,
        [System.Drawing.Color]::FromArgb(255, 8, 15, 30),
        [System.Drawing.Color]::FromArgb(255, 15, 35, 55),
        [single]45
    )
    $g.FillPath($brush, $path)

    $border = [System.Drawing.Pen]::new(
        [System.Drawing.Color]::FromArgb(120, 34, 211, 238),
        [single]([math]::Max(1.0, $Size * 0.012))
    )
    $g.DrawPath($border, $path)

    $points = @(
        @(0.16, 0.52), @(0.30, 0.52), @(0.36, 0.40), @(0.42, 0.68),
        @(0.48, 0.30), @(0.54, 0.62), @(0.60, 0.52), @(0.84, 0.52)
    ) | ForEach-Object {
        [System.Drawing.PointF]::new([single]($_[0] * $Size), [single]($_[1] * $Size))
    }

    $pen = [System.Drawing.Pen]::new(
        [System.Drawing.Color]::FromArgb(255, 34, 211, 238),
        [single]([math]::Max(1.2, $Size * 0.065))
    )
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $g.DrawLines($pen, [System.Drawing.PointF[]]$points)

    $dot = [single]([math]::Max(1.2, $Size * 0.045))
    $dotBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 163, 230, 53))
    $g.FillEllipse(
        $dotBrush,
        [single](0.84 * $Size - $dot),
        [single](0.52 * $Size - $dot),
        [single]($dot * 2),
        [single]($dot * 2)
    )

    $ms = [System.IO.MemoryStream]::new()
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $ms.ToArray()

    $ms.Dispose()
    $dotBrush.Dispose()
    $pen.Dispose()
    $border.Dispose()
    $brush.Dispose()
    $path.Dispose()
    $g.Dispose()
    $bmp.Dispose()

    # The comma prevents PowerShell from unrolling the byte array into the
    # pipeline, which would turn each image into a single byte.
    return ,$bytes
}

$sizes = @(16, 24, 32, 48, 64, 128, 256)
$images = @()
foreach ($size in $sizes) {
    $images += ,(New-IconPng -Size $size)
}

$stream = [System.IO.MemoryStream]::new()
$writer = [System.IO.BinaryWriter]::new($stream)
$writer.Write([UInt16]0)
$writer.Write([UInt16]1)
$writer.Write([UInt16]$sizes.Count)

$offset = 6 + (16 * $sizes.Count)
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $size = $sizes[$i]
    $data = $images[$i]
    $dimension = if ($size -ge 256) { 0 } else { $size }

    $writer.Write([byte]$dimension)
    $writer.Write([byte]$dimension)
    $writer.Write([byte]0)
    $writer.Write([byte]0)
    $writer.Write([UInt16]1)
    $writer.Write([UInt16]32)
    $writer.Write([UInt32]$data.Length)
    $writer.Write([UInt32]$offset)
    $offset += $data.Length
}
foreach ($data in $images) { $writer.Write($data) }
$writer.Flush()

$dir = Split-Path -Parent $Output
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
[System.IO.File]::WriteAllBytes($Output, $stream.ToArray())

$writer.Dispose()
$stream.Dispose()

Write-Host "wrote $Output"
