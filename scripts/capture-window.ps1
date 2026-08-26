<#
.SYNOPSIS
  Screenshot the REAL running Pushin window (Tauri + the Rust core), not the mocked-IPC browser build.

.DESCRIPTION
  `npm run capture` (tests/e2e/_capture.spec.ts) drives the React app in Chromium against a FAKE
  backend, so it can't show anything the Rust core computed — parser output, scheduled blocks,
  conflicts. This captures the actual desktop window instead, which is the only way to see the real
  pipeline end to end.

  Two capture paths, tried in order:
    1. PrintWindow with PW_RENDERFULLCONTENT — works while the window is behind others.
    2. Foreground + CopyFromScreen — fallback for when the WebView2 surface won't print
       (hardware-accelerated content sometimes comes back blank).
  A frame that is >99% one flat colour is treated as blank and triggers the fallback.

.EXAMPLE
  npm run tauri dev            # in another shell — leave it running
  pwsh scripts/capture-window.ps1 -Out target/ui-shots/real-app.png
#>
[CmdletBinding()]
param(
  [string]$ProcessName = "pushin",
  [string]$Out = "target/ui-shots/real-app.png",
  [int]$TimeoutSec = 180,
  # How many 1s-spaced PrintWindow attempts to make while waiting for the WebView to paint.
  [int]$PaintRetries = 20
)

Add-Type -AssemblyName System.Drawing, System.Windows.Forms

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Cap {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

try { [Win32Cap]::SetProcessDpiAwareness(2) | Out-Null } catch { }  # per-monitor DPI: capture real pixels

# --- wait for the window (a `tauri dev` build can take minutes to compile before it appears) ---
$deadline = (Get-Date).AddSeconds($TimeoutSec)
$hwnd = [IntPtr]::Zero
while ((Get-Date) -lt $deadline) {
  $p = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue |
       Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if ($p) { $hwnd = $p.MainWindowHandle; break }
  Start-Sleep -Milliseconds 500
}
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "no '$ProcessName' window within ${TimeoutSec}s"; exit 1 }

$r = New-Object Win32Cap+RECT
[Win32Cap]::GetWindowRect($hwnd, [ref]$r) | Out-Null
$w = $r.Right - $r.Left
$h = $r.Bottom - $r.Top
if ($w -le 0 -or $h -le 0) { Write-Error "window has no size ($w x $h)"; exit 1 }

function Test-Blank([System.Drawing.Bitmap]$bmp) {
  # Sample a grid over the INTERIOR only. The window border/title chrome is a different colour from
  # the client area, so sampling edge pixels makes an entirely unpainted frame look non-uniform —
  # which is exactly how an all-white "WebView2 hasn't painted yet" capture slipped through.
  $ix = [int]($bmp.Width * 0.1); $iy = [int]($bmp.Height * 0.1)
  $w = $bmp.Width - 2 * $ix; $h = $bmp.Height - 2 * $iy
  if ($w -lt 8 -or $h -lt 8) { return $false }
  $first = $null; $same = 0; $n = 0
  for ($x = $ix; $x -lt $ix + $w; $x += [Math]::Max(1, [int]($w / 40))) {
    for ($y = $iy; $y -lt $iy + $h; $y += [Math]::Max(1, [int]($h / 40))) {
      $c = $bmp.GetPixel($x, $y).ToArgb(); $n++
      if ($null -eq $first) { $first = $c; $same++ } elseif ($c -eq $first) { $same++ }
    }
  }
  return ($n -gt 0 -and ($same / $n) -gt 0.99)
}

function Get-PrintWindowShot([IntPtr]$hwnd, [int]$w, [int]$h) {
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc()
  $ok = [Win32Cap]::PrintWindow($hwnd, $hdc, 2)   # 2 = PW_RENDERFULLCONTENT
  $g.ReleaseHdc($hdc); $g.Dispose()
  if (-not $ok) { $bmp.Dispose(); return $null }
  return $bmp
}

function Save-Capture([System.Drawing.Bitmap]$bmp, [string]$path) {
  $dir = Split-Path -Parent $path
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
}

# --- 1. PrintWindow (works without stealing focus), retried until the WebView actually paints ---
# The window handle exists well before the web content renders, so a single shot right after launch
# reliably returns a blank frame.
$method = "PrintWindow"
$bmp = $null
for ($try = 0; $try -lt $PaintRetries; $try++) {
  if ($bmp) { $bmp.Dispose() }
  $bmp = Get-PrintWindowShot $hwnd $w $h
  if ($bmp -and -not (Test-Blank $bmp)) { break }
  Start-Sleep -Milliseconds 1000
}

if (-not $bmp -or (Test-Blank $bmp)) {
  # --- 2. foreground + screen grab ---
  $method = "CopyFromScreen"
  if ($bmp) { $bmp.Dispose() }
  [Win32Cap]::ShowWindow($hwnd, 9) | Out-Null      # SW_RESTORE
  [Win32Cap]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 900
  [Win32Cap]::GetWindowRect($hwnd, [ref]$r) | Out-Null
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $g.Dispose()
}

Save-Capture $bmp $Out
$blank = Test-Blank $bmp
$bmp.Dispose()
Write-Output "saved $Out (${w}x${h}) via $method$(if ($blank) { ' — WARNING: looks blank' })"
