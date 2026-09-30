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
2. Cleanup page with dry-run preview (next)
3. Executor: quarantine move of revalidated targets, restore, expiry purge, locked-file handling
4. Append-only, hash-chained audit log and Activity page
5. More providers: crash dumps, Recycle Bin; Windows TEMP / Update cache after the elevation design

## Phase 4 – Dev ecosystem ⬜
Project detection; npm/pnpm/yarn/pip/pytest caches; Maven/Gradle analysis (read-only first).

## Phase 5 – Inventory & graph ⬜
Software inventory (registry, winget, Store), runtime inventory, dependency graph.

## Phase 6 – Docker / WSL ⬜
Read-only inspection first (this machine has Docker 29.x and two WSL2 distros). Volumes and WSL data are never auto-deleted.

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

`feat(cleanup): provider framework with dry-run only` – `sentinel-cleanup` crate with the
`CleanupProvider` contract, a user-TEMP provider (discovery, age eligibility, risk,
explanation) and a dry-run preview built entirely on `sentinel-safety` validation. No
executor yet: execution, quarantine layout and elevation are security-sensitive design
decisions that need maintainer sign-off first (see SAFETY_MODEL.md open decisions).

## Completed first milestone (historical)

`chore: initialize cargo workspace with sentinel-core and sentinel-safety` —
canonical path handling and protected-path policy with tests. This is deliberately the
first code because every later component depends on it and it is the highest-risk logic.
