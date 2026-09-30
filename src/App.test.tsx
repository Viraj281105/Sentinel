import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import type { AppInfo } from "./bindings/AppInfo";
import type { CommandError } from "./bindings/CommandError";
import type { Drive } from "./bindings/Drive";
import type { ProtectedLocation } from "./bindings/ProtectedLocation";

// Test doubles for the IPC boundary only; the app itself never ships sample data.
const GB = 1024 ** 3;
const locations: ProtectedLocation[] = [
  { path: String.raw`C:\Users\u\Documents`, reason: "Documents" },
  { path: String.raw`C:\Windows`, reason: "Windows system files" },
];
const info: AppInfo = {
  version: "0.1.0",
  debugBuild: false,
  logDir: String.raw`C:\logs`,
  databasePath: String.raw`C:\data\sentinel.db`,
  databaseError: null,
};
const drives: Drive[] = [
  {
    root: "C:\\",
    kind: "fixed",
    label: "Windows",
    fileSystem: "NTFS",
    isSystem: true,
    status: { state: "ready" },
    space: { totalBytes: 400 * GB, usedBytes: 370 * GB, freeBytes: 30 * GB, availableBytes: 30 * GB, lowSpace: true },
  },
  {
    root: "D:\\",
    kind: "fixed",
    label: null,
    fileSystem: "NTFS",
    isSystem: false,
    status: { state: "ready" },
    space: { totalBytes: 600 * GB, usedBytes: 240 * GB, freeBytes: 360 * GB, availableBytes: 360 * GB, lowSpace: false },
  },
  { root: "E:\\", kind: "removable", label: null, fileSystem: null, isSystem: false, status: { state: "noMedia" }, space: null },
];

function mockBackend(opts: { fail?: boolean; drivesError?: CommandError } = {}) {
  mockIPC((cmd) => {
    if (opts.fail) throw new Error("backend unavailable");
    if (cmd === "protected_locations") return locations;
    if (cmd === "app_info") return info;
    if (cmd === "scan_status") return { running: null, last: null };
    if (cmd === "drive_trends") return [{ root: "C:\\", sinceMs: Date.UTC(2026, 8, 1), freeChangeBytes: -2 * GB, samples: 5 }];
    if (cmd === "list_drives") {
      if (opts.drivesError) throw opts.drivesError;
      return drives;
    }
    throw new Error(`unexpected command ${cmd}`);
  }, { shouldMockEvents: true });
}

describe("App", () => {
  it("summarizes real drive and safety data on the overview", async () => {
    mockBackend();
    render(<App />);
    expect(await screen.findByText("30.0 GB")).toBeInTheDocument();
    expect(screen.getByText(/free on the Windows drive \(C:\)/)).toBeInTheDocument();
    expect(screen.getByText(/C: is low on space/)).toBeInTheDocument();
    expect(await screen.findByText("−2.00 GB")).toBeInTheDocument();
    expect(await screen.findByText("2")).toBeInTheDocument();
  });

  it("marks unimplemented pages honestly instead of showing data", async () => {
    mockBackend();
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: /cleanup/i }));
    expect(screen.getByRole("button", { name: /cleanup/i })).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("Not implemented yet")).toBeInTheDocument();
    expect(screen.getByText(/roadmap phase 3/i)).toBeInTheDocument();
  });

  it("shows each drive with a usage meter, low-space badge and media state", async () => {
    mockBackend();
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: /storage/i }));

    const c = (await screen.findByRole("heading", { name: "Windows (C:)" })).closest("article");
    expect(c).not.toBeNull();
    const cCard = within(c as HTMLElement);
    expect(cCard.getByText("Low space")).toBeInTheDocument();
    expect(cCard.getByRole("meter")).toHaveAttribute("aria-valuenow", "93");
    expect(cCard.getByText(/Windows installed here/)).toBeInTheDocument();

    const d = screen.getByRole("heading", { name: "Local disk (D:)" }).closest("article") as HTMLElement;
    expect(within(d).queryByText("Low space")).not.toBeInTheDocument();
    expect(within(d).getByText("360 GB")).toBeInTheDocument();

    expect(screen.getByText("No media inserted.")).toBeInTheDocument();
  });

  it("shows the backend's explanation when drive discovery fails", async () => {
    mockBackend({
      drivesError: { kind: "system", message: "Windows could not list the drives on this computer: Access is denied." },
    });
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: /storage/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/could not list the drives/);
  });

  it("lists protected locations and diagnostics on settings", async () => {
    mockBackend();
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: /settings/i }));
    expect(await screen.findByText(String.raw`C:\Windows`)).toBeInTheDocument();
    expect(screen.getByText("Windows system files")).toBeInTheDocument();
    expect(await screen.findByText(String.raw`C:\logs`)).toBeInTheDocument();
    expect(screen.getByText(String.raw`C:\data\sentinel.db`)).toBeInTheDocument();
  });

  it("explains backend failures in plain language", async () => {
    mockBackend({ fail: true });
    render(<App />);
    expect((await screen.findAllByRole("alert"))[0]).toHaveTextContent("backend unavailable");
  });
});
