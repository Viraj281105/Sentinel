# Performance budget

What Sentinel should cost, what has actually been measured, and what has not. Budgets are
proposals with a reason; the maintainer can change them. Every measured number is from **one
machine** (Windows 11, 24 logical cores, NVMe, 2026-10-11 and the earlier runs noted below) and
shows the order of magnitude, not a guarantee for other hardware or a spinning disk.

## Measured

| What | Result | How to repeat |
|---|---|---|
| Full analysis of the system drive: 1.1 M files, 269 k folders | 21 s cold, 6.4 s warm, 4 threads (recorded earlier, ARCHITECTURE.md) | `cargo run --release -p sentinel-scanner --example scan -- C:\ 4` |
| Analysis of `D:\Projects`: 147,519 files, 12,546 folders, 64 GB | 473 ms on the first run, 151 ms on the next (warm cache) | same, with `D:\Projects` |
| Saving an analysis to the database: 269 k folders | about 32 ms, about 2.4 MiB (recorded earlier) | `cargo run --release -p sentinel-store --example save_scan -- C:\` |
| Saving the 12,546-folder analysis | 3.0 ms; opening the root listing 0.14 ms | same |
| Saved tree for restarts, at the 400,000-folder retention limit | 34.0 MiB (89 bytes per folder); encode 49 ms; decode plus structure validation 69 ms | `cargo run --release -p sentinel-scanner --example snapshot_bench -- 400000` |
| The same at 100,000 folders | 8.5 MiB; encode 12 ms; decode 17 ms | same, with `100000` |
| Folder watcher under a burst of about 100,000 events | no loss with a fast reader; CPU near the timer resolution (docs/spikes/PHASE7_CHANGE_TRACKING.md) | the spike's `burst` mode |

## Budgets

| Budget | Why | Status |
|---|---|---|
| A full analysis of a drive with about 1 M files finishes in **30 s or less cold** on an SSD, and never blocks the window (it runs on its own threads, with progress and Cancel) | Long enough to be tolerable for an explicit action, short enough not to need a background scheduler | Met on the measured machine (21 s). **Not measured** on a hard disk drive, where directory listing is far slower; expect minutes |
| Saving an analysis takes **under 100 ms** | It happens on the scan thread right after the scan | Met (32 ms at 269 k folders) |
| Restoring the saved tree at startup takes **under 1 s** and never delays the window | It runs on a background thread (`sentinel-restore`); the window does not wait for it | Met (69 ms at the limit). The disk read of the 34 MiB file was not timed separately |
| The saved tree file is **at most about 50 MiB** | The in-memory limit is 400,000 folders; the file must not exceed what the memory limit implies | Met (34 MiB at the limit) |
| An incremental update re-reads only changed folders, so its cost is proportional to the changes | The reason it exists | Verified in tests at fixture scale only. **Not measured** on a large drive |
| Idle: no scanning, no polling of the disk; one low-space check every 15 minutes, and only if the person turned alerts on | A background tool that is not wanted costs trust | By design. The check lists drive capacity only. CPU was not measured |
| Memory for the retained tree: **about 100 MB at most** (400,000 folders) | Stated as the reason for the limit in the code | **Not measured.** The figure is an estimate; the snapshot file size suggests tens of MB for the data, plus allocation overhead |
| Application start to a usable window | Should feel instant | **Not measured.** It needs the app launched, which was not done here |

## Known costs worth knowing

- Saving the retained tree briefly holds a second copy of it in memory (the tree is cloned into
  the snapshot before encoding). At the 400,000-folder limit that is a transient tens of MB.
- The drive's change journal is read once at the start of an analysis and once at startup; both
  are single metadata calls.
- A drive with millions of folders keeps no retained tree (over the 400,000 limit), so it has no
  incremental updates and no restart restore; each update is a full analysis.

## What to measure before a public release

1. Cold analysis time of a hard-disk drive, and of a drive over 5 M files.
2. Resident memory with a 400,000-folder tree retained, and during a scan.
3. Application start to window, and the time until the first drive card appears.
4. An incremental update on a real large tree after a typical day of changes.
