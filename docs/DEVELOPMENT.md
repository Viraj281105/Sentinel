# Sentinel Development Guide

Status: Phase 0. The project has no build yet; this documents the environment
discovered and what must be set up.

## Verified environment (maintainer machine)

Windows 11 (build 26300), 24 logical cores, ~31 GiB RAM. MSVC Build Tools and WebView2
present. Node 24.19 / npm 11.17, Python 3.14, JDK 21, .NET 10, Docker 29.7, WSL2
(Ubuntu), Git 2.55, VS Code, winget.

## Prerequisites

| Requirement | Status here | Action |
|---|---|---|
| Rust (stable, MSVC target `x86_64-pc-windows-msvc`) | **Missing** | Install via `rustup` (needs maintainer approval) |
| MSVC C++ build tools | Present | – |
| WebView2 runtime | Present | – |
| Node 20+ / npm | Present | – |
| Git | Present | – |

### Installing Rust (pending approval)

```powershell
winget install Rustlang.Rustup
rustup default stable-x86_64-pc-windows-msvc
```

Disk note: `C:` has limited free space (~37 GiB). Set `CARGO_HOME` and
`RUSTUP_HOME` to a directory on `D:` before installing, and keep `target/` on `D:`
(the repo lives there already).

## Planned commands

Filled in as each piece lands:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run lint && npm test && npm run build
npm run tauri build
```

## Testing rules

1. Destructive tests operate only on `tempfile` fixtures created within the test.
2. Every cleanup provider must test: detection, classification, dry-run, execution,
   protected-path behavior, symlink/junction handling, failure recovery.
3. Junction/symlink tests create real reparse points inside the temp fixture; tests
   requiring symlink privilege detect and report a skip rather than silently passing.
4. Never point a test at `%USERPROFILE%`, `C:\Windows`, or any real user path.

## Git workflow

- One coherent milestone per commit, conventional commit messages.
- Before each commit: `git status`, `git diff`, `git diff --cached`; verify no
  secrets, binaries or machine-specific data.
- Remote: `https://github.com/Viraj281105/Sentinel` (currently empty). Pushing is done
  only with the maintainer's approval.
- Never commit `.env`, credentials, tokens, `target/`, `node_modules/`.
