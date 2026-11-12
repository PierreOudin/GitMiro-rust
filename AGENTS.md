# Agent instructions

Guidance for coding agents (Codex, Claude Code) and contributors working in this repository. `CLAUDE.md` imports this
file.

GitMiro is a desktop Git client being rewritten with Tauri v2, React/TypeScript and Rust. The Rust backend runs every
Git operation through the Git CLI; there is no Kotlin, Gradle or JGit code here.

## Repositories and workflow

- Active repository: `PierreOudin/GitMiro-rust`. Product branch: `main`.
- Reference repository: `Isipharm/GitMiro`, the original Kotlin/Compose/JGit app. Read it to compare behavior and UI;
  never push, open PRs or change anything there. This repository is a GitHub fork of it, so GitHub proposes
  `Isipharm/GitMiro` as the default PR base: always set the base to `PierreOudin/GitMiro-rust:main` and check it
  before creating a PR.
- One branch per User Story, then one PR to `main` of this repository. Do not merge without the owner's approval, and
  do not force-push.
- Notion is the source of truth for Epics, Features, User Stories, tasks, dependencies, acceptance criteria and Quality
  Gates. Record commits, SHAs, test results and reviews on the Notion task; only record what was actually done.
- Commits follow Conventional Commits and cite the Notion task ID (e.g. `Refs: GM-68`).
- Evidence from GM-61 to GM-66 (ADR-001 snapshots, `docs/perf/`) refers to the former `rust/` folder of
  `Isipharm/GitMiro`; keep it as historical evidence and do not rewrite its results.

## Layout

```
src/              React/TypeScript UI (App.tsx, App.css, index.css)
src-tauri/        Rust backend (src/lib.rs: Tauri commands and Git CLI calls), tauri.conf.json, capabilities/
tests/            frontend tests (node:test, run with --experimental-strip-types)
assets/fonts/     Geist Regular and SemiBold (Git LFS) + OFL.txt
assets/icons/     app icons used by the Tauri bundle
docs/             ADR-001, GM-66 benchmark report and scripts (docs/perf/gm66/)
.impeccable/      design tool config and decision mocks
```

## Build and checks

Requirements: Node 22.6 or later (for `--experimental-strip-types`), the Rust stable toolchain, Git on PATH, and the Tauri v2 platform prerequisites (WebView2 on
Windows, Xcode command line tools on macOS, webkit2gtk 4.1 on Linux). Fonts are in Git LFS, so clone with Git LFS
installed.

```bash
npm ci
npm test                       # frontend tests
npm run build                  # tsc + Vite build into dist/
npm run tauri dev              # run the app
npm run tauri build            # release build and installers (MSI + NSIS on Windows)
```

```bash
cd src-tauri
cargo fmt --check
cargo clippy -- -D warnings
cargo test                     # Git tests run against temporary repositories
```

Avoid repeated full builds and benchmarks: the GM-66 campaign is already recorded; re-run it only when a task asks for
it (protocol in `docs/perf/gm66-benchmark-report.md`).

## Conventions

- Rust owns application behavior and Git access; the frontend only calls the narrow commands registered in
  `tauri::generate_handler!` (`src-tauri/src/lib.rs`). Treat every IPC argument as untrusted.
- Spawn Git through `git_command()`, which sets `CREATE_NO_WINDOW` on Windows; a bare `Command::new("git")` opens a
  console window per call and costs hundreds of milliseconds (GM-66).
- Keep Tauri capabilities (`src-tauri/capabilities/default.json`) and the CSP in `tauri.conf.json` minimal.
- Windows is the reference product platform. macOS is a supported development platform; do not claim a Windows
  validation from macOS.
- The installed identifier `com.isipharm.gitmiro` and the product name are unchanged on purpose; changing them moves
  the WebView data (recent repositories) and the install identity. See ADR-001.
- Never commit generated files: `dist/`, `node_modules/`, `src-tauri/target/`, `src-tauri/gen/schemas`, installers.

## CI

`.github/workflows/build_with_tests.yml` runs on pull requests to `main` and pushes to `main`, except docs-only
changes. The Windows job runs the frontend tests, `cargo fmt`, `clippy`, `cargo test` and builds the MSI/NSIS
installers; Linux and macOS jobs build their packages. There is no release workflow yet: do not publish releases.
