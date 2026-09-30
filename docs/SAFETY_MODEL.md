# Sentinel Safety Model

Status: path validation (`sentinel-safety`) and dry-run previews (`sentinel-cleanup`) are
implemented. No code that deletes, moves or modifies user files exists yet; the executor,
quarantine and audit log are designed below but not built.

## Maintainer decisions (2026-09-30)

1. **Quarantine location:** a per-volume `.sentinel-quarantine` folder at each drive root,
   so removal is a same-volume move (instant, no copy, reversible). Items are kept for
   14 days, then purged; the purge is itself audited. The folder name is a built-in
   protected name, so no provider can target it.
2. **Elevation:** the app always runs unelevated. Targets that need administrator rights
   (Windows TEMP, Windows Update cache) wait for a separate minimal elevated helper or a
   per-operation UAC prompt, designed and reviewed separately.
3. **No permanent deletion in v1:** every removal goes to quarantine.

## Principles

1. Sentinel never deletes what it cannot explain.
2. Deterministic code decides; AI only advises.
3. Default action is reversible (quarantine), not permanent deletion.
4. Every destructive operation is dry-runnable, approved, and audited.
5. Protected always wins: no rule, plugin, AI output or user setting lowers a
   `PROTECTED` path.
6. Classification describes; it never authorizes. A storage category (e.g. "Package
   caches", "Temporary files") is display information from `sentinel-classify`. Cleanup
   eligibility comes only from a cleanup provider's own allowed roots and rules, validated
   by `sentinel-safety`. Nothing may treat "classified as X" as "safe to delete".

## Risk levels

| Level | Meaning | Examples | Automation |
|---|---|---|---|
| `SAFE` | Regenerable, no user data | user TEMP (aged), expired crash dumps, known package caches | May be auto-cleaned only if the user enabled auto-cleanup |
| `LOW_RISK` | Regenerable with some cost | stale build artifacts | Per-plan approval |
| `MEDIUM_RISK` | Regenerable but costly / may affect a project | inactive `node_modules`, unused Docker images | Explicit approval with project impact shown |
| `HIGH_RISK` | Hard or impossible to recover | runtime versions, WSL data, Docker volumes | Never automatic. Runtimes/WSL: recommendation only (no uninstall in v1). Volumes: manual confirmation with typed acknowledgement |
| `PROTECTED` | Never touched | credentials, source, user documents, system-critical | Cannot be selected, planned, or executed |

A planner given a risk ceiling never includes candidates above it.

## Protected paths (minimum set)

Desktop, Documents, Downloads, Pictures, Videos, OneDrive roots, `.ssh`, any Git
working tree contents and `.git`, `.env*`, credential stores (Windows Credential
Manager vault, `.aws`, `.azure`, `.kube`, `.gnupg`, npm/pip auth config), browser
profile directories, application databases, WSL distribution VHDX files
(`ext4.vhdx`), Docker volume storage, the Windows directory itself (except specific
allowlisted cache subfolders), and Sentinel's own data/quarantine/audit directories.

Enforcement is **not string matching**. Procedure:

1. Canonicalize (resolve `..`, case-fold per NTFS rules, expand 8.3 names, strip `\\?\`
   prefix consistently, reject ADS and device namespaces).
2. Resolve known-folder locations via `SHGetKnownFolderPath`, not environment strings.
3. Reject if the path or any ancestor is a reparse point that leaves the provider root.
4. Test containment by component-wise ancestry, never by `starts_with` on strings.
5. Re-run steps 1–4 on the open handle immediately before the operation.

### Implemented behavior and known limits (`sentinel-safety`)

- Protection is checked in three directions: the path is *inside* a protected root, *is* one,
  or would *contain* one (so deleting a parent can never take a protected child with it).
  Provider roots may contain protected children; those are guarded per target.
- Folder redirection is handled: on a machine where Documents lives in OneDrive, the stale
  `%USERPROFILE%\Documents` is protected as well as the redirected location.
- A target must be in canonical form. A path using a junction, symlink or 8.3 alias in any
  ancestor is refused (`NotCanonical`). The final component may itself be a link and is then
  reported as `TargetKind::Link`; only the link may be removed.
- `revalidate` catches an object being swapped (different file index) or replaced by a
  junction between validation and use. It narrows, but cannot fully close, the
  check-to-use window; the executor must additionally operate through handles.
- **Not yet implemented:** working-tree protection for Git repositories (only `.git` itself
  is protected today; project-aware rules arrive with project detection, Phase 4) and
  per-descendant checks during recursive deletion (`Policy::check_protected` exists for
  the executor to use on each enumerated entry).
- Any reparse point, including cloud-file placeholders, is treated as a link.

## Dry-run preview (implemented)

`sentinel_cleanup::preview` contains no filesystem-mutating call. For each provider root:

1. Missing roots are reported, not errors. The root must pass `Policy::allowed_root`.
2. Every direct child is a candidate and must pass `Policy::validate` (canonical form, no
   link or short-name ancestors, inside the root, not protected, not containing a
   protected location). Refusals become `Protected` or `Skipped` with the reason.
3. Folders are walked without following links; **every** entry inside is checked with
   `Policy::check_protected`. One protected item (a `.git`, a `.env`, a key file) makes
   the whole candidate `Protected`.
4. The newest modification time anywhere inside, links included, must be older than the
   provider's minimum age, otherwise `TooRecent`. A newly created link counts as recent
   activity.
5. Anything that cannot be read completely (access denied, depth or entry limits,
   cancellation) is `Skipped`, never assumed safe.
6. A top-level link is shown as `Link` with zero size: a real run would remove the link
   only.

Tests prove the preview leaves a fixture byte-for-byte unchanged (paths, sizes, times).
Not yet shown: which applications or projects an item belongs to (needs Phase 4/5
inventory), and whether files are in use (only knowable at execution, where locked files
will be skipped).

## Cleanup providers (contract)

Each provider declares: name, category, allowed roots, discovery, eligibility
(including minimum age), risk level, explanation, regenerability, recovery behavior,
execution strategy, and validation. The provider is *data plus pure logic*; execution
always goes through the shared executor. Providers cannot touch paths outside their
declared roots.

## Operation lifecycle

```
discover → analyze → preview (dry-run) → plan (opaque IDs)
   → policy validate → user approval → re-validate → execute (quarantine|recycle|delete)
   → verify → audit
```

- **Dry-run**: full pipeline minus the final mutation. Reports file count, bytes,
  reason, risk, affected apps and projects. Provably performs no writes (tested via
  a read-only filesystem fixture).
- **Quarantine**: move to Sentinel's quarantine on the same volume when possible
  (cheap, atomic); manifest records original path, timestamp, reason, operation ID.
  Restore is supported; automatic expiry purges quarantined items after a
  configurable period (default 14 days), itself audited.
- **Audit**: append-only, hash-chained records: timestamp, user, operation ID,
  provider, files, bytes, policy decision, approval, result, errors.

## Special cases

- **Docker**: distinguish running/stopped containers, used/unused/dangling images,
  build cache, volumes, networks. Volumes are HIGH_RISK, never auto-deleted. Cleanup,
  when eventually implemented, calls typed Docker API operations, not shell strings.
- **WSL**: detect and report only. Never delete or modify distributions.
- **Maven/Gradle**: analysis only until cache structure handling is proven by tests.
- **Runtimes**: recommendation only; Sentinel does not uninstall software.
- **Files in use / locked**: skipped and reported as `Locked`, never forced.

## AI safety boundary

```
metadata → AiProvider → JSON → schema validation → policy engine → user approval
        → typed operation → executor
```

Malformed output is rejected. AI may suggest a classification or explanation; the
policy engine recomputes risk and may only ever *raise* it relative to AI's claim.

## Real-machine safety during development

Destructive tests run only against temp fixtures created by the test itself. Any code
path that deletes is compiled with a test hook to root all operations under a
sandbox directory. Inspection of the real machine is read-only.
