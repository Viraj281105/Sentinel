import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { FolderHistory as FolderHistoryData } from "../bindings/FolderHistory";
import type { GrowthHistory as GrowthHistoryData } from "../bindings/GrowthHistory";
import { FolderHistory, GrowthHistory } from "./GrowthHistory";

// IPC-boundary doubles only.
const GB = 1024 ** 3;
const DAY = 86_400_000;
const T0 = Date.UTC(2026, 8, 1);

const growth: GrowthHistoryData = {
  root: "C:\\",
  keptAnalyses: 10,
  points: [
    { scanId: 1, finishedAtMs: T0, totalBytes: 200 * GB, files: 100, partial: false },
    { scanId: 2, finishedAtMs: T0 + 3 * DAY, totalBytes: 50 * GB, files: 90, partial: true },
    { scanId: 3, finishedAtMs: T0 + 10 * DAY, totalBytes: 230 * GB, files: 120, partial: false },
  ],
  summary: {
    fromMs: T0,
    toMs: T0 + 10 * DAY,
    fromBytes: 200 * GB,
    toBytes: 230 * GB,
    changeBytes: 30 * GB,
    bytesPerDay: 3 * GB,
    analyses: 2,
  },
  categoryChanges: [
    { category: "userData", firstBytes: 10 * GB, lastBytes: 35 * GB, changeBytes: 25 * GB },
    { category: "windows", firstBytes: 40 * GB, lastBytes: 38 * GB, changeBytes: -2 * GB },
  ],
};

describe("GrowthHistory", () => {
  it("shows the trend, its basis and its limits", async () => {
    mockIPC((cmd) => {
      if (cmd === "growth_history") return growth;
      throw new Error(`unexpected ${cmd}`);
    });
    render(<GrowthHistory scanId={3} />);
    expect(await screen.findByText(/across 2 complete\s+analyses/)).toBeInTheDocument();
    expect(screen.getByText(/about .* a day/)).toBeInTheDocument();
    expect(screen.getByText(/3 analyses from/)).toBeInTheDocument();
    expect(screen.getByText(/keeps up to 10 per drive: the newest three always/)).toBeInTheDocument();
    expect(screen.getByText(/one per week after that, so older history is sparser/)).toBeInTheDocument();
    expect(screen.getByText(/Hollow dots are cancelled/)).toBeInTheDocument();
    expect(screen.getByText(/scale does not start at zero/)).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /across 3 analyses/ })).toBeInTheDocument();
    // The cancelled analysis is drawn hollow but the trend line skips it.
    expect(document.querySelectorAll("circle[fill='none']")).toHaveLength(1);
    expect(screen.getByText("What changed most, by category")).toBeInTheDocument();
  });

  it("explains that a single analysis has no history yet", async () => {
    mockIPC(() => ({ ...growth, points: [growth.points[0]], summary: null, categoryChanges: [] }));
    render(<GrowthHistory scanId={1} />);
    expect(await screen.findByText(/History appears after a second analysis/)).toBeInTheDocument();
  });

  it("does not invent a trend from one complete analysis", async () => {
    mockIPC(() => ({ ...growth, summary: null, categoryChanges: [] }));
    render(<GrowthHistory scanId={3} />);
    expect(await screen.findByText(/Only one complete analysis, so there is no trend yet/)).toBeInTheDocument();
  });
});

describe("FolderHistory", () => {
  const folder: FolderHistoryData = {
    path: String.raw`C:\Users\u\AppData`,
    summary: null,
    points: [
      { scanId: 1, finishedAtMs: T0, partial: false, state: "missing", bytes: null, maxBytes: null },
      { scanId: 2, finishedAtMs: T0 + DAY, partial: false, state: "belowThreshold", bytes: null, maxBytes: 1024 ** 2 },
      { scanId: 3, finishedAtMs: T0 + 2 * DAY, partial: false, state: "known", bytes: 5 * GB, maxBytes: null },
    ],
  };

  it("says what each point means instead of inventing sizes", async () => {
    mockIPC((cmd, args) => {
      if (cmd !== "folder_history") throw new Error(`unexpected ${cmd}`);
      expect(args).toEqual({ scanId: 3, node: 7 });
      return folder;
    });
    render(<FolderHistory scanId={3} node={7} />);
    expect(await screen.findByText("not there")).toBeInTheDocument();
    expect(screen.getByText(/too small to be stored/)).toBeInTheDocument();
    expect(screen.getAllByText(/5\.00 GB/).length).toBeGreaterThan(0); // sparkline range and the list
    expect(screen.getByText(/Not enough complete analyses that stored this folder/)).toBeInTheDocument();
  });

  it("asks only when the person opens it", async () => {
    // Opening is handled by the explorer; this only checks the component loads on mount.
    let calls = 0;
    mockIPC(() => {
      calls += 1;
      return folder;
    });
    expect(calls).toBe(0);
    render(<FolderHistory scanId={3} node={7} />);
    await screen.findByText("not there");
    expect(calls).toBe(1);
    await userEvent.click(document.body);
    expect(calls).toBe(1);
  });
});
