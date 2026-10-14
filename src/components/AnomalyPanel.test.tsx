import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AnomalyReport } from "../bindings/AnomalyReport";
import { AnomalyPanel } from "./AnomalyPanel";

// IPC-boundary doubles only.
const MB = 1024 ** 2;
const GB = 1024 ** 3;
const T = Date.UTC(2026, 8, 20);

const report: AnomalyReport = {
  root: "C:\\",
  baseline: { scanId: 1, finishedAtMs: T, totalBytes: 200 * GB },
  current: { scanId: 2, finishedAtMs: T + 7 * 86_400_000, totalBytes: 230 * GB },
  netChangeBytes: 30 * GB,
  grew: [
    {
      path: String.raw`C:\Users\u\AppData\Local\Docker`,
      kind: "grew",
      changeBytes: 24 * GB,
      nowBytes: 40 * GB,
      beforeBytes: 16 * GB,
      baseline: "known",
      shareOfNetChange: 80,
      driver: "ownFiles",
      classification: { category: "developerDependencies", rule: "x", reason: "y" },
    },
    {
      path: String.raw`C:\Users\u\Downloads\fresh`,
      kind: "grew",
      changeBytes: 3 * GB,
      nowBytes: 3 * GB,
      beforeBytes: null,
      baseline: "belowThreshold",
      shareOfNetChange: 10,
      driver: "wholeFolder",
      classification: null,
    },
  ],
  shrank: [
    {
      path: String.raw`C:\Windows\Temp\old`,
      kind: "removed",
      changeBytes: -2 * GB,
      nowBytes: 0,
      beforeBytes: 2 * GB,
      baseline: "known",
      shareOfNetChange: -7,
      driver: "wholeFolder",
      classification: null,
    },
  ],
  unattributedBytes: 5 * GB,
  largeFiles: [{ path: String.raw`C:\big.vhdx`, bytes: 9 * GB, beforeBytes: null, classification: null }],
  reportingThresholdBytes: 1.5 * GB,
  caveats: ["Growth marked \"at least\" is a lower bound: the folder was too small to be stored in the baseline."],
};

function backend(result: AnomalyReport | { kind: string; message: string }) {
  mockIPC((cmd, args) => {
    if (cmd !== "anomaly_report") throw new Error(`unexpected ${cmd}`);
    expect(args).toEqual({ scanId: 2 });
    if ("kind" in result) throw result;
    return result;
  });
}

describe("AnomalyPanel", () => {
  it("attributes the change to folders and says what it does not know", async () => {
    backend(report);
    render(<AnomalyPanel scanId={2} />);
    expect(await screen.findByText("Where it grew")).toBeInTheDocument();
    expect(screen.getByText(/Docker$/)).toBeInTheDocument();
    expect(screen.getByText(/mostly files directly in this folder/)).toBeInTheDocument();
    expect(screen.getByText(/80% of the net change/)).toBeInTheDocument();
    // A folder that was too small to be stored before: only a lower bound.
    expect(screen.getByText(/^at least /)).toBeInTheDocument();
    expect(screen.getByText(/is a lower bound/)).toBeInTheDocument();
    expect(screen.getByText("Where it shrank")).toBeInTheDocument();
    expect(screen.getByText(/Removed/)).toBeInTheDocument();
    expect(screen.getByText("New or bigger large files")).toBeInTheDocument();
    expect(screen.getByText(/they do not say why a folder changed or that it should be cleaned/)).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("says plainly when no single folder stands out", async () => {
    backend({ ...report, grew: [], shrank: [], largeFiles: [], netChangeBytes: 200 * MB });
    render(<AnomalyPanel scanId={2} />);
    expect(await screen.findByText(/No single folder changed by .* or more/)).toBeInTheDocument();
  });

  it("treats 'nothing earlier to compare with' as an ordinary state, not an error", async () => {
    backend({ kind: "notFound", message: "There is no earlier complete analysis of this drive to compare with." });
    render(<AnomalyPanel scanId={2} />);
    expect(await screen.findByText(/no earlier complete analysis/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows real failures as errors", async () => {
    backend({ kind: "internal", message: "The database could not be read." });
    render(<AnomalyPanel scanId={2} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The database could not be read.");
  });
});

describe("AnomalyPanel wording for an unchanged drive", () => {
  it("does not say the drive changed by 'unchanged'", async () => {
    mockIPC(() => ({
      ...report,
      netChangeBytes: 0,
      current: { ...report.current, totalBytes: report.baseline.totalBytes },
      grew: [],
      shrank: [],
      largeFiles: [],
    }));
    render(<AnomalyPanel scanId={2} />);
    expect(await screen.findByText(/has not changed noticeably/)).toBeInTheDocument();
    expect(screen.queryByText(/changed by\s*unchanged/)).not.toBeInTheDocument();
  });
});
