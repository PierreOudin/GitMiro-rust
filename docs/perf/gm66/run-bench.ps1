# GM-66: Kotlin 2.0.1 vs Tauri release, same fixture, same driver. See docs/perf/gm66-benchmark-report.md.
#   ./run-bench.ps1 -Calibrate -KotlinExe <GitMiro.exe> -TauriExe <gitmiro.exe> -Fixture <repo>   # once: capture golden frames
#   ./run-bench.ps1 -Runs 10 -KotlinExe ... -TauriExe ... -Fixture ...                            # interleaved K,T,K,T...
# Requirements: AC power, nobody touching mouse/keyboard, Tauri recent list seeded with exactly the fixture path
# (single entry), apps' windows free to take the foreground. Output (outside Git): -Out folder, one JSONL per app.
param(
  [switch]$Calibrate,
  [int]$Runs = 5,
  [int]$StartRun = 1,   # resume: continue appending to the existing JSONL from this run number
  [int]$IdleSec = 300,
  [string]$Out = "$env:TEMP\gm66\results",
  [Parameter(Mandatory)][string]$Fixture,
  [Parameter(Mandatory)][string]$KotlinExe,
  [Parameter(Mandatory)][string]$TauriExe
)
. "$PSScriptRoot\lib.ps1"
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
Enable-KeepAwake

# Window-relative click targets at 1280x800 outer size, 100% DPI (measured from screenshots in calibration).
$Targets = @{
  tauri_recent = @(640, 412)     # first entry of the recent-repositories list
  tauri_diff   = @(1160, 555)    # "Diff" button of src/file0.txt (unstaged)
  kotlin_diff  = @(1010, 403)    # src/file0.txt row of the Unstaged list
}

