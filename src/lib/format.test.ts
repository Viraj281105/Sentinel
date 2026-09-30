import { describe, expect, it } from "vitest";
import { formatAge, formatBytes, formatDelta, percent } from "./format";

describe("formatAge", () => {
  it("uses plain relative wording", () => {
    const now = Date.UTC(2026, 8, 30);
    const day = 86_400_000;
    expect(formatAge(now - 1000, now)).toBe("today");
    expect(formatAge(now - day, now)).toBe("yesterday");
    expect(formatAge(now - 3 * day, now)).toBe("3 days ago");
    expect(formatAge(now - 21 * day, now)).toBe("3 weeks ago");
    expect(formatAge(now - 90 * day, now)).toBe("3 months ago");
  });
});

describe("formatDelta", () => {
  it("signs changes and hides noise below 1 MB", () => {
    expect(formatDelta(3 * 1024 ** 3)).toBe("+3.00 GB");
    expect(formatDelta(-300 * 1024 ** 2)).toBe("−300 MB");
    expect(formatDelta(500 * 1024)).toBe("unchanged");
    expect(formatDelta(-1)).toBe("unchanged");
  });
});

describe("formatBytes", () => {
  it("uses Explorer-style binary units", () => {
    expect(formatBytes(0)).toBe("0 bytes");
    expect(formatBytes(1)).toBe("1 byte");
    expect(formatBytes(1023)).toBe("1023 bytes");
    expect(formatBytes(1024)).toBe("1.00 KB");
    expect(formatBytes(37.2 * 1024 ** 3)).toBe("37.2 GB");
    expect(formatBytes(953.8 * 1024 ** 3)).toBe("954 GB");
    expect(formatBytes(2 * 1024 ** 4)).toBe("2.00 TB");
  });

  it("refuses to format nonsense", () => {
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
  });
});

describe("percent", () => {
  it("guards against a zero total", () => {
    expect(percent(5, 0)).toBe(0);
    expect(percent(1, 4)).toBe(25);
  });
});
