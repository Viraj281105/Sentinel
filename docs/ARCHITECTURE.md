# Sentinel Architecture

Status: **Phase 2 in progress: `sentinel-safety`, `sentinel-scanner` (drive discovery), the Tauri shell and the React frontend exist.** This document describes
the discovered environment and the intended architecture. Sections marked *(planned)*
are not implemented.

## 1. Discovered starting state

| Item | Finding |
|---|---|
| Local repo | `D:\Projects\Sentinel` – empty, not a git repository |
| Remote | `github.com/Viraj281105/Sentinel` – public, **empty** (no branch, no license) |
| GitHub CLI | `gh` 2.87.3, authenticated as `Viraj281105` |
| OS | Windows 11 Home (build 26300) |
| Hardware | Intel Core Ultra 9 275HX (24 logical cores), ~31.4 GiB RAM |
| Drives | `C:` 286 GiB used / 37 GiB free (tight); `D:` 242 GiB used / 358 GiB free |

This is a greenfield project. There is no pre-existing architecture to preserve.

### Toolchain inventory

| Tool | Status |
|---|---|
| Rust / cargo / rustup | **Not installed** – blocking for Phase 1 |
| MSVC C++ Build Tools | Present (VS 18 Community, VS 2022 BuildTools, VS 18 BuildTools) |
| WebView2 runtime | Present (154.x) – required by Tauri |
| Node.js / npm | 24.19.0 / 11.17.0 |
| pnpm / yarn | Not installed |
| Python / pip | 3.14.7 / 26.2.1 |
| Java | JDK 21.0.9 |
| .NET SDK | 10.0.102 |
| Go | Not installed |
| Docker | 29.7.2 CLI; Docker Desktop present, WSL backend distro currently stopped |
| WSL | Distros: `Ubuntu` (WSL2, default), `docker-desktop` (WSL2) – both stopped |
| Git | 2.55.0 |
| VS Code | 1.139.1 |
| winget | 1.29.380 |

Implications: Rust must be installed (with explicit user authorization, see
DEVELOPMENT.md) before any backend work. The C: drive has limited free space, which
makes Rust's `target/` and `~/.cargo` placement a real concern; build output should
live on `D:`.

## 2. Product architecture

Core pipeline, enforced structurally by module boundaries:

```
OBSERVE → ANALYZE → EXPLAIN → PROPOSE → VALIDATE → AUTHORIZE → EXECUTE
```

AI (optional) participates only in EXPLAIN/PROPOSE, and its output is untrusted input
to VALIDATE. The executor accepts only typed `CleanupOperation` values produced by the
policy engine, never strings, never AI output.

```
 ┌─────────────────────────── React + TS + Vite + Tailwind ───────────────────────────┐
 │ Overview · Storage · Cleanup · Software · Dependencies · Projects · Environments …  │
 └───────────────▲──────────────────────────────────────────────┬──────────────────────┘
                 │ events (progress, alerts)                     │ typed commands (Tauri IPC)
 ┌───────────────┴──────────────────────────────────────────────▼──────────────────────┐
 │ commands/   thin IPC layer: validate args, call core services, map errors           │
 ├──────────────────────────────────────────────────────────────────────────────────────┤
 │ OBSERVE            ANALYZE              PLAN / POLICY          EXECUTE               │
 │ windows/           storage/ classify    safety/ (policy,       cleanup/ executor     │
 │ filesystem/        packages/ projects   protected paths)       quarantine, audit     │
 │ processes/         environments/        cleanup/ planner                             │
 │ containers/        dependencies/ graph  ai/ (advisory only)                          │
 ├──────────────────────────────────────────────────────────────────────────────────────┤
 │ database/ (SQLite)   scheduler/   monitoring/   plugins/ (capability-gated)          │
 └──────────────────────────────────────────────────────────────────────────────────────┘
```

### Layering rules (enforced by crate boundaries)

1. **Observation code is read-only.** It cannot depend on the executor.
2. **Only `cleanup::executor` may mutate the filesystem**, and only through
   `safety::validate` tokens (a `ValidatedOperation` type that can only be constructed
   by the policy engine).
3. **`ai` depends on nothing destructive.** It maps structured metadata to a
   schema-validated `Recommendation`. It has no handle to the executor.
4. **Frontend never sees a path it can delete by string.** It references candidates by
   opaque IDs issued by a plan; the backend re-validates at execute time.

### Proposed workspace layout

