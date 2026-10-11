# Writing a Sentinel plugin

A plugin tells Sentinel about **one more place where a tool keeps regenerable files**, such as a
cache folder. It is a JSON file. It cannot run code, use the network, add screens, or remove
anything on its own: Sentinel previews what it offers, the person approves the plugin, and every
cleanup still needs a separate confirmation listing each item. The reasoning is in
[PLUGINS.md](PLUGINS.md); a complete example is
[examples/acme-render.plugin.json](examples/acme-render.plugin.json).

## The file

```json
{
  "format": 1,
  "id": "example.acme-render",
  "name": "Acme Render caches",
  "version": "1.0.0",
  "author": "Acme Tools",
  "description": "Frame and shader caches kept by Acme Render.",
  "capabilities": ["filesystem.read", "filesystem.cleanup"],
  "cleanup": [ { ... one or more cleanup types ... } ]
}
```

| Field | Rule |
|---|---|
| `format` | `1` |
| `id` | 3 to 64 characters: lower-case letters, digits, `-` and `.`; no doubled, leading or trailing separators |
| `name`, `author` | 1 to 64 characters of plain text |
| `version` | `major.minor.patch`, digits only |
| `description` | 1 to 400 characters of plain text |
| `capabilities` | exactly `filesystem.read` and `filesystem.cleanup`; anything else is refused |
| `cleanup` | 1 to 16 cleanup types |

Plain text means: no line breaks or tabs, no control characters, and no invisible or
direction-changing characters. Everything the plugin writes is shown to the person marked
"written by the plugin".

### A cleanup type

| Field | Rule |
|---|---|
| `id` | 1 to 32 characters: lower-case letters, digits and `-`; unique within the plugin |
| `name` | 1 to 64 characters |
| `root.anchor` | `localAppData`, `roamingAppData` or `userProfile`. Nothing else exists: not Documents, Desktop, a drive, Windows or Program Files |
| `root.path` | the folder below the anchor, 1 to 8 parts separated by `\` or `/`. No `..`, drive letters, `:`, wildcards, `~`, reserved names (`CON`, `NUL`, ...), names ending in a dot or space, or protected names (`.git`, `.ssh`, `.env`, key and disk files). Under `userProfile` the first part cannot be a personal folder (`Documents`, `Pictures`, `OneDrive`, `AppData`, ...) |
| `children.names` | 1 to 32 names of the **direct children** of the root that may be offered. An exact name, or a name with one `*` (`dx-*`, `*.tmp`, or just `*`). Case does not matter |
| `risk` | `lowRisk` or `mediumRisk`. `safe` is reserved for Sentinel's own cleanup types, so a plugin's items are never part of a plan made with the Safe limit |
| `minAgeDays` | 3 to 3650; default 7. Anything changed more recently is never offered |
| `description` | 1 to 300 characters: what the files are |
| `onRemoval` | 1 to 400 characters: what happens if they are removed, and whether they come back. Be honest: this is what the person reads before approving |

A file is at most 64 KB. Unknown fields are an error, so a typo is caught rather than ignored.

## What happens to it

1. **Plugins page, Choose a plugin file.** Sentinel validates the file and shows the review
   screen: the exact folders on this computer, the names it would offer, the risk and age after
   Sentinel's limits, the fingerprint, and a live dry run of what is there right now. Nothing is
   stored yet.
2. **Approve.** Sentinel stores exactly what was shown. The file is never read again.
3. The plugin's cleanup types then appear on the Cleanup page, labelled as the plugin's, and
   in "Free up space" plans at or above their risk. Preview, confirmation, quarantine, restore
   and the audit log work exactly as for Sentinel's own cleanup types.

## What it cannot do, whatever it says

- Protected locations stay protected: if a candidate is, or contains, a protected item (Git data,
  keys, `.env` files, virtual disks, the machine's protected folders), it is not offered.
- A root that a link or junction leads out of its anchor is not used.
- Recent files, files in use and anything Sentinel cannot fully verify are left alone.
- Changing any byte of a plugin makes it a different plugin that must be approved again.

## Checking your file

The easiest check is the Plugins page itself; it reports the first problem it finds, naming the
field. The same rules are tested in `crates/sentinel-plugins`, and the example above is parsed by
a test, so it stays valid.
