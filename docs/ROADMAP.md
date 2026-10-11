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

## Phase 6 – Docker / WSL ✅
Read-only inspection first (this machine has Docker 29.x and two WSL2 distros). Volumes and WSL data are never auto-deleted.

1. ✅ Docker (through the CLI, fixed read-only commands) and WSL (registry + disk file sizes) on the Containers page. Verified against the live engine: 3 images (6.2 GB), 2 volumes, 23 build-cache entries (5.6 GB); WSL disks: docker-desktop 20.9 GB (data disk 19.8 GB), Ubuntu 2.6 GB. Docker is never started by Sentinel
2. ✅ Docker cleanup *preview*: what each prune command would remove, Docker's own reclaimable estimate beside the listed items, risk levels, the command shown for you to run yourself. Nothing is executed. Real engine: images 6.2 GB reclaimable, 51 MB of volumes, build cache 5.6 GB listed but 0 B reclaimable (all shared with images)

## Phase 7 – Monitoring ✅
Incremental change tracking (spike: USN vs `ReadDirectoryChangesW`), growth history, anomaly attribution.

1. ✅ Spike (docs/spikes/PHASE7_CHANGE_TRACKING.md): unelevated, the USN journal's metadata is readable but its records are not; `ReadDirectoryChangesW` lost no events with a fast reader and lost whole intervals when the reader stalled. Decision: watch with `ReadDirectoryChangesW` as a dirty-directory hint, rescan on overflow, and use the journal position to skip unchanged volumes
2. ✅ Folder watcher and incremental rescan in `sentinel-scanner` (crate level): overlapped watch with the first request issued before `start` returns, reader that only enqueues, dirty-directory set with noisy-folder coalescing, overflow and loss collapse to `root_dirty`; incremental rescan verified equal to a fresh scan. Wired into the app (see 3)
3. ✅ App wiring: watcher starts with each analysis, the Storage page shows how many folders changed and offers Update (incremental when reliable, otherwise a full analysis); never rescans by itself. The last tree is retained in memory up to 400,000 nodes

4. ✅ Growth history: the drive's size across analyses with a first-to-last summary and category changes, and one folder's size by path (known, below the storage threshold, or absent). Partial analyses excluded from trends; the span and the ten-analysis limit are stated

5. ✅ Anomaly attribution: what grew or shrank most since the previous complete analysis, traced down to the responsible folders with a driver and share of the net change, the unattributed remainder stated, small-in-baseline growth marked as a lower bound, and new or bigger large files listed only where certain

6. ✅ Analysis thinning: scans are thinned as they age (newest three always, one per day for two weeks, one per week beyond, incomplete old ones dropped, hard ceiling of 30 per root) so frequent updates cannot use up the history; about four months of history fit in the ceiling

7. ✅ Surviving a restart: the last full tree is saved to a file after every analysis or update (hand-written, checksummed, strictly validated format; a damaged or impossible file is refused, never repaired), together with the drive's change-journal position read *before* the scan (unelevated, per the spike). At startup the tree is brought back only if the journal shows nothing was recorded on the drive meanwhile; then updates stay incremental. If it moved, was recreated, or cannot be read, the file is discarded and the banner says why. The journal says only that something changed somewhere on the drive, never where, so this helps most on quiet data drives; on a busy system drive it will usually report a change

## Phase 8 – AI abstraction 🔄
`AiProvider` trait, redaction layer, schema validation, NL → structured query. Local models only.

