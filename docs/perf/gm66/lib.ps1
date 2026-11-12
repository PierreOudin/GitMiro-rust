# Shared helpers for the GM-66 benchmark. Dot-source: . ./lib.ps1
# Both apps are driven the same way: Win32 mouse clicks at fixed coordinates and screen-capture matching,
# so neither app gets an accessibility tree switched on or a debugger attached during memory/CPU runs.
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Win {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int w, int hgt, uint f);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
  [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint f);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static IntPtr[] TopWindows(uint[] pids) {
    var res = new System.Collections.Generic.List<IntPtr>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (Array.IndexOf(pids, p) >= 0 && IsWindowVisible(h) && GetWindowTextLength(h) > 0) res.Add(h); return true; }, IntPtr.Zero);
    return res.ToArray();
  }
  public static void Click(int x, int y) { SetCursorPos(x, y); mouse_event(2, 0, 0, 0, UIntPtr.Zero); mouse_event(4, 0, 0, 0, UIntPtr.Zero); }
}
'@

# Ask Windows not to blank the display while the script runs (per-thread request, no system setting is changed).
function Enable-KeepAwake { [void][Win]::SetThreadExecutionState([uint32]2147483651) }

function Get-Tree([int]$RootPid) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
  $ids = [System.Collections.Generic.List[int]]::new(); $ids.Add($RootPid)
  for ($i = 0; $i -lt $ids.Count; $i++) { foreach ($c in $all) { if ($c.ParentProcessId -eq $ids[$i] -and -not $ids.Contains([int]$c.ProcessId)) { $ids.Add([int]$c.ProcessId) } } }
  $ids
}

# One sample of the whole process tree. WS = working set (counts shared pages in every process, so summing double counts),
# PB = private bytes (commit), PWS = private working set (no shared pages, safe to sum), CPU = cumulative CPU seconds.
# RootCPU = the launched process only; Detail = per-process rows.
function Get-TreeSample([int]$RootPid) {
  $ids = Get-Tree $RootPid
  $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter ("IDProcess=" + ($ids -join " OR IDProcess="))
  $pws = @{}; foreach ($p in $perf) { $pws[[int]$p.IDProcess] = [double]$p.WorkingSetPrivate }
  $ws = 0.0; $pb = 0.0; $pw = 0.0; $cpu = 0.0; $rootCpu = 0.0; $detail = @()
  foreach ($id in $ids) {
    $p = Get-Process -Id $id -ErrorAction SilentlyContinue; if (-not $p) { continue }
    try {   # a WebView2 helper can exit between the lookup and the property reads
      $w = $p.WorkingSet64; $b = $p.PrivateMemorySize64; $c = $p.TotalProcessorTime.TotalSeconds
    } catch { continue }
    $ws += $w; $pb += $b; $pw += $pws[$id]; $cpu += $c
    if ($id -eq $RootPid) { $rootCpu = $c }
    $detail += [pscustomobject]@{ Id = $id; Name = $p.ProcessName; WS_MiB = [math]::Round($w / 1MB, 1); PB_MiB = [math]::Round($b / 1MB, 1); PWS_MiB = [math]::Round($pws[$id] / 1MB, 1); CPU_s = [math]::Round($c, 3) }
  }
  [pscustomobject]@{ Procs = $detail.Count; WS_MiB = $ws / 1MB; PB_MiB = $pb / 1MB; PWS_MiB = $pw / 1MB; CPU_s = $cpu; RootCPU_s = $rootCpu; Detail = $detail }
}

function Get-AppWindow([int]$RootPid) {
  $w = [Win]::TopWindows([uint32[]](Get-Tree $RootPid)); if ($w.Count) { $w[0] } else { [IntPtr]::Zero }
}

# Fixed outer window geometry so both apps render the same area.
$script:WinX = 100; $script:WinY = 60; $script:WinW = 1280; $script:WinH = 800
function Set-AppWindow([IntPtr]$h) {
  [void][Win]::SetWindowPos($h, [IntPtr]::Zero, $script:WinX, $script:WinY, $script:WinW, $script:WinH, 0x0040); [void][Win]::SetForegroundWindow($h)
}
# Click at window-relative coordinates, then park the cursor outside the window so hover styles are identical across runs.
function Click-Rel([int]$x, [int]$y) { [Win]::Click($script:WinX + $x, $script:WinY + $y); Start-Sleep -Milliseconds 30; [void][Win]::SetCursorPos(1700, 500) }

