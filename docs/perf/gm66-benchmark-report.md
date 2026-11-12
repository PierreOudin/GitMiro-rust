# GM-66: Kotlin 2.0.1 vs Tauri release benchmark

Task GM-66 (parent US4.1 / GM-35). Run date: 2026-11-12. Raw results (JSONL, goldens, traces) stay outside Git; the protocol and scripts are in [`docs/perf/gm66/`](gm66/).

## 1. Executive summary

- Same machine, same 1,000-commit fixture, same driver, both apps in release form. The two apps were run interleaved (Kotlin, Tauri, Kotlin, ...), 5 runs each for the startup/memory/CPU campaign (owner decision: 5 is enough, the original brief asked for at least 10), 10 runs for the Tauri frontend scenarios and for Git CLI alone.
- Memory: the Tauri process tree (app + 6 WebView2 processes) uses a similar working set to the Kotlin JVM (+7% median, 429 vs 403 MiB at 5 min idle) but about half the private memory (private bytes 228 vs 439 MiB, private working set 141 vs 337 MiB). Idle CPU is comparable (0.3 vs 0.2 CPU s per 60 s).
- Rendering: in the Tauri slice, scrolling 500 commits with the SVG graph ran at 16.7 ms median frame time (p95 17.0 ms, 0 frames over 25 ms), and the branch filter repainted in about 32 ms.
- Latency of Git-backed actions in the Tauri slice is the main finding. Opening the repository took 2.95 s p50 inside the app (3.0 s painted), while the same Git commands run alone take 0.6 s. A file diff took 1.18 s p50 (Git alone: 45 ms) and a status refresh 0.73 s (Git alone: 47 ms). Kotlin painted its diff in 0.30 s p50 and was ready 3.4 s after launch, Tauri 4.9 s (including the click on the recent repository).
- Startup to a visible window is much faster for Tauri (about 0.2 s vs 2.6 s p50; the window poll has about 0.2 s resolution, so 0.21 s is an upper bound); time to a repository ready on screen is slower (4.95 vs 3.41 s p50).
- The measured overhead is consistent with the release build starting `git.exe` without `CREATE_NO_WINDOW` (22 new `conhost.exe` observed during one repository open, for about 14 distinct Git commands plus `cmdgit.exe` shim children). The causal link was first inferred from the process counts; a follow-up run of the same in-page scenarios on a build with the flag (`f1793e27`, section 5b) confirmed it: open 2.95 s to 0.73 s, diff 1.19 s to 0.20 s, status refresh 0.73 s to 0.11 s (p50, only 3 repetitions).
- Conclusion (section 12): the Tauri + React/TypeScript + Rust foundation is acceptable to continue the replacement on memory, idle CPU and rendering; it is not shown to be faster than Kotlin, and the full campaign must be re-run on the build with the Git spawn fix (`f1793e27`) before any interaction-latency claim.

## 2. Versions and hardware

| Item | Value |
|---|---|
| CPU / RAM | AMD Ryzen 9 PRO 7940HS (8 cores, 16 threads), 62.7 GiB |
| OS | Windows 11 Enterprise build 26200 (UBR 9711), 1920x1200, 100% scale (96 DPI), 60 Hz |
| Power | AC connected, Windows power plan Balanced (not changed); a laptop, unlike the ADR machine |
| Git | 2.51.0.windows.1 (the ADR snapshots used 2.54.0 on a Ryzen 7 7800X3D desktop, so absolute numbers do not transfer) |
| Node / npm | 24.16.0 / 11.13.0 |
| Rust / Tauri | rustc 1.94.0, tauri 2.12.5, MSVC 14.51 (C++ tools and SDK 10.0.26100 installed for this task) |
| WebView2 runtime | 157.0.4341.46 (a 157.0.4341.42 was present earlier the same day; it auto-updated) |
| Tauri build | `c7ad8ac5` (code identical to `693d05b2`; only `docs/adr-001-tauri-react.md` differs), `npm run tauri build -- --no-bundle`, release profile, built once with `CARGO_TARGET_DIR` under `%TEMP%`; `gitmiro.exe` SHA-256 `CA598EE9...6396A9` |
| Kotlin build | Official release `v2.0.1` at `abde13a0`, `GitMiro_Windows_Portable_2.0.1.zip` (SHA-256 `9aebff4be038ea41d18d2aab9d29b636fe4a8228b57d69557a295bd3ab2a8f73`, verified equal to the published value), bundled Java runtime 25.0.4, launcher `GitMiro.exe` SHA-256 `533E8A25...DA984`. Not built locally (no JDK 25 / Gradle cache on this machine) |

