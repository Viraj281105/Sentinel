# Plugins (Phase 10 design)

Status: **proposed, awaiting maintainer review. No code has been written for this.**
Everything below is a recommendation with its reasoning; each decision can be overruled, and
the open questions at the end are the ones that most need an answer before building.

## What a plugin is, and is not

A plugin teaches Sentinel about **one more place that holds regenerable files**: the cache
folder of a tool Sentinel does not know about (a game launcher, a design tool, a language
toolchain). That is the whole feature.

A plugin is **data, never code**. It is a small JSON file. It cannot run anything, read
anything, delete anything, call the network, add UI, or schedule itself. All it can do is
*describe candidates*; the existing, tested machinery decides whether any candidate is
eligible and does the work:

```
plugin file --parse+validate--> Manifest --> DeclarativeProvider (implements CleanupProvider)
                                                   |
                      same preview  -> same planner -> same confirmation -> same quarantine executor
                      (policy checks)                                       (re-check, audit, restore)
```

Why this shape: Sentinel already has the one place where safety is decided (`sentinel-safety`
plus the executor). A plugin that can only produce a `CleanupProvider` description inherits all of
it, and gains no new way to touch the disk. Executable plugins (native code, scripts, WASM) are
explicitly out of scope; see "Later".

## What the format can say (v1)

```json
{
  "format": 1,
  "id": "example.acme-render",
  "name": "Acme Render caches",
  "version": "1.0.0",
  "author": "Acme Tools",
  "description": "Frame and shader caches kept by Acme Render.",
  "capabilities": ["filesystem.read", "filesystem.cleanup"],
  "cleanup": [
    {
      "id": "shader-cache",
      "name": "Acme shader cache",
      "root": { "anchor": "localAppData", "path": "Acme\\Render\\ShaderCache" },
      "children": { "names": ["gl", "vk", "dx-*"] },
      "risk": "lowRisk",
      "minAgeDays": 14,
      "description": "Compiled shaders.",
      "onRemoval": "Acme Render recompiles them the next time it runs; the first start is slower."
    }
  ]
}
```

- **`root`** is an *anchor plus a relative path*, never an absolute path or an environment
  variable. Allowed anchors: `localAppData`, `roamingAppData`, `userProfile`. Not allowed:
  Documents, Desktop, Downloads, Pictures, Videos, OneDrive, drive roots, Windows, Program Files.
  The relative path needs at least one component and cannot contain `..`, drive letters, `:`,
  or reserved device names.
- **`children`** are the direct children of the root that count as candidates, by exact name or
  a name with at most one `*`. Candidates are always children of the root, as with every built-in
  provider; a plugin cannot offer the root itself or anything deeper.
- **`risk`** is `lowRisk` or `mediumRisk`. **`safe` is reserved for built-in providers**, so a
  plugin's items are never in a plan made with the Safe ceiling. Higher is not expressible.
- **`minAgeDays`** has a floor (3) and a ceiling; default 7. A plugin cannot offer fresh files.
- **Text fields** (`name`, `description`, `onRemoval`, `author`) are plain text with length limits,
  no control or bidirectional-override characters, always rendered as text (React escapes it) and
  always labelled as written by the plugin.
- **Not in v1:** classification rules (SAFETY_MODEL principle 6: classification describes and is
  compiled in), project detectors, runtime or software inventory, anything with a network or
  process capability. The capability list exists so the format can grow; in v1 only the two above
  are valid, and `network.none` is simply how it is.

Limits enforced before anything else: file at most 64 KB, valid UTF-8, strict schema (unknown
fields are an error), at most 16 providers per plugin, each field length-bounded.

## What the plugin can never do

These are invariants, each with a test planned (see "Implementation plan"):

1. **Cannot reach protected paths.** Every plugin root is resolved and passed through
   `Policy::allowed_root`; every candidate through `Policy::validate`; every item inside a
   candidate through `check_protected` (exactly what built-ins get). A plugin root that is or
   contains a protected location is refused with the reason, at approval time and again at every
   preview. Protected always wins (SAFETY_MODEL principle 5).
2. **Cannot lower its own risk or age.** Risk is clamped as above; the age floor is enforced by
   the framework, not by the plugin.
3. **Cannot act without a person.** A plugin's provider is preview-only until a person approves
   the plugin; running it still needs the same per-run confirmation listing every item. Nothing
   is scheduled, automatic, or triggered by the plugin.
4. **Cannot change after approval.** The approved manifest is stored in the local database with
   its SHA-256 and re-verified on load; the file the user picked is never read again, so replacing
   it on disk does nothing. A new version is a new manifest: it needs a fresh approval, with a
   comparison against the version it replaces.
5. **Cannot pose as something else.** Provider ids are namespaced (`plugin:<id>:<provider>`) so
   they cannot collide with or replace built-ins; every card, preview and plan entry shows
   "Plugin: *name* by *author*"; the same plugin id from a different hash is treated as a
   different, unapproved plugin.