# Perceptual signature of the window: 32x20 grid of average luminance (byte[640]).
function Get-Frame([IntPtr]$h) {
  $r = New-Object Win+RECT; [void][Win]::GetWindowRect($h, [ref]$r)
  $w = $r.R - $r.L; $hh = $r.B - $r.T; if ($w -le 0 -or $hh -le 0) { return $null }
  $bmp = New-Object System.Drawing.Bitmap $w, $hh
  $g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size); $g.Dispose()
  $small = New-Object System.Drawing.Bitmap 32, 20; $g2 = [System.Drawing.Graphics]::FromImage($small)
  $g2.InterpolationMode = 'HighQualityBilinear'; $g2.DrawImage($bmp, 0, 0, 32, 20); $g2.Dispose(); $bmp.Dispose()
  $out = New-Object byte[] 640
  for ($y = 0; $y -lt 20; $y++) { for ($x = 0; $x -lt 32; $x++) { $c = $small.GetPixel($x, $y); $out[$y * 32 + $x] = [byte](($c.R + $c.G + $c.B) / 3) } }
  $small.Dispose(); , $out
}
# Number of grid cells whose luminance differs by more than 12 levels.
function Get-FrameDiff([byte[]]$a, [byte[]]$b) { $n = 0; for ($i = 0; $i -lt 640; $i++) { if ([Math]::Abs($a[$i] - $b[$i]) -gt 12) { $n++ } }; $n }
function Save-Golden([byte[]]$f, [string]$path) { [IO.File]::WriteAllBytes($path, $f) }
function Load-Golden([string]$path) { , [IO.File]::ReadAllBytes($path) }

# Ms (stopwatch $T0) of the FIRST frame that matches $Golden: <= $Tol differing cells and closer to $Golden than to $Other
# (the neighbouring state's golden). Frames that merely flash (e.g. a console window of a spawned git.exe covering the app)
# do not match either golden, so the first clean frame wins. Returns $null on timeout. Noise floor between runs is ~20 cells.
function Wait-Golden([IntPtr]$h, [System.Diagnostics.Stopwatch]$T0, [byte[]]$Golden, [byte[]]$Other, [int]$Tol = 36, [int]$TimeoutMs = 60000) {
  $deadline = $T0.ElapsedMilliseconds + $TimeoutMs
  while ($T0.ElapsedMilliseconds -lt $deadline) {
    $f = Get-Frame $h
    if ($f) { $d = Get-FrameDiff $f $Golden; if ($d -le $Tol -and $d -lt (Get-FrameDiff $f $Other)) { return $T0.ElapsedMilliseconds } }
    Start-Sleep -Milliseconds 20
  }
  $null
}

function Get-Stats([double[]]$v) {
  if (-not $v -or $v.Count -eq 0) { return $null }
  $s = $v | Sort-Object; $n = $s.Count
  # nearest-rank percentile: the value at rank ceil(p*n)
  $pct = { param($p) $s[[Math]::Min($n - 1, [Math]::Max(0, [int][Math]::Ceiling($p * $n) - 1))] }
  $mean = ($s | Measure-Object -Average).Average
  $sd = if ($n -gt 1) { [Math]::Sqrt((($s | ForEach-Object { ($_ - $mean) * ($_ - $mean) } | Measure-Object -Sum).Sum) / ($n - 1)) } else { 0 }
  [pscustomobject]@{ N = $n; Min = $s[0]; P50 = (& $pct 0.5); P95 = (& $pct 0.95); Max = $s[-1]; Mean = $mean; StdDev = $sd }
}

# Stop only the process tree rooted at $RootPid (never other apps' WebView2 or JVM processes).
function Stop-Tree([int]$RootPid) {
  foreach ($id in (Get-Tree $RootPid | Sort-Object -Descending)) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
}
