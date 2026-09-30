# Sentinel Architecture

Status: **Phase 1 in progress. Only `sentinel-safety` is implemented.** This document describes
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
| IPC | Typed Tauri commands; types shared to TS via generated bindings (e.g. `specta`/`ts-rs`) | No hand-written drift |
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
3. Type-sharing tool: `specta` vs `ts-rs` (Phase 1 spike).
