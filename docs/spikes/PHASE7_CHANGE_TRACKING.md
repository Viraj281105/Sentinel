# Phase 7 spike: NTFS change journal (USN) vs `ReadDirectoryChangesW`

Question (ARCHITECTURE.md, open question 2): does USN journal monitoring justify its admin
requirement, or should Sentinel track changes with `ReadDirectoryChangesW` on chosen roots?

Code: `spikes/change-tracking` (throwaway, never shipped). Measured on the maintainer's
machine, 2026-10-05, Windows 11, **not elevated** (as Sentinel runs), NTFS volumes C: and D:.
Run it yourself: `cargo run --release -p spike-change-tracking -- usn | burst | ambient <folder>`.

## Recommendation

**Use `ReadDirectoryChangesW`, unelevated, as a "this folder changed" hint while Sentinel
runs. Do not build on the USN journal now.** Treat events as hints only: they mark
directories dirty and the existing scanner re-reads those directories. Never reconstruct
state from event contents. An overflow then costs a rescan, not correctness.

Add one free extra from what the USN journal does allow without elevation: at startup,
query the journal's metadata (id and `NextUsn`) through a root-folder handle and compare it
with the values stored at the last scan. Unchanged means the volume saw no changes and the
rescan can be skipped. Changed means "rescan", with the size of the change as a hint.

Revisit the full USN journal only if startup rescans prove too slow *and* an elevation
helper exists for other reasons (open question 1). The numbers below say it would still not
bridge long gaps.

## What was measured

### 1. What an unelevated process can do with the USN journal

| Operation | Result |
|---|---|
| Open the volume device (`\\.\C:`) with no access or `FILE_READ_ATTRIBUTES` | opens, but `FSCTL_QUERY_USN_JOURNAL` fails (`ERROR_INVALID_FUNCTION`) |
| Open the volume device with `GENERIC_READ` | **Access denied** |
| `FSCTL_QUERY_USN_JOURNAL` through a handle to the **root folder** (`C:\`) | **works**: journal id, `NextUsn`, size cap |
| `FSCTL_READ_USN_JOURNAL` (the change records) through that handle | **Access denied** |
| `FSCTL_ENUM_USN_DATA` (every file on the volume) through that handle | **Access denied** |

So the journal's *existence and position* are readable without elevation; its *content* is
not. Reading records needs an elevated process (not tested here: Sentinel is unelevated by
decision, and this follows Microsoft's documented requirement). A per-file
`FSCTL_READ_FILE_USN_DATA` call failed with `ERROR_INVALID_PARAMETER` in this spike and was
not pursued: it only describes one file's last record anyway.

Journal facts observed:

| Volume | Size cap | `NextUsn` advance over 5 s of ordinary use |
|---|---|---|
| C: | 32 MB | 15,168 bytes (about 3 KB/s) |
| D: | 32 MB | 0 bytes |

At C:'s observed rate the 32 MB journal holds roughly **three hours** of history, and the
rate in this sample was light, so heavier use shortens it. Even with elevation, USN could
only bridge *short* gaps (app closed for a morning), not an overnight or a weekend, unless
the user enlarges the journal. Journal id changes (journal deleted or recreated) must also
be handled, and records carry file reference numbers, so paths need a second lookup.

### 2. `ReadDirectoryChangesW` under a burst

20,000 files created, rewritten and deleted in 50 subfolders (about 60,000 file operations
in about 6 s, about 100,000 events) with a recursive watch and the consumer simulated by a
per-batch delay:

| Buffer | Consumer | Events received | Overflows |
|---|---|---|---|
| 4 KB | none | 100,099 of ~100,000 | 0 |
| 64 KB | none | 100,048 | 0 |
| 512 KB | none | 100,047 | 0 |
| 64 KB | 2 ms/batch | 100,050 | 0 |
| 64 KB | 20 ms/batch | 100,051 | 0 |
| 512 KB | 20 ms/batch | 100,049 | 0 |
| 4 KB | 20 ms/batch | **293** | 217 |
| 64 KB | 200 ms/batch | **23** | 23 |
| 64 KB | 1000 ms/batch | **5** | 5 |
| 512 KB | 200 ms/batch | 100,046 | 0 |
| 512 KB | 1000 ms/batch | **11,660** | 4 |
| 1024 KB | 1000 ms/batch | 100,050 | 0 |

(Event counts slightly above 100,000 are directory events and rounding in the estimate.)

Findings:

- With a fast reader there was **no loss at any buffer size**, even at roughly 16,000
  events/s, and the reader's CPU was at or below the timer resolution (under about 0.7 s
  over 6 s, near zero once batched).
- Loss is **all or nothing**. On overflow the kernel discards the whole buffer and returns
  a bare "overflow" signal: an interval's events collapse into one notice. The reader must
  therefore never block: do no database or disk work in the thread that calls the API.
- The overflow boundary depends on buffer size times batch rate against the burst rate; a
  1-second stall lost data even at 512 KB, while 1 MB survived. A large buffer plus a
  reader that only enqueues is the defense, and overflow still has to be handled.

### 3. Ambient noise on a real machine (20 s each, 64 KB buffer, recursive)

| Folder | Events | Rate | Overflows |
|---|---|---|---|
| `C:\Users\<user>` | 2,838 | 142 /s | 0 |
| `D:\` (projects drive) | 0 | 0 /s | 0 |

96 % of the profile's events were under `AppData\Local\Temp`. Watching must filter or
aggregate known-noisy folders (the project detector already has a skip list) or growth
history will mostly record temp files.

## Comparison

| | USN journal | `ReadDirectoryChangesW` |
|---|---|---|
| Needs elevation | **Yes** to read changes (metadata does not) | No |
| Works while Sentinel is closed | Yes, within the journal's retention (about 3 h here) | **No**: only while running |
| Paths in events | No: file reference numbers, second lookup needed | Yes |
| Loss behavior | Wraps and discards the oldest records; detectable | Overflow discards a whole interval; detectable |
| Scope | Whole volume | Chosen roots, one handle each |
| Setup burden | Elevation helper, journal-reset handling, FRN to path | A reader thread, a dirty set, a rescan fallback |

## Design this implies for Phase 7

1. One watcher per searched/scanned root, recursive, 256 KB to 1 MB buffer.
2. A reader thread that only moves raw batches to a queue; a worker folds them into a
   *dirty-directory set* (deduplicated, noisy folders dropped).
3. On overflow, or a root that cannot be watched (network or removable drive), mark the
   root dirty: a full rescan of that root.
4. Incremental rescan of only the dirty directories through the existing scanner.
5. At startup, compare the unelevated journal position with the stored one: unchanged
   volume means skip, changed means rescan (a scan scheduler decides when).
6. Growth history records sizes from scans, not from events.

## Not measured / limits of this spike

- One machine, one 20 s ambient sample per folder, light load. Not a benchmark.
- Reading USN records was not tested because it needs elevation; those rows come from
  documentation, not measurement.
- Memory held by the kernel for large buffers was not measured; on network shares the
  buffer is limited to 64 KB (documented).
- Reparse points, junctions, removable and network drives, and rename-heavy workloads
  were not exercised.
- The per-file USN call failed with an invalid-parameter error and was not investigated.