## 3. Fixture

`new-fixture.mjs` builds the repository with `git fast-import` using fixed dates, author and content, so the same commit count gives the same HEAD everywhere (verified twice).

- 1,000 commits on `main`; every 25 commits a group of 3 side commits is merged back. Because of a ref-naming quirk in the generator (kept so the HEAD hash stays reproducible), each side commit has its own one-commit branch instead of one 3-commit branch: 118 local branches (Kotlin lists 118) and 128 refs including 10 tags. 50 files of 40 lines, `core.autocrlf=false`.
- HEAD `1f25259234af9b3120791435e10a0ca405bccd71`.
- Dirty worktree: 3 staged, 5 unstaged (tracked), 2 untracked files, used for status and diff scenarios.
- Lives in `%TEMP%\gm66\fixture-1k`, never versioned, never a business repository. The 100,000-commit corpus was not regenerated (the earlier ADR probe measured Git CLI only).

## 4. Protocol

Both apps are driven identically so neither gets an accessibility tree or a debugger during the memory/CPU runs.

1. `run-bench.ps1 -Calibrate` captures "golden" screenshots (32x20 luminance grid) of the welcome screen, the opened repository and the open file diff, at a fixed outer window of 1280x800 at (100, 60).
2. Each run: start the process (Kotlin with the fixture path as argument, Tauri with no argument), wait for a visible titled window, resize, then for Tauri wait for the welcome frame and click the single recent-repository entry (seeded once in WebView local storage). The time of the first screen frame that is closest to the "ready" golden (within 36 cells, closer than the neighbouring golden) is "repository ready". Frames covered by flashing console windows match neither golden and are skipped, so the first clean frame wins.
3. 5 s settle, then a process-tree sample (working set, private bytes, private working set, CPU seconds, per-process rows via Win32 performance data), a second sample 60 s later (idle 1 min and CPU over the window), a third at 300 s (idle 5 min). Working set double counts shared pages when summed; private working set does not.
4. A click on `src/file0.txt` (unstaged) is timed until the diff golden matches; memory is sampled 3 s later and CPU over the next 10 s.
5. Tauri only: `tauri-cdp.mjs` attaches over WebView2 remote debugging (a debug port is attached only in these runs) and times in-page scenarios with `performance.now()` plus resource timings of the IPC calls. Paint means two animation frames after the DOM change.
6. `git-cli.ps1` times the same Git commands from a console, process start included.
7. Percentiles use the nearest-rank method (value at rank ceil(p x n)); with n = 5 the p95 equals the maximum, with n = 10 it is the 10th value. Dispersion is the sample standard deviation.

## 5. Results: Kotlin vs Tauri (n = 5 runs each, interleaved)

Time values in ms, memory in MiB, CPU in seconds. p50 / min-max in parentheses.

