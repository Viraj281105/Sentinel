import { describe, expect, it } from "vitest";
import { formatBytes, percent } from "./format";

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
