const UNITS = ["bytes", "KB", "MB", "GB", "TB", "PB"] as const;

/**
 * Format a byte count the way Windows Explorer does: powers of 1024 with the
 * familiar KB/MB/GB labels, so numbers match what users see elsewhere.
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} ${bytes === 1 ? "byte" : "bytes"}`;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value < 10 ? 2 : value < 100 ? 1 : 0;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** Changes smaller than this are shown as "unchanged" (it is also the smallest folder
 *  size stored for comparison, so finer changes are not reliably known). */
export const CHANGE_THRESHOLD_BYTES = 1024 * 1024;

/** Signed size change, e.g. "+1.20 GB" or "−300 MB" (true minus sign). */
export function formatDelta(bytes: number): string {
  if (!Number.isFinite(bytes)) return "—";
  if (Math.abs(bytes) < CHANGE_THRESHOLD_BYTES) return "unchanged";
  return `${bytes > 0 ? "+" : "−"}${formatBytes(Math.abs(bytes))}`;
}

export function formatDateTime(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function formatDate(ms: number): string {
  return new Date(ms).toLocaleDateString(undefined, { dateStyle: "medium" });
}

export function percent(part: number, whole: number): number {
  return whole > 0 ? (part / whole) * 100 : 0;
}
