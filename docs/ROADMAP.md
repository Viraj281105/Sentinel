# Sentinel Roadmap

Legend: ✅ done · 🚧 in progress · ⬜ not started

Each milestone is a vertical slice: implementation + tests + docs + commit. The order
follows the master directive, adjusted for what was discovered in Phase 0.

## Phase 0 – Reconnaissance & design 🚧
- ✅ Inspect repo, git, environment, toolchain
- ✅ Architecture, threat model, safety model, development docs
- Finding: Rust toolchain is missing; C: has only ~37 GiB free.

## Phase 1 – Foundations ⬜
1. Install Rust toolchain (requires user authorization); place `CARGO_HOME`/`target` on D:
2. Cargo workspace + `sentinel-core` (errors, operation IDs, config)
3. `sentinel-safety` first: path canonicalization, protected-path set, risk levels — **before any scanner or cleanup code**, with a full test suite
4. Tauri 2 shell + React/Vite/Tailwind skeleton, IPC with generated types, tracing
5. CI skeleton (fmt, clippy, tests, frontend lint/build)

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

## First implementation milestone

`chore: initialize cargo workspace with sentinel-core and sentinel-safety` —
canonical path handling and protected-path policy with tests. This is deliberately the
first code because every later component depends on it and it is the highest-risk logic.
