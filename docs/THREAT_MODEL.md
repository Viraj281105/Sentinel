# Sentinel Threat Model

Status: rows T1, T2 and T3 are implemented in `sentinel-safety` (see SAFETY_MODEL.md for
limits). T14 is partly implemented: strict CSP with no remote origins, `freezePrototype`,
and a single capability granting `core:default` plus `dialog:allow-open` (the native folder picker; it returns only what the user picks). The only command that accepts a path is
`start_scan`, which is read-only; the path must be an existing absolute folder and is
canonicalized by `sentinel-safety` before use. Drill-down takes opaque node ids, not paths. T17 has
CI `npm audit`/`cargo audit` and committed lockfiles. All other mitigations are still
requirements, not claims about existing code.

## Assets

1. User data (documents, source repos, credentials, browser profiles, databases)
2. System integrity (Windows, installed software, WSL distros, Docker volumes)
3. User privacy (paths, software inventory, project names)
4. Integrity of the audit log and quarantine

## Trust boundaries

```
[Filesystem contents] --untrusted--> [Scanner/Detectors]
[Project files, package.json...] --untrusted--> [Detectors]
[AI provider response]  --untrusted--> [Schema validation → Policy]
[Plugin manifests/rules] --untrusted--> [Capability gate → Policy]
[Frontend/webview]  --semi-trusted--> [Tauri commands]
[Policy engine + Executor] = trusted computing base
```

The Trusted Computing Base is deliberately small: `sentinel-safety` and the executor
in `sentinel-cleanup`.

## Threats and mitigations

