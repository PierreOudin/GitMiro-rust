# GM-67: final audit of the Tauri/React/Rust slice (US4.1)

Task GM-67 (parent US4.1 / GM-35). Run date: 2026-11-12. Branch `feat/GM-35-us4-1-final-audit`, based on `main` at `1d348811` (GM-68 merge). Production code reviewed and validated at `01d0c352`; later commits only change comments, one Windows-only test, the documentation and the validation scripts under [`docs/gm67/`](gm67/).

The GM-61 to GM-66 evidence (ADR-001 snapshots, `docs/perf/`) was reused, not re-run, and is unchanged.

## 1. Defects found and fixed

The static audit of `main@1d348811` (Notion, 2026-11-12) listed these; each was compared with the current code, fixed and covered by a test.

| # | Area | Defect | Fix | Test |
|---|---|---|---|---|
| 1 | Frontend | Infinite scroll listened to `window`, but `.commit-history` scrolls | `onScroll` on the real container, `isNearEnd` helper, no retry loop after a failure | `tests/scroll.test.mjs`; e2e (500 to 1000 to 1300 commits) |
| 2 | Frontend | Title focus re-fired on every status refresh | Focus only when a repository is opened | e2e (stage keeps focus on the message box) |
| 3 | Frontend | A slow commit could overwrite another repository; open, stage, unstage, resolve were not blocked while committing | Guards on every entry point, late-result check by path | e2e (slow hook, 3 s) |
| 4 | Backend | Merge whose resolved index equals HEAD could not be committed | `status.merging`, `--allow-empty` during a merge, button state in the UI | `commits_a_merge_whose_resolved_index_equals_head`; e2e |
| 5 | Backend | Identity ignored `include.path` | `git config --includes` | `reads_the_identity_from_included_config_files` |
| 6 | Backend | Sign-off duplicated on amend | Skip when the signature line is already present | `amending_does_not_duplicate_the_sign_off` |
| 7 | Backend | Non-UTF-8 `COMMIT_EDITMSG` blocked opening | Lossy read | `a_non_utf8_commit_draft_does_not_block_opening` |
| 8 | Backend | Both-deleted (DD) conflict could not be viewed or resolved | `has_conflict_stages` | `resolves_a_both_deleted_conflict` |
| 9 | Backend | Repositories with thousands of refs failed ("output exceeded 512 KiB") | 16 MiB cap for the reference listing; set lookup in `commit_page` | `opens_a_repository_with_thousands_of_long_references` (3501 refs, 592 KB) |
| 10 | Backend | Stale draft after a commit made in a terminal (Git rewrites `COMMIT_EDITMSG`) | A draft equal to the last commit message is dropped | `a_terminal_commit_does_not_leave_a_stale_draft` |
| 11 | Frontend | Open errors and history errors hidden while a diff was open; diff stale after a commit | Error shown at repository level; diff re-read or closed after a commit | e2e (diff closed after commit) |
| 12 | Frontend | Identity dialog: errors hidden behind it, no Escape, no focus handling | Error in the dialog, Escape, Tab trap, focus restore | e2e |
| 13 | Security | `core.fsmonitor` of the opened repository ran on every status | `-c core.fsmonitor=false` in `git_command()` | `never_runs_a_repository_configured_fsmonitor_command` |
| 14 | Security | `log.showSignature` + `gpg.program` ran a program when listing history and when reading the last commit message (found by review) | `-c log.showSignature=false` on every Git call, plus `--no-show-signature` | `never_runs_a_repository_configured_signature_program` |
| 15 | Robustness | No Git timeout, terminal prompts possible | 120 s (600 s for commits), `GIT_TERMINAL_PROMPT=0`, process-tree kill | `stops_a_git_process_that_never_finishes`, `a_timeout_stops_processes_started_by_git` |
| 16 | Robustness | A rejecting hook's output was replaced by a BrokenPipe error for large messages; a hook leaving a background process hung the call and the lock (found by review) | stdin written on its own thread; pipes abandoned 3 s after Git exits | `a_hook_rejecting_a_large_message_still_reports_its_own_output`, `a_commit_does_not_wait_for_a_process_its_hook_left_running` |
| 17 | Security | Backend had no memory of the opened repository | `AppState` pins commands to the opened repository and serializes index writers | `only_the_opened_repository_is_accepted_by_commands` |
| 18 | CSP | `script-src`, `object-src`, `base-uri`, `form-action`, `frame-ancestors` missing; unused `asset:` allowed | Added / removed | e2e (inline script and `<base>` blocked, no violation in normal use) |
| 19 | Capabilities | `core:default` | `core:event:default` + `dialog:allow-open` | e2e (window and path plugins denied, focus listener still allowed) |
| 20 | Accessibility | Light theme `--error` / `--success` under 4.5:1 on the surface colour of 10 px labels | Darker tokens (5.23:1 minimum measured) | `docs/gm67/a11y.mjs` |
| 21 | UI | Focus fell to `<body>` after a commit; automatic history loading stayed off after a failure; `Math.max(...)` on all commits; amend stuck ticked during a merge; light accent text 4.14:1; Cancel usable mid-commit (found by review) | Fixed | e2e, `docs/gm67/a11y.mjs` |

