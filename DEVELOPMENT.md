# Developing GitMiro

GitMiro is a Tauri v2 app: React/TypeScript UI in `src/`, Rust backend in `src-tauri/`. The commands to build, test
and package are listed in [AGENTS.md](AGENTS.md#build-and-checks).

## Requirements

- **Git** on `PATH`: the backend runs the Git CLI for every operation. Install **Git LFS** before cloning, the fonts
  in `assets/fonts` are stored in LFS (`git lfs pull` fixes a clone made without it).
- **Node.js 22.6 or later** and npm (the frontend tests use `node --experimental-strip-types`).
- **Rust** stable toolchain with `rustfmt` and `clippy` (`rustup component add rustfmt clippy`).
- **Tauri v2 platform prerequisites** (see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)):
  - Windows: Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 10 1803+ and 11). The MSI bundle also
    needs the WiX toolset and the NSIS bundle needs NSIS; the Tauri CLI downloads both on first `npm run tauri build`.
  - macOS (Intel or Apple Silicon): Xcode command line tools.
  - Linux: webkit2gtk 4.1 and the packages listed in `.github/workflows/build_with_tests.yml`.

In a clone that was used for the Kotlin app, run `git clean -fdX` on the old `main` before switching, or delete the
leftover `app/`, `domain/`, `data/`, `rs/` and `build/` folders: they are no longer ignored.

## Windows checks

Windows-specific behavior (WebView2, `CREATE_NO_WINDOW` on Git spawns, installers) can only be validated on Windows.
Before calling a change that touches startup, packaging or Git spawning done, check on Windows:

1. `npm run tauri build` produces `src-tauri/target/release/bundle/msi/*.msi` and `bundle/nsis/*-setup.exe`.
2. The built app starts, opens a temporary Git repository, shows history and status, and stages, unstages and commits
   a file without console windows appearing.
3. Fonts (Geist) and the window/installer icons are displayed.
