import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DirListing } from "../bindings/DirListing";
import type { Drive } from "../bindings/Drive";
import type { LargeFile } from "../bindings/LargeFile";
import type { ScanFinishedEvent } from "../bindings/ScanFinishedEvent";
import type { ScanProgressEvent } from "../bindings/ScanProgressEvent";
import type { ScanStatus } from "../bindings/ScanStatus";
import { Storage } from "./Storage";

// IPC-boundary doubles only.
const GB = 1024 ** 3;
const drive: Drive = {
  root: "C:\\",
  kind: "fixed",
  label: "OS",
  fileSystem: "NTFS",
  isSystem: true,
  status: { state: "ready" },
  space: { totalBytes: 300 * GB, usedBytes: 200 * GB, freeBytes: 100 * GB, availableBytes: 100 * GB, lowSpace: false },
};

const finished: ScanFinishedEvent = {
  id: 1,
  root: "C:\\",
  stats: {
    dirs: 10,
    files: 40,
    totalBytes: 200 * GB,
    totalLogicalBytes: 210 * GB,
    accessDenied: 3,
    errors: 0,
    linksSkipped: 2,
    excluded: 0,
    cancelled: false,
    truncated: false,
    elapsedMs: 4200,
  },
};

const listings: Record<number, DirListing> = {
  0: {
    scanId: 1,
    node: 0,
    path: "C:\\",
    crumbs: [{ id: 0, name: "C:\\" }],
    totalBytes: 200 * GB,
    filesBytes: 20 * GB,
    filesHere: 2,
    status: { state: "complete" },
    children: [
      { id: 1, name: "Users", totalBytes: 150 * GB, fileCount: 30, hasChildren: true, status: { state: "complete" } },
      { id: 2, name: "Windows", totalBytes: 30 * GB, fileCount: 8, hasChildren: false, status: { state: "complete" } },
      { id: 3, name: "Documents and Settings", totalBytes: 0, fileCount: 0, hasChildren: false, status: { state: "link", kind: "junction" } },
      { id: 4, name: "System Volume Information", totalBytes: 0, fileCount: 0, hasChildren: false, status: { state: "accessDenied" } },
    ],
    hiddenChildren: 0,
    hiddenBytes: 0,
  },
  1: {
    scanId: 1,
    node: 1,
    path: "C:\\Users",
    crumbs: [
      { id: 0, name: "C:\\" },
      { id: 1, name: "Users" },
    ],
    totalBytes: 150 * GB,
    filesBytes: 0,
    filesHere: 0,
    status: { state: "complete" },
    children: [{ id: 5, name: "dev", totalBytes: 150 * GB, fileCount: 30, hasChildren: false, status: { state: "complete" } }],
    hiddenChildren: 0,
    hiddenBytes: 0,
  },
};
const largest: LargeFile[] = [{ path: "C:\\pagefile.sys", bytes: 8 * GB, logicalBytes: 8 * GB }];

function mockBackend(status: ScanStatus = { running: null, last: null }) {
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      switch (cmd) {
        case "list_drives":
          return [drive];
        case "scan_status":
          return status;
        case "start_scan":
          return 1;
        case "cancel_scan":
          return true;
        case "scan_listing":
          return listings[(args as { node: number }).node];
        case "scan_largest_files":
          return largest;
        default:
          throw new Error(`unexpected command ${cmd}`);
      }
    },
    { shouldMockEvents: true },
  );
  return calls;
}

const progress = (bytes: number): ScanProgressEvent => ({
  id: 1,
  root: "C:\\",
  progress: { dirs: 1234, files: 56789, bytes, problems: 2 },
});

describe("Storage analysis", () => {
  it("explains what analysis does before any scan", async () => {
    mockBackend();
    render(<Storage />);
    expect(await screen.findByText(/never opens file contents or changes anything/)).toBeInTheDocument();
  });

  it("starts a scan, shows live progress, and supports cancel", async () => {
    const calls = mockBackend();
    render(<Storage />);
    await userEvent.click(await screen.findByRole("button", { name: "Analyze C:\\" }));
    expect(calls).toContainEqual({ cmd: "start_scan", args: { root: "C:\\" } });

    await act(() => emit("scan-progress", progress(5 * GB)));
    expect(await screen.findByText(/5\.00 GB counted in 56,789 files and 1,234 folders/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Analyze C:\\" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(calls).toContainEqual({ cmd: "cancel_scan", args: { id: 1 } });
  });

  it("shows results with honest caveats and lets the user drill down", async () => {
    mockBackend();
    render(<Storage />);
    await screen.findByRole("button", { name: "Analyze C:\\" });
    await act(() => emit("scan-finished", finished));

    expect(await screen.findByText(/3 folders could not be read without administrator rights/)).toBeInTheDocument();
    expect(screen.getByText(/2 links \(junctions and symlinks\) were not followed/)).toBeInTheDocument();
    expect(await screen.findByText("Junction, not followed")).toBeInTheDocument();
    expect(screen.getByText("Access denied, not counted")).toBeInTheDocument();
    expect(screen.getByText("2 files directly in this folder")).toBeInTheDocument();
    expect(await screen.findByText("C:\\pagefile.sys")).toBeInTheDocument();

    // Windows has no subfolders in this listing, so it is not a drill-down target.
    expect(screen.queryByRole("button", { name: /^Windows/ })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^Users/ }));
    const path = await screen.findByRole("navigation", { name: "Folder path" });
    expect(within(path).getByText("Users")).toHaveAttribute("aria-current", "location");
    expect(await screen.findByText("dev")).toBeInTheDocument();

    await userEvent.click(within(path).getByRole("button", { name: "C:\\" }));
    expect(await screen.findByText("Windows")).toBeInTheDocument();
  });

  it("restores a scan that is already running when the page opens", async () => {
    mockBackend({ running: progress(9 * GB), last: null });
    render(<Storage />);
    expect(await screen.findByText(/9\.00 GB counted/)).toBeInTheDocument();
  });

  it("reports a failed scan", async () => {
    mockBackend();
    render(<Storage />);
    await screen.findByRole("button", { name: "Analyze C:\\" });
    await act(() => emit("scan-failed", { id: 1, root: "C:\\", message: "cannot scan this location: path does not exist" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/cannot scan this location/);
  });
});
