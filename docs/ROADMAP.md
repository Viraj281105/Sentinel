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

## Phase 2 – Storage intelligence ⬜
Drive discovery → cancellable parallel scanner → SQLite cache/history → classifier → Overview/Storage pages.

## Phase 3 – Cleanup framework ⬜
`CleanupProvider` trait, plan/dry-run, quarantine + restore, audit log, executor;
providers: user TEMP, crash dumps, Recycle Bin (then Windows TEMP / Update cache behind elevation design).

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

`feat(storage): implement drive discovery` – enumerate fixed volumes via Win32
(capacity, free space, filesystem, volume label), first fallible command with a typed
IPC error, Overview/Storage wired to real drive data.

## Completed first milestone (historical)

`chore: initialize cargo workspace with sentinel-core and sentinel-safety` —
canonical path handling and protected-path policy with tests. This is deliberately the
first code because every later component depends on it and it is the highest-risk logic.
