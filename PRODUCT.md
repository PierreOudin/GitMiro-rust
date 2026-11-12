# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

(Closest schema value. GitMiro is a Tauri v2 desktop app with a React/TypeScript UI rendered in the system WebView, used mainly on Windows; design tokens are CSS custom properties in `src/index.css`. The original Kotlin/Compose app in `Isipharm/GitMiro` is the visual reference.)

## Users

Developers at Isipharm, using GitMiro as their everyday Git client at work: committing, branching, rebasing, reviewing history and resolving conflicts several times a day, often in long sessions next to an IDE.

## Product Purpose

A desktop Git client that makes the repository's state legible at a glance and lets developers act on it without the command line. Success: a developer reads the log, understands a diff and resolves a conflict faster and with fewer mistakes than in their previous tool.

## Positioning

A rewrite of GitMiro, itself a fork of Gitnuro (FOSS), on Tauri, React and the Git CLI. GitMiro adds in-app conflict resolution (keep a side, or pick blocks with checkboxes, preview and edit the result), "show in file explorer" with selection, and its own settings so it runs next to the official Gitnuro.

## Operating Context

- Desktop window, multiple repository tabs, side panel (branches, remotes, tags, stashes, submodules), log graph, file status and diff panes in split layouts.
- Dense, keyboard-and-mouse work; dialogs and context menus for most Git operations.
- Used alongside IDEs and terminals, on office monitors under typical office lighting, sometimes for hours.

## Capabilities and Constraints

- UI is React in a Tauri WebView, styled with plain CSS and custom properties. The Kotlin app themes (Material 1 `Colors` + custom `ColorsScheme`, JSON-serializable and user-customizable) are the reference, so any visual system must map to a flat set of named color tokens.
- Bundled fonts: Geist (UI), OFL, stored in Git LFS in `assets/fonts`; the Kotlin reference also uses Geist Mono for code. Changing faces means shipping new font files.
- Light and dark themes are equal citizens; a "Dark Blue" legacy theme also exists.
- Log graph lanes use a repeating list of `graphColors`.

## Brand Commitments

- Name: GitMiro (French slang "miro", short-sighted). No logo or brand palette is binding beyond the existing app icon.

## Evidence on Hand

- Feature list in README.md. No user research, metrics or testimonials; none should be invented.

## Product Principles

1. Legibility of repository state comes first: log, diff and conflicts must scan instantly.
2. Density without noise: professionals want a lot on screen, with clear hierarchy.
3. Light and dark are both first-class.
4. Familiar desktop affordances over novelty; brand lives in precise details.
5. Destructive or irreversible Git actions must always be visually unambiguous.

## Accessibility & Inclusion

Long-session readability: sufficient contrast in both themes, diff and status colors distinguishable for color-vision deficiencies (not color alone).