A Cargo workspace rather than one giant crate, so that the boundaries above are
compiler-enforced and the safety core can be tested and audited in isolation:

```
Sentinel/
  Cargo.toml                  # workspace
  crates/
    sentinel-safety/          # protected paths, path canonicalization, risk levels, policy  (no I/O side effects)
    sentinel-core/            # shared types, errors, operation IDs, config
    sentinel-scanner/         # drive discovery, incremental scanner, classifier
    sentinel-cleanup/         # providers, planner, quarantine, executor, audit
    sentinel-devenv/          # project detection, runtimes, package managers, dep graph
    sentinel-store/           # SQLite schema + migrations
    sentinel-ai/              # provider trait, schema validation, redaction
  src-tauri/                  # Tauri app: commands, events, state wiring only
  src/                        # React frontend
  docs/
```

This deviates from the suggested single-crate `src-tauri/src/<modules>` layout, on
purpose: `sentinel-safety` having no dependency on the executor and `sentinel-ai` having
no dependency on `sentinel-cleanup` is a security property, and Cargo enforces it.
Crates are created only when their first real feature lands (no empty scaffolding). `sentinel-core` does not exist yet for that reason.

### Implemented: `sentinel-safety`

- `CanonicalPath::resolve` – lexical rejection (relative, `..`, UNC/device paths, ADS, reserved names, trailing dot/space, non-Unicode) then OS canonicalization.
- `ProtectedSet` – built-in name/extension rules plus path roots (with exceptions); `from_system()` uses shell known folders.
- `Policy::allowed_root` / `Policy::validate` – produce `AllowedRoot` / `ValidatedTarget`; links are never followed; `revalidate` compares volume serial + file index taken from an open handle.
- `RiskLevel` – ordered, `Protected` never actionable.

### Implemented: `src-tauri` (`sentinel-app`)

- Thin shell: `AppState` holds the log directory and a `Policy` built once at startup.
- Commands (`src-tauri/src/commands/`): `app_info`, `protected_locations`, `list_drives`,
  `start_scan`, `cancel_scan`, `scan_status`, `scan_listing`, `scan_largest_files`. Each wraps a
  plain function that is unit-tested without a running app. Commands are registered by
  full module path because `#[tauri::command]` companion items do not survive re-exports.
- IPC types derive `ts_rs::TS`; `cargo test -p sentinel-app` writes them to
  `src/bindings/` (via `TS_RS_EXPORT_DIR` in `.cargo/config.toml`). CI fails if the
  committed bindings are stale.
- Logging: `tracing` to a daily-rotated file (14 kept) in the app log dir
  (`%LOCALAPPDATA%\dev.sentinel.app\logs`), plus stderr in debug builds. Level via
  `SENTINEL_LOG` (EnvFilter syntax, default `info`).
- Webview hardening: strict CSP (no remote origins), `freezePrototype`, single
  capability file granting only `core:default`.
- Fallible commands return `CommandError { kind, message }` (`kind`: `system` for a failed
  Windows API, `internal` for a Sentinel bug). `message` is user-facing; every error is
  logged at `error` level where it is constructed. New kinds are added when a command needs
  them, not in advance.
- Blocking work (`list_drives`) runs on `spawn_blocking` so it never stalls the UI thread.
- `scans::ScanManager` owns background scans: one at a time (`busy` otherwise), on a
  dedicated thread, with a reporter thread emitting `scan-progress` every 250 ms and a
  final `scan-finished` / `scan-failed` event. The latest finished `ScanTree` is kept in
  memory; the UI fetches one level at a time (`scan_listing`, top 200 children plus a
  summary of the rest) and `scan_largest_files`, so the full tree never crosses IPC.
  `scan_status` lets a remounted page recover a running scan. Events go through a
  `ScanEvents` trait so the manager is tested without Tauri.

### Implemented: `sentinel-scanner`

Read-only storage observation. `drives::list_drives` enumerates drive letters with
`GetLogicalDriveStringsW` and classifies each with `GetDriveTypeW`. Only local volumes
(fixed, removable, RAM disk) are queried with `GetVolumeInformationW` and
`GetDiskFreeSpaceExW`; network and optical drives are listed as `notQueried` because an
offline share or empty tray can block for a long time. `SetThreadErrorMode` suppresses the
"no disk in drive" dialog during queries. The system drive comes from
`GetSystemWindowsDirectoryW` (not `%SystemDrive%`). Per-drive failures are data
(`DriveStatus`), not command errors. `Space::low_space` (< 10 % free) is computed in Rust so
the UI never re-implements policy.