| Metric | Kotlin 2.0.1 | Tauri (693d05b2) | Relative (Tauri vs Kotlin, p50) |
|---|---|---|---|
| Window visible (upper bound, about 0.2 s resolution) | 2,603 (2,473-2,769) | 210 (201-233) | about 10x faster |
| Welcome screen ready | n/a (opens the repository directly) | 522 (489-571) | n/a |
| Launch to repository ready | 3,414 (3,393-3,800; one outlier at 50,021) | 4,954 (4,094-5,187) | +45% |
| Diff click to diff painted | 299 (221-331) | 1,923 (1,711-2,413) | +543% |
| Working set after open | 396 (327-420) | 438 (435-446) | +10% |
| Private bytes after open | 437 (387-449) | 248 (246-257) | -43% |
| Private working set after open | 330 (262-347) | 159 (157-167) | -52% |
| Working set, idle 1 min | 400 (328-422) | 429 (427-430) | +7% |
| Private bytes, idle 1 min | 441 (389-451) | 233 (230-234) | -47% |
| Private working set, idle 1 min | 335 (262-349) | 143 (143-145) | -57% |
| Working set, idle 5 min | 403 (328-428) | 429 (425-445) | +7% |
| Private bytes, idle 5 min | 439 (381-456) | 228 (225-238) | -48% |
| Private working set, idle 5 min | 337 (262-355) | 141 (138-157) | -58% |
| Working set, diff view | 439 (351-461) | 444 (439-456) | +1% |
| Private bytes, diff view | 464 (454-484) | 260 (259-269) | -44% |
| Idle CPU, whole tree (s per 60 s) | 0.2 (0.2-0.3) | 0.3 (0.2-0.4) | same order |
| Diff view CPU (s per 10 s) | about 0.0 | 0.4 (0.4-0.6) | higher for Tauri |
| Processes in tree | 2 (launcher + JVM) | 7 (app + 6 WebView2) | n/a |

p95 equals the maximum at n = 5 (see `summarize.mjs` output for p95 and sd per row). The first run of each series is the closest to a cold start (Kotlin window 2,769 ms, ready 3,800 ms; Tauri window 233 ms, ready 4,094 ms), but OS file cache was not controlled.

Kotlin run 3 shows "repository ready" at 50,021 ms although its window, diff timing (221 ms) and memory were normal. The cause is not established (probably frame matching, for example the update banner or a rendering variation). The value is kept and flagged; the p50 is unaffected and the p95/max (n = 5) is meaningless for that row.

### Tauri frontend scenarios (n = 10, release build, debug port attached, same fixture)

| Scenario | p50 | p95 | min-max | Note |
|---|---|---|---|---|
| Open repository, history painted | 3,007 ms | 3,188 ms | 2,703-3,188 | 500 commits |
| Open repository, IPC `open_repository` | 2,950 ms | 3,131 ms | 2,633-3,131 | backend time seen by the page |
| Status entries painted after open | 3,040 ms | 3,221 ms | 2,736-3,221 | |
| Branch filter select / clear | 31.5 / 33.5 ms | 31.9 / 33.6 ms | | painted |
| Scroll 500 commits (155 frames) | 16.7 ms frame p50 | 17.0 ms frame p95 | max 17.3-21.4 | 0 frames over 25 ms, scrolls 24,939 px |
| Load older commits (500 more) | 533 ms | 583 ms | 432-583 | IPC 477 ms p50 |
| File diff painted | 1,185 ms | 1,435 ms | 985-1,435 | IPC 1,136 ms p50 |
| Status refresh (IPC) | 727 ms | 925 ms | 579-925 | |
| Stage / unstage painted | 907 / 1,252 ms | 1,011 / 1,536 ms | | untracked file toggle |

### Git CLI alone (n = 10, console PowerShell, process start included)

| Command | p50 | p95 |
|---|---|---|
| `rev-parse --show-toplevel` | 40.8 ms | 47.0 ms |
| `for-each-ref` | 51.5 ms | 55.8 ms |
| `log --stdin --date-order` (501 commits) | 50.6 ms | 57.7 ms |
| `status --porcelain=v2 -z --untracked-files=all` | 46.8 ms | 53.7 ms |
| `diff -- src/file0.txt` | 44.9 ms | 47.4 ms |
| Full open sequence (14 commands, as traced from the app) | 599 ms | 630 ms |

