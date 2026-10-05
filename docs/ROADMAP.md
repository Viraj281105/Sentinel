# Sentinel Roadmap

Legend: ✅ done · 🚧 in progress · ⬜ not started

Each milestone is a vertical slice: implementation + tests + docs + commit. The order
follows the master directive, adjusted for what was discovered in Phase 0.

## Phase 0 – Reconnaissance & design ✅
- ✅ Inspect repo, git, environment, toolchain
- ✅ Architecture, threat model, safety model, development docs
- Finding: Rust toolchain was missing (now installed); C: has only ~37 GiB free.

## Phase 1 – Foundations ✅
1. ✅ Rust 1.98 installed to `D:\Installed\Rust`
2. ✅ Cargo workspace; `sentinel-core` deferred until it has a real consumer (no premature abstraction)
3. ✅ `sentinel-safety`: path canonicalization, protected-path set, risk levels, `Policy::validate` → `ValidatedTarget` with identity revalidation (20 tests)
4. ✅ Tauri 2 shell + React/Vite/Tailwind, `ts-rs` IPC types, file logging; Overview and Settings show real protected-location data, other pages are honest placeholders
5. ✅ CI on `windows-latest`: fmt, typecheck, lint, tests, build, clippy, binding freshness, Tauri build, npm/cargo audit

## Phase 2 – Storage intelligence ✅
1. ✅ Drive discovery (`sentinel-scanner`): Win32 volume enumeration, capacity, low-space flag; Storage page and Overview summary on real data; typed `CommandError`
2. ✅ Cancellable parallel directory scanner (budgets, exclusions, link safety, largest files) with live progress, cancel, folder drill-down and largest files in the Storage page
3. ✅ SQLite persistence (`sentinel-store`): analyses survive restarts, per-folder change since the previous analysis, hourly drive capacity snapshots with 30-day free-space trend
4. ✅ Deterministic classifier (`sentinel-classify`): rule table with reasons, category breakdown per analysis, category tags on folders and files; unknown stays unknown

Deferred from Phase 2 to Phase 7 (monitoring): incremental rescans and a scan scheduler.

## Phase 3 – Cleanup framework 🚧
Decisions approved 2026-09-30: per-volume quarantine (14 days), unelevated app, no permanent deletion in v1.

1. ✅ `sentinel-cleanup`: provider contract, user-TEMP provider, dry-run preview (validation, protected descendants, age, links, limits)
2. ✅ Cleanup page with dry-run preview: provider description, risk and minimum age; what would be removed vs kept, with the reason for each item
3. ✅ Executor (`sentinel-quarantine`, fixture-tested, **not wired to the app**): audit-first, re-assessment before each move, rename by verified handle, manifest, restore without overwrite, 14-day purge without following links, in-use items skipped
3b. ✅ Wired into the app: "Move N items to quarantine…" with a confirmation listing every item, run result with reasons for anything left in place, Quarantine panel with Restore, purge of expired items at startup
4. ✅ Append-only, hash-chained audit log (SQLite triggers + SHA-256 chain + verifier); previews recorded as dry runs; Activity page with integrity check
5. More providers: crash dumps, Recycle Bin; Windows TEMP / Update cache after the elevation design

## Phase 4 – Dev ecosystem 🚧
1. ✅ `sentinel-devenv`: read-only project detection (Node, Python, Rust, Java/Maven/Gradle, .NET, Go, Ruby, Docker), package managers from lockfiles, runtime requirements, Git, last activity, rebuildable artifacts with sizes (real run: 19 projects under D:\Projects in under a second)
2. ✅ Projects page: folders chosen with the system folder picker, saved searches (store v4), rebuildable space, projects inactive for 90+ days, per-project runtimes and artifacts
3. ✅ Package caches: npm, Yarn and pip cleanable (default locations only, named cache folders only, 1-day minimum age); pnpm store analysis-only. Real machine: Yarn 592 MB and pip 438 MB eligible
4. ✅ Inactive project cleanup from the Projects page: re-inspected at preview and run time, 90-day inactivity over the whole project tree, artifact folders only, never anything Git tracks (own index parser). Real machine: 1.15 GB eligible in 3 projects; a committed venv correctly protected
5. ✅ Maven/Gradle cache analysis (read-only, Projects page): sizes, multiple versions, failed downloads, Gradle versions no searched project uses, exact declared versions. Real machine: Maven 101.5 MB, 452 versions, 50 artifacts with several versions (11.1 MB older)

## Phase 5 – Inventory & graph ✅
Software inventory (registry, winget, Store), runtime inventory, dependency graph.

1. ✅ Runtime inventory (Projects page, read-only): Node.js, Python, Java, .NET, Rust, Go and Android SDK installs with sizes, matched to the runtime versions searched projects state. Real machine: 25 installs found (Node 24.19.0, Python 3.11.9, Rust 1.98.1, JDK 21.0.9, .NET 10.0.102, Android SDK 34-36)
2. ✅ Software inventory from the uninstall registry (Software page, read-only): name, version, publisher, scope, install date, installer-reported size, with updates and system components left out. Real machine: 129 programs (475 entries skipped). Store apps (registry, per user) added: 86 apps, 39 frameworks, 62 system packages on this machine. Winget is not asked
3. ✅ Dependency graph (Dependencies page): projects ↔ installed runtimes they state a need for, unmet requirements, per-install footprint, installs nothing states a need for. Real machine: 20 projects, none state a runtime version, so only package-manager links exist; the page says so rather than guessing

## Phase 6 – Docker / WSL 🔄
Read-only inspection first (this machine has Docker 29.x and two WSL2 distros). Volumes and WSL data are never auto-deleted.

1. ✅ Docker (through the CLI, fixed read-only commands) and WSL (registry + disk file sizes) on the Containers page. Verified against the live engine: 3 images (6.2 GB), 2 volumes, 23 build-cache entries (5.6 GB); WSL disks: docker-desktop 20.9 GB (data disk 19.8 GB), Ubuntu 2.6 GB. Docker is never started by Sentinel

## Phase 7 – Monitoring ⬜
Incremental change tracking (spike: USN vs `ReadDirectoryChangesW`), growth history, anomaly attribution.

## Phase 8 – AI abstraction ⬜
`AiProvider` trait, Ollama + OpenAI-compatible, redaction layer, schema validation, NL → structured query.

## Phase 9 – Planner & recommendations ⬜
Deterministic "recover N GB" planner with risk ceiling; explainability panel.

## Phase 10 – Plugins ⬜
Declarative plugin format, capability manifest, user approval flow.

## Phase 11 – Polish ⬜
UI polish, notifications, installer, startup integration.

## Phase 12 – Hardening & release ⬜
Security review, fuzzing of path handling, performance budget, public-release docs.

## Next implementation milestone

Phase 6, next: a Docker cleanup *preview* only: what `docker image prune` / build-cache
prune would remove, shown from Docker's own accounting, with nothing executed. Whether to
ever offer running prune (a deletion Sentinel cannot quarantine) needs a maintainer
decision first. After that, Phase 7 (monitoring).

## Completed first milestone (historical)

`chore: initialize cargo workspace with sentinel-core and sentinel-safety` —
canonical path handling and protected-path policy with tests. This is deliberately the
first code because every later component depends on it and it is the highest-risk logic.
