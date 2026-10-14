import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { QueryResult } from "../bindings/QueryResult";
import { StaleFolders } from "./StaleFolders";

// IPC-boundary doubles only.
const result: QueryResult = {
  scanId: 7,
  root: "D:\\",
  finishedAtMs: Date.UTC(2026, 9, 10),
  target: "folders",
  description: "Folders",
  matches: 1,
  items: [
    {
      path: "D:\\Archive\\2022",
      bytes: 200 * 1024 ** 2,
      files: 40,
      lastChangedMs: Date.UTC(2025, 0, 5),
      classification: null,
    },
  ],
  uniqueBytes: 200 * 1024 ** 2,
  moreThanShown: false,
  notApplied: [],
  notes: [],
};

describe("StaleFolders", () => {
  it("sends the chosen age and size as a structured query and lists the folders", async () => {
    const calls: { scanId: number; queryJson: string }[] = [];
    mockIPC((cmd, args) => {
      if (cmd !== "query_saved_analysis") throw new Error(`unexpected command ${cmd}`);
      calls.push(args as { scanId: number; queryJson: string });
      return result;
    });
    render(<StaleFolders scanId={7} />);
    await userEvent.selectOptions(screen.getByLabelText(/Not changed for/), "180");
    await userEvent.click(screen.getByRole("button", { name: "Show folders" }));

    expect(await screen.findByText("D:\\Archive\\2022")).toBeInTheDocument();
    expect(calls).toHaveLength(1);
    expect(calls[0]?.scanId).toBe(7);
    expect(JSON.parse(calls[0]?.queryJson ?? "null")).toEqual({
      target: "folders",
      olderThanDays: 180,
      minBytes: 100 * 1024 ** 2,
      sort: "sizeDesc",
      limit: 50,
    });
  });

  it("shows an analysis that predates change times as not applied, not as an empty answer", async () => {
    mockIPC(() => ({
      ...result,
      notApplied: [{ field: "olderThanDays", reason: "This analysis has no record of when folders last changed." }],
    }));
    render(<StaleFolders scanId={7} />);
    await userEvent.click(screen.getByRole("button", { name: "Show folders" }));
    expect(await screen.findByRole("note")).toHaveTextContent("no record of when folders last changed");
  });

  it("reports a failure", async () => {
    mockIPC(() => {
      throw { kind: "notFound", message: "There is no saved analysis to query." };
    });
    render(<StaleFolders scanId={7} />);
    await userEvent.click(screen.getByRole("button", { name: "Show folders" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no saved analysis");
  });
  it("lists the outermost folders only, and says how many were left out", async () => {
    const item = (path: string, mb: number) => ({
      path,
      bytes: mb * 1024 ** 2,
      files: 1,
      lastChangedMs: Date.UTC(2025, 0, 5),
      classification: null,
    });
    mockIPC(() => ({
      ...result,
      matches: 4,
      items: [
        item("D:\\Data", 900),
        item("D:\\Data\\sub", 800),
        item("D:\\Data\\sub\\deep", 700),
        item("D:\\Database", 300), // shares a prefix with Data but is not inside it
      ],
    }));
    render(<StaleFolders scanId={7} />);
    await userEvent.click(screen.getByRole("button", { name: "Show folders" }));
    expect(await screen.findByText("D:\\Data")).toBeInTheDocument();
    expect(screen.getByText("D:\\Database")).toBeInTheDocument();
    expect(screen.queryByText("D:\\Data\\sub")).not.toBeInTheDocument();
    expect(screen.queryByText("D:\\Data\\sub\\deep")).not.toBeInTheDocument();
    expect(screen.getByText(/2 folders\s+inside them are not listed again/)).toBeInTheDocument();
  });
});