## 2. What was run

All on Windows 11 Enterprise 10.0.26200, 1920x1200 at 100 %, Git 2.51.0.windows.1, WebView2 157.0.4341.46, Node 24.16.0, rustc 1.94.0.

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`: 35 tests pass (20 existing, 15 new). Mutation checks: the tree-kill (ping child) and signature-program tests fail when their fix is removed.
- `npm test`: 4 tests pass. `npm run build` (tsc + Vite) passes.
- `npm run tauri build`: release exe, MSI and NSIS installer built from `01d0c352` (the later commits change no production behaviour).
- End-to-end, real app in WebView2, driven over the DevTools protocol (`docs/gm67/`): 37 of 37 checks on (a) the freshly built exe, (b) the app **installed by the NSIS installer** into a temporary directory, (c) the exe extracted from the MSI.
- The same driver on the previous release exe (provenance not recorded, built earlier the same day) fails the scroll, focus and merge checks, which shows the checks detect the defects.
- Accessibility smoke (`docs/gm67/a11y.mjs`): 13 interactive nodes all named, headings and landmarks exposed, every tab stop shows an outline or ring, history scroller reachable by keyboard and not a trap, contrast at least 4.5:1 in light and dark, no horizontal page overflow at 800x560 or at 150 % scale. The script also reports the wrap-around of Tab onto `<body>` at the end of the cycle; that is the browser cycling, not a defect.
- Informal timing of `open_repository` on the 1300-commit fixture, 12 invocations per exe: 465 ms (previous exe) vs 479 ms (new); not a replacement for GM-66.

Fixtures are generated by `docs/gm67/fixtures.mjs` in a temporary directory, never in Git. Screenshots were not kept.

## 3. Not done, and why

- **Kotlin 2.0.1 vs Tauri side-by-side visual comparison: done on 2026-11-12 after the first version of this report, see section 8.** Keyboard and focus behaviour was checked on the Tauri side only.
- **MSI installation: verified by the owner in an elevated PowerShell on 2026-11-12** (the MSI is per-machine and the agent session is not elevated). The file hash matched the build of `01d0c352` (SHA-256 `179023B6261D1A68BB80FD01008DD43E7D0369B138A9703AF2B2BB224D5D709B`). Silent install (`msiexec /i /qn`) exited 0; `C:\Program Files\GitMiro\gitmiro.exe` and `licenses\Geist-OFL.txt` were present; an uninstall registry entry for version 0.1.0 existed; the app started and showed a window titled `GitMiro` (throwaway WebView2 profile); silent uninstall (`msiexec /x /qn`) exited 0 and removed the executable. Repair, upgrade over an older version, per-user installation and the Start menu shortcut were not exercised.
- **NSIS** was installed silently to a temporary directory (`/S /D=`), run, and uninstalled (uninstaller exit 0, directory and registry key removed). The default per-user install directory, shortcuts and the WebView2 bootstrapper download path (used only when WebView2 is missing; the runtime was present) were not exercised.
- **macOS and Linux**: not executed. CI builds their packages but does not run the tests there. They are neither validated nor removed here.
- **Window focus refresh** (the OS window regaining focus): the listener registration is checked, the event itself was not simulated.
- **Open-error banner with a diff open** is verified by code reading and the type check, not in the running app (the folder picker is native).

## 4. IPC, CSP, capabilities

Nine commands are registered (`generate_handler!` in `src-tauri/src/lib.rs`): `open_repository`, `load_more_commits`, `refresh_repository_status`, `persist_commit_message`, `commit_repository`, `get_file_diff`, `resolve_conflict_file`, `stage_status_entry`, `unstage_status_entry`.

- Git is always spawned without a shell, through `git_command()`. Paths are passed after `--` with `--literal-pathspecs`, or through stdin with `--pathspec-from-file`. Paths must appear in `git status` before any action. History tips are accepted only as 40/64-digit hex. No injection or traversal was found by three independent reviews. Nothing secret is logged.
- Every command except `open_repository` only acts on the repository opened through it. This stops stale or mismatched calls. It is **not** a barrier against a compromised page, which can still call `open_repository` with any directory; the CSP and capabilities are what limit that page.
- **Opening a repository from an untrusted source runs its configuration, as with the Git CLI itself.** Hooks run on commit, and `post-index-change` runs whenever Git writes the index (a status that refreshes stat-dirty entries, add, rm, reset, checkout). `filter.<name>.clean/smudge` commands run on status, add, diff and checkout. gpg programs run on commit. Git has no switch for these. `core.fsmonitor` and log signature programs are disabled. Possible follow-ups, not done: `-c core.hooksPath=<nowhere>` on every call except commit, and a "trust this repository" prompt before the first status. Until then, treat a repository from an untrusted source as untrusted code.
- `core.fsmonitor=false` also turns off Git's built-in file system monitor, which can slow `status` on very large repositories.
- Read-only commands do not take the mutation lock and `GIT_OPTIONAL_LOCKS=0` is not set: a status refresh can briefly collide with a stage or with a terminal Git on `index.lock` (as before this task). Setting it would stop `post-index-change` on status too, at the price of re-hashing stat-dirty files on every status.
- Git is stopped after 120 s (600 s for commits), whatever the repository size: a very slow first `status` on a huge repository could be cut. The kill is forced, so a timed-out write command can leave `.git/index.lock`; on Unix only Git itself is killed, not its hooks.
- When a process started by a hook keeps Git's pipes open after Git exits, output is abandoned after 3 s: the exit status is kept but stdout and stderr are empty (a failed commit then shows only the exit code). Only `commit` can start such a process today.
- `COMMIT_EDITMSG` / `MERGE_MSG` are read without a size cap or a regular-file check (a FIFO there would hang the open); a draft identical to the last commit message is dropped on purpose.
- CSP: `default-src 'self'; script-src 'self'; style-src 'self'; connect-src ipc: http://ipc.localhost; img-src 'self' blob: data:; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. Tauri adds nonces and hashes itself. Verified in the running app: inline scripts and `<base>` are blocked, no violation during normal use, Geist fonts load.
- Capabilities for the `main` window: `core:event:default` (focus refresh) and `dialog:allow-open`. Verified in the running app: window title mutation and the path plugin are denied, event listen is allowed.

