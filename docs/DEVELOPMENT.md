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

Store benchmark (scans, then saves to a throwaway database in `%TEMP%` and deletes it):

```powershell
cargo run --release -p sentinel-store --example save_scan -- C:\
```

Saved-tree benchmark (synthetic tree, no disk scan; size and time of saving and restoring the
retained tree):

```powershell
cargo run --release -p sentinel-scanner --example snapshot_bench -- 400000
```

Measured numbers and the performance budget are in [PERFORMANCE.md](PERFORMANCE.md).

Reproducing CI's temp-folder spelling: GitHub's Windows runners spell the temp folder with 8.3 short
names (`C:\Users\RUNNER~1\...`), which is not its canonical form, and 8.3 names are often disabled on a
developer machine. A junction gives the same effect on any volume:

```powershell
New-Item -ItemType Junction -Path target\ci-sim-link -Target $env:TEMP\sentinel-ci-sim
$env:TEMP = "$PWD\target\ci-sim-link"; $env:TMP = $env:TEMP
cargo test --workspace --no-fail-fast
```

Code that compares or maps paths must work when a root is not spelled canonically.

## Driving the real app

The Tauri window is WebView2, so the real app (real UI, real backend, real files) can be driven and
screenshotted without a test double. Start the debug build with WebView2's debugging port on, then use
`scripts/drive-app.mjs` (Node 22 or newer, no dependencies):

```powershell
npx tauri build --debug --no-bundle
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
.\target\debug\sentinel-app.exe        # leave running

node scripts/drive-app.mjs nav Cleanup                  # click a sidebar item
node scripts/drive-app.mjs click "Make a plan"          # click a button by its text
node scripts/drive-app.mjs type "Amount" "1"            # fill an input (by aria-label, placeholder or label)
node scripts/drive-app.mjs text                         # the visible text
node scripts/drive-app.mjs shot out.png [light|dark]    # screenshot, optionally in a colour scheme
node scripts/drive-app.mjs eval "<js>"                  # run JavaScript in the page
```

Things to know:

- The app uses your **real** database, quarantine and drives. Back up
  `%LOCALAPPDATA%\dev.sentinel.app\sentinel.db*` first (starting a newer build upgrades its schema), and
  only move or restore fixtures you created yourself.
- The native file picker cannot be clicked. The Tauri bridge function is locked, so it cannot be replaced
  inside the page either; `drive-app.mjs intercept "dialog|open" '"C:/path/file.json"' 40` answers just that
  request at the network layer while the rest of the app runs unmodified. Start it before clicking, and
  check that it is `ready`, or the real dialog opens.
- A colour-scheme emulation lasts only as long as the DevTools connection, so use the `shot ... light` form.
- If your shell runs inside a packaged (MSIX) app, writes to `%LOCALAPPDATA%` can be redirected into that
  package's folder. Sentinel resolves the real location and refuses it as a protected path (correctly), so
  put test fixtures under the profile folder instead (anchor `userProfile` for a plugin).

## Local data

The app keeps its database at `%LOCALAPPDATA%\dev.sentinel.app\sentinel.db` and logs in
`...\logs`. Deleting the folder while the app is closed resets all history.

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
