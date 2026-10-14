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

## Verified install and uninstall

Run by hand on 2026-10-14 with the installer built from commit `ac7f5e3` (silent mode, the same
steps the interactive installer performs; the interactive screens themselves were not clicked
through):

| Step | Result |
|---|---|
| Install (`Sentinel_0.1.0_x64-setup.exe /S`, no elevation) | Exit code 0 in about 2 s. Installs `sentinel-app.exe` and `uninstall.exe` into `%LOCALAPPDATA%\Sentinel`; registers "Sentinel 0.1.0" (publisher Sentinel) under the current user's uninstall list; adds a Start Menu shortcut. No desktop shortcut, no startup entry, and the app is not launched |
| Run the installed program | Reports `0.1.0` (not a debug build), all ten pages load without errors, uses the existing database in `%LOCALAPPDATA%\dev.sentinel.app` |
| Background mode from the installed copy | Hides on close; the Start Menu shortcut raises the hidden window with a single process; the sign-in entry holds the installed path and is removed when switched off |
| Uninstall (`uninstall.exe /S`) | Exit code 0. Removes the program, the uninstaller, the uninstall entry and the shortcut. Leaves the quarantine, the database and the logs untouched, and no startup entry behind |

The install folder (`%LOCALAPPDATA%\Sentinel`) is also the parent of the quarantine
(`%LOCALAPPDATA%\Sentinel\.sentinel-quarantine`). That is safe because the uninstaller deletes only the
files it installed and then removes the folder without recursion, so a folder that still holds the quarantine
stays. The one recursive delete is the interactive uninstaller's optional "delete the application data"
checkbox, which targets `%LOCALAPPDATA%\dev.sentinel.app` (database, logs, saved scan tree) and never the
quarantine. Do not change the installer to remove its folder recursively.

Installing the same version twice in a row (setup run again over an existing install) also exits 0, leaves
the program unchanged, keeps a single uninstall entry and no duplicate files, and leaves the quarantine intact.

Not verified: the interactive installer and uninstaller pages, upgrading from an older version,
installing for another account, and a machine without WebView2.

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