Per-process rows at idle 5 min (last run) are reproducible with `summarize.mjs`: Kotlin 395.6 MiB working set JVM + 10.5 MiB launcher; Tauri app 29.9 MiB plus six `msedgewebview2` processes of 15-125 MiB each.

## 5b. Follow-up: Git spawned with CREATE_NO_WINDOW (n = 3, in-page only)

After the campaign, `rust/src-tauri/src/lib.rs` was changed so every production Git spawn goes through `git_command()`, which sets `CREATE_NO_WINDOW` (commit `f1793e27`, 5 call sites; test-only spawns unchanged). The release build was rebuilt once (same machine, same fixture, same `tauri-cdp.mjs`, debug port attached) and the scenarios were run once with 3 repetitions each. The user was working on the machine at the same time, and the screen-capture driver was not used, so these figures are in-page only.

| Scenario (p50) | Before (n = 10, `693d05b2`) | After (n = 3, `f1793e27`) |
|---|---|---|
| Open repository, history painted | 3,007 ms | 767 ms |
| IPC `open_repository` | 2,950 ms | 725 ms |
| File diff painted / IPC | 1,185 / 1,136 ms | 200 / 162 ms |
| Status refresh (IPC) | 727 ms | 105 ms |
| Load older commits painted / IPC | 533 / 477 ms | 165 / 107 ms |
| Stage / unstage painted | 907 / 1,252 ms | 241 / 250 ms |
| Branch filter select / clear | 32 / 34 ms | 31 / 34 ms |
| Scroll frame p50 / p95 | 17 / 17 ms | 17 / 17 ms |

The overhead is therefore mostly the per-spawn console creation: open is now close to the 0.6 s of the Git commands alone. Rendering numbers did not move, as expected. Not re-measured after the fix: memory, idle CPU, window and repository-ready time, and the Kotlin comparison of the diff time (screen-capture campaign). With n = 3 there is no usable p95, and the before and after runs differ in repetition count and in background load.

## 6. Differences from the ADR snapshots

- ADR release sample (Ryzen 7 7800X3D, Git 2.54.0): Tauri 465 MiB RSS / 229 MiB private vs Kotlin 281 MiB / 538 MiB. Here: Tauri 429 / 228 vs Kotlin 403 / 439. Tauri private memory matches; Kotlin's working set is larger here (400 vs 281 MiB), so the RSS gap narrowed from +65% to +7%. The Kotlin side here is the packaged release (launcher + JVM, Java 25.0.4), not a `gradle run` JVM, which probably explains part of the difference. Different machine, one warm sample versus five.
- ADR Git CLI probe on 100,000 commits: about 40 ms per command; here about 40-57 ms on 1,000 commits, the same order for process start dominated commands.
- The ADR did not measure application-level latency; the 3-4 s open and 1-2 s diff are new observations.

## 7. Memory analysis

Private memory (what the OS must back with RAM and page file) is roughly halved by the Tauri stack, which is the sensible comparison for the machine's overall load. The working set is similar because six WebView2 processes share DLL pages with each other and with other WebView2 hosts; summing working sets therefore double counts shared code, and the private working set is the cleaner number. Memory is flat from 1 to 5 minutes for both apps (Kotlin +2.6 MiB, Tauri about -0.4 MiB working set at p50), so no leak signal at that horizon. With the diff view open, private working set is about 10% higher for both apps (Kotlin 337 to 370 MiB, Tauri 141 to 155 MiB at p50).

## 8. Response time analysis

