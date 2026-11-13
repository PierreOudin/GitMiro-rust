# GM-69: keyboard navigation, feedback and safe cancellation (US4.2)

Task GM-69 (parent US4.2 / GM-36). Run date: 2026-11-13. Branch `feat/GM-36-keyboard-feedback`, based on `main` at
`f9794fa0` (GM-67 merge). Production code: `0fa53089` (implementation), `6e3d9330` (review fixes, final). Frontend
only: no Rust, IPC, CSP or capability change.

## 1. Initial audit

| Already done (GM-64 / GM-67), not redone | Missing or defective, fixed here | Out of scope |
|---|---|---|
| Open and reopen, recents with arrow keys, references, history filter, infinite scroll with a manual retry, diff, stage / unstage, conflict resolution, commit / amend / merge, Ctrl+Enter, title focus on open, focus back to the message box after a commit, guards against concurrent actions, `role="alert"` errors | Identity dialog not modal: page behind not inert, Escape / Tab only worked with the focus inside the form | Clone, fetch, pull, push, rebase, shell rework |
| | Disabled buttons used the `progress` cursor at 0.65 opacity: read as busy, not unavailable | Commit selection, graph interaction |
| | No success feedback and no persistent `role="status"` region: open, stage, commit ended silently for a screen reader | Progress bars: Git is run without `--progress`, nothing is measurable |
| | Focus dropped to `<body>` after stage / unstage, Back to history, a failed open, removing a recent entry | Kotlin keyboard comparison (not done in GM-67 either) |
| | No way to move between panes: a repository with 3501 references meant 3501 Tab presses to reach the history | |
| | Stale status / history errors and a history page for the old HEAD survived a successful commit | |

## 2. Changes (`src/App.tsx`, `src/App.css`)

- **Modal dialog.** The identity form is a native `<dialog>` opened with `showModal()`: the page behind is inert and
  Tab never reaches it. Escape is handled on the document and prevented, so the dialog only closes through React
  state: Escape closes it, except while the commit it started runs (Chromium only lets the first `cancel` of a series
  be prevented, which the Code Review found). Focus goes back to the control that opened it. The hand-written Tab
  trap and the backdrop element are gone.
- **Status line.** A persistent `role="status"` line in the title bar reports what the last operation is doing or
  did: `Opening <path>…` / `Opened <name> on <branch>`, `Loading older commits…` / `Loaded N older commits`,
  `Staging` / `Staged <path>`, `Unstaging` / `Unstaged <path>`, conflict resolution, `Committing…` /
  `Committed <short id> on <branch>`. An error clears it (the error has its own `role="alert"`), so no busy text stays
  behind. Diff loading keeps its own message in the diff view.
- **Focus after async work.** One effect moves the focus, once nothing is busy and only if it fell to `<body>`, to
  the place the action asked for: the same row of the list after stage / unstage (so the next file), the Diff button
  after Back to history, the activated entry after a failed open, the next entry after removing a recent, the opener
  after the dialog. Fallback: the commit message box.
- **Keys.** F6 / Shift+F6 cycle the reference, history (or diff) and status panes, skipping a pane with nothing to
  focus. Escape closes the diff view wherever the focus is. Existing keys are unchanged.
- **Disabled look.** Disabled buttons are grey with the default cursor; busy buttons keep their label
  (`Opening…`, `Committing…`, `Loading…`) and the status line says what runs.
- **Stale state.** A successful commit clears the repository and status errors and drops a history page still loading
  for the previous HEAD (`resetHistoryPaging`, shared with open).

## 3. Cancellation: not offered, on purpose

No Cancel button was added, and none is faked.

- **Mutations** (stage, unstage, resolve, commit with its hooks, draft save) must not be interrupted: a forced kill can
  leave `.git/index.lock` behind or stop a hook halfway. They run under the backend mutation lock with the GM-67
  timeouts (120 s, 600 s for a commit).
- **Reads** are not safe to kill either today. `git status` (open, refresh) and `git diff` (diff view) take Git's
  optional index lock to write refreshed stat data (`GIT_OPTIONAL_LOCKS` is not set, see the GM-67 report section
  4); killing them can leave `index.lock`. Only `git log` paging is lock-free, and its pages are short.
- A frontend-only Cancel would abandon the wait while Git keeps running, which the Story forbids.

Recommended path if a real need appears (for example `open_repository` on a very large repository): a per-request
cancel flag checked in the polling loop of `collect_output_inner` (`src-tauri/src/lib.rs`), which already kills the
process tree on timeout; `GIT_OPTIONAL_LOCKS=0` on the cancellable read calls only; a `cancel_read` command; a test that
cancels a slow `status` in a temporary repository and checks that no `index.lock` remains. Not done here: no measured
need, and it changes the backend and the IPC surface.

