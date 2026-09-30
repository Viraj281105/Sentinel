import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DirListing } from "../bindings/DirListing";
import type { Drive } from "../bindings/Drive";
import type { DriveTrend } from "../bindings/DriveTrend";
import type { LargeFileView } from "../bindings/LargeFileView";
import type { SavedScan } from "../bindings/SavedScan";
import type { ScanFinishedEvent } from "../bindings/ScanFinishedEvent";
import type { ScanProgressEvent } from "../bindings/ScanProgressEvent";
import type { ScanRef } from "../bindings/ScanRef";
import type { ScanStatus } from "../bindings/ScanStatus";
import { Storage } from "./Storage";

// IPC-boundary doubles only.
const GB = 1024 ** 3;
const C = "C:\\";
const drive: Drive = {
  root: C,
  kind: "fixed",
  label: "OS",
  fileSystem: "NTFS",
  isSystem: true,
  status: { state: "ready" },
  space: { totalBytes: 300 * GB, usedBytes: 200 * GB, freeBytes: 100 * GB, availableBytes: 100 * GB, lowSpace: false },
};

const previous: ScanRef = { scanId: 6, finishedAtMs: Date.UTC(2026, 8, 20, 10, 0), totalBytes: 190 * GB };
const saved: SavedScan = {
  scanId: 7,
  root: C,
  finishedAtMs: Date.UTC(2026, 8, 30, 10, 0),
  minFolderBytes: 1024 * 1024,
  previous,
  categories: [
    { category: "packageCaches", bytes: 100 * GB },
    { category: "windows", bytes: 60 * GB },
    { category: "unknown", bytes: 40 * GB },
  ],
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
const finished: ScanFinishedEvent = { id: 1, scan: saved };

const complete = { state: "complete" } as const;
const listings: Record<number, DirListing> = {
  0: {
    scanId: 7,
    node: 0,
    crumbs: [{ id: 0, name: C }],
    totalBytes: 200 * GB,
    filesBytes: 20 * GB,
    filesHere: 2,
    status: complete,
    classification: null,
    children: [
      {
        id: 1,
        name: "Users",
        totalBytes: 150 * GB,
        fileCount: 30,
        hasChildren: true,
        status: complete,
        previousBytes: 145 * GB,
        classification: null,
      },
      {
        id: 2,
        name: "Windows",
        totalBytes: 30 * GB,
        fileCount: 8,
        hasChildren: false,
        status: complete,
        previousBytes: null,
        classification: { category: "windows", rule: "windows", reason: "Windows system files" },
      },
      {
        id: 3,
        name: "Documents and Settings",
        totalBytes: 0,
        fileCount: 0,
        hasChildren: false,
        status: { state: "link", kind: "junction" },
        previousBytes: null,
        classification: null,
      },
      {
        id: 4,
        name: "System Volume Information",
        totalBytes: 0,
        fileCount: 0,
        hasChildren: false,
        status: { state: "accessDenied" },
        previousBytes: null,
        classification: {
          category: "windows",
          rule: "root.system-volume-information",
          reason: "System restore points and volume shadow copies",
        },
      },
    ],
    hiddenChildren: 12,
    hiddenBytes: 3 * 1024 * 1024,
    comparedTo: previous,
    previousTotalBytes: 190 * GB,
  },
  1: {
    scanId: 7,
    node: 1,
    crumbs: [
      { id: 0, name: C },
      { id: 1, name: "Users" },
    ],
    totalBytes: 150 * GB,
    filesBytes: 0,
    filesHere: 0,
    status: complete,
    classification: null,
    children: [
      {
        id: 5,
        name: "dev",
        totalBytes: 150 * GB,
        fileCount: 30,
        hasChildren: false,
        status: complete,
        previousBytes: 150 * GB,
        classification: null,
      },
    ],
    hiddenChildren: 0,
    hiddenBytes: 0,
    comparedTo: previous,
    previousTotalBytes: 145 * GB,
  },
};
const largest: LargeFileView[] = [
  {
    path: String.raw`C:\pagefile.sys`,
    bytes: 8 * GB,
    logicalBytes: 8 * GB,
    classification: { category: "windows", rule: "root.pagefile", reason: "Windows page file (virtual memory)" },
  },
];

function mockBackend(status: ScanStatus = { running: null, last: null }, trends: DriveTrend[] = []) {
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      switch (cmd) {
        case "list_drives":
          return [drive];
        case "drive_trends":
          return trends;
        case "scan_status":
          return status;
        case "start_scan":
          return 1;
        case "cancel_scan":
          return true;
        case "scan_listing": {
          const { scanId, node } = args as { scanId: number; node: number };
          if (scanId !== 7) throw new Error(`wrong scan ${scanId}`);
          return listings[node];
        }
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
  root: C,
  progress: { dirs: 1234, files: 56789, bytes, problems: 2 },
});

describe("Storage analysis", () => {
  it("explains what analysis does before any scan", async () => {
    mockBackend();
    render(<Storage />);
    expect(await screen.findByText(/never opens file contents or changes anything/)).toBeInTheDocument();
  });

  it("shows the free-space trend on a drive card", async () => {
    mockBackend(undefined, [{ root: C, sinceMs: Date.UTC(2026, 8, 1), freeChangeBytes: -2 * GB, samples: 4 }]);
    render(<Storage />);
    expect(await screen.findByText(/Free space \u22122\.00 GB since/)).toBeInTheDocument();
  });

  it("starts a scan, shows live progress, and supports cancel", async () => {
    const calls = mockBackend();
    render(<Storage />);
    await userEvent.click(await screen.findByRole("button", { name: `Analyze ${C}` }));
    expect(calls).toContainEqual({ cmd: "start_scan", args: { root: C } });

    await act(() => emit("scan-progress", progress(5 * GB)));
    expect(await screen.findByText(/5\.00 GB counted in 56,789 files and 1,234 folders/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: `Analyze ${C}` })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(calls).toContainEqual({ cmd: "cancel_scan", args: { id: 1 } });
  });

  it("shows results with caveats, comparison and drill-down", async () => {
    mockBackend();
    render(<Storage />);
    await screen.findByRole("button", { name: `Analyze ${C}` });
    await act(() => emit("scan-finished", finished));

    expect(await screen.findByText(/3 folders could not be read without administrator rights/)).toBeInTheDocument();
    expect(screen.getByText(/2 links \(junctions and symlinks\) were not followed/)).toBeInTheDocument();
    expect(await screen.findByText("Junction, not followed")).toBeInTheDocument();
    expect(screen.getByText("Access denied, not counted")).toBeInTheDocument();
    expect(screen.getByText("2 files directly in this folder")).toBeInTheDocument();
    expect(screen.getByText(/12 smaller folders \(3\.00 MB\) not listed individually/)).toBeInTheDocument();
    expect(await screen.findByText(String.raw`C:\pagefile.sys`)).toBeInTheDocument();

    // Category breakdown: largest first, "Not classified" last, shares of the total.
    const breakdown = screen.getByText("Package caches").closest("ul") as HTMLElement;
    const labels = within(breakdown).getAllByRole("listitem").map((li) => li.textContent ?? "");
    expect(labels[0]).toMatch(/^Package caches.*50\.0%$/);
    expect(labels[2]).toMatch(/^Not classified.*20\.0%$/);
    // Tags explain classified folders and files; unclassified ones get none.
    expect(screen.getByTitle("Windows page file (virtual memory)")).toHaveTextContent("Windows");
    expect(screen.getByTitle("System restore points and volume shadow copies")).toBeInTheDocument();

    // Comparison with the previous analysis of the same drive.
    expect(screen.getByText(/Since the previous analysis on/)).toBeInTheDocument();
    expect(screen.getByText("+10.0 GB")).toBeInTheDocument();
    expect(screen.getByText("+5.00 GB")).toBeInTheDocument();
    expect(screen.getByText("new or was under 1 MB")).toBeInTheDocument();

    // Windows has no subfolders here, so it is not a drill-down target.
    expect(screen.queryByRole("button", { name: /^Windows/ })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^Users/ }));
    const path = await screen.findByRole("navigation", { name: "Folder path" });
    expect(within(path).getByText("Users")).toHaveAttribute("aria-current", "location");
    expect(await screen.findByText("dev")).toBeInTheDocument();
    expect(screen.getByText("unchanged")).toBeInTheDocument();

    await userEvent.click(within(path).getByRole("button", { name: C }));
    expect((await screen.findAllByText("Windows")).length).toBeGreaterThan(0);
  });

  it("says when an old analysis has no category breakdown", async () => {
    mockBackend({ running: null, last: { ...saved, categories: [] } });
    render(<Storage />);
    expect(await screen.findByText(/saved before Sentinel classified storage/)).toBeInTheDocument();
  });

  it("shows the last saved analysis when the page opens", async () => {
    mockBackend({ running: null, last: saved });
    render(<Storage />);
    expect(await screen.findByText(`Analysis of ${C}`)).toBeInTheDocument();
    expect(await screen.findByText("Users")).toBeInTheDocument();
  });

  it("restores a scan that is already running when the page opens", async () => {
    mockBackend({ running: progress(9 * GB), last: null });
    render(<Storage />);
    expect(await screen.findByText(/9\.00 GB counted/)).toBeInTheDocument();
  });

  it("reports a failed scan", async () => {
    mockBackend();
    render(<Storage />);
    await screen.findByRole("button", { name: `Analyze ${C}` });
    await act(() => emit("scan-failed", { id: 1, root: C, message: "cannot scan this location: path does not exist" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/cannot scan this location/);
  });
});
