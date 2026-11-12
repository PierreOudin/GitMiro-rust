---
version: 1
slug: "kotlin-com-hugodehu-gitmiro-ui-apptab-kt-968e25e8"
primary_target: "src/App.tsx"
related_targets: ["app/src/main/kotlin/com/hugodehu/gitmiro/theme/Color.kt","app/src/main/kotlin/com/hugodehu/gitmiro/App.kt"]
---

## Scope

Main repository window of GitMiro (tabs, header, navigation, log, commit detail, commit composer). Mode: Operate. Light and dark are equal; the user picked the dark rendition as reference.

## Audience and task

Isipharm developers, daily long sessions: read the log/graph, read diffs and resolve conflicts, navigate branches/remotes/tags/stashes, stage and commit.

## Direction contract

THESIS: GitMiro as a calm Arc/Dia-like workspace: a tinted window frame, rounded floating cards, window controls inside the header. Refuses the stacked-rectangle Swing/Material shell with a system title bar and boxed panels.

OWN-WORLD: Dark frame #121219, cards #1C1C24, floating cards #262631 with a 1px white-7% outline; ink #ECECF3 / #B8BACB / #9A9DB0; periwinkle accent #7B8FFF (dark text on it), accent soft #272D57. Light twin: frame #EEF0F6, cards #FFFFFF, accent #4462F5. Lanes periwinkle, coral #FB7185, teal #2DD4BF, grey. Radii 14 cards, 16 floating, 9-12 controls, 11 pills. Geist + Geist Mono. Separation by ground and spacing, not borders.

STORY: The developer sees which repo they are on and switches between open and recent repos from the title bar switcher (Ctrl+O), scans history in the log card, and reads the selected commit or stages and commits in the docked details panel. The details panel collapses to give the log the room.

FIRST VIEWPORT: 48px title bar: logo, repository switcher pill (color mark, name, path, chevron; opens a palette of open and recent repos with search, close on hover or middle-click, and "Open or clone"), open count, then only the three remote/worktree split buttons Pull (rebase/merge, fetch all, prune deleted branches), Push (with tags, force) and Stash (with message, pop last stash; its menu stays open on a clean worktree), native min/max/close. No rail: the side panel tree is always shown, with a hover "+" on Local branches for a new branch (Ctrl+B). Rounded log card (History title, 10dp graph dots, tinted ref pills), details panel collapsing from its own header chevron to a 36px strip. Bottom bar: identity on the left; Terminal, Actions and Settings as labelled actions, then the version, on the right. The user rejected floating peek and composer cards and the icon rail (2026-11-03).

FORM: Hybrid of grounded structures 6 (split view: icon rail) and 7 (peek + floating composer), user-composed from the dealt hand. seed:9cc1e608730df220

Signature interaction: the title bar pill is the repo switcher (user pick 2026-11-03 over named rail tiles and a side panel list); collapsing the details panel hands its width to the log; application-level actions live in the bottom bar, repository actions in the title bar.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Unresolved

- Diff and conflict-resolver screens inside this world.
- Switcher rows show the path, not the current branch or pending changes of each repo.
