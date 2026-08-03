$ErrorActionPreference = 'SilentlyContinue'
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WinDiag2 {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);
  [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int nIndex);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string win);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr parent, IntPtr after, string cls, string win);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int left, top, right, bottom; }
}
"@

$GWL_STYLE = -16; $GWL_EXSTYLE = -20
$WS_CHILD = 0x40000000; $WS_EX_TOPMOST = 0x8

function Show-Win([IntPtr]$hwnd, [string]$label) {
  if ($hwnd -eq [IntPtr]::Zero) { Write-Output "$label : NULL"; return }
  $sb = New-Object System.Text.StringBuilder 256
  [WinDiag2]::GetClassName($hwnd, $sb, 256) | Out-Null
  $style = [WinDiag2]::GetWindowLongPtr($hwnd, $GWL_STYLE).ToInt64()
  $exstyle = [WinDiag2]::GetWindowLongPtr($hwnd, $GWL_EXSTYLE).ToInt64()
  $parent = [WinDiag2]::GetParent($hwnd)
  $visible = [WinDiag2]::IsWindowVisible($hwnd)
  $rect = New-Object WinDiag2+RECT
  [WinDiag2]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
  Write-Output ("{0}: hwnd=0x{1:X} class={2} style=0x{3:X} exstyle=0x{4:X} WS_CHILD={5} TOPMOST={6} parent=0x{7:X} visible={8} rect=({9},{10})-({11},{12})" -f $label, $hwnd.ToInt64(), $sb.ToString(), $style, $exstyle, (($style -band $WS_CHILD) -ne 0), (($exstyle -band $WS_EX_TOPMOST) -ne 0), $parent.ToInt64(), $visible, $rect.left, $rect.top, $rect.right, $rect.bottom)
}

$proc = Get-Process desktop-pet -ErrorAction SilentlyContinue | Select-Object -First 1
if ($proc) {
  Write-Output "=== desktop-pet (pid $($proc.Id)) ==="
  Show-Win $proc.MainWindowHandle "main-window"
  $child = [WinDiag2]::FindWindowExW($proc.MainWindowHandle, [IntPtr]::Zero, $null, $null)
  $n = 0
  while ($child -ne [IntPtr]::Zero -and $n -lt 10) {
    Show-Win $child "child-$n"
    $child = [WinDiag2]::FindWindowExW($proc.MainWindowHandle, $child, $null, $null)
    $n++
  }
} else {
  Write-Output "desktop-pet not running"
}

Write-Output "=== desktop layer windows ==="
Show-Win ([WinDiag2]::FindWindowW("Progman", $null)) "Progman"
$w = [WinDiag2]::FindWindowExW([IntPtr]::Zero, [IntPtr]::Zero, "WorkerW", $null)
$n = 0
while ($w -ne [IntPtr]::Zero -and $n -lt 10) {
  Show-Win $w "WorkerW-$n"
  $w = [WinDiag2]::FindWindowExW([IntPtr]::Zero, $w, "WorkerW", $null)
  $n++
}
$dv = [WinDiag2]::FindWindowW("SHELLDLL_DefView", $null)
Show-Win $dv "SHELLDLL_DefView"