6. **Cannot hide what it does.** The approval screen shows resolved absolute roots, the exact name
   patterns, the clamped risk and age, the protected-path verdict, and a **live dry run** of what
   would be offered right now, with sizes.

## Approval flow

1. *Add a plugin*: the person chooses a `.json` file with the system file picker (never a URL;
   v1 has no download, no marketplace and no auto-update).
2. Sentinel parses and validates it. Any failure shows the exact reason; nothing is stored.
3. The **approval screen** (above) shows everything, the file's SHA-256 and a plain statement:
   "This plugin can only ask Sentinel to offer these folders' contents for cleanup. It cannot run
   code, use the network, or remove anything without your confirmation each time."
4. **Approve** stores the manifest (+ hash, time) and writes an audit record (new kind `plugin`).
   **Cancel** stores nothing.
5. A **Plugins page** lists installed plugins with Disable and Remove. Disabling or removing never
   touches quarantined items; they remain restorable.

## Threats this adds (to go into THREAT_MODEL.md when built)

| # | Threat | Mitigation |
|---|---|---|
| T23 | A plugin names a root in user data or a protected area so that it is offered for cleanup | Anchors allow-list; relative-path rules; `allowed_root`/`validate`/`check_protected` on every root, candidate and descendant; refusal shown at approval and every preview |
| T24 | A plugin misdescribes its folders ("harmless") to talk the person into approving | Risk clamped (no `safe`); text always labelled as the plugin's own; approval shows resolved paths and a live dry run, not just prose; every run lists each item for confirmation |
| T25 | The plugin file is replaced after approval | Manifest stored in the database with its hash and re-verified; the file is never read again |
| T26 | A different plugin impersonates or silently replaces an approved one | Namespaced provider ids; identity is (id, hash); a changed hash is a new, unapproved plugin with a diff against the old one; no auto-update |
| T27 | Resource exhaustion through a huge manifest or pathological patterns | 64 KB cap, provider and field limits, at most one `*` per pattern (linear matching); the preview's existing entry and depth budgets apply |
| T28 | Deception in the approval screen (lookalike or control characters, very long text) | Character-class and length validation; plain-text rendering; the hash is shown |

## Implementation plan (after review)

Each step is a separate, tested commit.

1. **`sentinel-plugins` crate** (no I/O): manifest types, strict parser and validator, canonical
   form and SHA-256. Tests: a corpus of malformed and hostile manifests (path traversal, ADS,
   device names, `..`, absolute paths, forbidden anchors, bidi characters, oversized fields,
   unknown fields, two `*`), and round trip.
2. **`DeclarativeProvider`**: implements `CleanupProvider` from a validated manifest, resolving
   anchors through `known_folder`. Tests with fixture folders prove invariants 1 and 2 against
   the real preview: a plugin pointing at a protected folder or at a folder holding a protected
   file offers nothing and says why.
3. **Store**: `plugins` table (id, version, sha256, manifest JSON, enabled, approved_at), audit
   kind, and the load-time hash check. Tests for tamper detection and for the same id with a new
   hash being treated as new.
4. **Commands**: `plugin_inspect(file)` (parse + validate + dry run, stores nothing),
   `plugin_install`, `plugin_set_enabled`, `plugin_remove`; providers feed the existing
   `cleanup_providers`, preview, run and recovery-plan paths, with the clamped risk. Tests that a
   plugin's items never appear in a Safe-ceiling plan and always carry their plugin label.
5. **UI**: Plugins page, approval screen, plugin labels on cards and plan entries.
6. **Docs**: THREAT_MODEL rows above, a short authoring guide and a sample plugin in
   `docs/examples/`.

## Later (not v1, each needs its own design and threat model)

- **Sandboxed code (WASM)** behind the same capability gate, for detectors that data cannot
  express. Needs a runtime, a host API that offers only the capabilities approved, resource
  limits, and a review of the escape surface. Do not start before there is a real plugin that data
  cannot express.
- **Signing and distribution**: v1 trusts the person's approval plus the pinned hash. Signatures
  only make sense once there is a place to get plugins from.
- **Plugin-supplied classification or project detectors**: classification must stay compiled in
  unless a rule format is found that cannot grant eligibility (principle 6).

## Open questions for the maintainer

1. **Do you want plugins in v1 at all?** The same effect is available by adding built-in
   providers in code, which is safer and simpler. Plugins earn their cost only if other people (or
   you, without rebuilding) will add tools. If not, skip Phase 10 and go to polish.
2. **Anchors**: are `localAppData`, `roamingAppData`, `userProfile` the right set? `programData`
   was left out (shared, often protected by ACLs); say if you need it.
3. **Age floor of 3 days and default of 7**: built-in package caches use 1 day because they are
   fully regenerable; for unknown tools a stricter floor seems right.
4. **No `safe` risk for plugins**, so they never appear in the safest plans. Confirm.
5. **File format and extension**: plain `.json` (as drafted) or a dedicated extension?
