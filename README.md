# GitMiro - Desktop Git Client

GitMiro is a desktop Git client. This repository holds its rewrite with Tauri v2, React/TypeScript and Rust, where
every Git operation runs through the Git CLI. It looked at so many Git repositories that it went *miro* (French slang
for short-sighted), hence the name.

The rewrite is in progress. The current app opens an existing repository, shows a merge-aware commit graph across local
branches, remote branches and tags (500 commits per page, more on scroll), filters history by local branch, shows staged
and unstaged changes, stages and unstages files, previews diffs, resolves a conflict by keeping a whole side, and commits
or amends staged changes. It is not released yet.

## Repositories

| Repository | Role |
| --- | --- |
| [PierreOudin/GitMiro-rust](https://github.com/PierreOudin/GitMiro-rust) (this one) | Active Tauri/React/Rust application. All rewrite work lands on `main`. |
| [Isipharm/GitMiro](https://github.com/Isipharm/GitMiro) | Original Kotlin/Compose/JGit application, released and still evolving. Functional and visual reference for the rewrite; read-only from this project. |

The Rust port started in Isipharm/GitMiro and was moved here by GM-68 (source commits in
[ADR-001](docs/adr-001-tauri-react.md#repository-split-gm-68)). The Kotlin application remains in this repository's Git
history.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) to build, test and package the app, and [AGENTS.md](AGENTS.md) for the
project conventions. Architecture decisions are in [docs/adr-001-tauri-react.md](docs/adr-001-tauri-react.md),
performance measurements in [docs/perf/](docs/perf/gm66-benchmark-report.md).

## License

GPL-3.0-only, see [LICENSE](LICENSE). GitMiro started as a fork of [Gitnuro](https://github.com/JetpackDuba/Gitnuro)
(GPL-3.0), whose authors are credited in this repository's Git history. The Geist fonts in `assets/fonts` are under
the SIL Open Font License 1.1, see [assets/fonts/OFL.txt](assets/fonts/OFL.txt). Provenance of the other assets and
open licensing points are listed in [ADR-001](docs/adr-001-tauri-react.md#repository-split-gm-68).
