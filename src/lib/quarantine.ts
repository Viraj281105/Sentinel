import type { EntryStatus } from "../bindings/EntryStatus";

/** Plain-language status of one quarantine entry. */
export function statusText(s: EntryStatus): string {
  switch (s.state) {
    case "pending":
      return "Not moved (interrupted)";
    case "quarantined":
      return "In quarantine";
    case "restored":
      return "Restored";
    case "purged":
      return "Deleted after 14 days";
    case "skipped":
      return `Left in place: ${s.reason}`;
    case "failed":
      return `Failed: ${s.reason}`;
  }
}