Decisions recorded in docs/AI_POLICY.md (made 2026-10-06 under the maintainer's instruction to decide without asking): local models only, off by default, redaction always, no filesystem-derived text in prompts yet, output validated and never executed.

1. ✅ `sentinel-ai` crate: `AiProvider`, loopback-only Ollama provider (no TLS, no proxy, no redirects), allow-list redactor, strict structured-query schema, and the translation pipeline. 24 tests, including a no-leak property over realistic text and a real local HTTP server
2. ✅ Query execution: a validated `StructuredQuery` runs read-only and deterministically against a saved analysis (folders by size, path text and category with nested matches counted once; files from the saved largest-files list). Re-validated at the single entry point, so a model's answer and a person's form are treated alike. `olderThanDays` is evaluated from recorded change times (item 3)
3. ✅ Change times recorded per folder and per large file (scanner, incremental rescan, store migration); `olderThanDays` now works, as of the analysis date, and says so when an older analysis lacks times

4. ✅ Settings toggle and question box: AI is switched on only in Settings (store v6 `settings` table; model name, Ollama address checked to be on this computer at save time and again at use). `ask_assistant` redacts the question, asks the local model, serializes the validated query and runs it through the same `run_json` entry point. The AI Assistant page labels the answer "AI answered this", shows the exact redacted text sent, the plain-words search, and results with anything not applied stated first. Not exercised against a live Ollama in this environment (covered by a fake provider and the existing local-HTTP-server provider tests)

## Phase 9 – Planner & recommendations ✅
Deterministic "recover N GB" planner with risk ceiling; explainability panel.

1. ✅ Planner (`sentinel-cleanup::planner`, pure and deterministic): candidates are only items a provider's own preview judged eligible; the ceiling is at most medium risk (high and protected are refused and never offered); lowest risk tier first, a higher tier only when the lower is used up; inside a tier largest first, except the smallest item that finishes the plan, so overshoot is minimal; duplicates and nested items counted once, lower risk winning; input order never changes the result. Every entry carries the reason it was chosen; a short plan says by how much and what exists above the ceiling
2. ✅ "Free up space" panel on the Cleanup page (`plan_recovery`): previews every cleanable provider plus the saved projects (re-inspected, inactive only, never Git-tracked), shows the plan with per-item reasons, what removal costs, and what each source offered. Read-only
3. ✅ Run a plan: one confirmation lists every item, then each source's items go through the existing quarantine executor as their own audited operation (so every item is re-assessed immediately before it moves; anything that changed, is in use, or gained a protected file stays). Moves to quarantine only, restorable for 14 days. The project list is rebuilt on the backend from saved searches, never taken from the caller; a stale plan cannot be run twice

## Phase 10 – Plugins 🚧 (design only)
Declarative plugin format, capability manifest, user approval flow.

1. 📝 Design drafted in docs/PLUGINS.md: data-only plugins that can only describe cleanup candidates for the existing provider, planner and quarantine path; anchored roots, no `safe` risk, an age floor, a pinned hash and a live dry run in the approval screen. **Awaiting maintainer review; no code until the open questions there are answered** (the first is whether v1 needs plugins at all).

## Phase 11 – Polish 🚧
UI polish, notifications, installer, startup integration.

1. ✅ Installer: per-user NSIS installer (no administrator rights, Start menu entry, uninstaller; data and quarantine are kept on uninstall). Built locally: `Sentinel_0.1.0_x64-setup.exe`, 4.1 MiB. An on-demand/tag CI workflow builds it and keeps it with its SHA-256 as an artifact; it never publishes a release. Not code-signed (SmartScreen will warn), see docs/RELEASING.md. **Built but not yet installed and run on a clean account**
2. ✅ Low-space alerts (Settings, off by default): while Sentinel is open it checks fixed drives every 15 minutes and shows one Windows notification when a drive drops under 10% free, naming only the drive and its free space; warns again only after a clear recovery (pure, tested decision logic). Uses the small Windows-only `tauri-winrt-notification` crate rather than the Tauri notification plugin, which pulled about 40 extra packages into the lockfile. **Delivery of the toast itself has not been seen on screen**
3. ⬜ Run in the background (tray icon, start with Windows, close-to-tray) so alerts work while the window is closed. Needs a decision on the behavior and a visual check of the tray and window handling
4. ⬜ Visual pass over the pages (keyboard focus, contrast, empty states) with the app running

Also fixed here: `npm audit` reported a high-severity advisory in a build-time dependency (`source-map-js`), which would have failed the CI audit job; resolved with a three-line lockfile update.

## Phase 12 – Hardening & release 🚧
Security review, fuzzing of path handling, performance budget, public-release docs.

1. ✅ Fuzzing of path handling (`sentinel-safety/tests/fuzz.rs`, seeded and reproducible, no new dependency): the protected-path rules are checked against an independently written oracle on 200,000 random paths (random case, separators, doubled and trailing separators, every protected name and extension); `is_within` against a component oracle on 100,000 pairs (never a string-prefix test); 30,000 hostile strings (device and UNC prefixes, streams, reserved names, control and bidi characters, 400-character names) must never panic or yield a target outside the root; and 6,000 mutated real paths (including through a junction to a folder outside the root) must only ever validate to a real, unlinked, unprotected child of the root. **Checked that the tests can fail**: deliberately breaking the code (string-prefix `is_within`, dropping `.kube`, dropping the `pem` extension, removing both junction defences) was caught each time; removing only one of the two junction defences is correctly not caught, because the second layer still holds
2. ✅ Security review of the executor, quarantine and recovery-plan paths (docs/SECURITY_REVIEW.md): the main guarantees hold; three defects found and fixed with tests that fail on the old code (purge acting on the id inside a record rather than its folder, one odd folder aborting every later purge, an item stranded by a crash between rename and record update); residual risks listed, chiefly that purge relies on the system clock
3. ⬜ Performance budget (scan time and memory on a large drive, startup time, restore time of a saved tree)
4. ⬜ Public-release documentation (README, install and uninstall, what Sentinel never does, reporting security issues)

## Next implementation milestone

Phases 7 to 9 are complete. Candidates for next: a first manual run of the assistant against
a real local Ollama and of the recovery plan on real data; your review of docs/PLUGINS.md (whether to build Phase 10 at all);
then Phase 11 (polish).

## Completed first milestone (historical)

`chore: initialize cargo workspace with sentinel-core and sentinel-safety` —
canonical path handling and protected-path policy with tests. This is deliberately the
first code because every later component depends on it and it is the highest-risk logic.
