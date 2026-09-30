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
| T1 | Path traversal (`..`, mixed separators, `\\?\`, 8.3 short names, alternate data streams, trailing dots/spaces, device names) | Single canonicalization routine in `sentinel-safety`; all paths validated post-canonicalization; deny ADS and device-namespace paths |
| T2 | Symlink/junction redirection to a protected target | Never follow reparse points during scan or delete; delete the link, not the target; canonicalize and re-check ancestry before acting |
| T3 | TOCTOU (path swapped between validation and delete) | *Implemented:* the executor re-assesses each item immediately before acting, then `ValidatedTarget::open_verified` revalidates and opens the object itself, checking volume serial, file index and link status on the handle; the move is a rename through that handle (`SetFileInformationByHandle`, no replace, same volume only). Residual: a file added inside a folder item after its descendant check moves with the folder (reversible) |
| T4 | Malicious/erroneous cleanup rules | Rules are data validated against the same policy; a rule cannot lower a path below its protection level; built-in providers use an allowlist of roots |
| T20 | Misclassification leading to harmful advice or action (e.g. a folder named `node_modules` or `$Recycle.Bin` created to look disposable) | Classification rules are compiled in, not user-supplied; categories are display-only and never grant cleanup eligibility (SAFETY_MODEL principle 6); unmatched paths stay `Unknown` |
| T5 | Malicious plugin | Data-only in v1; capability manifest (`filesystem.read`, `filesystem.cleanup`, `process.inspect`, `network.none`); dangerous capabilities need explicit user approval; plugins cannot bypass the policy engine |
| T6 | Compromised or prompt-injected AI provider / malicious AI output | AI output is a suggestion only; strict JSON schema; unknown fields rejected; policy engine re-derives risk deterministically; AI has no executor handle |
| T7 | Prompt injection via filenames/project files | AI receives only structured metadata with sanitized, length-limited fields; never file contents |
| T8 | Data exfiltration to AI provider | Redaction layer; no file contents, `.env`, keys, tokens; external providers off by default; user-visible record of what was sent |
| T9 | Privilege escalation via elevated helper | Helper accepts only typed, pre-validated operations over an authenticated local channel; re-validates independently; no arbitrary path/command input |
| T10 | Malicious project files (e.g. crafted `package.json`, `.git` config) triggering code execution during detection | *Implemented in `sentinel-devenv`:* files are parsed only (serde_json, toml, line parsing), never executed; no package manager, build tool, script or Git command is run (a test plants a `preinstall` script and proves nothing ran); files over 1 MB are not read; parse failures become per-project warnings; names and versions are length-limited; junctions are not followed |
| T11 | Compromised package manager output when querying versions/caches | Prefer reading files/registry over invoking tools; when invoking, use absolute resolved paths, fixed args, timeouts, no shell |
| T12 | Registry manipulation / bogus uninstall entries | Registry treated as untrusted display data; never used as a path to delete |
| T19 | Local database disclosure or tampering (another local process reads or edits `sentinel.db`) | Stores metadata only (folder names, 50 largest file paths, sizes, times), never contents or secrets; lives in the per-user `%LOCALAPPDATA%` with default user-only ACLs; values read back are display data and are never used as paths to act on (the future executor revalidates everything through `sentinel-safety`); corrupt rows surface as errors, not crashes |
| T13 | Tampered audit log or quarantine | *Implemented for the audit log:* SQLite triggers reject UPDATE/DELETE; SHA-256 hash chain over canonical record JSON; verifier reports the first missing, reordered or edited record and the Activity page shows it. Known limits: an attacker with write access to the file can drop the triggers and rebuild a consistent chain, and truncating the newest records leaves a valid shorter chain (anchoring the head outside the database is a later hardening item). Quarantine (implemented, not wired to the app): lives in the user's private profile folder; must be a plain folder, not a link; manifests are versioned and operation ids restricted to `[A-Za-z0-9_-]` so they cannot escape the folder; restore never overwrites and re-checks the destination with the policy; purge deletes only inside the quarantine folder without following links |
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
