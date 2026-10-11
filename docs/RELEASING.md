# Building and releasing the installer

## What the installer is

A Windows NSIS installer built by Tauri (`bundle.targets = ["nsis"]`).

- **Per user, no administrator rights.** `installMode` is `currentUser`: it installs under the
  user's profile, adds a Start menu entry and an uninstaller, and never asks for elevation. This
  matches how Sentinel runs (unelevated by decision; see ARCHITECTURE.md).
- **WebView2** is required by Tauri. Windows 11 ships it; on older systems the installer downloads
  the Microsoft bootstrapper.
- **Not code-signed.** Windows SmartScreen will say the publisher is unknown the first time it is
  run ("More info" then "Run anyway"). Removing the warning needs a code-signing certificate, which
  is a purchase and an identity decision for the maintainer, not something the build can supply.
  Until then, publish the SHA-256 beside the file so people can verify what they downloaded.
- **Uninstalling** removes the program. It does not touch Sentinel's data (database, logs and the
  quarantine folder), so quarantined items stay restorable; delete those folders by hand to remove
  everything (`%LOCALAPPDATA%\dev.sentinel.app`, and the quarantine folder shown in Cleanup).

## Building locally

```
npm ci
npx tauri build --ci
```

The installer is written to `target/release/bundle/nsis/Sentinel_<version>_x64-setup.exe`. The
first build downloads the NSIS tooling. Compute the checksum with
`Get-FileHash -Algorithm SHA256 <file>`.

## Building in CI

The **Installer** workflow (`.github/workflows/installer.yml`) runs on demand
(Actions → Installer → Run workflow) and when a `v*` tag is pushed. It builds the installer and
stores it, with `SHA256SUMS.txt`, as a workflow artifact. **It does not publish a release.**
Publishing is outward-facing and stays a manual step: download the artifact, check it, and attach
it to a release yourself.

## Before tagging a version

1. Versions agree in `Cargo.toml` (`[workspace.package]`), `package.json` and
   `src-tauri/tauri.conf.json`.
2. CI on `main` is green (including the security audit job).
3. The Phase 12 release checklist in ROADMAP.md is satisfied (security review, fuzzing of path
   handling, performance budget).
4. Install the artifact on a clean user account, run it, and uninstall it.
