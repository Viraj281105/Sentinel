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

export function percent(part: number, whole: number): number {
  return whole > 0 ? (part / whole) * 100 : 0;
}
