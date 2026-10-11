# Security review: executor, quarantine and recovery-plan paths

Reviewed 2026-10-11 against THREAT_MODEL.md and SAFETY_MODEL.md. Scope: the code that
changes files, namely `sentinel-quarantine` (move, restore, purge), the commands that reach it
(`cleanup_run`, `project_cleanup_run`, `run_recovery_plan`, `quarantine_restore`) and the
recovery planner. This was a read-through of the source with targeted tests; it is not an
independent audit, and it did not exercise the code on a machine other than the maintainer's.

## What was checked, and held up

| Property | Where | Result |
|---|---|---|
| Audit record written *before* anything moves; failure stops the operation | `Quarantine::quarantine`, `restore`, `purge_expired` | Holds. Tested (`audit_failure_changes_nothing`) |
| Only approved paths, re-assessed immediately before each move (protected locations, age, links) | `move_one` calls `assess` then `open_verified` | Holds |
| Move is a rename through a handle whose identity was verified; never replaces; cannot cross volumes | `rename_by_handle` (`ReplaceIfExists = false`), drive check in `move_one` | Holds |
| Items outside the provider's roots, nested items, unapproved items untouched | `move_items` root match, `is_candidate`, `assess` | Holds. Tested |
| Operation ids cannot escape the quarantine folder | `valid_operation_id` (`[A-Za-z0-9_-]`, 64 chars) | Holds. Entry indexes are numbers joined to the operation folder |
| Quarantine folder and its parent must be plain folders, not links | `ensure_root` | Holds. Tested for both |
| Restore never overwrites and re-checks the destination against the policy | `restore` (`allowed_root` on the parent, `check_protected` on the destination) | Holds. A parent that is a junction into a protected folder resolves to it and is refused |
| Purge never follows links | `remove_without_following` | Holds. Tested with a junction to an outside folder |
| A plan cannot contain anything above medium risk or anything analysis-only | planner, plus a second check in `recovery::execute` | Holds. Tested |
| The frontend cannot name arbitrary project folders for a plan | `run_recovery_plan` rebuilds the project list from saved searches | Holds |

## Defects found and fixed

1. **Purge trusted the record's operation id instead of the folder it was in.** A record in
   folder `op-A` that said `op-B` made purge delete `op-B`'s items, which might not have expired.
   Reaching this needs write access to the quarantine folder (the same user), but it also
   protects against any bug that mislabels a record. *Fix:* `read_manifest` refuses a record
   whose operation id differs from its folder; listing reports it, restore and purge ignore it.
2. **One odd folder aborted the whole purge.** A folder whose name was not a valid operation id
   made `purge_expired` return an error through `?`, so no operation after it ever expired.
   *Fix:* it is reported in the purge report and skipped; the others continue.
3. **A crash between the rename and the record update stranded the item.** The item sat in
   quarantine marked `Pending`, so the UI could not restore it, purge ignored it, and its folder
   failed to delete on every start. No data was lost, but the space was never recovered and a
   person could not get the file back from the app. *Fix:* a `Pending` entry whose item is
   present is read as quarantined. (Its size and kind were never recorded in that case and show
   as zero.)

Each fix has a test that fails on the old code (verified by reverting the fix) and passes now.

## Residual risks (documented, not fixed)

- **Purge depends on the system clock.** Items are deleted permanently once 14 days have passed
  by the clock. A clock set far into the future (dead CMOS battery, a mistaken time change)
  would purge early at the next start. Windows offers no trusted time source to check against.
  A clock moved *back* only delays purging. A possible hardening, not done: refuse to purge in
  a session whose first reading of the clock is more than a few days ahead of the newest audit
  record, and ask first.
- **The quarantine records are plain files.** Anyone who can write to the user's
  `%LOCALAPPDATA%\Sentinel` can already read or delete the quarantined files themselves, so
  editing a record gains nothing, except that a record's `originalPath` decides where restore
  puts the item. Restore refuses protected destinations, existing files and non-directories,
  but would restore into any other existing folder. There is no integrity seal on the records
  (the audit log is hash-chained; the manifests are not).
- **A file added inside a folder item between its check and the rename** moves with the folder
  (reversibly; documented since T3).
- **Plan previews are not written to the audit log.** The Cleanup page previews are. Runs always
  are. This was a choice to avoid noise; it means "what was proposed" is not recoverable.
- **No size limit on a record when it is read.** Same-user only; a huge file would use memory.
- **Quarantine location.** It is in the user's profile, and items on other drives are refused,
  rather than a per-volume quarantine with an owner-only ACL (designed earlier, not built).

## Not reviewed

The elevation helper (it does not exist), plugins (design only), the AI path beyond
docs/AI_POLICY.md, and the Tauri webview configuration beyond reading `tauri.conf.json` (strict
CSP, no remote content, capabilities limited to the core defaults and opening the folder picker).