## 5. Platform and packaging

| Target | Kotlin reference CI | Rust fork CI | Verified here |
|---|---|---|---|
| Windows x64 | `release.yml` (installer + portable ZIP) | tests, `cargo fmt`, `clippy`, MSI + NSIS | build, NSIS install, run and uninstall; MSI install, run and uninstall (elevated, run by the owner); MSI payload and NSIS app pass the 37 end-to-end checks |
| Linux | `build_with_tests.yml` (Ubuntu) | package build only | no |
| macOS | none | package build only | no |

The installers detect WebView2 through the registry (`pv` under the EdgeUpdate client GUID) and fall back to downloading the Microsoft bootstrapper when it is missing (`downloadBootstrapper`). The runtime was present, so the fallback was not exercised. Licence files ship in the bundle (`GPL-3.0-only`, Geist OFL).

## 6. Review status

Three rounds, each with five independent agents (3 code reviews, 2 Ponytail reviews), none of which edited the repository. Each round ran on the code SHA of its time, as the task requires, and the code was fixed between rounds.

| Round | SHA | Outcome |
|---|---|---|
| 1 | `e02af222` | No blocker. Confirmed: signature program on open, post-exit pipe waits, quadratic ref check, stdin write before the deadline, focus and amend gaps. All fixed. |
| 2 | `287435ec` | No blocker. Confirmed: the last-commit metadata call still ran gpg.program, amend stuck during a merge, accent contrast, unbounded joins. All fixed. |
| 3 | `01d0c352` (final production code) | **No blocker in any of the five.** Required correction: the threat-model comment understated hooks (post-index-change on status); fixed in the comment and in section 4. Test fix: the Windows kill test now uses a per-process unique command line. |

Round-3 non-blocking items, not fixed (documented in sections 3 and 4, or optional): the identity dialog's Escape and Tab only work while focus is inside the form, and the page behind is not `inert`; a stale status error survives a successful commit; `loadMoreFailedRef` can be re-armed by a page request that fails after a commit; `GIT_OPTIONAL_LOCKS`; draft files read without a size cap; `only_the_opened_repository_is_accepted_by_commands` tests `AppState`, not the commands; `tests/scroll.test.mjs` covers the threshold, not the handler wiring (the end-to-end run does).