| # | Threat | Mitigation (required) |
|---|---|---|
| T1 | Path traversal (`..`, mixed separators, `\\?\`, 8.3 short names, alternate data streams, trailing dots/spaces, device names) | Single canonicalization routine in `sentinel-safety`; all paths validated post-canonicalization; deny ADS and device-namespace paths . *Fuzzed:* see `crates/sentinel-safety/tests/fuzz.rs` (oracle comparison on 200k random paths, 30k hostile strings, 6k mutated real paths including through a junction) |
| T2 | Symlink/junction redirection to a protected target | Never follow reparse points during scan or delete; delete the link, not the target; canonicalize and re-check ancestry before acting |
| T3 | TOCTOU (path swapped between validation and delete) | *Implemented:* the executor re-assesses each item immediately before acting, then `ValidatedTarget::open_verified` revalidates and opens the object itself, checking volume serial, file index and link status on the handle; the move is a rename through that handle (`SetFileInformationByHandle`, no replace, same volume only). Residual: a file added inside a folder item after its descendant check moves with the folder (reversible) |
| T4 | Malicious/erroneous cleanup rules | Rules are data validated against the same policy; a rule cannot lower a path below its protection level; built-in providers use an allowlist of roots |
| T20 | Misclassification leading to harmful advice or action (e.g. a folder named `node_modules` or `$Recycle.Bin` created to look disposable) | Classification rules are compiled in, not user-supplied; categories are display-only and never grant cleanup eligibility (SAFETY_MODEL principle 6); unmatched paths stay `Unknown` |
| T5 | Malicious plugin | *Implemented, see T23 to T28 and docs/PLUGINS.md.* Data-only in v1; capability manifest (`filesystem.read`, `filesystem.cleanup`, `process.inspect`, `network.none`); dangerous capabilities need explicit user approval; plugins cannot bypass the policy engine |
| T6 | Compromised or prompt-injected AI provider / malicious AI output | AI output is a suggestion only; strict JSON schema; unknown fields rejected; policy engine re-derives risk deterministically; AI has no executor handle |
| T7 | Prompt injection via filenames/project files | AI receives only structured metadata with sanitized, length-limited fields; never file contents |
| T8 | Data exfiltration to AI provider | Redaction layer; no file contents, `.env`, keys, tokens; external providers off by default; user-visible record of what was sent |
| T9 | Privilege escalation via elevated helper | Helper accepts only typed, pre-validated operations over an authenticated local channel; re-validates independently; no arbitrary path/command input |
| T10 | Malicious project files (e.g. crafted `package.json`, `.git` config) triggering code execution during detection | *Implemented in `sentinel-devenv`:* files are parsed only (serde_json, toml, line parsing), never executed; no package manager, build tool, script or Git command is run (a test plants a `preinstall` script and proves nothing ran); files over 1 MB are not read; parse failures become per-project warnings; names and versions are length-limited; junctions are not followed. Maven `pom.xml` is parsed with `roxmltree` (no external entities, 1 MB limit) and `settings.xml` (may hold passwords) is never read |
| T11 | Compromised package manager output when querying versions/caches | Prefer reading files/registry over invoking tools; when invoking, use absolute resolved paths, fixed args, timeouts, no shell. *Implemented for package caches:* no package manager is invoked; only default cache locations are used, `.npmrc` (may hold tokens) is never read, and each cache provider only accepts its own named folders |
| T12 | Registry manipulation / bogus uninstall entries | Registry treated as untrusted display data; never used as a path to delete |
| T19 | Local database disclosure or tampering (another local process reads or edits `sentinel.db`) | Stores metadata only (folder names, 50 largest file paths, sizes, times), never contents or secrets; lives in the per-user `%LOCALAPPDATA%` with default user-only ACLs; values read back are display data and are never used as paths to act on (the future executor revalidates everything through `sentinel-safety`); corrupt rows surface as errors, not crashes |
| T21 | The inspector runs an external program (`docker.exe`, and `node.exe --version`) that could be replaced or abused | *Implemented:* only absolute, resolved paths are run (never the current folder); the command set is a closed enum with fixed arguments, so no user, project-file or Docker output ever becomes an argument; no shell; bounded time and output; output is parsed as data. Only read-only Docker commands are ever sent. Residual: a malicious `docker.exe` earlier on `PATH` than Docker Desktop's folder would run (Docker Desktop's own folder is tried first) |
| T22 | A "local" AI request leaves the machine (remote endpoint, proxy, redirect, TLS to a cloud host), or prompt text discloses personal data | *Implemented in `sentinel-ai` (docs/AI_POLICY.md):* the endpoint must be a literal loopback address over plain HTTP (validated; `localhost` is normalized to 127.0.0.1 so no name resolution is involved), the HTTP client has no TLS support compiled in (verified with `cargo tree`), uses no proxy and follows no redirects; AI is off by default; the redactor removes secrets, e-mails, addresses, SIDs and personal names and hides every path component not on a short allow-list; in this milestone no filesystem-derived text is put in a prompt at all. Residual: a user could point the loopback address at a port-forward to a remote machine (a local proxy is outside Sentinel's control); free text naming a person or project is not detected by the redactor |
| T23 | A plugin names a root in user data or a protected area so that it is offered for cleanup | *Implemented in `sentinel-plugins`:* roots are an anchor (`localAppData`, `roamingAppData`, `userProfile`) plus a relative path with no `..`, drive, stream, wildcard, short-name or reserved-name parts; personal folders and the built-in protected names are refused at validation; at run time the root goes through `allowed_root`, every candidate through `validate` and every descendant through `check_protected`, as for built-ins; a root that links lead out of its anchor is not used |
| T24 | A plugin misdescribes its folders ("harmless") to talk the person into approving | Risk is `lowRisk` or `mediumRisk` only (never `safe`, so never in a Safe-ceiling plan) and the age has a floor; the review screen shows the resolved absolute folder, the name patterns and a live dry run, not just prose; the plugin's own words are marked "written by the plugin"; every run still lists each item for confirmation |
| T25 | The plugin file is replaced after approval | The approved manifest is stored in the database in canonical form with its SHA-256 and re-verified on every use (an edited row is skipped and reported); the file is never read again after inspection; approval takes only the fingerprint of an inspection made in this session |
| T26 | A different plugin impersonates or silently replaces an approved one | Provider ids are namespaced `plugin:<id>:<type>`; identity is (id, fingerprint); a changed file under an installed id is shown as a different plugin that replaces the old one and needs its own approval; there is no auto-update and no download |
| T27 | Resource exhaustion through a huge manifest or pathological patterns | 64 KB cap, at most 16 cleanup types, 32 names, 8 path parts, bounded text; one `*` per pattern, matched in linear time; at most 8 inspections held in memory; the preview's entry and depth budgets apply |
| T28 | Deception in the review screen (lookalike or control characters, bidi overrides, very long text) | Control, zero-width and direction-changing characters are refused in every text field; lengths are bounded; text renders as plain text; the fingerprint is shown |
| T29 | Sentinel makes itself resident or adds itself to startup without the person knowing, or starts with no way to open it | *Implemented:* background mode and the sign-in entry are both off by default and each needs its own switch in Settings; the entry is a single per-user (`HKCU`) Run value holding one quoted path and the fixed argument `--background`, never written elsewhere, never with elevation, removed when switched off, and read back from the registry as the source of truth (removing it in Windows' Startup apps list shows the switch as off); a hidden start only happens when background mode is also on and the tray icon was created, otherwise the window is shown and closing quits; the sign-in switch cannot be on without background mode |
| T30 | Another process signals the running Sentinel through its single-instance objects | The named mutex and event live in the per-session `Local\` namespace (no other user or session); a signal can only make the existing window show itself: it carries no data and runs no command |
| T13 | Tampered audit log or quarantine | *Implemented for the audit log:* SQLite triggers reject UPDATE/DELETE; SHA-256 hash chain over canonical record JSON; verifier reports the first missing, reordered or edited record and the Activity page shows it. Known limits: an attacker with write access to the file can drop the triggers and rebuild a consistent chain, and truncating the newest records leaves a valid shorter chain (anchoring the head outside the database is a later hardening item). Quarantine (implemented and connected; items move only after explicit confirmation): lives in the user's private profile folder; must be a plain folder, not a link; manifests are versioned and operation ids restricted to `[A-Za-z0-9_-]` so they cannot escape the folder; restore never overwrites and re-checks the destination with the policy; purge deletes only inside the quarantine folder without following links. *Reviewed (docs/SECURITY_REVIEW.md):* a record is only honoured by the folder it sits in, a bad folder cannot stop other purges, and an item stranded by a crash is still restorable. Residual: the records are unsealed plain files, and purge trusts the system clock |
| T14 | Webview compromise (XSS) invoking commands | Strict CSP, no remote content, minimal Tauri capability allowlist, commands take opaque plan IDs not raw paths |
| T15 | Command injection | No shell invocation with constructed strings; typed args only |
| T16 | DoS via huge/infinite trees, hard-link loops, or unresponsive volumes | Depth/entry budgets, cancellation, reparse-point non-traversal. *Implemented:* drive discovery never queries network/optical drives, suppresses OS error dialogs and runs off the UI thread; the directory scanner never enters reparse points (junction loops are tested), and enforces depth/entry budgets, a bounded thread pool and cancellation |
| T17 | Supply-chain compromise of dependencies | `cargo-deny`/`cargo audit`, `npm audit`, lockfiles committed, minimal dependencies, CI checks |
| T18 | Sensitive data in logs | Redaction in `tracing` layer; never log contents/tokens |

## Out of scope (initially)

Malicious local administrator; kernel-level attackers; physical access.

## Security review gates

Any change under `sentinel-safety`, executor, elevation helper, or plugin capability
handling requires tests covering the relevant rows above and an update to this file.
