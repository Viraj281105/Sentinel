# Sentinel

Sentinel shows what is using the disk space on a Windows PC, and helps you get it back
**safely**. It is built for developers' machines (project dependencies, package caches, Docker
and WSL disks, old toolchains) but works for anyone.

> **Status: pre-release.** Everything described here exists and is tested, but there is no
> signed release yet.

## What it does

- **Storage**: analyzes a drive and shows which folders and files use the space, grouped into
  plain categories (Windows, applications, developer dependencies, package caches, containers,
  and so on). Anything it cannot explain stays "Not classified"; it does not guess. It keeps
  history, so it can tell you what grew since last time and where.
- **Cleanup**: shows exactly what a cleanup would remove and why, then moves approved items to a
  private **quarantine** folder where you can restore them for 14 days. Nothing is deleted
  immediately.
- **Free up space**: tell it how much you want back and the highest risk you accept; it proposes
  items, safest first, with a reason for each. Nothing moves until you confirm the full list.
- **Projects, Software, Dependencies, Containers**: read-only views of your development
  projects, installed programs, the runtimes they need, and Docker and WSL disk use. It never
  starts Docker, and it never removes software or touches Docker volumes or WSL disks.
- **Plugins** (optional): data files that teach Sentinel about another tool's cache folders. A
  plugin cannot run code, and everything it offers goes through the same checks, preview and
  confirmation as Sentinel's own cleanup types. See [docs/PLUGIN_AUTHORING.md](docs/PLUGIN_AUTHORING.md).
- **Background mode** (optional, off by default): keep Sentinel in the notification area so alerts work
  with the window closed, and optionally start it hidden when you sign in. Sentinel adds nothing to startup
  unless you turn that on, and removes it when you turn it off.
- **Optional, off by default**: an AI assistant that answers questions about a saved analysis
  using a model running **on your own computer** (Ollama), and an alert when a drive is low on
  space. Everything works without them.

## What it will not do

- It does not run as administrator, and does not need to.
- It does not remove anything without your confirmation, and every removal is a reversible move.
- It does not touch protected locations: Documents, Desktop, Downloads, Pictures, Videos,
  OneDrive, program files, the Windows folder, Git repositories and their data, SSH and cloud
  credentials, `.env` files, browser profiles, virtual disks and password databases. No rule,
  setting or AI answer can lower that.
- It does not send your data anywhere. The AI assistant accepts only an address on your own
  computer, and sends only the question you typed, with names and paths hidden first.
- It does not read your documents or files. It reads folder listings, sizes and dates, plus a
  few small project and tool files to recognise projects (such as `package.json`, `Cargo.toml`
  and Git's index). Those files are parsed as data, never run.

How this is enforced is written down in [docs/SAFETY_MODEL.md](docs/SAFETY_MODEL.md) and
[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md); a security review of the code that moves files is in
[docs/SECURITY_REVIEW.md](docs/SECURITY_REVIEW.md).

## Install

There is no public release yet. To build the installer yourself, see
[docs/RELEASING.md](docs/RELEASING.md). The installer installs for the current user only, with
no administrator prompt. It is **not code-signed**, so Windows SmartScreen will warn you the first
time you run it.

**Uninstalling** removes the program but keeps Sentinel's data, so quarantined items stay
restorable. To remove everything, delete `%LOCALAPPDATA%\dev.sentinel.app` (database and logs)
and the quarantine folder shown on the Cleanup page. If you turned on "Start Sentinel when I sign in", turn it off
first (Settings), or remove "Sentinel" under Windows Settings, Apps, Startup.

## Build and run from source

Requirements: Windows, Rust (stable, MSVC), Node 20 or newer, WebView2 (included in Windows 11).
It is developed and tested on Windows 11 only.

```powershell
npm ci
npm run tauri dev
```

The checks that CI runs, and the rest of the development workflow, are in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). The design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
and the plan in [docs/ROADMAP.md](docs/ROADMAP.md).

## Where your data lives

- Database (analyses, history, settings, audit log): `%LOCALAPPDATA%\dev.sentinel.app\sentinel.db`
- Logs: `%LOCALAPPDATA%\dev.sentinel.app\logs`
- Quarantine: `%LOCALAPPDATA%\Sentinel\.sentinel-quarantine`

The database holds folder names, the paths of the largest files, sizes and dates. Never file
contents or secrets.

## Security

Please report vulnerabilities privately; see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE).