One reported defect did not reproduce: on Git 2.51.0.windows.1, `Child::kill` on the `cmd\git.exe` launcher also stopped the real Git. The tree kill is kept because it does stop a hook shell's children (verified with a `ping` child, and its test fails without it).

Optional simplifications reported by the Ponytail reviews and **not applied** (refactors, no defect; every Ponytail review judged the added complexity justified): collapse the `collect_output_*` and `git_*` wrappers and the nine command bodies (about -130 to -280 lines of Rust), reuse `TempRepo` in the eight older tests (about -170), merge the stage/unstage/resolve and open/reopen handlers in `App.tsx`, derive `diffLoading`, a native `<dialog>` for the identity form, remove the Tauri-mobile parts of `vite.config.ts` and dead `tsconfig` options, and an optional remount-by-key split of `App.tsx` (about -100 to -240 lines of frontend and config). They fit a dedicated cleanup task covered by the existing tests.

## 7. Known gaps assigned to later stories

Not part of this slice and not regressions: commit selection and detail view, graph interaction, remote operations, branch operations, and any Kotlin feature beyond the slice listed in ADR-001.

## 8. Kotlin 2.0.1 vs Tauri slice: visual parity

Method. Same fixture for both apps (`docs/gm67/fixtures.mjs`, repository `big`: 1300 commits on `main`, branch `feature`, tag `v1.0`, one staged and one unstaged file), both windows set to 1100x720 outer size on a 1920x1200 display at 100 % scale. Kotlin: the official portable release `GitMiro_Windows_Portable_2.0.1.zip` (167,473,164 bytes, SHA-256 `9aebff4be038ea41d18d2aab9d29b636fe4a8228b57d69557a295bd3ab2a8f73`, equal to the value recorded by GM-66), started with the repository path as argument and an isolated `APPDATA` / `LOCALAPPDATA`; deleted after the run. Tauri: the release build of `01d0c352`. Screenshots (repository view, diff view) were taken with a window-rectangle capture and kept outside Git, in a temporary directory. Only the repository view and the diff view were compared; the Tauri welcome screen has no Kotlin counterpart, and no Kotlin keyboard walk was done.

What the slice already matches: repository name and branch, local branches, tags, commit history with a colored graph, short hashes, ref chips on commits, author and date, staged and unstaged lists with kind, a commit message box with an amend option, and a file diff. Differences observed, none a regression, all outside the US4.1 slice and to be assigned to the later migration Stories (no ticket IDs are recorded here):

| Area | Kotlin 2.0.1 | Tauri slice |
|---|---|---|
| Shell | Custom title bar with repository tabs and "+", toolbar (Fetch, Pull, Push, Prune, Settings), status bar (identity, Actions, version, update notice) | Native title bar plus an in-app "GitMiro" bar, one repository ("Open another"), no toolbar actions, no status bar |
| Reference list | Filter field, counts, HEAD badge, local branches, remotes, tags, stashes, submodules, add buttons | Local branches, remotes, tags; no filter, counts, HEAD marker, stashes or submodules |
| History | Author, Date and branch filters, search, an "Uncommitted changes" row with counts, 40 px rows (12 visible), date as "Nov 14, 2023" | One reference filter, 50 px two-line rows (9 visible), date in the system locale format with seconds |
| Changes | Bulk "Stage all" / "Unstage all", tree view toggle, search, menu, status icons | Per-file Diff / Stage / Unstage buttons with the status written out; no bulk actions |
| Diff | Added and removed lines colored, line numbers, Unified / Split, Changes only / Whole file, Stage and Discard hunk | Plain unified patch text: no colors, no line numbers, no hunk actions |
| Commit button | Clearly grey when the message is empty | Disabled only through 0.65 opacity and a `progress` cursor, which reads as busy rather than unavailable (`.button:disabled` in `src/App.css`) |

The last row is a small visual defect of this slice, not fixed here.

## 9. Reproduce

```bash
node docs/gm67/fixtures.mjs <tempdir>/fixtures          # prints the fixture paths
node docs/gm67/e2e.mjs <path-to-gitmiro.exe> <tempdir>/fixtures   # regenerate the fixtures before each run
node docs/gm67/a11y.mjs <path-to-gitmiro.exe> <tempdir>/fixtures
```

The scripts start the app with a throwaway WebView2 profile and a DevTools port (9223 and 9226), so nothing touches the real application data. Close other GitMiro instances first. `e2e.mjs` mutates the fixtures (it commits in them).
