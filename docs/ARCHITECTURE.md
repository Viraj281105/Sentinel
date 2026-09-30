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
  `drive_trends`, `start_scan`, `cancel_scan`, `scan_status`, `scan_listing`,
  `scan_largest_files`, `cleanup_providers`, `cleanup_preview` (dry-run only), `project_searches`,
  `find_projects`, `remove_project_search`, `audit_log`, `audit_verify`, `cleanup_run`,
  `quarantine_contents`, `quarantine_restore`, `project_cleanup_preview`,
  `project_cleanup_run`. Each wraps a
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
  final `scan-finished` / `scan-failed` event. A finished tree is saved to SQLite and
  released from memory; the UI fetches one level at a time from the database
  (`scan_listing(scanId, node)`, top 200 children plus a summary of the rest, each with its
  size in the previous analysis) and `scan_largest_files(scanId)`, so the full tree never
  crosses IPC. `scan_status` returns any running scan and the latest saved one, so results
  survive restarts. `drive_trends` reports free-space change over 30 days. Events go through a
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

### Implemented: `sentinel-classify`

Deterministic mapping from a path to one of the directive's 14 categories. The rule
table (`rules.rs`) is compiled-in data with three kinds of rule, each with a stable id
and a user-facing reason:

- *Location rules*: a known folder (shell API; `CARGO_HOME`/`RUSTUP_HOME` honor their
  environment variables because the tools do) plus a relative path, e.g.
  `LocalAppData\npm-cache` → Package caches.
- *Name rules*: folder names that mean the same thing anywhere (`node_modules`,
  `__pycache__`, `.venv`, `.next`, ...). Ambiguous names (`target`, `dist`, `build`,
  `bin`, `obj`) are deliberately absent.
- *Drive-root rules*: `$Recycle.Bin`, `pagefile.sys`, `hiberfil.sys`, `Windows.old`,
  `PythonNNN`, ...

The path is walked from the drive root; at each component a location rule is tried,
then a name rule, then a root rule. The deepest match wins and descendants inherit it.
No match means `Unknown`.

`breakdown()` runs on the full tree before pruning: each folder's own bytes go to its
effective category, then each of the 50 largest files is moved to its own category when
a rule classifies it differently from its folder (so `pagefile.sys` counts as Windows,
not as the unclassified root). Totals are stored per scan; folder and file labels are
computed from the path when displayed. On the maintainer's C: drive 2.3 % stays
Unknown (tool caches such as `~\.cache`).

### Implemented: `sentinel-cleanup` (dry-run only)

`CleanupProvider` declares `info()` (id, name, category, risk, what the items are, what
happens on removal, minimum age) and `roots()`. Providers never decide safety:
`preview()` validates every candidate through `sentinel-safety` and applies age and
protection checks (see SAFETY_MODEL.md, "Dry-run preview"). It reuses the scanner's
batched enumeration (`sentinel_scanner::dirent`, now public, with last-write times), so
sizes are allocated bytes and ages need no extra system calls.

Built-in providers (the crate itself has no executor; `sentinel-quarantine` executes):

| Id | Root (default location only) | Candidates | Min age | Cleans |
|---|---|---|---|---|
| `user-temp` | `%LOCALAPPDATA%\Temp` (known folder, not `%TEMP%`) | every child | 7 days | yes |
| `npm-cache` | `%LOCALAPPDATA%\npm-cache` | `_cacache`, `_npx`, `_logs`, `_prebuilds` | 1 day | yes |
| `yarn-cache` | `%LOCALAPPDATA%\Yarn\Cache` | `v<digits>` | 1 day | yes |
| `pip-cache` | `%LOCALAPPDATA%\pip\cache` | `http`, `http-v2`, `wheels` | 1 day | yes |
| `pnpm-store` | `%LOCALAPPDATA%\pnpm\store` | `v<digits>` | 1 day | no (analysis only) |
| `project-artifacts` | each requested project folder (re-inspected) | that project's untracked artifact folders | 90 days | yes, medium risk |

Cache overrides (`.npmrc`, `npm_config_cache`, `PIP_CACHE_DIR`, `YARN_CACHE_FOLDER`) are
not honored: `.npmrc` may hold credentials and is never read, and an override could aim a
provider at unrelated data. Custom cache locations are therefore not cleaned.

### Implemented: `sentinel-devenv`

`detect(root, options, cancel)` walks a folder (read-only, depth 10, 300 k folders,
never following links, skipping `node_modules`, `.git`, `.venv`, `AppData`, caches and
system folders at a drive root) and recognizes a project wherever marker files appear.
Nested projects (monorepo packages, workspace members) are separate projects. For each:

- ecosystems and package managers from marker and lock files;
- name and runtime requirements parsed from `package.json` (`engines.node`),
  `pyproject.toml` (`requires-python`), `Cargo.toml` (`rust-version`),
  `rust-toolchain(.toml)`, `go.mod`, `.nvmrc`, `.node-version`, `.python-version`;
- artifacts only where the project type makes the folder unambiguous: `node_modules`,
  `.next`, any folder containing `pyvenv.cfg`, Python tool caches, Cargo `target`,
  Gradle `build`/`.gradle`, .NET `bin`/`obj`;
- last activity: newest change among top-level entries (artifacts excluded) and
  `.git/HEAD`, `index`, `FETCH_HEAD`, `ORIG_HEAD`;
- sizes from one scanner pass over the project folder (a parent project's total includes
  nested projects).

### Implemented: `sentinel-quarantine`

The only crate that moves or deletes user files. `Quarantine::quarantine`, `restore` and
`purge_expired` take a `Policy`, the provider, an audit sink (`FnMut(NewAuditRecord)`) and
a context (operation id, user, time); see SAFETY_MODEL.md, "Quarantine executor", for the
exact sequence. Moves are handle renames (`SetFileInformationByHandle(FileRenameInfo)`,
never replacing). Each operation folder holds a versioned `manifest.json`. The
`sentinel-cleanup` crate stays read-only; it gained `assess()` so the executor re-runs
the exact preview checks per item, and `sentinel-safety` gained
`ValidatedTarget::open_verified()`.

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

## 4. Data model (SQLite, `sentinel-store`)

Database: `%LOCALAPPDATA%\dev.sentinel.app\sentinel.db` (WAL, foreign keys on,
`synchronous=NORMAL`). Migrations are append-only SQL scripts; the applied count is
`PRAGMA user_version`, and a database from a newer build is refused rather than
modified. If the file cannot be opened the app runs on an in-memory database and says
so in Settings.

Implemented (schema v4; v2 added `scan_categories`, v3 `audit_log`, v4 `project_searches`):

| Table | Contents |
|---|---|
| `scans` | root, start/finish time, all `ScanStats`, the pruning threshold |
| `scan_nodes` | per scan: node id, parent, name (NOCASE), subtree and own bytes, file counts, child count, pruned-children count and bytes, status (JSON) |
| `scan_largest_files` | per scan: ranked path and size |
| `drive_snapshots` | root, time, total and free bytes (at most hourly, recorded when drives are listed) |
| `scan_categories` | per scan: category key and bytes (absent for scans saved before v2) |
| `project_searches` | per searched folder: time and the latest detection result as JSON (replaced on each search) |
| `audit_log` | append-only: seq, time, operation id, kind, provider, Windows user, items, bytes, policy, approval, outcome, errors, details (JSON), prev_hash, hash. Triggers reject UPDATE and DELETE |

Audit chain: `hash = SHA-256(canonical JSON of the record, including seq and prev_hash)`,
with 64 zeros before the first record. `Store::verify_audit` recomputes the chain and
reports the first record that is missing, out of order, or changed. The user name comes
from `GetUserNameW`, and operation ids are random UUIDs.

Scan trees are stored down to 1 MiB: arena order is pre-order and subtree sizes never
grow away from the root, so one forward pass keeps a connected tree; each kept parent
records how many smaller folders were dropped and their size. Non-complete folders
(links, access denied, skipped) are always kept so the explanation survives. Ten scans
are kept per root. A full C: scan (270 k folders) stores in about 32 ms as ~2.4 MiB.

Growth attribution: a listing is matched by folder path against the previous scan of the
same root, giving each folder's previous size (`null` when it is new or was under the
threshold).

Planned tables: `projects`, `runtimes`, `project_runtime_edges`, `software`,
`cleanup_operations`, `quarantine_items`, `settings`,
`plugins`.

Stored data is metadata only: folder names, the paths of the 50 largest files, sizes,
counts and times. No file contents or secrets are ever read, so none can be stored. The
database stays on this machine.

## 5. Honesty constraints

- Unimplemented UI pages render an explicit "not implemented yet" state.
- No hardcoded numbers, no mock inventories. Unknown data is classified `Unknown`.
- AI features display "unavailable" when no provider is configured.

## 6. Open questions

1. Elevation model: helper process vs. per-operation UAC prompt (decide at Phase 3).
2. Whether USN Journal monitoring justifies its admin requirement vs. `ReadDirectoryChangesW` on chosen roots (Phase 7 spike).
3. ~~Type-sharing tool~~ – resolved: `ts-rs`.
