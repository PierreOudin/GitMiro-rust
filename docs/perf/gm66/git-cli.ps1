# GM-66: wall-clock of the Git CLI commands GitMiro (Tauri) runs, from a console PowerShell, process start included.
# Usage: ./git-cli.ps1 -Fixture <repo> [-Runs 10] [-Out results.json]
param([Parameter(Mandatory)][string]$Fixture, [int]$Runs = 10, [string]$Out)
. "$PSScriptRoot\lib.ps1"
$tips = ((git -C $Fixture for-each-ref --format='%(objectname)' refs/heads refs/tags) + (git -C $Fixture rev-parse HEAD)) -join "`n"
$logFmt = '--format=%H%x00%P%x00%<(256,trunc)%s%x00%<(256,trunc)%an%x00%aI%x00'
$cmds = [ordered]@{
  'rev-parse --show-toplevel'    = { git -C $Fixture rev-parse --show-toplevel | Out-Null }
  'for-each-ref (heads+tags)'    = { git -C $Fixture for-each-ref '--format=%(*objectname)%00%(*objecttype)%00%(objectname)%00%(objecttype)%00%(refname)' refs/heads refs/remotes refs/tags | Out-Null }
  'log --stdin --date-order 501' = { $tips | git -C $Fixture log --stdin --date-order --skip=0 --max-count=501 $logFmt | Out-Null }
  'log --stdin --skip=500'       = { $tips | git -C $Fixture log --stdin --date-order --skip=500 --max-count=501 $logFmt | Out-Null }
  'status porcelain=v2'          = { git -C $Fixture status --porcelain=v2 -z --untracked-files=all | Out-Null }
  'diff src/file0.txt'           = { git -C $Fixture diff -- src/file0.txt | Out-Null }
  'open sequence (14 spawns)'    = {
    git -C $Fixture rev-parse --show-toplevel | Out-Null; git -C $Fixture rev-parse --verify --quiet 'HEAD^{commit}' | Out-Null
    git -C $Fixture for-each-ref '--format=%(*objectname)%00%(*objecttype)%00%(objectname)%00%(objecttype)%00%(refname)' refs/heads refs/remotes refs/tags | Out-Null
    $tips | git -C $Fixture log --stdin --date-order --skip=0 --max-count=501 $logFmt | Out-Null
    git -C $Fixture symbolic-ref --quiet --short HEAD | Out-Null; git -C $Fixture status --porcelain=v2 -z --untracked-files=all | Out-Null
    1..2 | ForEach-Object { git -C $Fixture rev-parse --path-format=absolute --git-path MERGE_MSG | Out-Null }
    git -C $Fixture rev-parse --verify --quiet 'HEAD^{commit}' | Out-Null; git -C $Fixture log -1 '--format=%B' | Out-Null
    git -C $Fixture config --local --get user.name | Out-Null; git -C $Fixture config --local --get user.email | Out-Null
    git -C $Fixture config --global --get user.name | Out-Null; git -C $Fixture config --global --get user.email | Out-Null
  }
}
$res = [ordered]@{}
foreach ($k in $cmds.Keys) {
  & $cmds[$k]  # warm-up, not counted
  $v = 1..$Runs | ForEach-Object { (Measure-Command { & $cmds[$k] }).TotalMilliseconds }
  $res[$k] = Get-Stats $v
}
$res.GetEnumerator() | ForEach-Object { '{0,-30} n={1} min={2:N1} p50={3:N1} p95={4:N1} max={5:N1} sd={6:N1} ms' -f $_.Key, $_.Value.N, $_.Value.Min, $_.Value.P50, $_.Value.P95, $_.Value.Max, $_.Value.StdDev }
if ($Out) { $res | ConvertTo-Json -Depth 4 | Set-Content $Out }
