# Sentinel Development Guide

Status: Phase 1 complete. The Tauri app builds and runs; pages other than Overview and
Settings are explicitly marked not implemented.

## Verified environment (maintainer machine)

Windows 11 (build 26300), 24 logical cores, ~31 GiB RAM. MSVC Build Tools and WebView2
present. Node 24.19 / npm 11.17, Python 3.14, JDK 21, .NET 10, Docker 29.7, WSL2
(Ubuntu), Git 2.55, VS Code, winget.

## Prerequisites

| Requirement | Status here | Action |
|---|---|---|
| Rust (stable, MSVC target `x86_64-pc-windows-msvc`) | Installed (1.98.1) under `D:\Installed\Rust` | – |
| MSVC C++ build tools | Present | – |
| WebView2 runtime | Present | – |
| Node 20+ / npm | Present | – |
| Git | Present | – |

### Installing Rust

On the maintainer machine Rust lives in `D:\Installed\Rust` (`CARGO_HOME=D:\Installed\Rust\cargo`,
`RUSTUP_HOME=D:\Installed\Rust\rustup`, `D:\Installed\Rust\cargo\bin` on `PATH`) because `C:` has limited free
space. Elsewhere, the standard `rustup` install works. `target/` stays in the repo on `D:`.

Minimum supported Rust version is 1.90 (required by Tauri 2.12).

## First-time setup

```powershell
npm ci
```

## Running the app

```powershell
npm run tauri dev
```

Starts Vite on port 1420 and the Tauri shell with hot reload. Logs go to
`%LOCALAPPDATA%\dev.sentinel.app\logs` and, in debug builds, the console. Set
`SENTINEL_LOG=debug` (or e.g. `sentinel_app=trace`) for more detail.

## Checks (same as CI)

```powershell
cargo fmt --all --check
npm run typecheck
npm run lint
npm test
npm run build            # must precede Rust builds: the app embeds dist/
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace   # also regenerates src/bindings; commit any changes
npx tauri build --debug --no-bundle
```

## Scanner benchmark

Read-only; prints stats, the largest folders and files:

```powershell
cargo run --release -p sentinel-scanner --example scan -- C:\ 4
```

## IPC types

Rust types sent over IPC derive `ts_rs::TS` with `#[ts(export)]`. Running
`cargo test -p sentinel-app` writes them to `src/bindings/`. Never hand-edit those files;
CI fails if they are out of date.

## CI

`.github/workflows/ci.yml` runs every check above on `windows-latest`, plus `npm audit`
and `cargo audit`.

## Testing rules

1. Destructive tests operate only on `tempfile` fixtures created within the test.
2. Every cleanup provider must test: detection, classification, dry-run, execution,
   protected-path behavior, symlink/junction handling, failure recovery.
3. Junction/symlink tests create real reparse points inside the temp fixture; tests
   requiring symlink privilege detect and report a skip rather than silently passing.
4. Symlink tests print `SKIPPED` when the OS denies symlink creation; junction tests
   (`mklink /J`) need no privilege and always run.
5. Never point a *destructive* test at `%USERPROFILE%`, `C:\Windows`, or any real user
   path. Read-only assertions about the real machine's protection set are allowed.
6. Frontend tests mock only the IPC boundary (`@tauri-apps/api/mocks`); test doubles
   never ship in the app.

## Git workflow

- One coherent milestone per commit, conventional commit messages.
- Before each commit: `git status`, `git diff`, `git diff --cached`; verify no
  secrets, binaries or machine-specific data.
- Remote: `https://github.com/Viraj281105/Sentinel`. Single-developer project: commits
  go directly to `main`, pushed by the maintainer's standing instruction.
- Never commit `.env`, credentials, tokens, `target/`, `node_modules/`.
