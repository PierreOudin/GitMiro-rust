// GM-66: turn the raw results folder into Markdown tables. Usage: node summarize.mjs <resultsDir>
// Percentiles use the nearest-rank method: value at rank ceil(p*n) of the sorted samples; sd is the sample standard deviation.
import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const dir = process.argv[2];
const jsonl = (f) => (existsSync(join(dir, f)) ? readFileSync(join(dir, f), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : []);
const stats = (v) => {
  v = v.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b);
  const n = v.length;
  if (!n) return null;
  const pct = (p) => v[Math.min(n - 1, Math.max(0, Math.ceil(p * n) - 1))];
  const mean = v.reduce((a, b) => a + b, 0) / n;
  const sd = n > 1 ? Math.sqrt(v.reduce((a, b) => a + (b - mean) ** 2, 0) / (n - 1)) : 0;
  return { n, min: v[0], p50: pct(0.5), p95: pct(0.95), max: v[n - 1], sd };
};
const f = (x, d = 1) => (x == null ? "n/a" : x.toFixed(d));
const row = (label, s, d = 1) => (s ? `| ${label} | ${s.n} | ${f(s.min, d)} | ${f(s.p50, d)} | ${f(s.p95, d)} | ${f(s.max, d)} | ${f(s.sd, d)} |` : `| ${label} | 0 | Non mesurée | | | | |`);
const head = "| Metric | n | min | p50 | p95 | max | sd |\n|---|---|---|---|---|---|---|";

const apps = { kotlin: jsonl("kotlin.jsonl"), tauri: jsonl("tauri.jsonl") };
const errors = Object.entries(apps).flatMap(([a, rs]) => rs.filter((r) => r.error).map((r) => `${a} run ${r.run}: ${r.error}`));
const metric = {
  "Window visible (ms)": (r) => r.window_ms,
  "Welcome screen ready (ms)": (r) => r.welcome_ms,
  "Launch to repository ready (ms)": (r) => (r.ready_ok ? r.ready_ms : null),
  "Diff click to diff painted (ms)": (r) => r.diff_response_ms,
  "Open: working set (MiB)": (r) => r.open?.WS_MiB,
  "Open: private bytes (MiB)": (r) => r.open?.PB_MiB,
  "Open: private working set (MiB)": (r) => r.open?.PWS_MiB,
  "Idle 1 min: working set (MiB)": (r) => r.idle60?.WS_MiB,
  "Idle 1 min: private bytes (MiB)": (r) => r.idle60?.PB_MiB,
  "Idle 1 min: private working set (MiB)": (r) => r.idle60?.PWS_MiB,
  "Idle 5 min: working set (MiB)": (r) => r.idle300?.WS_MiB,
  "Idle 5 min: private bytes (MiB)": (r) => r.idle300?.PB_MiB,
  "Idle 5 min: private working set (MiB)": (r) => r.idle300?.PWS_MiB,
  "Diff view: working set (MiB)": (r) => r.diff?.WS_MiB,
  "Diff view: private bytes (MiB)": (r) => r.diff?.PB_MiB,
  "Diff view: private working set (MiB)": (r) => r.diff?.PWS_MiB,
  "Processes in tree (idle 5 min)": (r) => r.idle300?.Procs,
  "Idle CPU, whole tree (s per 60 s)": (r) => r.idle60 && r.open ? ((r.idle60.CPU_s - r.open.CPU_s) / r.idle60_window_s) * 60 : null,
  "Idle CPU, launched process only (s per 60 s)": (r) => r.idle60 && r.open ? ((r.idle60.RootCPU_s - r.open.RootCPU_s) / r.idle60_window_s) * 60 : null,
  "Diff view CPU, whole tree (s per 10 s)": (r) => (r.diff_cpu_delta_s / r.diff_cpu_window_s) * 10,
};
const out = [];
for (const [app, rs] of Object.entries(apps)) {
  const ok = rs.filter((r) => !r.error);
  const firstRun = ok.find((r) => r.first);
  out.push(`### ${app} (${ok.length} completed runs, ${rs.length - ok.length} failed)\n\n${head}`);
  for (const [label, fn] of Object.entries(metric)) out.push(row(label, stats(ok.map(fn)), label.includes("MiB") || label.includes("CPU") ? 1 : 0));
  out.push(`\nFirst run only (OS cache not controlled, closest to cold; also included in the statistics above): ${firstRun ? `window ${firstRun.window_ms} ms, repository ready ${firstRun.ready_ms} ms` : "run 1 failed or is missing"}.\n`);
}
if (errors.length) out.push(`Failed runs:\n${errors.map((e) => `- ${e}`).join("\n")}\n`);

// Per-process breakdown at the 5 min sample (median run by total working set is not meaningful; show the last run).
for (const [app, rs] of Object.entries(apps)) {
  const last = rs.filter((r) => r.idle300?.Detail).at(-1);
  if (last) out.push(`#### ${app}: process tree at idle 5 min, run ${last.run}\n\n| Process | PID | WS MiB | Private bytes MiB | Private WS MiB | CPU s |\n|---|---|---|---|---|---|\n${[].concat(last.idle300.Detail).map((p) => `| ${p.Name} | ${p.Id} | ${p.WS_MiB} | ${p.PB_MiB} | ${p.PWS_MiB} | ${p.CPU_s} |`).join("\n")}\n`);
}

if (existsSync(join(dir, "tauri-cdp.json"))) {
  const cdp = JSON.parse(readFileSync(join(dir, "tauri-cdp.json"), "utf8"));
  out.push(`### Tauri frontend scenarios (WebView2 DevTools protocol, release build, debug port attached)\n\n${head}`);
  for (const [scenario, rows] of Object.entries(cdp)) for (const key of new Set(rows.flatMap(Object.keys))) {
    if (rows.some((r) => typeof r[key] === "number")) out.push(row(`${scenario}: ${key}`, stats(rows.map((r) => r[key])), key.endsWith("_ms") ? 1 : 0));
  }
}
if (existsSync(join(dir, "git-cli.json"))) {
  const g = JSON.parse(readFileSync(join(dir, "git-cli.json"), "utf8"));
  out.push(`\n### Git CLI alone (console PowerShell, process start included)\n\n${head}`);
  for (const [k, s] of Object.entries(g)) out.push(row(k + " (ms)", { n: s.N, min: s.Min, p50: s.P50, p95: s.P95, max: s.Max, sd: s.StdDev }));
}
console.log(out.join("\n"));