Navigating away from a loading diff (Back to history, Escape) still only discards the result of a read; it is not
labelled Cancel.

## 4. What was run

Windows 11 Enterprise 10.0.26200, 1920x1200 at 100 %, Git 2.51.0.windows.1, WebView2 157, Node 24.16.0.

- `npm test`: 4 pass. `npm run build` (tsc + Vite): pass.
- `npm run tauri build -- --no-bundle`: release exe, no installers (no packaging change). Final exe from `6e3d9330`,
  SHA-256 `A8DD947F935D2DC060D85F341AE8973D48D7961423299281BCB1B8674CDE506C`.
- `docs/gm67/e2e.mjs` on the real app (WebView2, temporary fixtures from `docs/gm67/fixtures.mjs`): **54 / 54** on
  `6e3d9330` (53 / 53 on `0fa53089` before the review checks were added). The 17 new checks press real keys through
  the DevTools input pipeline (Enter, Escape, Tab, Shift+Tab, F6, Shift+F6): status line after open, stage, commit and
  during a slow commit; F6 cycle; Enter on Diff and Escape back with the focus restored to the Diff button; no
  `progress` cursor on a disabled button; focus to the next staged row after a keyboard unstage; dialog modal with the
  page behind inert; Tab and Shift+Tab never reach the page behind; Escape pressed three times during a slow commit
  leaves the dialog open; failed open shows an alert, returns the focus and leaves no stale `Opening…`; removing a
  recent entry moves the focus to the next one. The existing GM-67 checks still pass; their `[role=dialog]` selectors
  now read `dialog[open]`.
- `docs/gm67/a11y.mjs` on `6e3d9330`: 10 / 11, the same single finding as GM-67 (Tab wrapping onto `<body>` at the end
  of the cycle, the browser's own behaviour). 13 named interactive nodes, contrast 5:1 (light) and 5.15:1 (dark)
  minimum, no horizontal overflow at 800x560 or at 150 % scale.
- Screenshots (dialog, disabled commit button, status line) checked by eye; kept outside Git.

Not done: a screen reader (NVDA / Narrator) listening pass, so the `role="status"` announcements are checked in the
DOM, not heard; OS-level key presses and a physical Windows scaling change (scale emulated by DevTools); macOS and
Linux; the Kotlin keyboard comparison; GM-66 benchmarks (no backend change).

## 5. Reviews (one Code Review, one Ponytail Review)

Both ran once, by independent agents that did not edit the repository, on `0fa53089`. Fixes were then checked with the
e2e run above on `6e3d9330`; no further review round.

**Code Review** (`0fa53089`): no blocker, no IPC or security change.

| # | Finding | Outcome |
|---|---|---|
| 1 | A second Escape closes the dialog during its commit (cancel only preventable once) | Fixed: Escape handled and prevented on the document; e2e presses it 3 times |
| 2 | Escape on the diff view only worked with the focus inside it; the test focused it by hand | Fixed: document-level; the manual focus is removed from the test |
| 3 | The Tab check counted `<body>` as inside | Kept the native behaviour (Chromium passes once through the document when wrapping, never to the page behind); check renamed to what it proves |
| 4 | Inline ref re-ran `showModal()` on every render, which could reopen the dialog before the queued `close` | Fixed: stable `showModal` ref, and no native close any more |
| 5 | F6 stuck on a pane with nothing to focus | Fixed: skips it |
| 6 | Unreachable early return could leave `Committing…` | Not changed: opening is blocked while committing |
| 7 | An error in one operation clears the status text of another running at the same time | Not changed: the line then shows the next result; nothing wrong is shown |
| 8 | Remove buttons of recents enabled while opening | Fixed |

**Ponytail Review** (`0fa53089`): no blocker; the diff is a net simplification (hand-written Tab trap, backdrop
element, `reopenRepository` and two focus effects removed). Applied: the duplicated history paging reset became
`resetHistoryPaging`, and a comment restating the code was deleted. Not applied (optional): folding the title focus
into the focus effect (it would change behaviour), replacing `data-group` with a structural selector, dropping
ticket tags from e2e comments, a `clickLabel` helper in e2e.

## 6. Reproduce

```bash
npm run tauri build -- --no-bundle
node docs/gm67/fixtures.mjs <tempdir>/fixtures
node docs/gm67/e2e.mjs src-tauri/target/release/gitmiro.exe <tempdir>/fixtures
node docs/gm67/fixtures.mjs <tempdir>/fixtures
node docs/gm67/a11y.mjs src-tauri/target/release/gitmiro.exe <tempdir>/fixtures
```