`scan::scan` walks a folder tree read-only and returns a `ScanTree` arena (node 0 is the
root; each node has subtree allocated/logical bytes, file and folder counts, and a
`NodeStatus`) plus the largest files.

- Enumeration uses `GetFileInformationByHandleEx(FileFullDirectoryInfo)` on a directory
  handle opened with full sharing and `FILE_FLAG_OPEN_REPARSE_POINT`. One call returns a
  batch of entries *with allocation size*, so sizes reflect space actually used
  (compressed, sparse and cloud-placeholder files are not overstated).
- Reparse points: directories with junction, symlink, mount-point or unknown tags are
  recorded as `Link` nodes and never entered. Cloud-files directories (OneDrive) are
  entered because they are ordinary directories on the same volume, except online-only
  ones (`FILE_ATTRIBUTE_RECALL_ON_OPEN`), which would make the sync provider fetch data.
- Bounds: rayon pool of `threads` (default 4), `max_depth` (256), `max_entries`
  (50 M); hitting a budget marks nodes `NotScanned` and sets `stats.truncated`.
- `ScanControl` carries the cancel flag and lock-free progress counters for observers.
- Measured on the maintainer machine (C:, 1.1 M files, 269 k folders, NVMe): 21 s cold,
  6.4 s warm with 4 threads; about 270 k nodes held in memory.
- Limitation: hard links are counted once per link, as Explorer does.

### Implemented: frontend (`src/`)

React 19 + TypeScript (strict) + Vite + Tailwind 4, `lucide-react` icons. No component
library yet; one will be chosen when real data tables/charts arrive (Phase 2). Pages
without a backend render `NotImplemented` with their roadmap phase and no data. Tests
use Vitest + Testing Library with `@tauri-apps/api/mocks` at the IPC boundary only.

## 3. Key design decisions

| Decision | Choice | Rationale |
|---|---|---|
| Shell | Tauri 2.x | Small footprint, Rust backend, WebView2 already present |
| Backend | Rust, Windows APIs via the `windows` crate | Memory safety in security-sensitive code |
| Frontend | React + TS + Vite + Tailwind | As specified; component library chosen at UI milestone |
| Persistence | SQLite (`rusqlite`, bundled), WAL mode | Embedded, local-first, inspectable |
| Scanning | Parallel walker, cancellable, bounded thread pool; cache results in SQLite keyed by dir mtime | No full rescans on dashboard open |
| Deletion | Default = quarantine (move) with manifest; permanent delete only for provider-declared safe cases; Recycle Bin via `IFileOperation` where useful | Reversibility |
| Path safety | Canonicalize (`\\?\` aware), reject reparse points unless explicitly resolved, re-check by handle immediately before act | TOCTOU/junction attacks |
| Logging | `tracing` with structured fields + operation IDs, redaction of paths under sensitive dirs | Observability without leaking secrets |
| IPC | Typed Tauri commands; types shared to TS via `ts-rs` generated bindings | No hand-written drift; `ts-rs` is stable and independent of Tauri's release cycle, unlike `tauri-specta` |
| AI | `AiProvider` trait (Ollama, OpenAI-compatible); off by default | Functions fully without AI |
| Plugins | Declarative (data-only) first; capability manifest; no native code initially | Limits blast radius |
| Elevation | App runs unelevated; privileged actions (Windows TEMP, Update cache) go through a separate, minimal elevated helper *(planned, needs security review)* | Least privilege |

## 4. Data model (planned, SQLite)

`scans`, `dir_nodes` (path, size, mtime, category, scan_id), `drives_history`,
`projects`, `runtimes`, `project_runtime_edges`, `software`, `cleanup_operations`,
`audit_log` (append-only), `quarantine_items`, `settings`, `plugins`. No file contents
or secrets are ever stored.

## 5. Honesty constraints

- Unimplemented UI pages render an explicit "not implemented yet" state.
- No hardcoded numbers, no mock inventories. Unknown data is classified `Unknown`.
- AI features display "unavailable" when no provider is configured.

## 6. Open questions

1. Elevation model: helper process vs. per-operation UAC prompt (decide at Phase 3).
2. Whether USN Journal monitoring justifies its admin requirement vs. `ReadDirectoryChangesW` on chosen roots (Phase 7 spike).
3. ~~Type-sharing tool~~ – resolved: `ts-rs`.