function Start-App([string]$App) {
  if ($App -eq 'kotlin') { Start-Process $KotlinExe -ArgumentList "`"$Fixture`"" -PassThru } else { Start-Process $TauriExe -PassThru }
}

function Invoke-Run([string]$App, [int]$Idx) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-App $App
  try {
  $h = [IntPtr]::Zero
  # Get-AppWindow walks the process tree with WMI (about 200 ms per call), so window_ms has ~0.2 s resolution and is an upper bound.
  while ($h -eq [IntPtr]::Zero -and $sw.Elapsed.TotalSeconds -lt 60) { Start-Sleep -Milliseconds 20; $h = Get-AppWindow $p.Id }
  if ($h -eq [IntPtr]::Zero) { Stop-Tree $p.Id; throw "$App window did not appear" }
  $r = [ordered]@{ app = $App; run = $Idx; first = ($Idx -eq 1); window_ms = $sw.ElapsedMilliseconds }
  Set-AppWindow $h
  $g = @{}; foreach ($s in 'welcome', 'ready', 'diff') { $f = "$Out\golden-$App-$s.bin"; if (Test-Path $f) { $g[$s] = Load-Golden $f } }
  foreach ($s in 'ready', 'diff') { if (-not $g[$s]) { throw "golden-$App-$s.bin missing: run -Calibrate first" } }
  if ($App -eq 'tauri' -and -not $g.welcome) { throw 'golden-tauri-welcome.bin missing: run -Calibrate first' }
  if ($App -eq 'tauri') {
    $r.welcome_ms = Wait-Golden $h $sw $g.welcome $g.ready
    $r.click_ms = $sw.ElapsedMilliseconds; Click-Rel $Targets.tauri_recent[0] $Targets.tauri_recent[1]
  }
  $r.ready_ms = Wait-Golden $h $sw $g.ready $g.diff
  $r.ready_ok = ($null -ne $r.ready_ms)
  Start-Sleep -Seconds 5   # settle: let post-open git refreshes finish before the idle window
  $a0 = Get-TreeSample $p.Id; $ta0 = $sw.Elapsed.TotalSeconds
  $r.open = $a0 | Select-Object Procs, WS_MiB, PB_MiB, PWS_MiB, CPU_s, RootCPU_s
  Start-Sleep -Milliseconds ([Math]::Max(0, ($ta0 + 60 - $sw.Elapsed.TotalSeconds) * 1000))
  $a60 = Get-TreeSample $p.Id; $ta60 = $sw.Elapsed.TotalSeconds
  $r.idle60 = $a60 | Select-Object Procs, WS_MiB, PB_MiB, PWS_MiB, CPU_s, RootCPU_s
  $r.idle60_window_s = $ta60 - $ta0
  if ($IdleSec -gt 60) {
    Start-Sleep -Milliseconds ([Math]::Max(0, ($ta0 + $IdleSec - $sw.Elapsed.TotalSeconds) * 1000))
    $a300 = Get-TreeSample $p.Id
    $r.idle300 = $a300 | Select-Object Procs, WS_MiB, PB_MiB, PWS_MiB, CPU_s, RootCPU_s, Detail
  }
  # Open a file diff (src/file0.txt, unstaged) and time the click until the diff is visually complete.
  [void][Win]::SetForegroundWindow($h)
  $key = if ($App -eq 'tauri') { 'tauri_diff' } else { 'kotlin_diff' }
  $r.diff_click_ms = $sw.ElapsedMilliseconds; Click-Rel $Targets[$key][0] $Targets[$key][1]
  $d = Wait-Golden $h $sw $g.diff $g.ready
  $r.diff_response_ms = if ($null -ne $d) { $d - $r.diff_click_ms } else { $null }
  Start-Sleep -Seconds 3
  $d1 = Get-TreeSample $p.Id; $td1 = $sw.Elapsed.TotalSeconds
  Start-Sleep -Seconds 10
  $d2 = Get-TreeSample $p.Id; $td2 = $sw.Elapsed.TotalSeconds
  $r.diff = $d1 | Select-Object Procs, WS_MiB, PB_MiB, PWS_MiB, CPU_s, RootCPU_s
  $r.diff_cpu_window_s = $td2 - $td1; $r.diff_cpu_delta_s = $d2.CPU_s - $d1.CPU_s
  [pscustomobject]$r
  } finally { Stop-Tree $p.Id }   # never leave an app running into the next run
}

function Capture-Golden([string]$App) {
  $p = Start-App $App; $h = [IntPtr]::Zero
  while ($h -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100; $h = Get-AppWindow $p.Id }
  Set-AppWindow $h
  $shots = [ordered]@{}
  if ($App -eq 'tauri') {
    Start-Sleep -Seconds 4; $shots.welcome = Get-Frame $h
    Click-Rel $Targets.tauri_recent[0] $Targets.tauri_recent[1]
  }
  Start-Sleep -Seconds 14; [void][Win]::SetForegroundWindow($h); Start-Sleep -Milliseconds 500; $shots.ready = Get-Frame $h
  $again = Get-Frame $h
  $key = if ($App -eq 'tauri') { 'tauri_diff' } else { 'kotlin_diff' }
  Click-Rel $Targets[$key][0] $Targets[$key][1]; Start-Sleep -Seconds 5; $shots.diff = Get-Frame $h
  foreach ($k in $shots.Keys) { Save-Golden $shots[$k] "$Out\golden-$App-$k.bin" }
  Write-Host "$App goldens saved; ready self-diff cells: $(Get-FrameDiff $shots.ready $again); ready-vs-diff cells: $(Get-FrameDiff $shots.ready $shots.diff)"
  Stop-Tree $p.Id
}

if ($Calibrate) { foreach ($a in 'kotlin', 'tauri') { Capture-Golden $a; Start-Sleep -Seconds 3 }; return }

$git = (git --version); $os = Get-CimInstance Win32_OperatingSystem; $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
Add-Type -AssemblyName System.Windows.Forms
[ordered]@{
  date = (Get-Date).ToString('s'); cpu = $cpu.Name.Trim(); cores = $cpu.NumberOfCores; threads = $cpu.NumberOfLogicalProcessors
  ram_gib = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1); os_build = "$($os.Version) UBR $((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR)"
  git = $git; power_plan = ((powercfg /getactivescheme) -replace '^.*: ', ''); ac_line = [string][System.Windows.Forms.SystemInformation]::PowerStatus.PowerLineStatus
  webview2 = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}').pv
  tauri_sha256 = (Get-FileHash $TauriExe).Hash; fixture_head = (git -C $Fixture rev-parse HEAD); runs = $Runs; idle_sec = $IdleSec
} | ConvertTo-Json | Set-Content "$Out\env.json"

for ($i = $StartRun; $i -le $Runs; $i++) {
  foreach ($a in 'kotlin', 'tauri') {
    try { (Invoke-Run $a $i | ConvertTo-Json -Depth 6 -Compress) | Add-Content "$Out\$a.jsonl"; Write-Host "$a run $i done" }
    catch { Write-Host "$a run $i FAILED: $_"; ([ordered]@{ app = $a; run = $i; error = "$_" } | ConvertTo-Json -Compress) | Add-Content "$Out\$a.jsonl" }
    Start-Sleep -Seconds 5
  }
}
