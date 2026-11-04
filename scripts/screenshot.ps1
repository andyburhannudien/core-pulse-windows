param(
    [Parameter(Mandatory = $true)][string]$Output,
    [string]$ProcessName = "core-pulse",
    [int]$DelayMs = 1400,
    [switch]$ClientArea,
    [int]$MoveX = -1,
    [int]$MoveY = -1,
    # Force the legacy BitBlt path (captures actual screen pixels, so anything
    # covering the window is included). Default uses PrintWindow so occluding
    # windows do not matter.
    [switch]$ScreenCopy
)

Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class WinShot {
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")]
    public static extern bool GetClientRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")]
    public static extern bool ClientToScreen(IntPtr hWnd, ref POINT point);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X, Y; }
}
"@

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 } |
    Select-Object -First 1

if (-not $proc) {
    Write-Error "No $ProcessName window found"
    exit 1
}

$hwnd = $proc.MainWindowHandle
if ([WinShot]::IsIconic($hwnd)) {
    [WinShot]::ShowWindow($hwnd, 9) | Out-Null
}

if ($MoveX -ge 0 -and $MoveY -ge 0) {
    # SWP_NOSIZE | SWP_NOZORDER
    [WinShot]::SetWindowPos($hwnd, [IntPtr]::Zero, $MoveX, $MoveY, 0, 0, 0x0001 -bor 0x0004) | Out-Null
}

[WinShot]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds $DelayMs

$x = 0
$y = 0
$width = 0
$height = 0

if ($ClientArea) {
    $client = New-Object WinShot+RECT
    [WinShot]::GetClientRect($hwnd, [ref]$client) | Out-Null
    $origin = New-Object WinShot+POINT
    $origin.X = 0
    $origin.Y = 0
    [WinShot]::ClientToScreen($hwnd, [ref]$origin) | Out-Null
    $x = $origin.X
    $y = $origin.Y
    $width = $client.Right - $client.Left
    $height = $client.Bottom - $client.Top
} else {
    $rect = New-Object WinShot+RECT
    [WinShot]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
    $x = $rect.Left
    $y = $rect.Top
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
}

if ($width -le 0 -or $height -le 0) {
    Write-Error "Invalid capture size"
    exit 1
}

$bmp = New-Object System.Drawing.Bitmap $width, $height
$gfx = [System.Drawing.Graphics]::FromImage($bmp)

if ($ScreenCopy) {
    $gfx.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
} else {
    # PrintWindow with PW_RENDERFULLCONTENT (2) asks the window to paint itself
    # into our HDC, so occluding windows are irrelevant. If it reports failure
    # (some GL surfaces refuse), fall back to a screen copy.
    $hdc = $gfx.GetHdc()
    $ok = $false
    try {
        $ok = [WinShot]::PrintWindow($hwnd, $hdc, 2)
    } finally {
        $gfx.ReleaseHdc($hdc)
    }
    if (-not $ok) {
        $gfx.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    }
}

$dir = Split-Path -Parent $Output
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$bmp.Save($Output, [System.Drawing.Imaging.ImageFormat]::Png)
$gfx.Dispose()
$bmp.Dispose()

Write-Output "$Output ($width x $height)"
