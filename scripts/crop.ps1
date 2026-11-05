param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Dest,
    [int]$X = 0,
    [int]$Y = 0,
    [int]$Width = 200,
    [int]$Height = 400,
    [int]$Scale = 2
)

Add-Type -AssemblyName System.Drawing

# NOTE: do not name a parameter `$Input` — it collides with PowerShell's
# automatic pipeline variable and silently arrives empty.
$resolved = (Resolve-Path -LiteralPath $Source).Path
$img = [System.Drawing.Image]::FromFile($resolved)
try {
    $w = [Math]::Min($Width, $img.Width - $X)
    $h = [Math]::Min($Height, $img.Height - $Y)
    if ($w -le 0 -or $h -le 0) { throw "crop rectangle is outside the image" }

    $crop = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($crop)
    $g.DrawImage(
        $img,
        (New-Object System.Drawing.Rectangle 0, 0, $w, $h),
        (New-Object System.Drawing.Rectangle $X, $Y, $w, $h),
        [System.Drawing.GraphicsUnit]::Pixel
    )
    $g.Dispose()

    $big = New-Object System.Drawing.Bitmap ($w * $Scale), ($h * $Scale)
    $g2 = [System.Drawing.Graphics]::FromImage($big)
    $g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
    $g2.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
    $g2.DrawImage($crop, 0, 0, $big.Width, $big.Height)
    $g2.Dispose()

    $dir = Split-Path -Parent $Dest
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
    $big.Save($Dest, [System.Drawing.Imaging.ImageFormat]::Png)
    $big.Dispose()
    $crop.Dispose()
} finally {
    $img.Dispose()
}

Write-Output "$Dest ($($w * $Scale) x $($h * $Scale))"