- Frontend rendering is fine: filter 32 ms, smooth scrolling at the 60 Hz frame time with the SVG graph, load-more 0.5 s.
- Backend-bound actions are slow in the Tauri slice because each Git operation adds hundreds of milliseconds beyond the Git process itself: open 2.95 s vs 0.6 s of Git, status 0.73 s vs 0.05 s, diff 1.14 s vs 0.05 s. During a repository open, 22 new `conhost.exe` appeared, evenly spaced about 400 ms, for about 14 distinct Git commands (about 20 `git.exe` processes seen, some being the `cmdgit.exe` shim's children). `rust/src-tauri/src/lib.rs` starts Git with `Command::new("git")` and no `creation_flags(CREATE_NO_WINDOW)`, and the release binary uses the Windows GUI subsystem, so each spawn creates a console host (and a visible console flash that also disturbed the screen-capture frames). This was consistent with the numbers and was then confirmed by the follow-up in section 5b (same scenarios with the flag: open 0.73 s, diff 0.20 s, status 0.11 s; n = 3, in-page only).
- The external (screen) measure of open, 3.6-4.5 s from the click, is about 0.6-1.5 s higher than the in-page measure (3.0 s). The gap was not reconciled; it includes capture latency, frame flashing and the debug port being attached only in the in-page runs.
- Kotlin: window 2.6 s (JVM and Compose start), repository ready 0.9 s later, diff 0.3 s. JGit time versus UI time is not separable in the release package.

## 8b. Not measured

- True cold start (OS file cache cannot be dropped without privileged tooling): only run 1 of each series is indicative.
- Kotlin interactions beyond the diff: branch filter, history scroll smoothness, commit selection, status refresh. The Compose window exposes no accessibility tree and has no debugger port, so these were not scriptable reliably; they are `Non mesurée` for Kotlin.
- Tauri commit selection and commit diff: not implemented in this slice (only worktree file diffs), so `Non mesurée` for both.
- Kotlin backend (JGit) timing versus UI timing: not isolable in the release package.
- 100,000-commit fixture in the apps: not run.
- Full campaign (startup, memory, CPU, screen-captured diff and ready times) on the fixed build `f1793e27`: only a 3-repetition in-page run was done.
- 10 repetitions for the startup/memory/CPU campaign: 5 were run (owner decision after the campaign was paused at run 5 of 10).
- Other DPI factors, a desktop-class machine, and battery power.

## 9. Limits and biases

- One laptop, one fixture of 1,000 commits and 128 refs. The ref count drives some costs; a repository with few refs would behave differently.
- The Tauri open requires a click on the recent entry (the app takes no path argument), while Kotlin opens the path given on the command line, so launch-to-ready includes the click for Tauri only. Kotlin also writes its own preference entries when it opens a repository; Tauri keeps the recent list in the WebView profile.
- Screen-capture matching has a noise floor of about 20 of 640 cells; one capture plus comparison costs about 15-20 ms (measured), so ready/diff times carry roughly that much overhead, plus a 20 ms sleep per poll. The window-visible poll walks the process tree with WMI (about 200 ms per call), so `window_ms` has about 0.2 s resolution and is an upper bound; it also saw the Kotlin update banner ("Update 2.0.2 available", a network check) as part of its frame.
- Console flashes by git.exe in the Tauri release build briefly cover the window and can delay the first clean frame.
- The CDP runs have a debug port attached; the memory/CPU runs do not.
- Interleaved runs reduce but do not remove drift; the machine was kept untouched during the runs, and the campaign was paused once and resumed from the saved results.
- n = 5 gives a coarse p95 (equal to the maximum; with n = 10 it is the 10th value, also the maximum).
- Order bias: in every pair Kotlin ran first and Tauri second (fixed order, 5 s apart); cache or thermal effects from this order are not controlled.
- CPU deltas sum `TotalProcessorTime` over whichever processes exist at each sample; a WebView2 helper that exits or starts between two samples changes the set. Idle CPU values (all positive, 0.2-0.4 s per 60 s) are therefore approximate.
- Tauri `status_painted_ms` is waited after history is painted; the backend returns status in the same `open_repository` response, so the two coincide (3,007 vs 3,040 ms).

## 10. Technical risks

1. Git process spawn overhead (high, fixed in `f1793e27`): about 2.3 s was added to opening a repository and 0.7-1.1 s to status refresh and file diff in the measured build, plus visible console windows and focus stealing. The fix (`creation_flags(0x08000000)` on every production Git `Command`) removed most of it in a 3-repetition in-page check (section 5b). The full screen-capture campaign was not re-run on the fixed build, so the campaign tables above still describe `693d05b2`.
2. WebView2 dependency (medium): memory is spread across six processes and the runtime auto-updates (157.0.4341.42 became 157.0.4341.46 during the day); installer or enterprise-policy variations were not tested.
3. Large histories (medium): load-more adds 0.5 s per 500 commits and the list holds all loaded rows in the DOM; only 1,000 commits were scrolled.
4. Parity gaps (medium): no commit selection or commit diff yet, so key flows cannot be compared.
5. Measurement method (low): screen-capture timing is coarse; a JGit-side and a Rust-side trace would give better separation.

## 11. Reproduce

```powershell
node docs/perf/gm66/new-fixture.mjs $env:TEMP\gm66\fixture-1k 1000
# seed the Tauri recent list with exactly that path once (WebView2 debug port + localStorage), then:
docs/perf/gm66/run-bench.ps1 -Calibrate -Fixture <fixture> -KotlinExe <GitMiro.exe> -TauriExe <gitmiro.exe>
docs/perf/gm66/run-bench.ps1 -Runs 5 -Fixture <fixture> -KotlinExe <GitMiro.exe> -TauriExe <gitmiro.exe>
node docs/perf/gm66/tauri-cdp.mjs <port> <fixture> 10 <out.json>   # app started with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>
docs/perf/gm66/git-cli.ps1 -Fixture <fixture> -Out <git-cli.json>
node docs/perf/gm66/summarize.mjs <results folder>
```

Use AC power, leave the machine untouched, and write results outside the repository. Click targets are window-relative at 1280x800 and 100% scale.

## 12. Conclusion

Does Tauri + React/TypeScript + Rust show acceptable performance to continue replacing GitMiro Kotlin, for the scenarios actually measured? Yes for the foundation, with one condition. Memory (half the private memory, similar working set), idle CPU, window start, scrolling and filtering are acceptable and give no reason to reopen the framework choice. The condition is that Git-backed actions take 5 to 25 times the time of the Git commands alone (open 5x, status 15x, diff 25x) and the Tauri diff is about 6 times slower than Kotlin's, because of console creation on every Git spawn. That cause was fixed afterwards (`f1793e27`) and a 3-repetition in-page check shows open 0.73 s, diff 0.20 s and status 0.11 s (section 5b); the complete protocol (5 runs with screen capture, memory, CPU, startup) must still be re-run on the fixed build before any claim about interaction latency against Kotlin. These data do not show that Tauri is faster than Kotlin, nor that parity is reached; scenarios not implemented or not scriptable (commit selection, Kotlin filter/scroll/status) remain unmeasured.

## 13. Review status

One independent Code review pass over `docs/perf/gm66/` (subagent, read-only) and one Ponytail pass were done on the scripts only; no product code changed in GM-66. The Code review reported 17 findings. Fixed: cleanup of the launched app tree when a run fails, abort when a golden frame is missing, a race when a WebView2 helper exits during sampling, the summary table dropping columns whose first sample was null, first-run reporting, the Git CLI label (14 commands, not 20), the WebView2 registry lookup. Documented as limits above (not fixed, because they would change how the existing data was produced): fixed Kotlin-first order, WMI window polling resolution, CPU sums over changing process sets, fixture ref-naming quirk. Not done: 3 Code Reviews and 2 Ponytail Reviews on the Story's last code SHA, which stay for the Story closure.
